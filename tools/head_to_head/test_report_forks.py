#!/usr/bin/env python3
"""Unit tests for fork artifact validation and aggregation."""

from __future__ import annotations

import json
import pathlib
import tempfile
import unittest

import record_fork
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
        "cpu_affinity": {
            "requested_cpu_list": [2, 3],
            "mode": "per-thread",
            "verified_all": True,
            "role_cpu_map": {"publisher": 2, "consumer": 3},
        },
        "rounds": [
            {"phase": "warmup", "ops_per_sec": ops / 2, "checksum_valid": True},
            {"phase": "measured", "ops_per_sec": ops, "checksum_valid": True},
        ],
        "summary": {"checksum_valid_all": True},
    }


class ForkReportTest(unittest.TestCase):
    def test_records_fork_timestamps_load_and_exit_status(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = pathlib.Path(temp) / "rust_unicast-001.json"
            path.write_text(json.dumps(artifact("rust", "unicast-001", 1, 100.0)))
            started = {
                "timestamp_utc": "2026-07-19T01:02:03.000000Z",
                "unix_ns": 100,
                "loadavg": [1.0, 2.0, 3.0],
                "linux_host": {"procs_running": 1, "cpu_times": {"cpu": {"steal": 0}}},
            }
            ended = {
                "timestamp_utc": "2026-07-19T01:02:04.000000Z",
                "unix_ns": 250,
                "loadavg": [1.5, 2.5, 3.5],
                "linux_host": {"procs_running": 2, "cpu_times": {"cpu": {"steal": 0}}},
            }

            record_fork.annotate(path, started, ended, 0)

            provenance = json.loads(path.read_text())["fork_provenance"]
            self.assertEqual(150, provenance["orchestrator_elapsed_ns"])
            self.assertEqual([1.0, 2.0, 3.0], provenance["loadavg_at_start"])
            self.assertEqual([1.5, 2.5, 3.5], provenance["loadavg_at_end"])
            self.assertEqual(1, provenance["linux_host_at_start"]["procs_running"])
            self.assertEqual(2, provenance["linux_host_at_end"]["procs_running"])
            self.assertEqual(0, provenance["process_exit_code"])

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

    def test_rejects_affinity_mismatch_within_pair(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            pair_id = "unicast-001"
            rust = artifact("rust", pair_id, 1, 100.0)
            java = artifact("java", pair_id, 1, 100.0)
            java["cpu_affinity"]["role_cpu_map"]["consumer"] = 4
            (root / "rust_unicast-001.json").write_text(json.dumps(rust))
            (root / "java_unicast-001.json").write_text(json.dumps(java))
            with self.assertRaisesRegex(ValueError, "metadata mismatch"):
                report_forks.load_pairs(root)

    def test_loads_legacy_artifact_without_affinity_metadata(self) -> None:
        data = artifact("rust", "unicast-001", 1, 100.0)
        del data["cpu_affinity"]
        with tempfile.TemporaryDirectory() as temp:
            path = pathlib.Path(temp) / "rust_unicast-001.json"
            path.write_text(json.dumps(data))
            sample = report_forks.load_sample(path)
            self.assertEqual("legacy-unrecorded", sample.cpu_affinity_mode)
            self.assertFalse(sample.affinity_verified_all)


if __name__ == "__main__":
    unittest.main()
