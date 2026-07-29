#!/usr/bin/env python3
"""Validate the adjacent A-arm regression evidence for the F.5 harness."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from statistics import median
from typing import Any

from validate_tail_latency import RAW_HEADER, full_revision, validate_raw


SUMMARY_FIELDS = ("p50", "p99", "p99.9")


def load_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def ranges_overlap(left: list[float], right: list[float]) -> bool:
    return max(min(left), min(right)) <= min(max(left), max(right))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results-dir", required=True, type=Path)
    parser.add_argument("--expected-baseline-rev", required=True)
    parser.add_argument("--expected-current-rev", required=True)
    parser.add_argument("--pairs", type=int, default=5)
    args = parser.parse_args()

    root = args.results_dir.expanduser().resolve()
    errors: list[str] = []

    def check(condition: bool, message: str) -> None:
        if not condition:
            errors.append(message)

    check(args.pairs >= 5, "fewer than five A-equivalence pairs")
    check(full_revision(args.expected_baseline_rev), "bad expected baseline revision")
    check(full_revision(args.expected_current_rev), "bad expected current revision")

    artifacts: dict[str, list[dict[str, Any]]] = {
        "baseline": [],
        "current": [],
    }
    mtimes: dict[tuple[str, int], int] = {}
    for pair in range(1, args.pairs + 1):
        for arm_name in ("baseline", "current"):
            json_path = root / f"{arm_name}-{pair}.json"
            csv_path = root / f"{arm_name}-{pair}.csv"
            artifact = load_json(json_path)
            artifacts[arm_name].append(artifact)
            mtimes[(arm_name, pair)] = json_path.stat().st_mtime_ns

            label = json_path.name
            check(artifact.get("language") == "rust", f"{label}: wrong language")
            check(
                artifact.get("impl") == "badbatch-builder-tail-latency",
                f"{label}: wrong implementation",
            )
            check(
                artifact.get("scenario") == "unicast_tail_latency",
                f"{label}: wrong scenario",
            )
            check(
                artifact.get("arrival_model") == "open_loop_fixed_schedule",
                f"{label}: wrong arrival model",
            )
            check(
                artifact.get("latency_origin") == "planned_send_time",
                f"{label}: wrong latency origin",
            )
            check(
                artifact.get("raw_sample_columns") == RAW_HEADER.split(","),
                f"{label}: raw schema mismatch",
            )
            check(
                artifact.get("wait_strategy") == "busy-spin",
                f"{label}: wait strategy mismatch",
            )
            check(
                artifact.get("event_padding") == "none",
                f"{label}: event padding mismatch",
            )
            check(
                artifact.get("provenance_valid") is True,
                f"{label}: invalid provenance",
            )
            check(artifact.get("dirty") is False, f"{label}: dirty build")

            loads = artifact.get("loads", [])
            check(len(loads) == 1, f"{label}: expected one load")
            if not loads:
                continue
            load = loads[0]
            check(load.get("valid_run") is True, f"{label}: invalid load")
            check(
                int(load.get("target_rate", -1)) == 100_000,
                f"{label}: target-rate mismatch",
            )
            check(
                abs(float(load.get("actual_target_ratio", 0.0)) - 1.0) <= 0.01,
                f"{label}: achieved/target outside 1%",
            )
            measured_events = int(artifact["events_total"]) - int(
                artifact["warmup_events"]
            )
            errors.extend(
                validate_raw(
                    csv_path,
                    warmup_events=int(artifact["warmup_events"]),
                    measured_events=measured_events,
                    target_rate=int(load["target_rate"]),
                    expected_latency=load.get("latency_ns"),
                )
            )

            if arm_name == "baseline":
                check(
                    artifact.get("git_rev") == args.expected_baseline_rev,
                    f"{label}: baseline revision mismatch",
                )
            else:
                check(
                    artifact.get("artifact_valid") is True,
                    f"{label}: invalid current artifact",
                )
                check(
                    artifact.get("git_rev") == args.expected_current_rev,
                    f"{label}: current revision mismatch",
                )
                check(
                    artifact.get("harness_git_rev") == args.expected_current_rev,
                    f"{label}: current harness revision mismatch",
                )
                check(
                    artifact.get("implementation_git_rev")
                    == args.expected_current_rev,
                    f"{label}: current implementation revision mismatch",
                )
                check(
                    artifact.get("handler_mode") == "allocation-free",
                    f"{label}: current handler is not A",
                )
                workload = load.get("workload", {})
                check(
                    int(workload.get("allocations", -1)) == 0,
                    f"{label}: current A allocated",
                )
                check(
                    int(workload.get("retained_objects", -1)) == 0,
                    f"{label}: current A retained objects",
                )

    expected_order: list[tuple[str, int]] = []
    for pair in range(1, args.pairs + 1):
        if pair % 2 == 1:
            expected_order.extend((("baseline", pair), ("current", pair)))
        else:
            expected_order.extend((("current", pair), ("baseline", pair)))
    observed_mtimes = [mtimes[key] for key in expected_order if key in mtimes]
    check(
        len(observed_mtimes) == 2 * args.pairs
        and all(
            earlier < later
            for earlier, later in zip(observed_mtimes, observed_mtimes[1:])
        ),
        "artifact mtimes do not prove the declared adjacent alternating order",
    )

    summaries: dict[str, dict[str, Any]] = {}
    for field in SUMMARY_FIELDS:
        baseline_values = [
            float(artifact["loads"][0]["latency_ns"][field])
            for artifact in artifacts["baseline"]
            if artifact.get("loads")
        ]
        current_values = [
            float(artifact["loads"][0]["latency_ns"][field])
            for artifact in artifacts["current"]
            if artifact.get("loads")
        ]
        overlap = bool(
            baseline_values
            and current_values
            and ranges_overlap(baseline_values, current_values)
        )
        check(overlap, f"{field}: baseline/current replicate ranges do not overlap")
        summaries[field] = {
            "baseline_values_ns": baseline_values,
            "current_values_ns": current_values,
            "baseline_median_ns": float(median(baseline_values)),
            "current_median_ns": float(median(current_values)),
            "baseline_range_ns": [
                min(baseline_values),
                max(baseline_values),
            ],
            "current_range_ns": [
                min(current_values),
                max(current_values),
            ],
            "ranges_overlap": overlap,
        }

    achieved = {
        arm_name: [
            float(artifact["loads"][0]["actual_target_ratio"])
            for artifact in arm_artifacts
            if artifact.get("loads")
        ]
        for arm_name, arm_artifacts in artifacts.items()
    }
    report = {
        "schema_version": 1,
        "passed": not errors,
        "errors": errors,
        "measurement_apparatus_regression_only": True,
        "portable_performance_evidence": False,
        "pairs": args.pairs,
        "execution_order": [
            f"{arm_name}-{pair}" for arm_name, pair in expected_order
        ],
        "baseline_revision": args.expected_baseline_rev,
        "current_revision": args.expected_current_rev,
        "target_rate": 100_000,
        "achieved_target_ratios": achieved,
        "achieved_target_medians": {
            arm_name: float(median(values))
            for arm_name, values in achieved.items()
        },
        "latency_summaries": summaries,
    }
    report_path = root / "a_equivalence_report.json"
    report_path.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    if errors:
        for error in errors:
            print(f"ERROR: {error}")
        raise SystemExit(f"A-equivalence validation failed; see {report_path}")
    print(f"A-equivalence validation passed: {report_path}")


if __name__ == "__main__":
    main()
