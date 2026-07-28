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
                ),
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

    def test_pause_is_predeclared_from_calibration_without_tail_results(self) -> None:
        selected = runner.select_inject_sleep_ms(
            common_max=30_000_000,
            load_levels=[50, 70, 90],
            measured_events=1_000_000,
            requested_ms=None,
        )
        self.assertEqual(1, selected)
        self.assertEqual(
            100,
            runner.select_inject_sleep_ms(
                common_max=100_000,
                load_levels=[50, 70, 90],
                measured_events=1_000_000,
                requested_ms=None,
            ),
        )
        with self.assertRaises(SystemExit):
            runner.select_inject_sleep_ms(
                common_max=30_000_000,
                load_levels=[50, 70, 90],
                measured_events=1_000_000,
                requested_ms=50,
            )


if __name__ == "__main__":
    unittest.main()
