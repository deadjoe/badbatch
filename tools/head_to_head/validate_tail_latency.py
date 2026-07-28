#!/usr/bin/env python3
"""Validate artifacts from the frozen Rust/Java tail-latency protocol."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path
from typing import Any


ARMS = ("a", "bw", "b4w")
LANGUAGES = ("rust", "java")
PHASES = ("control", "injected")
RAW_HEADER = "sequence,planned_ns,completion_ns,latency_ns"


def load_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def full_revision(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 40
        and all(character in "0123456789abcdefABCDEF" for character in value)
    )


def sample_path(base: Path, load_pct: int) -> Path:
    return base.with_name(f"{base.stem}-{load_pct}{base.suffix or '.csv'}")


def relative_difference(left: float, right: float) -> float:
    denominator = max(abs(left), abs(right))
    if denominator == 0.0:
        return 0.0
    return abs(left - right) / denominator


def p50_equivalent(
    control: float,
    injected: float,
    *,
    relative_tolerance: float,
    absolute_tolerance_ns: int,
) -> bool:
    allowed_delta = max(
        float(absolute_tolerance_ns),
        relative_tolerance * max(abs(control), abs(injected)),
    )
    return abs(control - injected) <= allowed_delta


def required_latency_summary(latencies: list[int]) -> dict[str, int]:
    ordered = sorted(latencies)
    if not ordered:
        return {
            "count": 0,
            "p50": 0,
            "p99": 0,
            "p99.9": 0,
            "p99.99": 0,
            "max": 0,
        }

    def nearest_rank(basis_points: int) -> int:
        rank = (len(ordered) * basis_points + 9_999) // 10_000
        return ordered[max(rank, 1) - 1]

    return {
        "count": len(ordered),
        "p50": nearest_rank(5_000),
        "p99": nearest_rank(9_900),
        "p99.9": nearest_rank(9_990),
        "p99.99": nearest_rank(9_999),
        "max": ordered[-1],
    }


def validate_raw(
    path: Path,
    *,
    warmup_events: int,
    measured_events: int,
    target_rate: int,
    expected_latency: dict[str, Any] | None = None,
) -> list[str]:
    errors: list[str] = []
    latencies: list[int] = []
    try:
        with path.open(encoding="utf-8") as handle:
            header = handle.readline().rstrip("\n")
            if header != RAW_HEADER:
                errors.append(f"{path.name}: raw header mismatch: {header!r}")
                return errors
            count = 0
            for count, line in enumerate(handle, start=1):
                parts = line.rstrip("\n").split(",")
                if len(parts) != 4:
                    errors.append(f"{path.name}:{count + 1}: expected four columns")
                    break
                try:
                    sequence, planned, completion, latency = map(int, parts)
                except ValueError as error:
                    errors.append(f"{path.name}:{count + 1}: {error}")
                    break
                expected_sequence = warmup_events + count - 1
                expected_planned = expected_sequence * 1_000_000_000 // target_rate
                if sequence != expected_sequence:
                    errors.append(
                        f"{path.name}:{count + 1}: sequence "
                        f"{sequence} != {expected_sequence}"
                    )
                    break
                if planned != expected_planned:
                    errors.append(
                        f"{path.name}:{count + 1}: planned "
                        f"{planned} != {expected_planned}"
                    )
                    break
                if completion < 0 or latency != max(0, completion - planned):
                    errors.append(
                        f"{path.name}:{count + 1}: invalid completion/latency"
                    )
                    break
                latencies.append(latency)
            if count != measured_events:
                errors.append(
                    f"{path.name}: raw row count {count} != {measured_events}"
                )
    except OSError as error:
        errors.append(f"{path.name}: cannot read raw samples: {error}")
    if not errors and expected_latency is not None:
        observed = required_latency_summary(latencies)
        for field, value in observed.items():
            try:
                reported = float(expected_latency[field])
            except (KeyError, TypeError, ValueError):
                errors.append(f"{path.name}: missing latency summary field {field}")
                continue
            if not math.isfinite(reported) or reported != float(value):
                errors.append(
                    f"{path.name}: latency summary {field} "
                    f"{reported} != raw {value}"
                )
    return errors


def expected_retention(arm: str, buffer_size: int) -> int | None:
    if arm == "a":
        return None
    return buffer_size if arm == "bw" else 4 * buffer_size


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results-dir", required=True, type=Path)
    args = parser.parse_args()
    root = args.results_dir.expanduser().resolve()
    manifest_path = root / "run_manifest.json"
    manifest = load_json(manifest_path)
    errors: list[str] = []

    def check(condition: bool, message: str) -> None:
        if not condition:
            errors.append(message)

    check(manifest.get("schema_version") == 1, "unsupported run manifest schema")
    buffer_size = int(manifest["buffer_size"])
    measured_events = int(manifest["measured_events"])
    warmup_by_language = {
        "rust": int(manifest["rust_warmup_events"]),
        "java": int(manifest["java_warmup_events"]),
    }
    events_by_language = {
        "rust": int(manifest["rust_events_total"]),
        "java": int(manifest["java_events_total"]),
    }
    load_levels = [int(value) for value in manifest["load_levels"]]
    common_max = int(manifest["common_max"])
    allocation_tolerance = float(manifest["allocation_tolerance"])
    p50_tolerance = float(manifest["co_p50_relative_tolerance"])
    p50_absolute_tolerance_ns = int(
        manifest["co_p50_absolute_tolerance_ns"]
    )
    achieved_tolerance = float(manifest["co_achieved_target_tolerance"])
    expected_allocation = int(manifest["expected_allocation_bytes"])
    java_heap = str(manifest["java_heap"])
    java_options = [str(value) for value in manifest["java_options"]]
    provenance_pairs: dict[str, set[tuple[str, str]]] = {
        language: set() for language in LANGUAGES
    }
    java_runtime_identities: set[tuple[str, str, str, tuple[str, ...]]] = set()

    def record_provenance(
        artifact: dict[str, Any],
        *,
        language: str,
        label: str,
    ) -> None:
        check(artifact.get("provenance_valid") is True, f"{label}: provenance")
        harness_rev = artifact.get("harness_git_rev")
        implementation_rev = artifact.get("implementation_git_rev")
        check(full_revision(harness_rev), f"{label}: bad harness revision")
        check(
            full_revision(implementation_rev),
            f"{label}: bad implementation revision",
        )
        check(
            artifact.get("harness_git_dirty") is False,
            f"{label}: dirty harness",
        )
        check(
            artifact.get("implementation_git_dirty") is False,
            f"{label}: dirty implementation",
        )
        check(
            artifact.get("git_rev") == harness_rev,
            f"{label}: legacy Git revision alias mismatch",
        )
        check(
            artifact.get("dirty") == artifact.get("harness_git_dirty"),
            f"{label}: legacy dirty-state alias mismatch",
        )
        if language == "rust":
            check(
                harness_rev == implementation_rev,
                f"{label}: Rust implementation revision differs from harness",
            )
        if isinstance(harness_rev, str) and isinstance(implementation_rev, str):
            provenance_pairs[language].add(
                (harness_rev.lower(), implementation_rev.lower())
            )

    def validate_common_schema(
        artifact: dict[str, Any],
        *,
        language: str,
        label: str,
    ) -> None:
        expected_impl = (
            "badbatch-builder-tail-latency"
            if language == "rust"
            else "lmax-ring-buffer-tail-latency"
        )
        check(artifact.get("impl") == expected_impl, f"{label}: implementation name")
        check(
            artifact.get("scenario") == "unicast_tail_latency",
            f"{label}: scenario mismatch",
        )
        check(
            artifact.get("arrival_model") == "open_loop_fixed_schedule",
            f"{label}: arrival model mismatch",
        )
        check(
            artifact.get("latency_origin") == "planned_send_time",
            f"{label}: latency origin mismatch",
        )
        check(
            artifact.get("raw_sample_columns") == RAW_HEADER.split(","),
            f"{label}: raw-sample schema mismatch",
        )
        check(
            artifact.get("wait_strategy") == manifest["wait_strategy"],
            f"{label}: wait strategy mismatch",
        )
        check(
            int(artifact.get("buffer_size", -1)) == buffer_size,
            f"{label}: buffer-size mismatch",
        )

    def validate_java_runtime(
        artifact: dict[str, Any],
        *,
        label: str,
        expected_jfr: Path,
        expected_gc: Path,
    ) -> None:
        check(expected_jfr.is_file(), f"{label}: missing JFR")
        check(expected_gc.is_file(), f"{label}: missing GC/safepoint log")
        check(
            artifact.get("jfr_file") == str(expected_jfr),
            f"{label}: JFR path metadata mismatch",
        )
        check(
            artifact.get("gc_log") == str(expected_gc),
            f"{label}: GC-log path metadata mismatch",
        )
        jvm = artifact.get("jvm", {})
        check(isinstance(jvm, dict), f"{label}: JVM metadata")
        input_arguments = (
            [str(value) for value in jvm.get("input_arguments", [])]
            if isinstance(jvm, dict)
            else []
        )
        required_jvm_arguments = {
            f"-Xms{java_heap}",
            f"-Xmx{java_heap}",
            "-XX:+AlwaysPreTouch",
            "-XX:+UseG1GC",
            *java_options,
        }
        check(
            required_jvm_arguments.issubset(set(input_arguments)),
            f"{label}: fixed JVM arguments missing",
        )
        check(
            any(
                argument.startswith("-XX:StartFlightRecording=")
                for argument in input_arguments
            ),
            f"{label}: JFR was not active",
        )
        check(
            any(argument.startswith("-Xlog:gc*") for argument in input_arguments),
            f"{label}: GC/safepoint logging was not active",
        )
        check(
            bool(jvm.get("gc_names")) if isinstance(jvm, dict) else False,
            f"{label}: collector metadata missing",
        )
        if isinstance(jvm, dict):
            java_version = str(jvm.get("java_version", ""))
            vm_name = str(jvm.get("vm_name", ""))
            vm_version = str(jvm.get("vm_version", ""))
            gc_names = tuple(str(value) for value in jvm.get("gc_names", []))
            check(
                bool(java_version and vm_name and vm_version),
                f"{label}: JVM identity missing",
            )
            java_runtime_identities.add(
                (java_version, vm_name, vm_version, gc_names)
            )

    for language in LANGUAGES:
        check(
            measured_events
            == events_by_language[language] - warmup_by_language[language],
            f"manifest {language} measured event count is inconsistent",
        )
    check(measured_events >= 100_000, "fewer than 100000 measured events")
    check(
        measured_events >= 4 * buffer_size,
        "measured event count does not fill B-4W",
    )

    calibration_maxima: dict[str, int] = {}
    for arm in ARMS:
        for language in LANGUAGES:
            key = f"{language}-{arm}"
            entry = manifest["calibrations"].get(key)
            if not isinstance(entry, dict):
                errors.append(f"missing calibration manifest entry: {key}")
                continue
            path = root / str(entry["path"])
            artifact = load_json(path)
            check(artifact.get("run_mode") == "calibration", f"{path.name}: wrong mode")
            check(artifact.get("language") == language, f"{path.name}: wrong language")
            check(
                artifact.get("event_padding") == "none",
                f"{path.name}: unmatched event padding",
            )
            validate_common_schema(
                artifact,
                language=language,
                label=path.name,
            )
            check(artifact.get("artifact_valid") is True, f"{path.name}: invalid")
            record_provenance(
                artifact,
                language=language,
                label=path.name,
            )
            check(
                artifact.get("handler_mode")
                == ("allocation-free" if arm == "a" else "allocating"),
                f"{path.name}: handler mode mismatch",
            )
            check(
                artifact.get("retention_window")
                == expected_retention(arm, buffer_size),
                f"{path.name}: retention mismatch",
            )
            check(
                int(artifact.get("warmup_events", -1))
                == warmup_by_language[language],
                f"{path.name}: calibration warmup input mismatch",
            )
            check(
                int(artifact.get("calibration_warmup_events", -1))
                == max(
                    warmup_by_language[language],
                    expected_retention(arm, buffer_size) or 0,
                ),
                f"{path.name}: effective calibration warmup mismatch",
            )
            check(artifact.get("common_max") is None, f"{path.name}: common_max set")
            check(artifact.get("loads") == [], f"{path.name}: calibration has loads")
            if language == "java":
                expected_jfr = root / f"calibration-{key}.jfr"
                expected_gc = root / f"calibration-{key}-gc.log"
                validate_java_runtime(
                    artifact,
                    label=path.name,
                    expected_jfr=expected_jfr,
                    expected_gc=expected_gc,
                )
            own_max = math.floor(float(artifact["own_max"]))
            calibration_maxima[key] = own_max
            check(
                own_max == int(entry["own_max_floor"]),
                f"{path.name}: floored own max mismatch",
            )

    if calibration_maxima:
        check(
            common_max == min(calibration_maxima.values()),
            "common_max is not the minimum of all six calibrations",
        )
    check(common_max > 0, "common_max must be positive")

    preflight = manifest.get("preflight", {})
    check(
        preflight.get("a", {}).get("handler_allocation_samples") == 0,
        "A JFR allocation preflight failed",
    )
    check(
        preflight.get("bw", {}).get("allocation_size") == expected_allocation,
        "B JFR allocation-size preflight failed",
    )
    check(
        int(preflight.get("bw", {}).get("samples", 0)) > 0,
        "B JFR allocation preflight has no observations",
    )
    expected_order = [
        "control-rust-a",
        "control-java-a",
        "control-java-bw",
        "control-rust-bw",
        "control-rust-b4w",
        "control-java-b4w",
        "injected-java-a",
        "injected-rust-a",
        "injected-rust-bw",
        "injected-java-bw",
        "injected-java-b4w",
        "injected-rust-b4w",
    ]
    check(
        manifest.get("execution_order") == expected_order,
        "execution order is not the frozen balanced order",
    )

    artifacts: dict[tuple[str, str, str], dict[str, Any]] = {}
    for phase in PHASES:
        for arm in ARMS:
            for language in LANGUAGES:
                label = f"{phase}-{language}-{arm}"
                path = root / f"{label}.json"
                artifact = load_json(path)
                artifacts[(phase, language, arm)] = artifact
                check(
                    artifact.get("run_mode") == "measurement",
                    f"{path.name}: wrong mode",
                )
                check(
                    artifact.get("language") == language,
                    f"{path.name}: wrong language",
                )
                check(
                    artifact.get("event_padding") == "none",
                    f"{path.name}: unmatched event padding",
                )
                validate_common_schema(
                    artifact,
                    language=language,
                    label=path.name,
                )
                check(
                    artifact.get("artifact_valid") is True,
                    f"{path.name}: invalid artifact",
                )
                record_provenance(
                    artifact,
                    language=language,
                    label=path.name,
                )
                check(
                    artifact.get("handler_mode")
                    == ("allocation-free" if arm == "a" else "allocating"),
                    f"{path.name}: handler mode mismatch",
                )
                check(
                    artifact.get("retention_window")
                    == expected_retention(arm, buffer_size),
                    f"{path.name}: retention mismatch",
                )
                if language == "java" and arm != "a":
                    check(
                        artifact.get("allocation_measurement_source")
                        == "jfr_object_allocation",
                        f"{path.name}: allocation measurement source mismatch",
                    )
                check(
                    int(artifact.get("events_total", -1))
                    == events_by_language[language],
                    f"{path.name}: events-total mismatch",
                )
                check(
                    int(artifact.get("warmup_events", -1))
                    == warmup_by_language[language],
                    f"{path.name}: warmup mismatch",
                )
                check(
                    math.floor(float(artifact["common_max"])) == common_max,
                    f"{path.name}: common_max mismatch",
                )
                own_max = calibration_maxima.get(f"{language}-{arm}")
                check(
                    math.floor(float(artifact["own_max"])) == own_max,
                    f"{path.name}: own_max mismatch",
                )
                loads = artifact.get("loads", [])
                check(
                    len(loads) == len(load_levels),
                    f"{path.name}: load count mismatch",
                )
                for index, load_pct in enumerate(load_levels):
                    if index >= len(loads):
                        break
                    load = loads[index]
                    target_rate = (common_max * load_pct + 50) // 100
                    check(
                        int(load.get("load_pct", -1)) == load_pct,
                        f"{path.name}: load percentage mismatch",
                    )
                    check(
                        int(load.get("target_rate", -1)) == target_rate,
                        f"{path.name}: target rate mismatch",
                    )
                    check(load.get("valid_run") is True, f"{path.name}: invalid load")
                    check(load.get("rate_valid") is True, f"{path.name}: rate gate")
                    check(
                        load.get("workload_valid") is True,
                        f"{path.name}: workload gate",
                    )
                    check(
                        int(load.get("latency_ns", {}).get("count", -1))
                        == measured_events,
                        f"{path.name}: sample-count summary mismatch",
                    )
                    check(
                        int(load.get("measurement_epoch_unix_ns", 0)) > 0,
                        f"{path.name}: missing wall/monotonic anchor",
                    )
                    check(
                        int(load.get("clock_anchor_uncertainty_ns", -1)) >= 0,
                        f"{path.name}: invalid clock-anchor uncertainty",
                    )
                    workload = load.get("workload", {})
                    if arm == "a":
                        check(
                            int(workload.get("allocations", -1)) == 0,
                            f"{path.name}: A allocated",
                        )
                        check(
                            int(workload.get("retained_objects", -1)) == 0,
                            f"{path.name}: A retained objects",
                        )
                    else:
                        retention = expected_retention(arm, buffer_size)
                        assert retention is not None
                        check(
                            int(workload.get("allocations", -1)) == measured_events,
                            f"{path.name}: B allocation count mismatch",
                        )
                        check(
                            int(workload.get("retained_objects", -1))
                            == retention,
                            f"{path.name}: retained object count mismatch",
                        )
                        check(
                            int(workload.get("estimated_logical_live_bytes", -1))
                            == retention * 32,
                            f"{path.name}: logical live bytes mismatch",
                        )
                        check(
                            int(workload.get("observed_allocated_live_bytes", -1))
                            == retention * expected_allocation,
                            f"{path.name}: observed live bytes mismatch",
                        )
                        check(
                            relative_difference(
                                float(workload["allocation_payload_bytes"]),
                                float(expected_allocation),
                            )
                            <= allocation_tolerance,
                            f"{path.name}: allocation bytes outside tolerance",
                        )
                    raw = sample_path(root / f"{label}.csv", load_pct)
                    errors.extend(
                        validate_raw(
                            raw,
                            warmup_events=warmup_by_language[language],
                            measured_events=measured_events,
                            target_rate=target_rate,
                            expected_latency=load.get("latency_ns"),
                        )
                    )
                    if phase == "injected":
                        pause = load.get("pause_validation", {})
                        check(
                            pause.get("valid") is True,
                            f"{path.name}: injected pause structural gate",
                        )
                if language == "java":
                    expected_jfr = root / f"{label}.jfr"
                    expected_gc = root / f"{label}-gc.log"
                    validate_java_runtime(
                        artifact,
                        label=path.name,
                        expected_jfr=expected_jfr,
                        expected_gc=expected_gc,
                    )
    for language in LANGUAGES:
        check(
            len(provenance_pairs[language]) == 1,
            f"{language} calibration/measurement provenance is inconsistent",
        )
    check(
        len(java_runtime_identities) == 1,
        "Java calibration/measurement runtime identity is inconsistent",
    )
    if all(len(provenance_pairs[language]) == 1 for language in LANGUAGES):
        rust_harness = next(iter(provenance_pairs["rust"]))[0]
        java_harness = next(iter(provenance_pairs["java"]))[0]
        check(
            rust_harness == java_harness,
            "Rust and Java artifacts do not share one BadBatch harness revision",
        )

    for arm in ARMS:
        for language in LANGUAGES:
            control = artifacts.get(("control", language, arm), {})
            injected = artifacts.get(("injected", language, arm), {})
            for index, load_pct in enumerate(load_levels):
                control_loads = control.get("loads", [])
                injected_loads = injected.get("loads", [])
                if index >= len(control_loads) or index >= len(injected_loads):
                    continue
                control_load = control_loads[index]
                injected_load = injected_loads[index]
                control_p50 = float(control_load["latency_ns"]["p50"])
                injected_p50 = float(injected_load["latency_ns"]["p50"])
                check(
                    p50_equivalent(
                        control_p50,
                        injected_p50,
                        relative_tolerance=p50_tolerance,
                        absolute_tolerance_ns=p50_absolute_tolerance_ns,
                    ),
                    f"{language}-{arm}-{load_pct}: injected p50 outside tolerance",
                )
                control_ratio = float(control_load["actual_target_ratio"])
                injected_ratio = float(injected_load["actual_target_ratio"])
                check(
                    abs(control_ratio - 1.0) <= achieved_tolerance,
                    f"{language}-{arm}-{load_pct}: control achieved ratio not tight",
                )
                check(
                    abs(injected_ratio - 1.0) <= achieved_tolerance,
                    f"{language}-{arm}-{load_pct}: injected achieved ratio not tight",
                )
                check(
                    injected_ratio + achieved_tolerance >= control_ratio,
                    f"{language}-{arm}-{load_pct}: achieved rate materially dropped",
                )

    for phase in PHASES:
        for arm in ARMS:
            rust_loads = artifacts.get((phase, "rust", arm), {}).get("loads", [])
            java_loads = artifacts.get((phase, "java", arm), {}).get("loads", [])
            for index, load_pct in enumerate(load_levels):
                if index >= len(rust_loads) or index >= len(java_loads):
                    continue
                check(
                    rust_loads[index].get("target_rate")
                    == java_loads[index].get("target_rate"),
                    f"{phase}-{arm}-{load_pct}: unequal absolute target rates",
                )
                if arm != "a":
                    rust_bytes = float(
                        rust_loads[index]["workload"]["allocation_payload_bytes"]
                    )
                    java_bytes = float(
                        java_loads[index]["workload"]["allocation_payload_bytes"]
                    )
                    check(
                        relative_difference(rust_bytes, java_bytes)
                        <= allocation_tolerance,
                        f"{phase}-{arm}-{load_pct}: allocation alignment failed",
                    )

    report = {
        "schema_version": 1,
        "passed": not errors,
        "errors": errors,
        "common_max": common_max,
        "calibration_maxima": calibration_maxima,
        "raw_samples_validated_per_load": measured_events,
        "load_levels": load_levels,
        "co_p50_relative_tolerance": p50_tolerance,
        "co_p50_absolute_tolerance_ns": p50_absolute_tolerance_ns,
        "co_achieved_target_tolerance": achieved_tolerance,
        "allocation_tolerance": allocation_tolerance,
    }
    report_path = root / "validation_report.json"
    report_path.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    if errors:
        for error in errors:
            print(f"ERROR: {error}")
        raise SystemExit(f"tail-latency validation failed; see {report_path}")
    print(f"Tail-latency validation passed: {report_path}")


if __name__ == "__main__":
    main()
