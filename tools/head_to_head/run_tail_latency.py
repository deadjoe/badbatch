#!/usr/bin/env python3
"""Run the frozen six-arm Rust/Java open-loop tail-latency protocol."""

from __future__ import annotations

import argparse
import json
import math
import os
import platform
import shlex
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from statistics import median
from typing import Any


PAYLOAD_CLASS = (
    "com/lmax/disruptor/headtohead/TailLatency$AllocationPayload"
)
ALLOCATION_EVENTS = (
    "jdk.ObjectAllocationInNewTLAB,jdk.ObjectAllocationOutsideTLAB"
)
MINIMUM_CONTROL_REPLICATES = 3
MINIMUM_INJECTED_REPLICATES = 3
MINIMUM_CALIBRATION_REPLICATES = 3
PAUSE_PRECISION_CANDIDATES_US = (
    10,
    20,
    50,
    100,
    200,
    500,
    1_000,
    2_000,
    5_000,
)
PAUSE_PRECISION_REPLICATES = 5
PAUSE_PRECISION_MAX_OVERSHOOT_RATIO = 1.75
PAUSE_PRECISION_MAX_RELATIVE_RANGE = 0.10
PAUSE_PRECISION_RATE = 1_000_000
PAUSE_PRECISION_COMMON_MAX = 2_000_000
PAUSE_PRECISION_MEASURED_EVENTS = 100_000
PAUSE_PRECISION_WARMUP_EVENTS = 100_000
A_EQUIVALENCE_BASELINE_REV = "36f6abce33925bd37fa898726a47018b6045d154"
A_EQUIVALENCE_CURRENT_REV = "067f71813ecd24a8df5e1536ab9959d096fceeec"


@dataclass(frozen=True)
class Arm:
    name: str
    mode: str
    retention_window: int | None


def positive_int(text: str) -> int:
    value = int(text.replace("_", ""))
    if value <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return value


def nonnegative_int(text: str) -> int:
    value = int(text.replace("_", ""))
    if value < 0:
        raise argparse.ArgumentTypeError("must be non-negative")
    return value


def parse_load_levels(text: str) -> list[int]:
    levels = [positive_int(part) for part in text.split(",") if part]
    if not levels or any(level >= 100 for level in levels):
        raise argparse.ArgumentTypeError("load levels must be in 1..=99")
    return levels


def parse_load_duration_map(text: str) -> dict[int, int]:
    result: dict[int, int] = {}
    for item in text.split(","):
        try:
            load_text, duration_text = item.split(":", 1)
            load_pct = positive_int(load_text)
            duration_us = positive_int(duration_text)
        except (ValueError, argparse.ArgumentTypeError) as error:
            raise argparse.ArgumentTypeError(
                "expected comma-separated LOAD:DURATION_US pairs"
            ) from error
        if load_pct >= 100:
            raise argparse.ArgumentTypeError("load percentage must be in 1..=99")
        if load_pct in result:
            raise argparse.ArgumentTypeError(f"duplicate load percentage: {load_pct}")
        result[load_pct] = duration_us
    if not result:
        raise argparse.ArgumentTypeError("at least one load duration is required")
    return result


def expected_affected_samples(
    *,
    common_max: int,
    target_rate: int,
    pause_us: int,
) -> int:
    if not 0 < target_rate < common_max:
        raise ValueError("target rate must be in 1..common_max")
    numerator = target_rate * pause_us * common_max
    denominator = 1_000_000 * (common_max - target_rate)
    return (numerator + denominator - 1) // denominator


def worst_observed_pause_us(requested_us: int, max_overshoot_ratio: float) -> int:
    return math.ceil(requested_us * max_overshoot_ratio)


def maximum_safe_requested_pause_us(
    *,
    common_max: int,
    target_rate: int,
    maximum_affected: int,
    requested_upper_us: int,
    max_overshoot_ratio: float,
) -> int:
    lower = 0
    upper = requested_upper_us
    while lower < upper:
        candidate = (lower + upper + 1) // 2
        worst_affected = expected_affected_samples(
            common_max=common_max,
            target_rate=target_rate,
            pause_us=worst_observed_pause_us(
                candidate,
                max_overshoot_ratio,
            ),
        )
        if worst_affected <= maximum_affected:
            lower = candidate
        else:
            upper = candidate - 1
    return lower


def select_inject_sleep_us_by_load(
    *,
    common_max: int,
    load_levels: list[int],
    measured_events: int,
    requested_us_by_load: dict[int, int] | None,
    minimum_requested_us: int,
    max_overshoot_ratio: float,
) -> dict[int, int]:
    minimum_affected = (measured_events + 999) // 1_000
    maximum_affected = measured_events // 10
    preferred_affected = max(minimum_affected, maximum_affected // 2)
    if requested_us_by_load is not None and set(requested_us_by_load) != set(
        load_levels
    ):
        raise SystemExit(
            "inject-sleep-us-by-load keys must exactly match load-levels"
        )

    selected: dict[int, int] = {}
    for load_pct in load_levels:
        target_rate = (common_max * load_pct + 50) // 100
        if not 0 < target_rate < common_max:
            raise SystemExit(
                f"load {load_pct}% produced a target outside 1..common_max-1"
            )
        numerator_per_us = target_rate * common_max
        denominator = 1_000_000 * (common_max - target_rate)
        lower_us = max(
            minimum_requested_us,
            ((minimum_affected - 1) * denominator) // numerator_per_us + 1,
        )
        requested_upper_us = (maximum_affected * denominator) // numerator_per_us
        preferred_upper_us = maximum_safe_requested_pause_us(
            common_max=common_max,
            target_rate=target_rate,
            maximum_affected=preferred_affected,
            requested_upper_us=requested_upper_us,
            max_overshoot_ratio=max_overshoot_ratio,
        )
        if lower_us > preferred_upper_us:
            worst_affected_at_minimum = expected_affected_samples(
                common_max=common_max,
                target_rate=target_rate,
                pause_us=worst_observed_pause_us(
                    minimum_requested_us,
                    max_overshoot_ratio,
                ),
            )
            required_measured_events = max(
                100_000,
                20 * worst_affected_at_minimum,
            )
            raise SystemExit(
                "no precision-qualified microsecond pause satisfies the "
                f"preferred affected-sample band at load {load_pct}%: "
                f"minimum requested={minimum_requested_us}us, "
                f"preferred-safe interval={lower_us}..={preferred_upper_us}us; "
                "increase measured-events to at least "
                f"{required_measured_events}"
            )
        pause_us = (
            preferred_upper_us
            if requested_us_by_load is None
            else requested_us_by_load[load_pct]
        )
        if not lower_us <= pause_us <= preferred_upper_us:
            raise SystemExit(
                f"inject pause {pause_us}us at load {load_pct}% lies outside "
                "the precision-qualified preferred-safe interval "
                f"{lower_us}..={preferred_upper_us}us"
            )
        affected = expected_affected_samples(
            common_max=common_max,
            target_rate=target_rate,
            pause_us=pause_us,
        )
        if not minimum_affected <= affected <= maximum_affected:
            raise SystemExit(
                f"inject pause {pause_us}us at load {load_pct}% produces "
                f"{affected} affected samples outside "
                f"{minimum_affected}..={maximum_affected}"
            )
        worst_affected = expected_affected_samples(
            common_max=common_max,
            target_rate=target_rate,
            pause_us=worst_observed_pause_us(
                pause_us,
                max_overshoot_ratio,
            ),
        )
        if worst_affected > preferred_affected:
            raise SystemExit(
                f"inject pause {pause_us}us at load {load_pct}% can produce "
                f"{worst_affected} affected samples at the precision-preflight "
                f"overshoot ceiling, above preferred {preferred_affected}"
            )
        selected[load_pct] = pause_us
    return selected


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description=(
            "Build, preflight, calibrate, and run the frozen Rust/Java "
            "tail-latency matrix. Announce a shared-machine resource window "
            "before invoking this script."
        )
    )
    result.add_argument("--results-dir", required=True, type=Path)
    result.add_argument("--java-home", type=Path)
    result.add_argument("--lmax-root", type=Path)
    result.add_argument("--rust-bin", type=Path)
    result.add_argument(
        "--a-equivalence-dir",
        required=True,
        type=Path,
        help=(
            "external five-pair adjacent A-equivalence evidence produced at "
            f"{A_EQUIVALENCE_BASELINE_REV[:7]}.."
            f"{A_EQUIVALENCE_CURRENT_REV[:7]}"
        ),
    )
    result.add_argument("--buffer-size", type=positive_int, default=65_536)
    result.add_argument("--measured-events", type=positive_int, default=1_000_000)
    result.add_argument(
        "--rust-warmup-events", type=nonnegative_int, default=100_000
    )
    result.add_argument(
        "--java-warmup-events", type=nonnegative_int, default=1_000_000
    )
    result.add_argument(
        "--calibration-events", type=positive_int, default=100_000_000
    )
    result.add_argument(
        "--calibration-duration-ms", type=positive_int, default=2_000
    )
    result.add_argument(
        "--calibration-replicates",
        type=positive_int,
        default=MINIMUM_CALIBRATION_REPLICATES,
        help=(
            "independent fresh-process calibrations per language/arm; the "
            "conservative selected maximum is their minimum"
        ),
    )
    result.add_argument(
        "--load-levels", type=parse_load_levels, default=parse_load_levels("50,70,90")
    )
    result.add_argument(
        "--wait-strategy", choices=("busy-spin", "yielding"), default="busy-spin"
    )
    result.add_argument("--cpu-list", default="")
    result.add_argument(
        "--expected-allocation-bytes", type=positive_int, default=48
    )
    result.add_argument("--allocation-tolerance", type=float, default=0.05)
    result.add_argument("--preflight-rate", type=positive_int, default=100_000)
    result.add_argument(
        "--inject-sleep-us-by-load",
        type=parse_load_duration_map,
        help=(
            "comma-separated LOAD:DURATION_US overrides; omitted derives one "
            "microsecond-resolution pause per load from total affected samples"
        ),
    )
    result.add_argument(
        "--inject-at-measured-pct", type=positive_int, default=25
    )
    result.add_argument("--co-p50-relative-tolerance", type=float, default=0.05)
    result.add_argument(
        "--co-p50-max-relative-range",
        "--co-control-p50-max-relative-range",
        dest="co_p50_max_relative_range",
        type=float,
        default=0.05,
        help=(
            "per-side full-range/median stability prerequisite; an unstable "
            "control or injected side makes p50 equivalence inconclusive "
            "(the old --co-control-p50-max-relative-range spelling remains "
            "an alias)"
        ),
    )
    result.add_argument(
        "--control-replicates",
        type=positive_int,
        default=MINIMUM_CONTROL_REPLICATES,
        help=(
            "independent control processes per language/arm; must be at least "
            f"{MINIMUM_CONTROL_REPLICATES} so the p50 band can use the observed "
            "control full range"
        ),
    )
    result.add_argument(
        "--injected-replicates",
        type=positive_int,
        default=MINIMUM_INJECTED_REPLICATES,
        help=(
            "independent injected-pause processes per language/arm; must be "
            f"at least {MINIMUM_INJECTED_REPLICATES} so both sides contribute "
            "a measured p50 dispersion"
        ),
    )
    result.add_argument(
        "--co-achieved-target-tolerance", type=float, default=0.01
    )
    result.add_argument("--java-heap", default="2g")
    result.add_argument(
        "--java-option",
        action="append",
        default=[],
        help="additional JVM option; repeat as needed",
    )
    return result


def ensure_external_empty(path: Path, forbidden_roots: list[Path]) -> Path:
    resolved = path.expanduser().resolve()
    for root in forbidden_roots:
        try:
            resolved.relative_to(root.resolve())
        except ValueError:
            continue
        raise SystemExit(f"results-dir must be outside source repositories: {resolved}")
    if resolved.exists() and any(resolved.iterdir()):
        raise SystemExit(f"results-dir must be absent or empty: {resolved}")
    resolved.mkdir(parents=True, exist_ok=True)
    return resolved


def run(
    command: list[str],
    *,
    cwd: Path,
    log_path: Path,
    env: dict[str, str] | None = None,
    allowed_returncodes: tuple[int, ...] = (0,),
) -> int:
    with log_path.open("w", encoding="utf-8") as log:
        log.write("$ " + shlex.join(command) + "\n")
        log.flush()
        completed = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            text=True,
            check=False,
        )
    if completed.returncode not in allowed_returncodes:
        raise SystemExit(
            f"command failed with status {completed.returncode}; see {log_path}"
        )
    return completed.returncode


def read_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def arm_arguments(arm: Arm) -> list[str]:
    result = ["--handler-mode", arm.mode]
    if arm.retention_window is not None:
        result.extend(["--retention-window", str(arm.retention_window)])
    return result


def warmup_events(args: argparse.Namespace, language: str) -> int:
    return int(
        args.rust_warmup_events
        if language == "rust"
        else args.java_warmup_events
    )


def common_arguments(
    args: argparse.Namespace,
    language: str,
    *,
    measured_override: int | None = None,
    warmup_override: int | None = None,
) -> list[str]:
    measured = args.measured_events if measured_override is None else measured_override
    warmup = (
        warmup_events(args, language)
        if warmup_override is None
        else warmup_override
    )
    result = [
        "--wait-strategy",
        args.wait_strategy,
        "--buffer-size",
        str(args.buffer_size),
        "--events-total",
        str(measured + warmup),
        "--warmup-events",
        str(warmup),
    ]
    if args.cpu_list:
        result.extend(["--cpu-list", args.cpu_list])
    return result


def rust_arguments(args: argparse.Namespace) -> list[str]:
    del args
    return ["--event-padding", "none"]


def java_allocation_arguments(args: argparse.Namespace, arm: Arm) -> list[str]:
    if arm.mode == "allocation-free":
        return []
    return [
        "--allocation-bytes-per-event",
        str(args.expected_allocation_bytes),
        "--allocation-measurement-source",
        "jfr_object_allocation",
    ]


def java_base_options(args: argparse.Namespace) -> list[str]:
    return [
        f"-Xms{args.java_heap}",
        f"-Xmx{args.java_heap}",
        "-XX:+AlwaysPreTouch",
        "-XX:+UseG1GC",
        *args.java_option,
    ]


def java_command(
    java: Path,
    classes: Path,
    args: argparse.Namespace,
    arm: Arm,
    *,
    jfr_path: Path | None = None,
    gc_log_path: Path | None = None,
    allocation_events: bool = False,
    measured_override: int | None = None,
    warmup_override: int | None = None,
) -> list[str]:
    command = [str(java), *java_base_options(args)]
    if jfr_path is not None:
        event_settings = (
            f",{ALLOCATION_EVENTS.split(',')[0]}#enabled=true"
            f",{ALLOCATION_EVENTS.split(',')[1]}#enabled=true"
            if allocation_events
            else ""
        )
        command.append(
            "-XX:StartFlightRecording="
            f"filename={jfr_path},settings=profile,dumponexit=true{event_settings}"
        )
    if gc_log_path is not None:
        command.append(
            f"-Xlog:gc*,safepoint:file={gc_log_path}:time,uptime,level,tags"
        )
    command.extend(
        [
            "-cp",
            str(classes),
            "com.lmax.disruptor.headtohead.TailLatency",
            *arm_arguments(arm),
            *java_allocation_arguments(args, arm),
            *common_arguments(
                args,
                "java",
                measured_override=measured_override,
                warmup_override=warmup_override,
            ),
        ]
    )
    return command


def method_code(javap_output: str, signature: str) -> str:
    marker = f"  {signature}"
    lines = javap_output.splitlines()
    try:
        start = lines.index(marker)
    except ValueError:
        raise SystemExit(f"javap output missing method: {signature}")
    end = len(lines)
    for index in range(start + 1, len(lines)):
        if not lines[index].strip():
            end = index
            break
    return "\n".join(lines[start:end])


def bytecode_gate(javap: Path, classes: Path, results: Path) -> None:
    outputs: dict[str, str] = {}
    for name in ("AllocationFreeHandler", "AllocatingHandler"):
        completed = subprocess.run(
            [
                str(javap),
                "-classpath",
                str(classes),
                "-c",
                "-p",
                f"com.lmax.disruptor.headtohead.TailLatency${name}",
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        outputs[name] = completed.stdout
        (results / f"bytecode-{name}.txt").write_text(
            completed.stdout, encoding="utf-8"
        )

    signature = (
        "public void onEvent("
        "com.lmax.disruptor.headtohead.TailLatency$TailEvent, long, boolean);"
    )
    allocation_free = method_code(outputs["AllocationFreeHandler"], signature)
    forbidden = (
        " new ",
        "anewarray",
        "invokedynamic",
        "java/lang/Long.valueOf",
        "java/lang/Integer.valueOf",
    )
    if any(token in allocation_free for token in forbidden):
        raise SystemExit("allocation-free onEvent bytecode contains an allocation opcode")

    allocating = method_code(outputs["AllocatingHandler"], signature)
    new_count = sum(
        1
        for line in allocating.splitlines()
        if ": new " in line and "AllocationPayload" in line
    )
    if new_count != 1:
        raise SystemExit(
            f"allocating onEvent must contain exactly one payload new; got {new_count}"
        )


def jfr_events(jfr_tool: Path, recording: Path) -> list[dict[str, Any]]:
    completed = subprocess.run(
        [
            str(jfr_tool),
            "print",
            "--json",
            "--events",
            ALLOCATION_EVENTS,
            "--stack-depth",
            "8",
            str(recording),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(completed.stdout)["recording"]["events"]


def frame_type_names(event: dict[str, Any]) -> list[str]:
    stack_trace = event.get("values", {}).get("stackTrace") or {}
    frames = stack_trace.get("frames", [])
    return [
        frame.get("method", {}).get("type", {}).get("name", "")
        for frame in frames
    ]


def allocation_preflight(
    args: argparse.Namespace,
    *,
    root: Path,
    results: Path,
    java: Path,
    jfr_tool: Path,
    classes: Path,
    arms: list[Arm],
) -> dict[str, Any]:
    preflight_events = max(
        110_000,
        10_000 + max(arm.retention_window or 0 for arm in arms),
    )
    preflight_warmup = 10_000
    preflight_measured = preflight_events - preflight_warmup
    observed: dict[str, Any] = {}
    for arm in (arms[0], arms[1]):
        jfr_path = results / f"preflight-{arm.name}.jfr"
        output = results / f"preflight-{arm.name}.json"
        samples = results / f"preflight-{arm.name}.csv"
        command = java_command(
            java,
            classes,
            args,
            arm,
            jfr_path=jfr_path,
            allocation_events=True,
            measured_override=preflight_measured,
            warmup_override=preflight_warmup,
        )
        command.extend(
            [
                "--rate",
                str(args.preflight_rate),
                "--own-max",
                str(args.preflight_rate * 2),
                "--jfr-file",
                str(jfr_path),
                "--output",
                str(output),
                "--samples-output",
                str(samples),
            ]
        )
        run(
            command,
            cwd=root,
            log_path=results / f"preflight-{arm.name}.log",
        )
        artifact = read_json(output)
        if not artifact.get("artifact_valid"):
            raise SystemExit(f"invalid JFR preflight artifact: {output}")
        events = jfr_events(jfr_tool, jfr_path)
        if arm.mode == "allocation-free":
            attributed = [
                event
                for event in events
                if any(
                    name.endswith("TailLatency$AllocationFreeHandler")
                    for name in frame_type_names(event)
                )
            ]
            if attributed:
                raise SystemExit(
                    "JFR attributed an allocation to AllocationFreeHandler.onEvent"
                )
            observed[arm.name] = {"handler_allocation_samples": 0}
            continue

        payload_events = [
            event
            for event in events
            if event.get("values", {}).get("objectClass", {}).get("name")
            == PAYLOAD_CLASS
        ]
        if not payload_events:
            raise SystemExit("JFR did not observe AllocationPayload")
        sizes = {int(event["values"]["allocationSize"]) for event in payload_events}
        if sizes != {args.expected_allocation_bytes}:
            raise SystemExit(
                "JFR allocation size mismatch: "
                f"expected {args.expected_allocation_bytes}, observed {sorted(sizes)}"
            )
        if not all(
            frame_type_names(event)
            and frame_type_names(event)[0].endswith(
                "TailLatency$AllocatingHandler"
            )
            for event in payload_events
        ):
            raise SystemExit("JFR payload allocation stack is not the B handler")
        observed[arm.name] = {
            "allocation_size": args.expected_allocation_bytes,
            "samples": len(payload_events),
            "event_types": sorted({event["type"] for event in payload_events}),
        }
    return observed


def summarize_pause_precision(
    observed_ns: list[int],
    *,
    requested_us: int,
    max_overshoot_ratio: float,
    max_relative_range: float,
) -> dict[str, Any]:
    if not observed_ns:
        raise ValueError("pause precision observations must not be empty")
    requested_ns = requested_us * 1_000
    ratios = [observed / requested_ns for observed in observed_ns]
    ratio_median = float(median(ratios))
    ratio_full_range = max(ratios) - min(ratios)
    relative_range = (
        ratio_full_range / ratio_median
        if ratio_median != 0.0
        else math.inf
    )
    passed = (
        min(ratios) >= 1.0
        and max(ratios) <= max_overshoot_ratio
        and relative_range <= max_relative_range
    )
    return {
        "requested_us": requested_us,
        "observed_ns": observed_ns,
        "observed_median_ns": float(median(observed_ns)),
        "observed_full_range_ns": max(observed_ns) - min(observed_ns),
        "observed_over_requested": ratios,
        "ratio_median": ratio_median,
        "ratio_max": max(ratios),
        "ratio_full_range": ratio_full_range,
        "ratio_full_range_over_median": relative_range,
        "passed": passed,
    }


def pause_precision_preflight(
    args: argparse.Namespace,
    *,
    root: Path,
    results: Path,
    rust_bin: Path,
    java: Path,
    classes: Path,
    arm: Arm,
) -> dict[str, Any]:
    observations: dict[str, dict[int, list[int]]] = {
        language: {} for language in ("rust", "java")
    }
    artifact_paths: dict[str, dict[int, list[str]]] = {
        language: {} for language in ("rust", "java")
    }
    exit_statuses: dict[str, dict[int, list[int]]] = {
        language: {} for language in ("rust", "java")
    }
    execution_order: list[str] = []
    attempted_candidates: list[int] = []
    selected: int | None = None

    for candidate_index, requested_us in enumerate(
        PAUSE_PRECISION_CANDIDATES_US
    ):
        attempted_candidates.append(requested_us)
        for replicate in range(1, PAUSE_PRECISION_REPLICATES + 1):
            languages = (
                ("rust", "java")
                if (candidate_index + replicate - 1) % 2 == 0
                else ("java", "rust")
            )
            for language in languages:
                label = (
                    f"pause-precision-{requested_us}us-r{replicate}-{language}"
                )
                execution_order.append(label)
                output = results / f"{label}.json"
                samples = Path(os.devnull)
                jfr_path = (
                    results / f"{label}.jfr"
                    if language == "java"
                    else None
                )
                gc_log_path = (
                    results / f"{label}-gc.log"
                    if language == "java"
                    else None
                )
                if language == "rust":
                    command = [
                        str(rust_bin),
                        *arm_arguments(arm),
                        *common_arguments(
                            args,
                            "rust",
                            measured_override=PAUSE_PRECISION_MEASURED_EVENTS,
                            warmup_override=PAUSE_PRECISION_WARMUP_EVENTS,
                        ),
                        *rust_arguments(args),
                    ]
                else:
                    command = java_command(
                        java,
                        classes,
                        args,
                        arm,
                        jfr_path=jfr_path,
                        gc_log_path=gc_log_path,
                        measured_override=PAUSE_PRECISION_MEASURED_EVENTS,
                        warmup_override=PAUSE_PRECISION_WARMUP_EVENTS,
                    )
                command.extend(
                    [
                        "--rate",
                        str(PAUSE_PRECISION_RATE),
                        "--max-rate",
                        str(PAUSE_PRECISION_COMMON_MAX),
                        "--own-max",
                        str(PAUSE_PRECISION_COMMON_MAX),
                        "--inject-sleep-us-by-load",
                        str(requested_us),
                        "--inject-at-measured-pct",
                        str(args.inject_at_measured_pct),
                        "--output",
                        str(output),
                        "--samples-output",
                        str(samples),
                    ]
                )
                if language == "java":
                    command.extend(
                        [
                            "--jfr-file",
                            str(jfr_path),
                            "--gc-log",
                            str(gc_log_path),
                        ]
                    )
                status = run(
                    command,
                    cwd=root,
                    log_path=results / f"{label}.log",
                    allowed_returncodes=(0, 3),
                )
                artifact = read_json(output)
                loads = artifact.get("loads", [])
                if len(loads) != 1:
                    raise SystemExit(
                        f"pause precision artifact has wrong load count: {output}"
                    )
                pause = loads[0].get("pause_validation", {})
                requested_ns = int(pause.get("requested_sleep_ns", 0))
                observed_ns = int(pause.get("observed_sleep_ns", 0))
                if requested_ns != requested_us * 1_000 or observed_ns <= 0:
                    raise SystemExit(
                        f"invalid pause precision observation: {output}"
                    )
                observations[language].setdefault(requested_us, []).append(
                    observed_ns
                )
                artifact_paths[language].setdefault(requested_us, []).append(
                    output.name
                )
                exit_statuses[language].setdefault(requested_us, []).append(
                    status
                )

        candidate_passed = True
        for language in ("rust", "java"):
            summary = summarize_pause_precision(
                observations[language][requested_us],
                requested_us=requested_us,
                max_overshoot_ratio=PAUSE_PRECISION_MAX_OVERSHOOT_RATIO,
                max_relative_range=PAUSE_PRECISION_MAX_RELATIVE_RANGE,
            )
            candidate_passed &= bool(summary["passed"])
        if candidate_passed:
            selected = requested_us
            break

    summaries: dict[str, dict[str, Any]] = {
        language: {} for language in ("rust", "java")
    }
    for language in ("rust", "java"):
        for requested_us in attempted_candidates:
            summary = summarize_pause_precision(
                observations[language][requested_us],
                requested_us=requested_us,
                max_overshoot_ratio=PAUSE_PRECISION_MAX_OVERSHOOT_RATIO,
                max_relative_range=PAUSE_PRECISION_MAX_RELATIVE_RANGE,
            )
            summary["artifacts"] = artifact_paths[language][requested_us]
            summary["exit_statuses"] = exit_statuses[language][requested_us]
            summaries[language][str(requested_us)] = summary

    report = {
        "schema_version": 1,
        "candidates_us": list(PAUSE_PRECISION_CANDIDATES_US),
        "attempted_candidates_us": attempted_candidates,
        "replicates": PAUSE_PRECISION_REPLICATES,
        "max_overshoot_ratio": PAUSE_PRECISION_MAX_OVERSHOOT_RATIO,
        "max_ratio_full_range_over_median": (
            PAUSE_PRECISION_MAX_RELATIVE_RANGE
        ),
        "rate": PAUSE_PRECISION_RATE,
        "common_max": PAUSE_PRECISION_COMMON_MAX,
        "measured_events": PAUSE_PRECISION_MEASURED_EVENTS,
        "warmup_events": PAUSE_PRECISION_WARMUP_EVENTS,
        "inject_at_measured_pct": args.inject_at_measured_pct,
        "execution_order": execution_order,
        "selected_minimum_requested_us": selected,
        "results": summaries,
    }
    report_path = results / "pause_precision_report.json"
    report_path.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    if selected is None:
        raise SystemExit(
            "no pause-precision candidate satisfied both runtimes; see "
            f"{report_path}"
        )
    return report


def calibration_command(
    language: str,
    arm: Arm,
    args: argparse.Namespace,
    *,
    rust_bin: Path,
    java: Path,
    classes: Path,
    jfr_path: Path | None = None,
    gc_log_path: Path | None = None,
) -> list[str]:
    if language == "rust":
        command = [
            str(rust_bin),
            *arm_arguments(arm),
            *common_arguments(args, "rust"),
            *rust_arguments(args),
        ]
    else:
        command = java_command(
            java,
            classes,
            args,
            arm,
            jfr_path=jfr_path,
            gc_log_path=gc_log_path,
        )
    command.extend(
        [
            "--calibration-events",
            str(args.calibration_events),
            "--calibration-duration-ms",
            str(args.calibration_duration_ms),
            "--calibrate-only",
        ]
    )
    if language == "java":
        if jfr_path is not None:
            command.extend(["--jfr-file", str(jfr_path)])
        if gc_log_path is not None:
            command.extend(["--gc-log", str(gc_log_path)])
    return command


def measurement_command(
    language: str,
    arm: Arm,
    args: argparse.Namespace,
    *,
    rust_bin: Path,
    java: Path,
    classes: Path,
    own_max: int,
    common_max: int,
    output: Path,
    samples: Path,
    jfr_path: Path | None = None,
    gc_log_path: Path | None = None,
    injected: bool = False,
) -> list[str]:
    if language == "rust":
        command = [
            str(rust_bin),
            *arm_arguments(arm),
            *common_arguments(args, "rust"),
            *rust_arguments(args),
        ]
    else:
        command = java_command(
            java,
            classes,
            args,
            arm,
            jfr_path=jfr_path,
            gc_log_path=gc_log_path,
        )
    command.extend(
        [
            "--max-rate",
            str(common_max),
            "--own-max",
            str(own_max),
            "--load-levels",
            ",".join(str(level) for level in args.load_levels),
            "--output",
            str(output),
            "--samples-output",
            str(samples),
        ]
    )
    if language == "java":
        if jfr_path is not None:
            command.extend(["--jfr-file", str(jfr_path)])
        if gc_log_path is not None:
            command.extend(["--gc-log", str(gc_log_path)])
    if injected:
        command.extend(
            [
                "--inject-sleep-us-by-load",
                ",".join(
                    str(args.inject_sleep_us_by_load[load])
                    for load in args.load_levels
                ),
                "--inject-at-measured-pct",
                str(args.inject_at_measured_pct),
            ]
        )
    return command


def build_measurement_plan(
    arms: list[Arm],
    control_replicates: int = MINIMUM_CONTROL_REPLICATES,
    injected_replicates: int = MINIMUM_INJECTED_REPLICATES,
) -> list[tuple[bool, Arm, str, str]]:
    plan: list[tuple[bool, Arm, str, str]] = []
    for replicate in range(1, control_replicates + 1):
        phase = "control" if replicate == 1 else f"control-r{replicate}"
        for arm_index, arm in enumerate(arms):
            languages = (
                ("rust", "java")
                if (arm_index + replicate - 1) % 2 == 0
                else ("java", "rust")
            )
            for language in languages:
                plan.append(
                    (
                        False,
                        arm,
                        language,
                        f"{phase}-{language}-{arm.name}",
                    )
                )
    for replicate in range(1, injected_replicates + 1):
        phase = "injected" if replicate == 1 else f"injected-r{replicate}"
        for arm_index, arm in enumerate(arms):
            languages = (
                ("java", "rust")
                if (arm_index + replicate - 1) % 2 == 0
                else ("rust", "java")
            )
            for language in languages:
                plan.append(
                    (
                        True,
                        arm,
                        language,
                        f"{phase}-{language}-{arm.name}",
                    )
                )
    return plan


def build_calibration_plan(
    arms: list[Arm],
    calibration_replicates: int = MINIMUM_CALIBRATION_REPLICATES,
) -> list[tuple[int, Arm, str, str]]:
    plan: list[tuple[int, Arm, str, str]] = []
    for replicate in range(1, calibration_replicates + 1):
        phase = "calibration" if replicate == 1 else f"calibration-r{replicate}"
        for arm_index, arm in enumerate(arms):
            languages = (
                ("rust", "java")
                if (arm_index + replicate - 1) % 2 == 0
                else ("java", "rust")
            )
            for language in languages:
                plan.append(
                    (
                        replicate,
                        arm,
                        language,
                        f"{phase}-{language}-{arm.name}",
                    )
                )
    return plan


def main() -> None:
    args = parser().parse_args()
    measured = args.measured_events
    if measured < 100_000:
        raise SystemExit("at least 100000 measured events are required")
    if not 0.0 <= args.allocation_tolerance <= 1.0:
        raise SystemExit("allocation-tolerance must be in 0..=1")
    if not 0.0 <= args.co_p50_relative_tolerance <= 1.0:
        raise SystemExit("CO p50 tolerance must be in 0..=1")
    if not 0.0 <= args.co_p50_max_relative_range <= 1.0:
        raise SystemExit("CO p50 range limit must be in 0..=1")
    if not 0.0 <= args.co_achieved_target_tolerance <= 1.0:
        raise SystemExit("CO achieved tolerance must be in 0..=1")
    if args.control_replicates < MINIMUM_CONTROL_REPLICATES:
        raise SystemExit(
            "at least "
            f"{MINIMUM_CONTROL_REPLICATES} independent control replicates "
            "are required"
        )
    if args.injected_replicates < MINIMUM_INJECTED_REPLICATES:
        raise SystemExit(
            "at least "
            f"{MINIMUM_INJECTED_REPLICATES} independent injected replicates "
            "are required"
        )
    if args.calibration_replicates < MINIMUM_CALIBRATION_REPLICATES:
        raise SystemExit(
            "at least "
            f"{MINIMUM_CALIBRATION_REPLICATES} independent calibration "
            "replicates are required"
        )
    if not 1 <= args.inject_at_measured_pct <= 99:
        raise SystemExit("inject-at-measured-pct must be in 1..=99")

    root = Path(__file__).resolve().parents[2]
    lmax_root = (
        args.lmax_root.expanduser().resolve()
        if args.lmax_root
        else (root / "examples/disruptor").resolve()
    )
    a_equivalence_source = args.a_equivalence_dir.expanduser().resolve()
    if not a_equivalence_source.is_dir():
        raise SystemExit(
            f"A-equivalence evidence directory missing: {a_equivalence_source}"
        )
    results = ensure_external_empty(
        args.results_dir,
        [root, lmax_root, a_equivalence_source],
    )
    a_equivalence_results = results / "a-equivalence"
    shutil.copytree(
        a_equivalence_source,
        a_equivalence_results,
        copy_function=shutil.copy2,
    )
    run(
        [
            sys.executable,
            str(
                root
                / "tools/head_to_head/validate_tail_a_equivalence.py"
            ),
            "--results-dir",
            str(a_equivalence_results),
            "--expected-baseline-rev",
            A_EQUIVALENCE_BASELINE_REV,
            "--expected-current-rev",
            A_EQUIVALENCE_CURRENT_REV,
        ],
        cwd=root,
        log_path=results / "validation-a-equivalence.log",
    )
    a_equivalence = read_json(
        a_equivalence_results / "a_equivalence_report.json"
    )
    if measured < 4 * args.buffer_size:
        raise SystemExit(
            "measured events must cover B-4W: "
            f"{measured} < {4 * args.buffer_size}"
        )

    java_home_value = args.java_home or (
        Path(os.environ["JAVA_HOME"]) if os.environ.get("JAVA_HOME") else None
    )
    if java_home_value is None:
        raise SystemExit("--java-home or JAVA_HOME is required")
    java_home = java_home_value.expanduser().resolve()
    java = java_home / "bin/java"
    javap = java_home / "bin/javap"
    jfr_tool = java_home / "bin/jfr"
    for tool in (java, javap, jfr_tool, java_home / "bin/javac"):
        if not tool.is_file():
            raise SystemExit(f"required JDK tool missing: {tool}")

    classes = results / "java-classes"
    generated = results / "java-generated"
    build_env = os.environ.copy()
    build_env["JAVA_HOME"] = str(java_home)
    run(
        [
            str(root / "scripts/build_tail_latency_java.sh"),
            "--classes-dir",
            str(classes),
            "--generated-dir",
            str(generated),
            "--lmax-root",
            str(lmax_root),
        ],
        cwd=root,
        env=build_env,
        log_path=results / "build-java.log",
    )

    if args.rust_bin:
        rust_bin = args.rust_bin.expanduser().resolve()
    else:
        run(
            [
                "cargo",
                "build",
                "--release",
                "--features",
                "bench-tools",
                "--bin",
                "h2h_tail_latency",
            ],
            cwd=root,
            log_path=results / "build-rust.log",
        )
        rust_bin = root / "target/release/h2h_tail_latency"
    if not rust_bin.is_file():
        raise SystemExit(f"Rust tail harness missing: {rust_bin}")

    bytecode_gate(javap, classes, results)
    arms = [
        Arm("a", "allocation-free", None),
        Arm("bw", "allocating", args.buffer_size),
        Arm("b4w", "allocating", 4 * args.buffer_size),
    ]
    preflight = allocation_preflight(
        args,
        root=root,
        results=results,
        java=java,
        jfr_tool=jfr_tool,
        classes=classes,
        arms=arms,
    )
    pause_precision = pause_precision_preflight(
        args,
        root=root,
        results=results,
        rust_bin=rust_bin,
        java=java,
        classes=classes,
        arm=arms[0],
    )

    calibration_plan = build_calibration_plan(
        arms,
        calibration_replicates=args.calibration_replicates,
    )
    calibration_observations: dict[str, list[dict[str, Any]]] = {}
    for _, arm, language, label in calibration_plan:
        key = f"{language}-{arm.name}"
        output = results / f"{label}.json"
        calibration_jfr = (
            results / f"{label}.jfr" if language == "java" else None
        )
        calibration_gc_log = (
            results / f"{label}-gc.log" if language == "java" else None
        )
        command = calibration_command(
            language,
            arm,
            args,
            rust_bin=rust_bin,
            java=java,
            classes=classes,
            jfr_path=calibration_jfr,
            gc_log_path=calibration_gc_log,
        )
        command.extend(["--output", str(output)])
        run(
            command,
            cwd=root,
            log_path=results / f"{label}.log",
        )
        artifact = read_json(output)
        if not artifact.get("artifact_valid"):
            raise SystemExit(f"invalid calibration artifact: {output}")
        own_max = math.floor(float(artifact["own_max"]))
        if own_max <= 0:
            raise SystemExit(f"non-positive calibration maximum: {output}")
        calibration_observations.setdefault(key, []).append(
            {
                "path": output.name,
                "own_max": float(artifact["own_max"]),
                "own_max_floor": own_max,
            }
        )

    calibrations: dict[str, dict[str, Any]] = {}
    own_maxima: dict[str, int] = {}
    for key, observations in calibration_observations.items():
        selected = min(int(entry["own_max_floor"]) for entry in observations)
        calibrations[key] = {
            "selection_rule": "minimum_of_independent_replicates",
            "replicates": observations,
            "own_max_floor": selected,
        }
        own_maxima[key] = selected

    common_max = min(own_maxima.values())
    if common_max <= 0:
        raise SystemExit("global common maximum is not positive")
    target_rates = [
        (common_max * load_pct + 50) // 100
        for load_pct in args.load_levels
    ]
    if any(not 0 < target_rate < common_max for target_rate in target_rates):
        raise SystemExit(
            "every load must produce a target rate strictly between zero "
            "and common_max"
        )
    maximum_planned_measured_duration_ms = max(
        (measured * 1_000 + target_rate - 1) // target_rate
        for target_rate in target_rates
    )
    if args.calibration_duration_ms < maximum_planned_measured_duration_ms:
        raise SystemExit(
            "calibration-duration-ms must not be shorter than the longest "
            "planned measured load: "
            f"{args.calibration_duration_ms} < "
            f"{maximum_planned_measured_duration_ms}"
        )
    args.inject_sleep_us_by_load = select_inject_sleep_us_by_load(
        common_max=common_max,
        load_levels=args.load_levels,
        measured_events=measured,
        requested_us_by_load=args.inject_sleep_us_by_load,
        minimum_requested_us=int(
            pause_precision["selected_minimum_requested_us"]
        ),
        max_overshoot_ratio=float(
            pause_precision["max_overshoot_ratio"]
        ),
    )

    measurement_plan = build_measurement_plan(
        arms,
        control_replicates=args.control_replicates,
        injected_replicates=args.injected_replicates,
    )

    manifest = {
        "schema_version": 4,
        "protocol": "docs/f5_tail_latency_protocol.md",
        "results_dir": str(results),
        "buffer_size": args.buffer_size,
        "measured_events": measured,
        "rust_warmup_events": args.rust_warmup_events,
        "java_warmup_events": args.java_warmup_events,
        "rust_events_total": measured + args.rust_warmup_events,
        "java_events_total": measured + args.java_warmup_events,
        "load_levels": args.load_levels,
        "wait_strategy": args.wait_strategy,
        "event_padding": "none",
        "cpu_list": args.cpu_list,
        "expected_allocation_bytes": args.expected_allocation_bytes,
        "allocation_tolerance": args.allocation_tolerance,
        "inject_sleep_us_by_load": args.inject_sleep_us_by_load,
        "inject_at_measured_pct": args.inject_at_measured_pct,
        "calibration_replicates": args.calibration_replicates,
        "calibration_duration_ms": args.calibration_duration_ms,
        "maximum_planned_measured_duration_ms": (
            maximum_planned_measured_duration_ms
        ),
        "control_replicates": args.control_replicates,
        "injected_replicates": args.injected_replicates,
        "co_p50_relative_tolerance": args.co_p50_relative_tolerance,
        "co_p50_max_relative_range": args.co_p50_max_relative_range,
        "co_p50_empirical_tolerance_rule": (
            "max_control_and_injected_p50_full_range_ns"
        ),
        "co_achieved_target_tolerance": args.co_achieved_target_tolerance,
        "calibrations": calibrations,
        "common_max": common_max,
        "preflight": preflight,
        "pause_precision": {
            "path": "pause_precision_report.json",
            "selected_minimum_requested_us": (
                pause_precision["selected_minimum_requested_us"]
            ),
            "max_overshoot_ratio": pause_precision["max_overshoot_ratio"],
            "max_ratio_full_range_over_median": (
                pause_precision["max_ratio_full_range_over_median"]
            ),
            "replicates": pause_precision["replicates"],
        },
        "a_equivalence": {
            "path": "a-equivalence/a_equivalence_report.json",
            "passed": a_equivalence.get("passed"),
            "pairs": a_equivalence.get("pairs"),
            "baseline_revision": a_equivalence.get("baseline_revision"),
            "current_revision": a_equivalence.get("current_revision"),
        },
        "execution_order": [label for _, _, _, label in measurement_plan],
        "calibration_execution_order": [
            label for _, _, _, label in calibration_plan
        ],
        "host": {
            "system": platform.system(),
            "release": platform.release(),
            "version": platform.version(),
            "machine": platform.machine(),
            "processor": platform.processor(),
            "cpu_count": os.cpu_count(),
        },
        "java_home": str(java_home),
        "java_heap": args.java_heap,
        "java_options": args.java_option,
        "rust_bin": str(rust_bin),
    }
    # Freeze every post-calibration choice before the first measured command.
    (results / "run_manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    for injected, arm, language, label in measurement_plan:
        key = f"{language}-{arm.name}"
        output = results / f"{label}.json"
        samples = results / f"{label}.csv"
        jfr_path = (
            results / f"{label}.jfr" if language == "java" else None
        )
        gc_log_path = (
            results / f"{label}-gc.log" if language == "java" else None
        )
        command = measurement_command(
            language,
            arm,
            args,
            rust_bin=rust_bin,
            java=java,
            classes=classes,
            own_max=own_maxima[key],
            common_max=common_max,
            output=output,
            samples=samples,
            jfr_path=jfr_path,
            gc_log_path=gc_log_path,
            injected=injected,
        )
        run(
            command,
            cwd=root,
            log_path=results / f"{label}.log",
        )

    run(
        [
            sys.executable,
            str(root / "tools/head_to_head/validate_tail_latency.py"),
            "--results-dir",
            str(results),
        ],
        cwd=root,
        log_path=results / "validation.log",
    )
    print(f"Tail-latency matrix complete and valid: {results}")


if __name__ == "__main__":
    main()
