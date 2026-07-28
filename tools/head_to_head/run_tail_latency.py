#!/usr/bin/env python3
"""Run the frozen six-arm Rust/Java open-loop tail-latency protocol."""

from __future__ import annotations

import argparse
import json
import math
import os
import platform
import shlex
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


PAYLOAD_CLASS = (
    "com/lmax/disruptor/headtohead/TailLatency$AllocationPayload"
)
ALLOCATION_EVENTS = (
    "jdk.ObjectAllocationInNewTLAB,jdk.ObjectAllocationOutsideTLAB"
)


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
    if not levels or any(level > 100 for level in levels):
        raise argparse.ArgumentTypeError("load levels must be in 1..=100")
    return levels


def select_inject_sleep_ms(
    *,
    common_max: int,
    load_levels: list[int],
    measured_events: int,
    requested_ms: int | None,
) -> int:
    minimum_affected = (measured_events + 999) // 1_000
    maximum_affected = measured_events // 10
    lower_ms = 1
    preferred_lower_ms = 1
    upper_ms: int | None = None
    preferred_affected = min(maximum_affected, minimum_affected * 5)
    for load_pct in load_levels:
        target_rate = (common_max * load_pct + 50) // 100
        if target_rate <= 0:
            raise SystemExit(f"load {load_pct}% produced a non-positive target")
        arm_lower = ((minimum_affected - 1) * 1_000) // target_rate + 1
        arm_preferred = ((preferred_affected - 1) * 1_000) // target_rate + 1
        arm_upper = (maximum_affected * 1_000) // target_rate
        lower_ms = max(lower_ms, arm_lower)
        preferred_lower_ms = max(preferred_lower_ms, arm_preferred)
        upper_ms = arm_upper if upper_ms is None else min(upper_ms, arm_upper)
    if upper_ms is None or lower_ms > upper_ms:
        raise SystemExit(
            "no integer-millisecond pause satisfies the frozen backlog bounds; "
            "increase measured-events"
        )
    automatic_ms = (
        preferred_lower_ms
        if preferred_lower_ms <= upper_ms
        else lower_ms
    )
    selected = automatic_ms if requested_ms is None else requested_ms
    if not lower_ms <= selected <= upper_ms:
        raise SystemExit(
            f"inject-sleep-ms {selected} violates the calibrated backlog interval "
            f"{lower_ms}..={upper_ms}; choose a value in range or omit the flag"
        )
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
        "--inject-sleep-ms",
        type=positive_int,
        help=(
            "fixed pause duration; omitted selects the smallest integer "
            "millisecond satisfying the frozen backlog bounds after calibration"
        ),
    )
    result.add_argument(
        "--inject-at-measured-pct", type=positive_int, default=25
    )
    result.add_argument("--co-p50-relative-tolerance", type=float, default=0.05)
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
) -> None:
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
    if completed.returncode != 0:
        raise SystemExit(
            f"command failed with status {completed.returncode}; see {log_path}"
        )


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
                "--inject-sleep-ms",
                str(args.inject_sleep_ms),
                "--inject-at-measured-pct",
                str(args.inject_at_measured_pct),
            ]
        )
    return command


def main() -> None:
    args = parser().parse_args()
    measured = args.measured_events
    if measured < 100_000:
        raise SystemExit("at least 100000 measured events are required")
    if not 0.0 <= args.allocation_tolerance <= 1.0:
        raise SystemExit("allocation-tolerance must be in 0..=1")
    if not 0.0 <= args.co_p50_relative_tolerance <= 1.0:
        raise SystemExit("CO p50 tolerance must be in 0..=1")
    if not 0.0 <= args.co_achieved_target_tolerance <= 1.0:
        raise SystemExit("CO achieved tolerance must be in 0..=1")
    if not 1 <= args.inject_at_measured_pct <= 99:
        raise SystemExit("inject-at-measured-pct must be in 1..=99")

    root = Path(__file__).resolve().parents[2]
    lmax_root = (
        args.lmax_root.expanduser().resolve()
        if args.lmax_root
        else (root / "examples/disruptor").resolve()
    )
    results = ensure_external_empty(args.results_dir, [root, lmax_root])
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

    calibrations: dict[str, dict[str, Any]] = {}
    own_maxima: dict[str, int] = {}
    for arm in arms:
        for language in ("rust", "java"):
            key = f"{language}-{arm.name}"
            output = results / f"calibration-{key}.json"
            calibration_jfr = (
                results / f"calibration-{key}.jfr"
                if language == "java"
                else None
            )
            calibration_gc_log = (
                results / f"calibration-{key}-gc.log"
                if language == "java"
                else None
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
                log_path=results / f"calibration-{key}.log",
            )
            artifact = read_json(output)
            if not artifact.get("artifact_valid"):
                raise SystemExit(f"invalid calibration artifact: {output}")
            own_max = math.floor(float(artifact["own_max"]))
            if own_max <= 0:
                raise SystemExit(f"non-positive calibration maximum: {output}")
            calibrations[key] = {
                "path": output.name,
                "own_max": float(artifact["own_max"]),
                "own_max_floor": own_max,
            }
            own_maxima[key] = own_max

    common_max = min(own_maxima.values())
    if common_max <= 0:
        raise SystemExit("global common maximum is not positive")
    args.inject_sleep_ms = select_inject_sleep_ms(
        common_max=common_max,
        load_levels=args.load_levels,
        measured_events=measured,
        requested_ms=args.inject_sleep_ms,
    )

    execution_order: list[str] = []
    for injected in (False, True):
        phase = "injected" if injected else "control"
        for arm_index, arm in enumerate(arms):
            languages = (
                ("rust", "java")
                if (arm_index + int(injected)) % 2 == 0
                else ("java", "rust")
            )
            for language in languages:
                key = f"{language}-{arm.name}"
                label = f"{phase}-{key}"
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
                execution_order.append(label)

    manifest = {
        "schema_version": 1,
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
        "inject_sleep_ms": args.inject_sleep_ms,
        "inject_at_measured_pct": args.inject_at_measured_pct,
        "co_p50_relative_tolerance": args.co_p50_relative_tolerance,
        "co_achieved_target_tolerance": args.co_achieved_target_tolerance,
        "calibrations": calibrations,
        "common_max": common_max,
        "preflight": preflight,
        "execution_order": execution_order,
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
    (results / "run_manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
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
