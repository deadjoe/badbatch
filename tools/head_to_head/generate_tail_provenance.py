#!/usr/bin/env python3
"""Generate and verify build-time provenance for the Java tail harness."""

from __future__ import annotations

import argparse
import json
import subprocess
from dataclasses import asdict, dataclass
from pathlib import Path


@dataclass(frozen=True)
class RepoState:
    root: str
    rev: str
    dirty: bool | None


def git(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(root), *args],
        check=False,
        capture_output=True,
        text=True,
    )


def repo_state(root: Path) -> RepoState:
    resolved = root.resolve()
    rev_result = git(resolved, "rev-parse", "--verify", "HEAD")
    status_result = git(
        resolved,
        "status",
        "--porcelain=v1",
        "--untracked-files=normal",
    )
    if rev_result.returncode != 0 or status_result.returncode != 0:
        return RepoState(str(resolved), "unknown", None)
    rev = rev_result.stdout.strip()
    if len(rev) != 40 or any(ch not in "0123456789abcdefABCDEF" for ch in rev):
        return RepoState(str(resolved), "unknown", None)
    return RepoState(str(resolved), rev.lower(), bool(status_result.stdout))


def dirty_literal(dirty: bool | None) -> str:
    if dirty is True:
        return "true"
    if dirty is False:
        return "false"
    return "unknown"


def java_source(badbatch: RepoState, lmax: RepoState) -> str:
    return f"""package com.lmax.disruptor.headtohead;

/** Generated build-time provenance. Do not edit. */
final class TailBuildProvenance
{{
    static final String BADBATCH_GIT_REV = "{badbatch.rev}";
    static final String BADBATCH_GIT_DIRTY = "{dirty_literal(badbatch.dirty)}";
    static final String LMAX_GIT_REV = "{lmax.rev}";
    static final String LMAX_GIT_DIRTY = "{dirty_literal(lmax.dirty)}";

    private TailBuildProvenance()
    {{
    }}
}}
"""


def manifest(badbatch: RepoState, lmax: RepoState) -> dict:
    return {
        "schema_version": 1,
        "badbatch": asdict(badbatch),
        "lmax": asdict(lmax),
    }


def generate(
    badbatch_root: Path,
    lmax_root: Path,
    output: Path,
    manifest_path: Path,
) -> None:
    badbatch = repo_state(badbatch_root)
    lmax = repo_state(lmax_root)
    output.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(java_source(badbatch, lmax), encoding="utf-8")
    manifest_path.write_text(
        json.dumps(manifest(badbatch, lmax), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def verify(manifest_path: Path) -> None:
    recorded = json.loads(manifest_path.read_text(encoding="utf-8"))
    if recorded.get("schema_version") != 1:
        raise SystemExit("unsupported provenance manifest schema")
    badbatch_root = Path(recorded["badbatch"]["root"])
    lmax_root = Path(recorded["lmax"]["root"])
    observed = manifest(repo_state(badbatch_root), repo_state(lmax_root))
    if observed != recorded:
        raise SystemExit(
            "source provenance changed during Java compilation:\n"
            f"before={json.dumps(recorded, sort_keys=True)}\n"
            f"after={json.dumps(observed, sort_keys=True)}"
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--badbatch-root", type=Path)
    parser.add_argument("--lmax-root", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()

    if args.verify:
        verify(args.manifest)
        return
    if args.badbatch_root is None or args.lmax_root is None or args.output is None:
        parser.error("generation requires --badbatch-root, --lmax-root, and --output")
    generate(args.badbatch_root, args.lmax_root, args.output, args.manifest)


if __name__ == "__main__":
    main()
