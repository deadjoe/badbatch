#!/usr/bin/env python3
"""Validate artifacts from the frozen Rust/Java tail-latency protocol."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path
from statistics import median
from typing import Any


ARMS = ("a", "bw", "b4w")
LANGUAGES = ("rust", "java")
PHASES = ("control", "injected")
RAW_HEADER = "sequence,planned_ns,completion_ns,latency_ns"
MINIMUM_CONTROL_REPLICATES = 3
MINIMUM_INJECTED_REPLICATES = 3
MINIMUM_CALIBRATION_REPLICATES = 3
MEASUREMENT_ORDER_RULE = "replicate_interleaved_control_then_injected"
SIGNED_RESIDUAL_RULE = "two_sided_exact_sign_test_and_load_monotonicity"
SIGNED_RESIDUAL_ALPHA = 0.05
CROSS_RUN_MAGNITUDE_RATIO_MIN = 0.5
CROSS_RUN_MAGNITUDE_RATIO_MAX = 2.0
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


def expected_pause_counts(
    *,
    common_max: int,
    target_rate: int,
    observed_sleep_ns: int,
) -> tuple[int, int]:
    if not 0 < target_rate < common_max:
        raise ValueError("target rate must be in 1..common_max")
    backlog_numerator = target_rate * observed_sleep_ns
    backlog = (backlog_numerator + 999_999_999) // 1_000_000_000
    affected_denominator = 1_000_000_000 * (common_max - target_rate)
    affected_numerator = backlog_numerator * common_max
    affected = (
        affected_numerator + affected_denominator - 1
    ) // affected_denominator
    return max(backlog, 1), max(affected, 1)


def p50_equivalent(
    control_median: float,
    injected_median: float,
    *,
    relative_tolerance: float,
    control_full_range_ns: float,
    injected_full_range_ns: float,
) -> bool:
    allowed_delta = max(
        control_full_range_ns,
        injected_full_range_ns,
        relative_tolerance
        * max(abs(control_median), abs(injected_median)),
    )
    return abs(control_median - injected_median) <= allowed_delta


def p50_stability(
    p50s: list[float],
    *,
    max_relative_range: float,
) -> tuple[float, float, float, bool]:
    if not p50s:
        raise ValueError("p50 samples must not be empty")
    observed_median = float(median(p50s))
    full_range = max(p50s) - min(p50s)
    relative_range = (
        full_range / abs(observed_median)
        if observed_median != 0.0
        else (0.0 if full_range == 0.0 else math.inf)
    )
    return (
        observed_median,
        full_range,
        relative_range,
        relative_range <= max_relative_range,
    )


def exact_two_sided_sign_test(positive: int, negative: int) -> float:
    if positive < 0 or negative < 0:
        raise ValueError("sign counts must be non-negative")
    trials = positive + negative
    if trials == 0:
        return 1.0
    smaller = min(positive, negative)
    lower_tail = sum(math.comb(trials, index) for index in range(smaller + 1))
    return min(1.0, 2.0 * lower_tail / (2**trials))


def signed_delta_summary(deltas: list[float]) -> dict[str, Any]:
    positive = sum(delta > 0.0 for delta in deltas)
    negative = sum(delta < 0.0 for delta in deltas)
    zero = len(deltas) - positive - negative
    nonzero = positive + negative
    if nonzero >= 2 and negative == 0:
        common_direction = "positive"
    elif nonzero >= 2 and positive == 0:
        common_direction = "negative"
    else:
        common_direction = None
    return {
        "total": len(deltas),
        "positive": positive,
        "negative": negative,
        "zero": zero,
        "nonzero": nonzero,
        "common_nonzero_direction": common_direction,
        "two_sided_exact_sign_test_p_value": exact_two_sided_sign_test(
            positive,
            negative,
        ),
    }


def signed_residual_analysis(
    p50_results: dict[str, dict[str, Any]],
    load_levels: list[int],
) -> dict[str, Any]:
    def stratified_summary(
        languages: tuple[str, ...],
        arms: tuple[str, ...],
    ) -> dict[str, Any]:
        entries = [
            p50_results[key]
            for language in languages
            for arm in arms
            for load in load_levels
            if (key := f"{language}-{arm}-{load}") in p50_results
        ]
        return {
            "all_cells": signed_delta_summary(
                [
                    float(entry["injected_minus_control_median_ns"])
                    for entry in entries
                ]
            ),
            "decided_cells": signed_delta_summary(
                [
                    float(entry["injected_minus_control_median_ns"])
                    for entry in entries
                    if not str(entry["equivalence_status"]).startswith(
                        "inconclusive"
                    )
                ]
            ),
        }

    all_deltas = [
        float(entry["injected_minus_control_median_ns"])
        for entry in p50_results.values()
    ]
    decided_deltas = [
        float(entry["injected_minus_control_median_ns"])
        for entry in p50_results.values()
        if not str(entry["equivalence_status"]).startswith("inconclusive")
    ]
    groups: dict[str, dict[str, Any]] = {}
    monotonic_groups: list[str] = []
    for language in LANGUAGES:
        for arm in ARMS:
            cells = [
                (
                    load,
                    float(
                        p50_results[f"{language}-{arm}-{load}"][
                            "injected_minus_control_median_ns"
                        ]
                    ),
                )
                for load in load_levels
                if f"{language}-{arm}-{load}" in p50_results
            ]
            deltas = [delta for _, delta in cells]
            nondecreasing = len(deltas) >= 2 and all(
                left <= right for left, right in zip(deltas, deltas[1:])
            )
            nonincreasing = len(deltas) >= 2 and all(
                left >= right for left, right in zip(deltas, deltas[1:])
            )
            constant = len(set(deltas)) <= 1
            load_monotonic_nonconstant = (
                not constant and (nondecreasing or nonincreasing)
            )
            key = f"{language}-{arm}"
            if load_monotonic_nonconstant:
                monotonic_groups.append(key)
            groups[key] = {
                "by_load": [
                    {"load_pct": load, "delta_ns": delta}
                    for load, delta in cells
                ],
                "nondecreasing": nondecreasing,
                "nonincreasing": nonincreasing,
                "constant": constant,
                "load_monotonic_nonconstant": load_monotonic_nonconstant,
            }

    all_summary = signed_delta_summary(all_deltas)
    decided_summary = signed_delta_summary(decided_deltas)
    inconclusive_cells = [
        {
            "cell": key,
            "delta_ns": float(entry["injected_minus_control_median_ns"]),
            "equivalence_status": str(entry["equivalence_status"]),
        }
        for key, entry in sorted(p50_results.items())
        if str(entry["equivalence_status"]).startswith("inconclusive")
    ]
    language_summaries = {
        language: stratified_summary((language,), ARMS)
        for language in LANGUAGES
    }
    arm_summaries = {
        arm: stratified_summary(LANGUAGES, (arm,))
        for arm in ARMS
    }
    language_arm_summaries = {
        f"{language}-{arm}": stratified_summary((language,), (arm,))
        for language in LANGUAGES
        for arm in ARMS
    }
    monotonic_group_count = len(LANGUAGES) * len(ARMS)
    monotonic_null_probability = (
        2.0 / math.factorial(len(load_levels))
        if len(load_levels) >= 2
        else None
    )
    monotonic_chance_baseline = {
        "assumption": "independent_continuous_deltas_without_ties",
        "loads_per_group": len(load_levels),
        "group_count": monotonic_group_count,
        "per_group_probability": monotonic_null_probability,
        "expected_flagged_groups": (
            monotonic_group_count * monotonic_null_probability
            if monotonic_null_probability is not None
            else None
        ),
        "interpretation": (
            "descriptive chance baseline only; integer-nanosecond ties and "
            "cross-load dependence can change the realized frequency"
        ),
    }
    observations: list[str] = []
    if (
        all_summary["nonzero"] > 0
        and all_summary["two_sided_exact_sign_test_p_value"]
        <= SIGNED_RESIDUAL_ALPHA
    ):
        observations.append("all_cells_signed_imbalance")
    if (
        decided_summary["nonzero"] > 0
        and decided_summary["two_sided_exact_sign_test_p_value"]
        <= SIGNED_RESIDUAL_ALPHA
    ):
        observations.append("decided_cells_signed_imbalance")
    if monotonic_groups:
        observations.append("load_monotonic_nonconstant_groups")
    return {
        "rule": SIGNED_RESIDUAL_RULE,
        "alpha": SIGNED_RESIDUAL_ALPHA,
        "primary_population": "decided_cells",
        "primary_summary": decided_summary,
        "all_cells": all_summary,
        "decided_cells": decided_summary,
        "inconclusive_cells": inconclusive_cells,
        "language_summaries": language_summaries,
        "arm_summaries": arm_summaries,
        "language_arm_summaries": language_arm_summaries,
        "groups": groups,
        "load_monotonic_nonconstant_groups": monotonic_groups,
        "load_monotonic_chance_baseline": monotonic_chance_baseline,
        "observations": observations,
        "requires_residual_observation": bool(observations),
    }


def cross_run_residual_comparison(
    current_results: dict[str, dict[str, Any]],
    prior_results: dict[str, dict[str, Any]],
    *,
    current_gate_context: dict[str, Any],
    prior_gate_context: dict[str, Any],
) -> dict[str, Any]:
    cells: dict[str, dict[str, Any]] = {}
    exact_nonzero_delta_reproductions: list[str] = []
    current_inconclusive_exact_nonzero_delta_reproductions: list[str] = []
    same_nonzero_direction_cells: list[str] = []
    comparable_magnitude_residual_reproductions: list[str] = []
    current_inconclusive_comparable_magnitude_reproductions: list[str] = []
    for key in sorted(current_results.keys() & prior_results.keys()):
        current = current_results[key]
        prior = prior_results[key]
        current_delta = float(current["injected_minus_control_median_ns"])
        prior_delta = float(prior["injected_minus_control_median_ns"])
        current_status = str(current["equivalence_status"])
        prior_status = str(prior["equivalence_status"])
        exact_nonzero_delta_reproduction = (
            current_delta != 0.0 and current_delta == prior_delta
        )
        same_nonzero_direction = (
            current_delta != 0.0
            and prior_delta != 0.0
            and (current_delta > 0.0) == (prior_delta > 0.0)
        )
        magnitude_ratio = (
            abs(current_delta) / abs(prior_delta)
            if current_delta != 0.0 and prior_delta != 0.0
            else None
        )
        comparable_magnitude_residual_reproduction = (
            same_nonzero_direction
            and magnitude_ratio is not None
            and CROSS_RUN_MAGNITUDE_RATIO_MIN
            <= magnitude_ratio
            <= CROSS_RUN_MAGNITUDE_RATIO_MAX
        )
        current_inconclusive = current_status.startswith("inconclusive")
        if exact_nonzero_delta_reproduction:
            exact_nonzero_delta_reproductions.append(key)
        if current_inconclusive and exact_nonzero_delta_reproduction:
            current_inconclusive_exact_nonzero_delta_reproductions.append(key)
        if same_nonzero_direction:
            same_nonzero_direction_cells.append(key)
        if comparable_magnitude_residual_reproduction:
            comparable_magnitude_residual_reproductions.append(key)
        if current_inconclusive and comparable_magnitude_residual_reproduction:
            current_inconclusive_comparable_magnitude_reproductions.append(
                key
            )
        cells[key] = {
            "prior_equivalence_status": prior_status,
            "current_equivalence_status": current_status,
            "prior_delta_ns": prior_delta,
            "current_delta_ns": current_delta,
            "exact_nonzero_delta_reproduction": (
                exact_nonzero_delta_reproduction
            ),
            "same_nonzero_direction": same_nonzero_direction,
            "current_to_prior_absolute_magnitude_ratio": magnitude_ratio,
            "comparable_magnitude_residual_reproduction": (
                comparable_magnitude_residual_reproduction
            ),
            "current_inconclusive": current_inconclusive,
        }
    gate_context_matches = current_gate_context == prior_gate_context
    return {
        "acceptance_effect": "none_descriptive_only",
        "primary_reproduction_rule": (
            "both_nonzero_same_direction_and_current_to_prior_absolute_"
            "magnitude_ratio_inclusive_0.5_to_2.0"
        ),
        "magnitude_ratio_bounds": {
            "minimum_inclusive": CROSS_RUN_MAGNITUDE_RATIO_MIN,
            "maximum_inclusive": CROSS_RUN_MAGNITUDE_RATIO_MAX,
        },
        "resolution_limit": (
            "for small integer-nanosecond deltas near the 1 ns quantization "
            "floor, the magnitude-ratio rule has little discrimination "
            "beyond requiring the same nonzero sign"
        ),
        "gate_context": {
            "current": current_gate_context,
            "prior": prior_gate_context,
            "matches": gate_context_matches,
            "status_attribution": (
                "gate contexts match; status differences may be compared "
                "without a known schema/tolerance confound"
                if gate_context_matches
                else "gate contexts differ; status differences cannot be "
                "attributed directly to measurement changes"
            ),
            "signed_delta_attribution": (
                "signed median deltas are gate-independent and remain "
                "comparable"
            ),
        },
        "cells": cells,
        "exact_nonzero_delta_reproductions": (
            exact_nonzero_delta_reproductions
        ),
        "current_inconclusive_exact_nonzero_delta_reproductions": (
            current_inconclusive_exact_nonzero_delta_reproductions
        ),
        "same_nonzero_direction_cells": same_nonzero_direction_cells,
        "comparable_magnitude_residual_reproductions": (
            comparable_magnitude_residual_reproductions
        ),
        "current_inconclusive_comparable_magnitude_reproductions": (
            current_inconclusive_comparable_magnitude_reproductions
        ),
    }


def expected_measurement_order(
    control_replicates: int,
    injected_replicates: int,
) -> list[str]:
    if control_replicates != injected_replicates:
        raise ValueError(
            "control and injected replicate counts must match for paired "
            "phase interleaving"
        )
    order: list[str] = []
    for replicate in range(1, control_replicates + 1):
        for injected in (False, True):
            phase_name = "injected" if injected else "control"
            phase = (
                phase_name
                if replicate == 1
                else f"{phase_name}-r{replicate}"
            )
            for arm_index, arm in enumerate(ARMS):
                control_languages = (
                    ("rust", "java")
                    if (arm_index + replicate - 1) % 2 == 0
                    else ("java", "rust")
                )
                languages = (
                    tuple(reversed(control_languages))
                    if injected
                    else control_languages
                )
                order.extend(
                    f"{phase}-{language}-{arm}" for language in languages
                )
    return order


def expected_calibration_order(calibration_replicates: int) -> list[str]:
    order: list[str] = []
    for replicate in range(1, calibration_replicates + 1):
        phase = "calibration" if replicate == 1 else f"calibration-r{replicate}"
        for arm_index, arm in enumerate(ARMS):
            languages = (
                ("rust", "java")
                if (arm_index + replicate - 1) % 2 == 0
                else ("java", "rust")
            )
            order.extend(f"{phase}-{language}-{arm}" for language in languages)
    return order


def expected_pause_precision_order(
    candidates_us: list[int],
    replicates: int,
) -> list[str]:
    order: list[str] = []
    for candidate_index, requested_us in enumerate(candidates_us):
        for replicate in range(1, replicates + 1):
            languages = (
                ("rust", "java")
                if (candidate_index + replicate - 1) % 2 == 0
                else ("java", "rust")
            )
            order.extend(
                f"pause-precision-{requested_us}us-r{replicate}-{language}"
                for language in languages
            )
    return order


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
    parser.add_argument(
        "--prior-validation-report",
        type=Path,
        help=(
            "optional prior report for descriptive cross-run signed-residual "
            "comparison; never changes acceptance"
        ),
    )
    args = parser.parse_args()
    root = args.results_dir.expanduser().resolve()
    manifest_path = root / "run_manifest.json"
    manifest = load_json(manifest_path)
    errors: list[str] = []

    def check(condition: bool, message: str) -> None:
        if not condition:
            errors.append(message)

    check(manifest.get("schema_version") == 5, "unsupported run manifest schema")
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
    calibration_replicates = int(manifest["calibration_replicates"])
    calibration_duration_ms = int(manifest["calibration_duration_ms"])
    maximum_planned_measured_duration_ms = int(
        manifest["maximum_planned_measured_duration_ms"]
    )
    control_replicates = int(manifest["control_replicates"])
    injected_replicates = int(manifest["injected_replicates"])
    p50_tolerance = float(manifest["co_p50_relative_tolerance"])
    p50_max_relative_range = float(manifest["co_p50_max_relative_range"])
    p50_empirical_rule = str(manifest["co_p50_empirical_tolerance_rule"])
    p50_signed_residual_rule = str(
        manifest["co_p50_signed_residual_rule"]
    )
    measurement_order_rule = str(manifest["measurement_order_rule"])
    achieved_tolerance = float(manifest["co_achieved_target_tolerance"])
    expected_allocation = int(manifest["expected_allocation_bytes"])
    java_heap = str(manifest["java_heap"])
    java_options = [str(value) for value in manifest["java_options"]]
    inject_sleep_us_by_load = {
        int(load): int(duration)
        for load, duration in manifest["inject_sleep_us_by_load"].items()
    }
    pause_precision_entry = manifest["pause_precision"]
    pause_precision_path = root / str(pause_precision_entry["path"])
    pause_precision = load_json(pause_precision_path)
    precision_minimum_requested_us = int(
        pause_precision["selected_minimum_requested_us"]
    )
    precision_max_overshoot_ratio = float(
        pause_precision["max_overshoot_ratio"]
    )
    provenance_pairs: dict[str, set[tuple[str, str]]] = {
        language: set() for language in LANGUAGES
    }
    java_runtime_identities: set[tuple[str, str, str, tuple[str, ...]]] = set()

    a_equivalence_entry = manifest.get("a_equivalence", {})
    check(
        isinstance(a_equivalence_entry, dict),
        "A-equivalence manifest entry missing",
    )
    a_equivalence_path = root / str(
        a_equivalence_entry.get("path", "")
    )
    if a_equivalence_path.is_file():
        a_equivalence = load_json(a_equivalence_path)
        check(
            a_equivalence_entry.get("passed") is True,
            "A-equivalence manifest status failed",
        )
        check(
            int(a_equivalence_entry.get("pairs", 0))
            == int(a_equivalence.get("pairs", -1)),
            "A-equivalence manifest pair count mismatch",
        )
        check(a_equivalence.get("passed") is True, "A-equivalence gate failed")
        check(
            int(a_equivalence.get("pairs", 0)) >= 5,
            "A-equivalence has fewer than five pairs",
        )
        check(
            a_equivalence.get("baseline_revision")
            == A_EQUIVALENCE_BASELINE_REV,
            "A-equivalence baseline revision mismatch",
        )
        check(
            a_equivalence.get("current_revision") == A_EQUIVALENCE_CURRENT_REV,
            "A-equivalence current revision mismatch",
        )
        check(
            a_equivalence_entry.get("baseline_revision")
            == a_equivalence.get("baseline_revision"),
            "A-equivalence manifest baseline revision mismatch",
        )
        check(
            a_equivalence_entry.get("current_revision")
            == a_equivalence.get("current_revision"),
            "A-equivalence manifest current revision mismatch",
        )
        check(
            a_equivalence.get("measurement_apparatus_regression_only") is True,
            "A-equivalence evidence scope missing",
        )
        check(
            a_equivalence.get("portable_performance_evidence") is False,
            "A-equivalence portable-performance disclaimer missing",
        )
    else:
        errors.append("A-equivalence report missing")

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
        check(
            int(artifact.get("calibration_duration_ms", -1))
            == calibration_duration_ms,
            f"{label}: calibration-duration mismatch",
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

    check(
        pause_precision_entry.get("path") == "pause_precision_report.json",
        "pause precision report path mismatch",
    )
    check(
        pause_precision.get("schema_version") == 1,
        "unsupported pause precision schema",
    )
    check(
        pause_precision_entry.get("selected_minimum_requested_us")
        == precision_minimum_requested_us,
        "pause precision manifest selection mismatch",
    )
    check(
        float(pause_precision_entry.get("max_overshoot_ratio", -1.0))
        == precision_max_overshoot_ratio,
        "pause precision manifest overshoot ceiling mismatch",
    )
    check(
        int(pause_precision.get("replicates", 0))
        == PAUSE_PRECISION_REPLICATES,
        "pause precision replicate count mismatch",
    )
    check(
        list(pause_precision.get("candidates_us", []))
        == list(PAUSE_PRECISION_CANDIDATES_US),
        "pause precision candidate list mismatch",
    )
    check(
        float(pause_precision.get("max_overshoot_ratio", -1.0))
        == PAUSE_PRECISION_MAX_OVERSHOOT_RATIO,
        "pause precision overshoot ceiling mismatch",
    )
    check(
        float(
            pause_precision.get(
                "max_ratio_full_range_over_median",
                -1.0,
            )
        )
        == PAUSE_PRECISION_MAX_RELATIVE_RANGE,
        "pause precision stability limit mismatch",
    )
    check(
        int(pause_precision.get("rate", 0)) == PAUSE_PRECISION_RATE,
        "pause precision rate mismatch",
    )
    check(
        int(pause_precision.get("common_max", 0))
        == PAUSE_PRECISION_COMMON_MAX,
        "pause precision common max mismatch",
    )
    check(
        int(pause_precision.get("measured_events", 0))
        == PAUSE_PRECISION_MEASURED_EVENTS,
        "pause precision measured count mismatch",
    )
    check(
        int(pause_precision.get("warmup_events", 0))
        == PAUSE_PRECISION_WARMUP_EVENTS,
        "pause precision warmup count mismatch",
    )
    check(
        int(pause_precision.get("inject_at_measured_pct", 0))
        == int(manifest["inject_at_measured_pct"]),
        "pause precision injection point mismatch",
    )
    attempted_precision_candidates = [
        int(value)
        for value in pause_precision.get("attempted_candidates_us", [])
    ]
    if precision_minimum_requested_us in PAUSE_PRECISION_CANDIDATES_US:
        selected_index = PAUSE_PRECISION_CANDIDATES_US.index(
            precision_minimum_requested_us
        )
        check(
            attempted_precision_candidates
            == list(PAUSE_PRECISION_CANDIDATES_US[: selected_index + 1]),
            "pause precision did not stop at the first passing candidate",
        )
    else:
        errors.append("pause precision selected an unknown candidate")
    check(
        pause_precision.get("execution_order")
        == expected_pause_precision_order(
            attempted_precision_candidates,
            PAUSE_PRECISION_REPLICATES,
        ),
        "pause precision execution order mismatch",
    )

    precision_candidate_passes: dict[int, dict[str, bool]] = {}
    precision_results = pause_precision.get("results", {})
    check(
        isinstance(precision_results, dict)
        and set(precision_results) == set(LANGUAGES),
        "pause precision result languages mismatch",
    )
    if isinstance(precision_results, dict):
        for language in LANGUAGES:
            language_results = precision_results.get(language, {})
            check(
                isinstance(language_results, dict)
                and set(language_results)
                == {
                    str(candidate)
                    for candidate in attempted_precision_candidates
                },
                f"{language}: pause precision candidate results mismatch",
            )
    for requested_us in attempted_precision_candidates:
        precision_candidate_passes[requested_us] = {}
        for language in LANGUAGES:
            entry = precision_results.get(language, {}).get(
                str(requested_us),
                {},
            )
            expected_paths = [
                (
                    f"pause-precision-{requested_us}us-r{replicate}-"
                    f"{language}.json"
                )
                for replicate in range(1, PAUSE_PRECISION_REPLICATES + 1)
            ]
            check(
                entry.get("artifacts") == expected_paths,
                f"{language}-{requested_us}us: precision artifact list mismatch",
            )
            observed_values: list[int] = []
            artifact_validity: list[bool] = []
            for path_text in expected_paths:
                path = root / path_text
                artifact = load_json(path)
                label = path.name
                check(
                    artifact.get("language") == language,
                    f"{label}: wrong language",
                )
                check(
                    artifact.get("run_mode") == "measurement",
                    f"{label}: wrong mode",
                )
                check(
                    artifact.get("handler_mode") == "allocation-free",
                    f"{label}: precision handler mismatch",
                )
                check(
                    artifact.get("retention_window") is None,
                    f"{label}: precision retention mismatch",
                )
                validate_common_schema(
                    artifact,
                    language=language,
                    label=label,
                )
                record_provenance(
                    artifact,
                    language=language,
                    label=label,
                )
                check(
                    int(artifact.get("events_total", -1))
                    == (
                        PAUSE_PRECISION_MEASURED_EVENTS
                        + PAUSE_PRECISION_WARMUP_EVENTS
                    ),
                    f"{label}: precision event count mismatch",
                )
                check(
                    int(artifact.get("warmup_events", -1))
                    == PAUSE_PRECISION_WARMUP_EVENTS,
                    f"{label}: precision warmup mismatch",
                )
                check(
                    math.floor(float(artifact.get("common_max", 0.0)))
                    == PAUSE_PRECISION_COMMON_MAX,
                    f"{label}: precision common max mismatch",
                )
                check(
                    math.floor(float(artifact.get("own_max", 0.0)))
                    == PAUSE_PRECISION_COMMON_MAX,
                    f"{label}: precision own max mismatch",
                )
                check(
                    artifact.get("inject_sleep_us_by_load")
                    == [requested_us],
                    f"{label}: precision requested pause mismatch",
                )
                check(
                    int(artifact.get("inject_at_measured_pct", 0))
                    == int(manifest["inject_at_measured_pct"]),
                    f"{label}: precision injection point mismatch",
                )
                loads = artifact.get("loads", [])
                if len(loads) != 1:
                    errors.append(f"{label}: precision load count mismatch")
                    continue
                load = loads[0]
                check(
                    int(load.get("target_rate", 0)) == PAUSE_PRECISION_RATE,
                    f"{label}: precision target rate mismatch",
                )
                pause = load.get("pause_validation", {})
                requested_ns = int(pause.get("requested_sleep_ns", 0))
                observed_ns = int(pause.get("observed_sleep_ns", 0))
                check(
                    requested_ns == requested_us * 1_000,
                    f"{label}: precision requested duration mismatch",
                )
                check(
                    observed_ns > 0,
                    f"{label}: precision observed duration missing",
                )
                check(
                    int(pause.get("pause_completed_ns", -1))
                    - int(pause.get("pause_started_ns", 0))
                    == observed_ns,
                    f"{label}: precision observed duration mismatch",
                )
                observed_values.append(observed_ns)
                artifact_validity.append(
                    artifact.get("artifact_valid") is True
                )
                if language == "java":
                    validate_java_runtime(
                        artifact,
                        label=label,
                        expected_jfr=root / f"{path.stem}.jfr",
                        expected_gc=root / f"{path.stem}-gc.log",
                    )

            exit_statuses = entry.get("exit_statuses", [])
            check(
                isinstance(exit_statuses, list)
                and len(exit_statuses) == PAUSE_PRECISION_REPLICATES
                and all(status in (0, 3) for status in exit_statuses),
                f"{language}-{requested_us}us: precision exit statuses invalid",
            )
            if isinstance(exit_statuses, list):
                check(
                    [status == 0 for status in exit_statuses]
                    == artifact_validity,
                    f"{language}-{requested_us}us: precision status mismatch",
                )

            ratios = [
                observed / (requested_us * 1_000)
                for observed in observed_values
            ]
            ratio_median = float(median(ratios)) if ratios else math.inf
            ratio_full_range = (
                max(ratios) - min(ratios)
                if ratios
                else math.inf
            )
            relative_range = (
                ratio_full_range / ratio_median
                if ratio_median not in (0.0, math.inf)
                else math.inf
            )
            passed = (
                len(ratios) == PAUSE_PRECISION_REPLICATES
                and min(ratios, default=0.0) >= 1.0
                and max(ratios, default=math.inf)
                <= PAUSE_PRECISION_MAX_OVERSHOOT_RATIO
                and relative_range <= PAUSE_PRECISION_MAX_RELATIVE_RANGE
            )
            precision_candidate_passes[requested_us][language] = passed
            check(
                entry.get("observed_ns") == observed_values,
                f"{language}-{requested_us}us: precision observations mismatch",
            )
            check(
                entry.get("observed_full_range_ns")
                == (
                    max(observed_values) - min(observed_values)
                    if observed_values
                    else None
                ),
                f"{language}-{requested_us}us: precision observed range mismatch",
            )
            check(
                math.isclose(
                    float(entry.get("observed_median_ns", math.inf)),
                    float(median(observed_values))
                    if observed_values
                    else math.inf,
                    rel_tol=0.0,
                    abs_tol=1e-12,
                ),
                f"{language}-{requested_us}us: precision observed median mismatch",
            )
            reported_ratios = entry.get("observed_over_requested", [])
            check(
                isinstance(reported_ratios, list)
                and len(reported_ratios) == len(ratios)
                and all(
                    math.isclose(
                        float(reported),
                        expected,
                        rel_tol=0.0,
                        abs_tol=1e-12,
                    )
                    for reported, expected in zip(
                        reported_ratios,
                        ratios,
                    )
                ),
                f"{language}-{requested_us}us: precision ratios mismatch",
            )
            check(
                entry.get("passed") == passed,
                f"{language}-{requested_us}us: precision verdict mismatch",
            )
            check(
                math.isclose(
                    float(entry.get("ratio_median", math.inf)),
                    ratio_median,
                    rel_tol=0.0,
                    abs_tol=1e-12,
                ),
                f"{language}-{requested_us}us: precision median ratio mismatch",
            )
            check(
                math.isclose(
                    float(entry.get("ratio_max", math.inf)),
                    max(ratios, default=math.inf),
                    rel_tol=0.0,
                    abs_tol=1e-12,
                ),
                f"{language}-{requested_us}us: precision max ratio mismatch",
            )
            check(
                math.isclose(
                    float(entry.get("ratio_full_range", math.inf)),
                    ratio_full_range,
                    rel_tol=0.0,
                    abs_tol=1e-12,
                ),
                f"{language}-{requested_us}us: precision ratio range mismatch",
            )
            check(
                math.isclose(
                    float(
                        entry.get(
                            "ratio_full_range_over_median",
                            math.inf,
                        )
                    ),
                    relative_range,
                    rel_tol=0.0,
                    abs_tol=1e-12,
                ),
                f"{language}-{requested_us}us: precision range mismatch",
            )

    combined_precision_passes = {
        candidate: all(by_language.values())
        and set(by_language) == set(LANGUAGES)
        for candidate, by_language in precision_candidate_passes.items()
    }
    check(
        combined_precision_passes.get(precision_minimum_requested_us) is True,
        "selected pause precision candidate did not pass both runtimes",
    )
    check(
        not any(
            passed
            for candidate, passed in combined_precision_passes.items()
            if candidate < precision_minimum_requested_us
        ),
        "pause precision skipped an earlier passing candidate",
    )

    for language in LANGUAGES:
        check(
            measured_events
            == events_by_language[language] - warmup_by_language[language],
            f"manifest {language} measured event count is inconsistent",
        )
    check(measured_events >= 100_000, "fewer than 100000 measured events")
    check(
        control_replicates >= MINIMUM_CONTROL_REPLICATES,
        "fewer than three independent control replicates",
    )
    check(
        injected_replicates >= MINIMUM_INJECTED_REPLICATES,
        "fewer than three independent injected replicates",
    )
    check(
        calibration_replicates >= MINIMUM_CALIBRATION_REPLICATES,
        "fewer than three independent calibration replicates",
    )
    check(
        calibration_duration_ms >= maximum_planned_measured_duration_ms,
        "calibration window is shorter than a planned measured load",
    )
    expected_maximum_measured_ms = max(
        (
            measured_events * 1_000
            + ((common_max * load_pct + 50) // 100)
            - 1
        )
        // ((common_max * load_pct + 50) // 100)
        for load_pct in load_levels
    )
    check(
        maximum_planned_measured_duration_ms
        == expected_maximum_measured_ms,
        "maximum planned measured duration mismatch",
    )
    check(
        0.0 <= p50_max_relative_range <= 1.0,
        "invalid p50 relative-range limit",
    )
    check(
        set(inject_sleep_us_by_load) == set(load_levels),
        "per-load injection durations do not match load levels",
    )
    check(
        all(
            duration >= precision_minimum_requested_us
            for duration in inject_sleep_us_by_load.values()
        ),
        "injection duration falls below precision-qualified minimum",
    )
    check(
        float(
            pause_precision_entry.get(
                "max_ratio_full_range_over_median",
                -1.0,
            )
        )
        == PAUSE_PRECISION_MAX_RELATIVE_RANGE,
        "pause precision manifest stability limit mismatch",
    )
    for load_pct in load_levels:
        target_rate = (common_max * load_pct + 50) // 100
        worst_observed_ns = math.ceil(
            inject_sleep_us_by_load[load_pct]
            * 1_000
            * precision_max_overshoot_ratio
        )
        _, worst_affected = expected_pause_counts(
            common_max=common_max,
            target_rate=target_rate,
            observed_sleep_ns=worst_observed_ns,
        )
        check(
            worst_affected <= measured_events // 20,
            f"load {load_pct}: precision overshoot exceeds preferred affected band",
        )
    check(
        p50_empirical_rule
        == "max_control_and_injected_p50_full_range_ns",
        "unsupported empirical p50 tolerance rule",
    )
    check(
        0.0 <= p50_tolerance < p50_max_relative_range <= 1.0,
        "p50 tolerance must be strictly below the stability range limit",
    )
    check(
        control_replicates == injected_replicates,
        "control and injected replicate counts must match",
    )
    check(
        measurement_order_rule == MEASUREMENT_ORDER_RULE,
        "unsupported measurement order rule",
    )
    check(
        p50_signed_residual_rule == SIGNED_RESIDUAL_RULE,
        "unsupported signed residual rule",
    )
    check(
        measured_events >= 4 * buffer_size,
        "measured event count does not fill B-4W",
    )
    check(
        manifest.get("calibration_execution_order")
        == expected_calibration_order(calibration_replicates),
        "calibration execution order is not the frozen balanced order",
    )

    calibration_maxima: dict[str, int] = {}
    calibration_replicate_maxima: dict[str, list[int]] = {}
    for arm in ARMS:
        for language in LANGUAGES:
            key = f"{language}-{arm}"
            entry = manifest["calibrations"].get(key)
            if not isinstance(entry, dict):
                errors.append(f"missing calibration manifest entry: {key}")
                continue
            check(
                entry.get("selection_rule")
                == "minimum_of_independent_replicates",
                f"{key}: wrong calibration selection rule",
            )
            replicate_entries = entry.get("replicates", [])
            check(
                isinstance(replicate_entries, list)
                and len(replicate_entries) == calibration_replicates,
                f"{key}: calibration replicate count mismatch",
            )
            observed_maxima: list[int] = []
            for replicate_entry in replicate_entries:
                if not isinstance(replicate_entry, dict):
                    errors.append(f"{key}: malformed calibration replicate")
                    continue
                path = root / str(replicate_entry["path"])
                artifact = load_json(path)
                check(
                    artifact.get("run_mode") == "calibration",
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
                    f"{path.name}: invalid",
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
                check(
                    artifact.get("common_max") is None,
                    f"{path.name}: common_max set",
                )
                check(
                    artifact.get("loads") == [],
                    f"{path.name}: calibration has loads",
                )
                if language == "java":
                    expected_jfr = root / f"{path.stem}.jfr"
                    expected_gc = root / f"{path.stem}-gc.log"
                    validate_java_runtime(
                        artifact,
                        label=path.name,
                        expected_jfr=expected_jfr,
                        expected_gc=expected_gc,
                    )
                own_max = math.floor(float(artifact["own_max"]))
                observed_maxima.append(own_max)
                check(
                    own_max == int(replicate_entry["own_max_floor"]),
                    f"{path.name}: floored own max mismatch",
                )
            selected = min(observed_maxima) if observed_maxima else 0
            calibration_replicate_maxima[key] = observed_maxima
            calibration_maxima[key] = selected
            check(
                selected == int(entry["own_max_floor"]),
                f"{key}: selected calibration maximum mismatch",
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
    expected_order = (
        expected_measurement_order(
            control_replicates,
            injected_replicates,
        )
        if control_replicates == injected_replicates
        else None
    )
    check(
        expected_order is not None
        and manifest.get("execution_order") == expected_order,
        "execution order is not the frozen replicate-interleaved order",
    )

    artifacts: dict[tuple[str, str, str], dict[str, Any]] = {}
    control_artifacts: dict[tuple[int, str, str], dict[str, Any]] = {}
    injected_artifacts: dict[tuple[int, str, str], dict[str, Any]] = {}
    for phase in PHASES:
        replicates = (
            range(1, control_replicates + 1)
            if phase == "control"
            else range(1, injected_replicates + 1)
        )
        for replicate in replicates:
            phase_label = (
                "control"
                if phase == "control" and replicate == 1
                else (
                    f"control-r{replicate}"
                    if phase == "control"
                    else (
                        "injected"
                        if replicate == 1
                        else f"injected-r{replicate}"
                    )
                )
            )
            for arm in ARMS:
                for language in LANGUAGES:
                    label = f"{phase_label}-{language}-{arm}"
                    path = root / f"{label}.json"
                    artifact = load_json(path)
                    if phase == "control":
                        control_artifacts[(replicate, language, arm)] = artifact
                    else:
                        injected_artifacts[(replicate, language, arm)] = artifact
                    if replicate == 1:
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
                    expected_injection_us = (
                        [inject_sleep_us_by_load[load] for load in load_levels]
                        if phase == "injected"
                        else []
                    )
                    check(
                        artifact.get("inject_sleep_us_by_load")
                        == expected_injection_us,
                        f"{path.name}: per-load injection durations mismatch",
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
                                int(workload.get("allocations", -1))
                                == measured_events,
                                f"{path.name}: B allocation count mismatch",
                            )
                            check(
                                int(workload.get("retained_objects", -1))
                                == retention,
                                f"{path.name}: retained object count mismatch",
                            )
                            check(
                                int(
                                    workload.get(
                                        "estimated_logical_live_bytes", -1
                                    )
                                )
                                == retention * 32,
                                f"{path.name}: logical live bytes mismatch",
                            )
                            check(
                                int(
                                    workload.get(
                                        "observed_allocated_live_bytes", -1
                                    )
                                )
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
                            requested_sleep_ns = (
                                inject_sleep_us_by_load[load_pct] * 1_000
                            )
                            observed_sleep_ns = int(
                                pause.get("observed_sleep_ns", 0)
                            )
                            expected_backlog, expected_affected = (
                                expected_pause_counts(
                                    common_max=common_max,
                                    target_rate=target_rate,
                                    observed_sleep_ns=observed_sleep_ns,
                                )
                            )
                            check(
                                int(pause.get("requested_sleep_ns", -1))
                                == requested_sleep_ns,
                                f"{path.name}: requested pause mismatch",
                            )
                            check(
                                observed_sleep_ns > 0,
                                f"{path.name}: observed pause duration missing",
                            )
                            check(
                                int(pause.get("pause_completed_ns", -1))
                                - int(pause.get("pause_started_ns", 0))
                                == observed_sleep_ns,
                                f"{path.name}: observed pause duration mismatch",
                            )
                            check(
                                int(pause.get("expected_backlog_samples", -1))
                                == expected_backlog,
                                f"{path.name}: pause backlog mismatch",
                            )
                            check(
                                int(pause.get("expected_affected_samples", -1))
                                == expected_affected,
                                f"{path.name}: drain-amplified affected count mismatch",
                            )
                            minimum_affected = (
                                measured_events + 999
                            ) // 1_000
                            maximum_affected = measured_events // 10
                            check(
                                minimum_affected
                                <= expected_affected
                                <= maximum_affected,
                                f"{path.name}: affected population outside bounds",
                            )
                            check(
                                pause.get("affected_in_range") is True,
                                f"{path.name}: affected-range gate",
                            )
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

    p50_equivalence_results: dict[str, dict[str, Any]] = {}
    for arm in ARMS:
        for language in LANGUAGES:
            for index, load_pct in enumerate(load_levels):
                control_replicate_loads = [
                    control_artifacts.get((replicate, language, arm), {}).get(
                        "loads", []
                    )
                    for replicate in range(1, control_replicates + 1)
                ]
                injected_replicate_loads = [
                    injected_artifacts.get((replicate, language, arm), {}).get(
                        "loads", []
                    )
                    for replicate in range(1, injected_replicates + 1)
                ]
                if (
                    any(
                        index >= len(loads)
                        for loads in (
                            control_replicate_loads
                            + injected_replicate_loads
                        )
                    )
                ):
                    continue
                control_loads = [
                    loads[index] for loads in control_replicate_loads
                ]
                injected_loads = [
                    loads[index] for loads in injected_replicate_loads
                ]
                control_p50s = [
                    float(load["latency_ns"]["p50"]) for load in control_loads
                ]
                injected_p50s = [
                    float(load["latency_ns"]["p50"]) for load in injected_loads
                ]
                (
                    control_p50_median,
                    control_p50_full_range,
                    control_relative_range,
                    control_stable,
                ) = p50_stability(
                    control_p50s,
                    max_relative_range=p50_max_relative_range,
                )
                (
                    injected_p50_median,
                    injected_p50_full_range,
                    injected_relative_range,
                    injected_stable,
                ) = p50_stability(
                    injected_p50s,
                    max_relative_range=p50_max_relative_range,
                )
                allowed_p50_delta_components = {
                    "control_full_range_ns": control_p50_full_range,
                    "injected_full_range_ns": injected_p50_full_range,
                    "relative_floor_ns": p50_tolerance
                    * max(
                        abs(control_p50_median),
                        abs(injected_p50_median),
                    ),
                }
                allowed_p50_delta = max(
                    allowed_p50_delta_components.values()
                )
                allowed_p50_delta_sources = sorted(
                    name
                    for name, value in allowed_p50_delta_components.items()
                    if value == allowed_p50_delta
                )
                equivalent = (
                    control_stable
                    and injected_stable
                    and p50_equivalent(
                        control_p50_median,
                        injected_p50_median,
                        relative_tolerance=p50_tolerance,
                        control_full_range_ns=control_p50_full_range,
                        injected_full_range_ns=injected_p50_full_range,
                    )
                )
                if not control_stable and not injected_stable:
                    errors.append(
                        f"{language}-{arm}-{load_pct}: control and injected "
                        "p50 unstable; equivalence inconclusive"
                    )
                    equivalence_status = "inconclusive_both_instability"
                elif not control_stable:
                    errors.append(
                        f"{language}-{arm}-{load_pct}: control p50 unstable; "
                        "equivalence inconclusive"
                    )
                    equivalence_status = "inconclusive_control_instability"
                elif not injected_stable:
                    errors.append(
                        f"{language}-{arm}-{load_pct}: injected p50 unstable; "
                        "equivalence inconclusive"
                    )
                    equivalence_status = "inconclusive_injected_instability"
                elif not equivalent:
                    errors.append(
                        f"{language}-{arm}-{load_pct}: "
                        "injected median p50 outside tolerance"
                    )
                    equivalence_status = "fail"
                else:
                    equivalence_status = "pass"
                control_ratios = [
                    float(load["actual_target_ratio"]) for load in control_loads
                ]
                control_ratio_median = float(median(control_ratios))
                injected_ratios = [
                    float(load["actual_target_ratio"])
                    for load in injected_loads
                ]
                injected_ratio_median = float(median(injected_ratios))
                for replicate, control_ratio in enumerate(
                    control_ratios, start=1
                ):
                    check(
                        abs(control_ratio - 1.0) <= achieved_tolerance,
                        f"{language}-{arm}-{load_pct}: control-r{replicate} "
                        "achieved ratio not tight",
                    )
                for replicate, injected_ratio in enumerate(
                    injected_ratios, start=1
                ):
                    check(
                        abs(injected_ratio - 1.0) <= achieved_tolerance,
                        f"{language}-{arm}-{load_pct}: injected-r{replicate} "
                        "achieved ratio not tight",
                    )
                check(
                    injected_ratio_median + achieved_tolerance
                    >= control_ratio_median,
                    f"{language}-{arm}-{load_pct}: achieved rate materially dropped",
                )
                p50_equivalence_results[f"{language}-{arm}-{load_pct}"] = {
                    "control_p50_ns": control_p50s,
                    "control_p50_median_ns": control_p50_median,
                    "control_p50_min_ns": min(control_p50s),
                    "control_p50_max_ns": max(control_p50s),
                    "control_p50_full_range_ns": control_p50_full_range,
                    "control_p50_relative_range": control_relative_range,
                    "injected_p50_ns": injected_p50s,
                    "injected_p50_median_ns": injected_p50_median,
                    "injected_p50_min_ns": min(injected_p50s),
                    "injected_p50_max_ns": max(injected_p50s),
                    "injected_p50_full_range_ns": injected_p50_full_range,
                    "injected_p50_relative_range": injected_relative_range,
                    "p50_stability_limit": p50_max_relative_range,
                    "control_stable": control_stable,
                    "injected_stable": injected_stable,
                    "injected_minus_control_median_ns": (
                        injected_p50_median - control_p50_median
                    ),
                    "control_achieved_target_ratios": control_ratios,
                    "control_achieved_target_median": control_ratio_median,
                    "injected_achieved_target_ratios": injected_ratios,
                    "injected_achieved_target_median": (
                        injected_ratio_median
                    ),
                    "allowed_delta_ns": allowed_p50_delta,
                    "allowed_delta_components_ns": (
                        allowed_p50_delta_components
                    ),
                    "allowed_delta_sources": allowed_p50_delta_sources,
                    "equivalent": equivalent,
                    "equivalence_status": equivalence_status,
                }

    p50_signed_residual_analysis = signed_residual_analysis(
        p50_equivalence_results,
        load_levels,
    )
    cross_run_comparison = None
    if args.prior_validation_report is not None:
        prior_report_path = (
            args.prior_validation_report.expanduser().resolve()
        )
        prior_report = load_json(prior_report_path)
        prior_p50_results = prior_report.get("p50_equivalence")
        if not isinstance(prior_p50_results, dict):
            raise SystemExit(
                "prior validation report has no p50_equivalence object: "
                f"{prior_report_path}"
            )
        cross_run_comparison = cross_run_residual_comparison(
            p50_equivalence_results,
            prior_p50_results,
            current_gate_context={
                "schema_version": int(manifest["schema_version"]),
                "co_p50_relative_tolerance": p50_tolerance,
                "co_p50_max_relative_range": p50_max_relative_range,
            },
            prior_gate_context={
                "schema_version": int(prior_report["schema_version"]),
                "co_p50_relative_tolerance": float(
                    prior_report["co_p50_relative_tolerance"]
                ),
                "co_p50_max_relative_range": float(
                    prior_report["co_p50_max_relative_range"]
                ),
            },
        )
        cross_run_comparison["prior_validation_report"] = str(
            prior_report_path
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
        "schema_version": 5,
        "passed": not errors,
        "errors": errors,
        "common_max": common_max,
        "calibration_maxima": calibration_maxima,
        "calibration_replicate_maxima": calibration_replicate_maxima,
        "calibration_replicates": calibration_replicates,
        "calibration_duration_ms": calibration_duration_ms,
        "maximum_planned_measured_duration_ms": (
            maximum_planned_measured_duration_ms
        ),
        "raw_samples_validated_per_load": measured_events,
        "load_levels": load_levels,
        "control_replicates": control_replicates,
        "injected_replicates": injected_replicates,
        "co_p50_relative_tolerance": p50_tolerance,
        "co_p50_max_relative_range": p50_max_relative_range,
        "co_p50_empirical_tolerance_rule": p50_empirical_rule,
        "co_p50_signed_residual_rule": p50_signed_residual_rule,
        "measurement_order_rule": measurement_order_rule,
        "inject_sleep_us_by_load": inject_sleep_us_by_load,
        "pause_precision": {
            "selected_minimum_requested_us": precision_minimum_requested_us,
            "max_overshoot_ratio": precision_max_overshoot_ratio,
            "max_ratio_full_range_over_median": (
                PAUSE_PRECISION_MAX_RELATIVE_RANGE
            ),
            "attempted_candidates_us": attempted_precision_candidates,
            "candidate_passes": combined_precision_passes,
        },
        "p50_equivalence": p50_equivalence_results,
        "p50_signed_residual_analysis": p50_signed_residual_analysis,
        "p50_cross_run_residual_comparison": cross_run_comparison,
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
