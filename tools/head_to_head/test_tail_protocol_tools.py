#!/usr/bin/env python3
"""Unit tests for tail-protocol runner and validator helpers."""

from __future__ import annotations

import sys
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_tail_latency as runner
import validate_tail_a_equivalence as a_equivalence
import validate_tail_latency as validator


class TailProtocolToolsTest(unittest.TestCase):
    def test_arm_arguments_keep_allocation_choice_outside_the_hot_path(self) -> None:
        self.assertEqual(
            ["--handler-mode", "allocation-free"],
            runner.arm_arguments(runner.Arm("a", "allocation-free", None)),
        )
        self.assertEqual(
            ["--handler-mode", "allocating", "--retention-window", "262144"],
            runner.arm_arguments(runner.Arm("b4w", "allocating", 262_144)),
        )

    def test_language_warmups_are_separate_with_one_measured_count(self) -> None:
        args = Namespace(
            measured_events=100_000,
            rust_warmup_events=10_000,
            java_warmup_events=50_000,
            wait_strategy="busy-spin",
            buffer_size=65_536,
            cpu_list="",
        )
        rust = runner.common_arguments(args, "rust")
        java = runner.common_arguments(args, "java")
        self.assertEqual(
            "110000", rust[rust.index("--events-total") + 1]
        )
        self.assertEqual(
            "150000", java[java.index("--events-total") + 1]
        )
        self.assertEqual(
            "10000", rust[rust.index("--warmup-events") + 1]
        )
        self.assertEqual(
            "50000", java[java.index("--warmup-events") + 1]
        )

    def test_javap_method_extraction_keeps_code_but_not_neighbor_methods(self) -> None:
        output = """class Probe {
  public void onEvent(Probe$Event, long, boolean);
    Code:
       0: new #1
       3: return

  public void neighbor();
    Code:
       0: return
}
"""
        extracted = runner.method_code(
            output, "public void onEvent(Probe$Event, long, boolean);"
        )
        self.assertIn("0: new #1", extracted)
        self.assertNotIn("neighbor", extracted)

    def test_jfr_event_without_stack_trace_is_not_an_allocation_match(self) -> None:
        self.assertEqual(
            [],
            runner.frame_type_names({"values": {"stackTrace": None}}),
        )

    def test_measurement_plan_interleaves_paired_phases_before_execution(
        self,
    ) -> None:
        arms = [
            runner.Arm("a", "allocation-free", None),
            runner.Arm("bw", "allocating", 65_536),
            runner.Arm("b4w", "allocating", 262_144),
        ]
        plan = runner.build_measurement_plan(arms)
        self.assertEqual(
            [
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
                "control-r2-java-a",
                "control-r2-rust-a",
                "control-r2-rust-bw",
                "control-r2-java-bw",
                "control-r2-java-b4w",
                "control-r2-rust-b4w",
                "injected-r2-rust-a",
                "injected-r2-java-a",
                "injected-r2-java-bw",
                "injected-r2-rust-bw",
                "injected-r2-rust-b4w",
                "injected-r2-java-b4w",
                "control-r3-rust-a",
                "control-r3-java-a",
                "control-r3-java-bw",
                "control-r3-rust-bw",
                "control-r3-rust-b4w",
                "control-r3-java-b4w",
                "injected-r3-java-a",
                "injected-r3-rust-a",
                "injected-r3-rust-bw",
                "injected-r3-java-bw",
                "injected-r3-java-b4w",
                "injected-r3-rust-b4w",
            ],
            [label for _, _, _, label in plan],
        )
        self.assertEqual(
            ([False] * 6 + [True] * 6) * 3,
            [item[0] for item in plan],
        )
        self.assertEqual(
            [label for _, _, _, label in plan],
            validator.expected_measurement_order(3, 3),
        )
        with self.assertRaisesRegex(ValueError, "counts must match"):
            runner.build_measurement_plan(
                arms,
                control_replicates=3,
                injected_replicates=4,
            )
        with self.assertRaisesRegex(ValueError, "counts must match"):
            validator.expected_measurement_order(3, 4)
        calibration_plan = runner.build_calibration_plan(arms)
        self.assertEqual(
            [label for _, _, _, label in calibration_plan],
            validator.expected_calibration_order(3),
        )
        self.assertEqual(
            [
                "pause-precision-10us-r1-rust",
                "pause-precision-10us-r1-java",
                "pause-precision-10us-r2-java",
                "pause-precision-10us-r2-rust",
            ],
            validator.expected_pause_precision_order([10], 2),
        )

    def test_raw_validator_checks_schedule_and_complete_row_count(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "samples.csv"
            path.write_text(
                validator.RAW_HEADER
                + "\n"
                + "2,20000000,20000007,7\n"
                + "3,30000000,30000011,11\n",
                encoding="utf-8",
            )
            self.assertEqual(
                [],
                validator.validate_raw(
                    path,
                    warmup_events=2,
                    measured_events=2,
                    target_rate=100,
                    expected_latency={
                        "count": 2,
                        "p50": 7,
                        "p99": 11,
                        "p99.9": 11,
                        "p99.99": 11,
                        "max": 11,
                    },
                ),
            )
            summary_errors = validator.validate_raw(
                path,
                warmup_events=2,
                measured_events=2,
                target_rate=100,
                expected_latency={
                    "count": 2,
                    "p50": 8,
                    "p99": 11,
                    "p99.9": 11,
                    "p99.99": 11,
                    "max": 11,
                },
            )
            self.assertTrue(
                any("summary p50" in error for error in summary_errors),
                summary_errors,
            )

            path.write_text(
                validator.RAW_HEADER + "\n2,20000001,20000007,6\n",
                encoding="utf-8",
            )
            errors = validator.validate_raw(
                path,
                warmup_events=2,
                measured_events=2,
                target_rate=100,
            )
            self.assertTrue(any("planned" in error for error in errors), errors)
            self.assertTrue(any("row count" in error for error in errors), errors)

    def test_retention_and_relative_difference_are_explicit(self) -> None:
        self.assertIsNone(validator.expected_retention("a", 65_536))
        self.assertEqual(65_536, validator.expected_retention("bw", 65_536))
        self.assertEqual(262_144, validator.expected_retention("b4w", 65_536))
        self.assertEqual(0.0, validator.relative_difference(0.0, 0.0))
        self.assertLess(validator.relative_difference(48.0, 48.0), 0.05)
        self.assertGreater(validator.relative_difference(48.0, 40.0), 0.05)

    def test_a_equivalence_uses_observed_range_overlap(self) -> None:
        self.assertTrue(
            a_equivalence.ranges_overlap(
                [2_500.0, 5_000.0, 9_875.0],
                [4_709.0, 5_000.0, 7_958.0],
            )
        )
        self.assertFalse(
            a_equivalence.ranges_overlap(
                [1.0, 2.0],
                [3.0, 4.0],
            )
        )

    def test_p50_empirical_range_is_reachable_after_stability(self) -> None:
        defaults = runner.parser().parse_args(
            [
                "--results-dir",
                "/tmp/results",
                "--a-equivalence-dir",
                "/tmp/a-equivalence",
            ]
        )
        self.assertEqual(0.025, defaults.co_p50_relative_tolerance)
        self.assertEqual(0.05, defaults.co_p50_max_relative_range)
        self.assertLess(
            defaults.co_p50_relative_tolerance,
            defaults.co_p50_max_relative_range,
        )
        self.assertTrue(
            validator.p50_equivalent(
                100,
                103,
                relative_tolerance=0.025,
                control_full_range_ns=4,
                injected_full_range_ns=2,
            )
        )
        self.assertTrue(
            validator.p50_equivalent(
                100,
                103,
                relative_tolerance=0.025,
                control_full_range_ns=2,
                injected_full_range_ns=4,
            )
        )
        self.assertFalse(
            validator.p50_equivalent(
                100,
                103,
                relative_tolerance=0.025,
                control_full_range_ns=2,
                injected_full_range_ns=2,
            )
        )
        self.assertEqual(
            (100.0, 4.0, 0.04, True),
            validator.p50_stability(
                [98.0, 100.0, 102.0],
                max_relative_range=0.05,
            ),
        )
        unstable = validator.p50_stability(
            [134.0, 119.0, 1_905.0],
            max_relative_range=0.05,
        )
        self.assertFalse(unstable[3])
        self.assertFalse(
            validator.p50_equivalent(
                1_000,
                1_100,
                relative_tolerance=0.025,
                control_full_range_ns=25,
                injected_full_range_ns=25,
            )
        )

    def test_signed_residual_analysis_surfaces_pattern_without_failing_cells(
        self,
    ) -> None:
        self.assertAlmostEqual(
            0.0001220703125,
            validator.exact_two_sided_sign_test(14, 0),
        )
        results = {
            f"rust-a-{load}": {
                "injected_minus_control_median_ns": delta,
                "equivalence_status": "pass",
            }
            for load, delta in ((50, 1.0), (70, 2.0), (90, 3.0))
        }
        analysis = validator.signed_residual_analysis(
            results,
            [50, 70, 90],
        )
        self.assertEqual(3, analysis["all_cells"]["positive"])
        self.assertEqual("decided_cells", analysis["primary_population"])
        self.assertEqual(3, analysis["primary_summary"]["positive"])
        self.assertEqual(
            3,
            analysis["language_summaries"]["rust"]["all_cells"]["positive"],
        )
        self.assertEqual(
            3,
            analysis["arm_summaries"]["a"]["all_cells"]["positive"],
        )
        self.assertEqual(
            3,
            analysis["language_arm_summaries"]["rust-a"]["all_cells"][
                "positive"
            ],
        )
        self.assertEqual(
            ["rust-a"],
            analysis["load_monotonic_nonconstant_groups"],
        )
        self.assertEqual(
            1.0 / 3.0,
            analysis["load_monotonic_chance_baseline"][
                "per_group_probability"
            ],
        )
        self.assertEqual(
            2.0,
            analysis["load_monotonic_chance_baseline"][
                "expected_flagged_groups"
            ],
        )
        self.assertTrue(analysis["requires_residual_observation"])
        self.assertIn(
            "load_monotonic_nonconstant_groups",
            analysis["observations"],
        )
        comparison = validator.cross_run_residual_comparison(
            {
                "java-b4w-70": {
                    "injected_minus_control_median_ns": 9.0,
                    "equivalence_status": "inconclusive_injected_instability",
                },
                "java-a-90": {
                    "injected_minus_control_median_ns": -1.0,
                    "equivalence_status": "inconclusive_injected_instability",
                },
                "java-a-50": {
                    "injected_minus_control_median_ns": 2.0,
                    "equivalence_status": "pass",
                },
                "java-bw-90": {
                    "injected_minus_control_median_ns": 20.0,
                    "equivalence_status": "inconclusive_both_instability",
                },
                "rust-a-50": {
                    "injected_minus_control_median_ns": 0.0,
                    "equivalence_status": "pass",
                },
            },
            {
                "java-b4w-70": {
                    "injected_minus_control_median_ns": 9.0,
                    "equivalence_status": "fail",
                },
                "java-a-90": {
                    "injected_minus_control_median_ns": 3.0,
                    "equivalence_status": "inconclusive_injected_instability",
                },
                "java-a-50": {
                    "injected_minus_control_median_ns": 4.0,
                    "equivalence_status": "pass",
                },
                "java-bw-90": {
                    "injected_minus_control_median_ns": 5_925.0,
                    "equivalence_status": "inconclusive_both_instability",
                },
                "rust-a-50": {
                    "injected_minus_control_median_ns": 0.0,
                    "equivalence_status": "pass",
                },
            },
            current_gate_context={
                "schema_version": 5,
                "co_p50_relative_tolerance": 0.025,
                "co_p50_max_relative_range": 0.05,
            },
            prior_gate_context={
                "schema_version": 4,
                "co_p50_relative_tolerance": 0.05,
                "co_p50_max_relative_range": 0.05,
            },
        )
        self.assertEqual(
            ["java-b4w-70"],
            comparison[
                "current_inconclusive_exact_nonzero_delta_reproductions"
            ],
        )
        self.assertEqual(
            ["java-a-50", "java-b4w-70"],
            comparison["comparable_magnitude_residual_reproductions"],
        )
        self.assertEqual(
            ["java-b4w-70"],
            comparison[
                "current_inconclusive_comparable_magnitude_reproductions"
            ],
        )
        self.assertEqual(
            ["java-a-50", "java-b4w-70", "java-bw-90"],
            comparison["same_nonzero_direction_cells"],
        )
        self.assertNotIn(
            "rust-a-50",
            comparison["exact_nonzero_delta_reproductions"],
        )
        self.assertIn("quantization", comparison["resolution_limit"])
        self.assertFalse(comparison["gate_context"]["matches"])
        self.assertIn(
            "cannot be attributed directly",
            comparison["gate_context"]["status_attribution"],
        )
        self.assertIn(
            "gate-independent",
            comparison["gate_context"]["signed_delta_attribution"],
        )

    def test_pause_is_predeclared_per_load_from_total_affected_samples(self) -> None:
        selected = runner.select_inject_sleep_us_by_load(
            common_max=30_000_000,
            load_levels=[50, 70, 90],
            measured_events=1_000_000,
            requested_us_by_load=None,
            minimum_requested_us=1,
            max_overshoot_ratio=1.0,
        )
        self.assertEqual({50: 1_666, 70: 714, 90: 185}, selected)
        self.assertEqual(
            {50: 500_000, 70: 214_285, 90: 55_555},
            runner.select_inject_sleep_us_by_load(
                common_max=100_000,
                load_levels=[50, 70, 90],
                measured_events=1_000_000,
                requested_us_by_load=None,
                minimum_requested_us=1,
                max_overshoot_ratio=1.0,
            ),
        )
        self.assertEqual(
            5_130,
            runner.expected_affected_samples(
                common_max=10_000_000,
                target_rate=9_000_000,
                pause_us=57,
            ),
        )
        self.assertEqual(
            (513, 5_130),
            validator.expected_pause_counts(
                common_max=10_000_000,
                target_rate=9_000_000,
                observed_sleep_ns=57_000,
            ),
        )
        with self.assertRaises(SystemExit):
            runner.select_inject_sleep_us_by_load(
                common_max=30_000_000,
                load_levels=[50, 70, 90],
                measured_events=1_000_000,
                requested_us_by_load={50: 1, 70: 1, 90: 1},
                minimum_requested_us=1,
                max_overshoot_ratio=1.0,
            )

    def test_pause_precision_preflight_sets_an_independent_lower_bound(self) -> None:
        unstable = runner.summarize_pause_precision(
            [350_209, 389_625, 356_750, 368_291, 360_334],
            requested_us=200,
            max_overshoot_ratio=1.75,
            max_relative_range=0.10,
        )
        self.assertFalse(unstable["passed"])
        stable = runner.summarize_pause_precision(
            [808_417, 803_125, 804_209, 808_666, 819_708],
            requested_us=500,
            max_overshoot_ratio=1.75,
            max_relative_range=0.10,
        )
        self.assertTrue(stable["passed"])
        with self.assertRaises(SystemExit):
            runner.select_inject_sleep_us_by_load(
                common_max=12_619_000,
                load_levels=[50, 70, 90],
                measured_events=262_144,
                requested_us_by_load=None,
                minimum_requested_us=200,
                max_overshoot_ratio=1.75,
            )
        selected = runner.select_inject_sleep_us_by_load(
            common_max=12_619_000,
            load_levels=[50, 70, 90],
            measured_events=1_048_576,
            requested_us_by_load=None,
            minimum_requested_us=200,
            max_overshoot_ratio=1.75,
        )
        self.assertGreaterEqual(min(selected.values()), 200)
        with self.assertRaises(SystemExit):
            runner.select_inject_sleep_us_by_load(
                common_max=12_619_000,
                load_levels=[50, 70, 90],
                measured_events=1_048_576,
                requested_us_by_load=None,
                minimum_requested_us=500,
                max_overshoot_ratio=1.75,
            )
        selected = runner.select_inject_sleep_us_by_load(
            common_max=12_619_000,
            load_levels=[50, 70, 90],
            measured_events=2_097_152,
            requested_us_by_load=None,
            minimum_requested_us=500,
            max_overshoot_ratio=1.75,
        )
        self.assertGreaterEqual(min(selected.values()), 500)


if __name__ == "__main__":
    unittest.main()
