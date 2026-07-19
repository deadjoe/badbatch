#!/usr/bin/env python3
"""Validate paired fork artifacts and render the head-to-head report."""

from __future__ import annotations

import argparse
import csv
import json
import pathlib
import statistics
from dataclasses import asdict, dataclass
from typing import Any


SCENARIO_ORDER = ["unicast", "unicast_batch", "mpsc_batch", "pipeline"]


@dataclass(frozen=True)
class Sample:
    language: str
    scenario: str
    pair_id: str
    fork_index: int
    run_order: str
    ops_per_sec: float
    checksum_valid: bool
    path: str
    wait_strategy: str
    buffer_size: int
    events_total: int
    batch_size: int


@dataclass(frozen=True)
class Pair:
    scenario: str
    pair_id: str
    fork_index: int
    run_order: str
    rust_ops_per_sec: float
    java_ops_per_sec: float
    checksum_valid: bool

    @property
    def ratio(self) -> float:
        return self.rust_ops_per_sec / self.java_ops_per_sec

    @property
    def difference(self) -> float:
        return self.rust_ops_per_sec - self.java_ops_per_sec


def _one_measured_round(data: dict[str, Any], path: pathlib.Path) -> dict[str, Any]:
    measured = [round_ for round_ in data.get("rounds", []) if round_.get("phase") == "measured"]
    if len(measured) != 1:
        raise ValueError(
            f"{path}: fork artifact must contain exactly one measured round, got {len(measured)}"
        )
    return measured[0]


def load_sample(path: pathlib.Path) -> Sample:
    data = json.loads(path.read_text())
    measured = _one_measured_round(data, path)
    required = ["language", "scenario", "pair_id", "fork_index", "run_order"]
    missing = [key for key in required if key not in data]
    if missing:
        raise ValueError(f"{path}: missing fork metadata: {', '.join(missing)}")
    return Sample(
        language=str(data["language"]),
        scenario=str(data["scenario"]),
        pair_id=str(data["pair_id"]),
        fork_index=int(data["fork_index"]),
        run_order=str(data["run_order"]),
        ops_per_sec=float(measured["ops_per_sec"]),
        checksum_valid=bool(measured["checksum_valid"])
        and bool(data.get("summary", {}).get("checksum_valid_all", False)),
        path=str(path),
        wait_strategy=str(data["wait_strategy"]),
        buffer_size=int(data["buffer_size"]),
        events_total=int(data["events_total"]),
        batch_size=int(data["batch_size"]),
    )


def load_pairs(results_dir: pathlib.Path) -> list[Pair]:
    samples = [
        load_sample(path)
        for pattern in ("rust_*.json", "java_*.json")
        for path in results_dir.glob(pattern)
    ]
    if not samples:
        raise ValueError(f"{results_dir}: no rust_*.json/java_*.json fork artifacts")

    grouped: dict[tuple[str, str], dict[str, Sample]] = {}
    for sample in samples:
        if sample.language not in {"rust", "java"}:
            raise ValueError(f"{sample.path}: unsupported language {sample.language!r}")
        key = (sample.scenario, sample.pair_id)
        by_language = grouped.setdefault(key, {})
        if sample.language in by_language:
            raise ValueError(
                f"duplicate {sample.language} artifact for {sample.scenario}/{sample.pair_id}"
            )
        by_language[sample.language] = sample

    pairs: list[Pair] = []
    for (scenario, pair_id), by_language in grouped.items():
        if set(by_language) != {"rust", "java"}:
            raise ValueError(f"incomplete pair {scenario}/{pair_id}: found {sorted(by_language)}")
        rust = by_language["rust"]
        java = by_language["java"]
        comparable = (
            rust.fork_index == java.fork_index
            and rust.run_order == java.run_order
            and rust.wait_strategy == java.wait_strategy
            and rust.buffer_size == java.buffer_size
            and rust.events_total == java.events_total
            and rust.batch_size == java.batch_size
        )
        if not comparable:
            raise ValueError(f"metadata mismatch in pair {scenario}/{pair_id}")
        pairs.append(
            Pair(
                scenario=scenario,
                pair_id=pair_id,
                fork_index=rust.fork_index,
                run_order=rust.run_order,
                rust_ops_per_sec=rust.ops_per_sec,
                java_ops_per_sec=java.ops_per_sec,
                checksum_valid=rust.checksum_valid and java.checksum_valid,
            )
        )
    return sorted(pairs, key=lambda pair: (SCENARIO_ORDER.index(pair.scenario), pair.fork_index))


def _stats(values: list[float]) -> dict[str, float]:
    mean = statistics.fmean(values)
    stddev = statistics.pstdev(values)
    return {
        "median": statistics.median(values),
        "mean": mean,
        "min": min(values),
        "max": max(values),
        "stddev": stddev,
        "cv": stddev / mean if mean else 0.0,
    }


def summarize_pairs(pairs: list[Pair]) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    scenarios = sorted({pair.scenario for pair in pairs}, key=SCENARIO_ORDER.index)
    for scenario in scenarios:
        subset = [pair for pair in pairs if pair.scenario == scenario]
        rust = _stats([pair.rust_ops_per_sec for pair in subset])
        java = _stats([pair.java_ops_per_sec for pair in subset])
        ratios = _stats([pair.ratio for pair in subset])
        differences = _stats([pair.difference for pair in subset])
        result[scenario] = {
            "pair_count": len(subset),
            "checksum_valid_all": all(pair.checksum_valid for pair in subset),
            "rust": rust,
            "java": java,
            "paired_ratio": ratios,
            "paired_difference": differences,
        }
    return result


def write_csv(results_dir: pathlib.Path, pairs: list[Pair]) -> None:
    path = results_dir / "fork_samples.csv"
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(
            handle,
            fieldnames=[
                "scenario",
                "pair_id",
                "fork_index",
                "run_order",
                "rust_ops_per_sec",
                "java_ops_per_sec",
                "rust_java_ratio",
                "rust_java_difference",
                "checksum_valid",
            ],
        )
        writer.writeheader()
        for pair in pairs:
            row = asdict(pair)
            row["rust_java_ratio"] = pair.ratio
            row["rust_java_difference"] = pair.difference
            writer.writerow(row)


def render_report(
    results_dir: pathlib.Path,
    pairs: list[Pair],
    summary: dict[str, dict[str, Any]],
) -> str:
    environment_path = results_dir / "environment.txt"
    environment = environment_path.read_text() if environment_path.exists() else "missing"
    lines = [
        "# Fork-level head-to-head report\n",
        "## Environment\n\n",
        f"```text\n{environment.rstrip()}\n```\n\n",
        "## Fork summary\n\n",
        "| scenario | pairs | rust median Melem/s | java median Melem/s | median paired ratio | rust fork CV | java fork CV | checksum |\n",
        "|---|---:|---:|---:|---:|---:|---:|:---:|\n",
    ]
    for scenario, item in summary.items():
        lines.append(
            f"| {scenario} | {item['pair_count']} | "
            f"{item['rust']['median'] / 1_000_000:.2f} | "
            f"{item['java']['median'] / 1_000_000:.2f} | "
            f"{item['paired_ratio']['median']:.4f} | "
            f"{item['rust']['cv']:.3f} | {item['java']['cv']:.3f} | "
            f"{item['checksum_valid_all']} |\n"
        )

    lines.extend(
        [
            "\n## Paired fork samples\n\n",
            "| scenario | pair | order | rust Melem/s | java Melem/s | ratio | difference Melem/s | checksum |\n",
            "|---|---|---|---:|---:|---:|---:|:---:|\n",
        ]
    )
    for pair in pairs:
        lines.append(
            f"| {pair.scenario} | {pair.pair_id} | {pair.run_order} | "
            f"{pair.rust_ops_per_sec / 1_000_000:.2f} | "
            f"{pair.java_ops_per_sec / 1_000_000:.2f} | {pair.ratio:.4f} | "
            f"{pair.difference / 1_000_000:.2f} | {pair.checksum_valid} |\n"
        )

    minimum_pairs = min(item["pair_count"] for item in summary.values())
    lines.extend(
        [
            "\n## Interpretation boundary\n\n",
            "- Each row above is one pair of fresh Rust/Java processes; each process contributes exactly one measured sample after in-process warmup.\n",
            "- Fork-level CV describes dispersion across processes. It does not establish unimodality or a portable ranking.\n",
            "- Inspect `fork_samples.csv` or the raw JSON scatter before interpreting the aggregate medians.\n",
            "- No automatic fast/slow-mode threshold is applied by this report.\n",
        ]
    )
    if minimum_pairs < 20:
        lines.append(
            f"- **Exploratory only:** the smallest scenario has {minimum_pairs} pairs; use at least 20 paired forks for a serious modal-frequency/ranking study.\n"
        )
    return "".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("results_dir", type=pathlib.Path)
    args = parser.parse_args()
    pairs = load_pairs(args.results_dir)
    summary = summarize_pairs(pairs)
    write_csv(args.results_dir, pairs)
    (args.results_dir / "fork_summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    report = render_report(args.results_dir, pairs, summary)
    (args.results_dir / "REPORT.md").write_text(report)

    print("Fork-level Head-to-Head Summary")
    for scenario, item in summary.items():
        print(
            f"{scenario:<16} pairs={item['pair_count']:>3} "
            f"rust={item['rust']['median'] / 1_000_000:>8.2f} "
            f"java={item['java']['median'] / 1_000_000:>8.2f} "
            f"paired-ratio={item['paired_ratio']['median']:.4f} "
            f"ok={item['checksum_valid_all']}"
        )


if __name__ == "__main__":
    main()
