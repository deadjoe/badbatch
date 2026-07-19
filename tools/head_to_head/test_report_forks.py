#!/usr/bin/env python3
"""Unit tests for fork artifact validation and aggregation."""

from __future__ import annotations

import json
import pathlib
import tempfile
import unittest

import report_forks


def artifact(language: str, pair_id: str, fork_index: int, ops: float) -> dict:
    return {
        "language": language,
        "scenario": "unicast",
        "pair_id": pair_id,
        "fork_index": fork_index,
        "run_order": "rust-then-java" if fork_index == 1 else "java-then-rust",
        "wait_strategy": "yielding",
        "buffer_size": 65536,
        "events_total": 1000,
        "batch_size": 1,
        "rounds": [
            {"phase": "warmup", "ops_per_sec": ops / 2, "checksum_valid": True},
            {"phase": "measured", "ops_per_sec": ops, "checksum_valid": True},
        ],
        "summary": {"checksum_valid_all": True},
    }


class ForkReportTest(unittest.TestCase):
    def test_loads_and_summarizes_complete_pairs(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            for fork_index, (rust_ops, java_ops) in enumerate(((120.0, 100.0), (80.0, 100.0)), 1):
                pair_id = f"unicast-{fork_index:03d}"
                (root / f"rust_{pair_id}.json").write_text(
                    json.dumps(artifact("rust", pair_id, fork_index, rust_ops))
                )
                (root / f"java_{pair_id}.json").write_text(
                    json.dumps(artifact("java", pair_id, fork_index, java_ops))
                )

            pairs = report_forks.load_pairs(root)
            summary = report_forks.summarize_pairs(pairs)["unicast"]
            self.assertEqual(2, summary["pair_count"])
            self.assertEqual(100.0, summary["rust"]["median"])
            self.assertAlmostEqual(1.0, summary["paired_ratio"]["median"])
            self.assertTrue(summary["checksum_valid_all"])

    def test_rejects_multiple_measured_rounds_per_fork(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = pathlib.Path(temp) / "rust_bad.json"
            data = artifact("rust", "unicast-001", 1, 100.0)
            data["rounds"].append(
                {"phase": "measured", "ops_per_sec": 101.0, "checksum_valid": True}
            )
            path.write_text(json.dumps(data))
            with self.assertRaisesRegex(ValueError, "exactly one measured round"):
                report_forks.load_sample(path)

    def test_rejects_incomplete_pair(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            pair_id = "unicast-001"
            (root / f"rust_{pair_id}.json").write_text(
                json.dumps(artifact("rust", pair_id, 1, 100.0))
            )
            with self.assertRaisesRegex(ValueError, "incomplete pair"):
                report_forks.load_pairs(root)


if __name__ == "__main__":
    unittest.main()
