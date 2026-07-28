#!/usr/bin/env python3
"""Unit tests for Java tail-harness build provenance generation."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import generate_tail_provenance as provenance


def run(*args: str, cwd: Path) -> None:
    subprocess.run(args, cwd=cwd, check=True, capture_output=True, text=True)


def initialize_repo(root: Path) -> str:
    run("git", "init", "-q", cwd=root)
    run("git", "config", "user.name", "Tail Test", cwd=root)
    run("git", "config", "user.email", "tail-test@example.invalid", cwd=root)
    (root / "tracked.txt").write_text("a\n", encoding="utf-8")
    run("git", "add", "tracked.txt", cwd=root)
    run("git", "commit", "-qm", "initial", cwd=root)
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


class TailProvenanceTest(unittest.TestCase):
    def test_generates_clean_full_revisions_and_verifies_unchanged_state(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            badbatch = root / "badbatch"
            lmax = root / "lmax"
            badbatch.mkdir()
            lmax.mkdir()
            badbatch_rev = initialize_repo(badbatch)
            lmax_rev = initialize_repo(lmax)
            output = root / "generated" / "TailBuildProvenance.java"
            manifest = root / "generated" / "provenance.json"

            provenance.generate(badbatch, lmax, output, manifest)
            provenance.verify(manifest)

            source = output.read_text(encoding="utf-8")
            self.assertIn(badbatch_rev, source)
            self.assertIn(lmax_rev, source)
            self.assertEqual(2, source.count('GIT_DIRTY = "false"'))
            recorded = json.loads(manifest.read_text(encoding="utf-8"))
            self.assertEqual(badbatch_rev, recorded["badbatch"]["rev"])
            self.assertFalse(recorded["badbatch"]["dirty"])

    def test_detects_tracked_and_untracked_dirty_state(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            badbatch = root / "badbatch"
            lmax = root / "lmax"
            badbatch.mkdir()
            lmax.mkdir()
            initialize_repo(badbatch)
            initialize_repo(lmax)
            (badbatch / "tracked.txt").write_text("changed\n", encoding="utf-8")
            (lmax / "untracked.java").write_text("class Probe {}\n", encoding="utf-8")

            self.assertTrue(provenance.repo_state(badbatch).dirty)
            self.assertTrue(provenance.repo_state(lmax).dirty)

    def test_unknown_repository_is_explicit_and_invalidatable(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            state = provenance.repo_state(Path(temp))
            self.assertEqual("unknown", state.rev)
            self.assertIsNone(state.dirty)
            source = provenance.java_source(state, state)
            self.assertEqual(2, source.count('GIT_REV = "unknown"'))
            self.assertEqual(2, source.count('GIT_DIRTY = "unknown"'))

    def test_verify_fails_closed_when_source_state_changes(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            badbatch = root / "badbatch"
            lmax = root / "lmax"
            badbatch.mkdir()
            lmax.mkdir()
            initialize_repo(badbatch)
            initialize_repo(lmax)
            output = root / "TailBuildProvenance.java"
            manifest = root / "provenance.json"
            provenance.generate(badbatch, lmax, output, manifest)
            (badbatch / "tracked.txt").write_text("changed\n", encoding="utf-8")

            with self.assertRaises(SystemExit):
                provenance.verify(manifest)


if __name__ == "__main__":
    unittest.main()
