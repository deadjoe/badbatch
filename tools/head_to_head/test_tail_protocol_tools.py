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

    def test_measurement_plan_is_complete_and_balanced_before_execution(self) -> None:
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
                "control-r2-java-a",
                "control-r2-rust-a",
                "control-r2-rust-bw",
                "control-r2-java-bw",
                "control-r2-java-b4w",
                "control-r2-rust-b4w",
                "control-r3-rust-a",
                "control-r3-java-a",
                "control-r3-java-bw",
                "control-r3-rust-bw",
                "control-r3-rust-b4w",
                "control-r3-java-b4w",
                "injected-java-a",
                "injected-rust-a",
                "injected-rust-bw",
                "injected-java-bw",
                "injected-java-b4w",
                "injected-rust-b4w",
            ],
            [label for _, _, _, label in plan],
        )
        self.assertEqual([False] * 18 + [True] * 6, [item[0] for item in plan])
        self.assertEqual(
            [label for _, _, _, label in plan],
            validator.expected_measurement_order(3),
        )
        calibration_plan = runner.build_calibration_plan(arms)
        self.assertEqual(
            [label for _, _, _, label in calibration_plan],
            validator.expected_calibration_order(3),
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

    def test_p50_equivalence_uses_relative_or_empirical_control_range(self) -> None:
        self.assertTrue(
            validator.p50_equivalent(
                110,
                124,
                relative_tolerance=0.05,
                control_full_range_ns=16,
            )
        )
        self.assertFalse(
            validator.p50_equivalent(
                110,
                124,
                relative_tolerance=0.05,
                control_full_range_ns=4,
            )
        )
        self.assertEqual(
            (120.0, 3.0, 0.025, True),
            validator.control_p50_stability(
                [120.0, 117.0, 120.0],
                max_relative_range=0.05,
            ),
        )
        unstable = validator.control_p50_stability(
            [134.0, 119.0, 1_905.0],
            max_relative_range=0.05,
        )
        self.assertFalse(unstable[3])
        self.assertFalse(
            validator.p50_equivalent(
                1_000,
                1_100,
                relative_tolerance=0.05,
                control_full_range_ns=25,
            )
        )

    def test_pause_is_predeclared_per_load_from_total_affected_samples(self) -> None:
        selected = runner.select_inject_sleep_us_by_load(
            common_max=30_000_000,
            load_levels=[50, 70, 90],
            measured_events=1_000_000,
            requested_us_by_load=None,
        )
        self.assertEqual({50: 167, 70: 72, 90: 19}, selected)
        self.assertEqual(
            {50: 49_991, 70: 21_425, 90: 5_555},
            runner.select_inject_sleep_us_by_load(
                common_max=100_000,
                load_levels=[50, 70, 90],
                measured_events=1_000_000,
                requested_us_by_load=None,
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
            )


if __name__ == "__main__":
    unittest.main()
