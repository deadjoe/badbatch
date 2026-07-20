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
    cpu_affinity_mode: str
    requested_cpu_list: tuple[int, ...]
    affinity_verified_all: bool
    role_cpu_map: tuple[tuple[str, int], ...]
    round_diagnostics: bool


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
    affinity = data.get(
        "cpu_affinity",
        {
            "requested_cpu_list": [],
            "mode": "legacy-unrecorded",
            "verified_all": False,
            "role_cpu_map": {},
        },
    )
    if not isinstance(affinity, dict):
        raise ValueError(f"{path}: cpu_affinity must be an object")
    requested_cpu_list = tuple(int(cpu) for cpu in affinity.get("requested_cpu_list", []))
    role_cpu_map = tuple(
        sorted((str(role), int(cpu)) for role, cpu in affinity.get("role_cpu_map", {}).items())
    )
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
        cpu_affinity_mode=str(affinity.get("mode", "none")),
        requested_cpu_list=requested_cpu_list,
        affinity_verified_all=bool(affinity.get("verified_all", False)),
        role_cpu_map=role_cpu_map,
        round_diagnostics=bool(data.get("round_diagnostics", False)),
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
            and rust.cpu_affinity_mode == java.cpu_affinity_mode
            and rust.requested_cpu_list == java.requested_cpu_list
            and rust.affinity_verified_all == java.affinity_verified_all
            and rust.role_cpu_map == java.role_cpu_map
            and rust.round_diagnostics == java.round_diagnostics
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


def _validated_histogram(
    histogram: Any, *, path: pathlib.Path, label: str
) -> dict[str, Any]:
    if not isinstance(histogram, dict):
        raise ValueError(f"{path}: {label} must be an object")
    required = ("count", "sum", "min", "max", "mean", "log2_bins")
    missing = [key for key in required if key not in histogram]
    if missing:
        raise ValueError(f"{path}: {label} missing {', '.join(missing)}")
    bins = histogram["log2_bins"]
    if not isinstance(bins, list) or len(bins) != 64:
        raise ValueError(f"{path}: {label}.log2_bins must contain 64 counters")
    if sum(int(value) for value in bins) != int(histogram["count"]):
        raise ValueError(f"{path}: {label} bin counts do not sum to count")
    return histogram


def write_diagnostic_csvs(results_dir: pathlib.Path) -> int:
    """Validate and flatten opt-in diagnostics without aggregating away round identity."""
    artifacts: list[tuple[pathlib.Path, dict[str, Any]]] = []
    for pattern in ("rust_*.json", "java_*.json"):
        for path in sorted(results_dir.glob(pattern)):
            artifacts.append((path, json.loads(path.read_text())))
    enabled = [bool(data.get("round_diagnostics", False)) for _, data in artifacts]
    if not any(enabled):
        return 0
    if not all(enabled):
        raise ValueError("diagnostic and canonical artifacts cannot share one results directory")

    batch_rows: list[dict[str, Any]] = []
    producer_rows: list[dict[str, Any]] = []
    for path, data in artifacts:
        for round_ in data.get("rounds", []):
            diagnostics = round_.get("diagnostics")
            if not isinstance(diagnostics, dict):
                raise ValueError(f"{path}: every diagnostic round must contain diagnostics")
            consumers = diagnostics.get("batch_processing")
            if not isinstance(consumers, list) or not consumers:
                raise ValueError(f"{path}: diagnostics.batch_processing must be non-empty")
            for consumer in consumers:
                role = str(consumer.get("role", ""))
                if not role:
                    raise ValueError(f"{path}: diagnostic consumer role is missing")
                batch = _validated_histogram(
                    consumer.get("batch_size"), path=path, label=f"{role}.batch_size"
                )
                queue = _validated_histogram(
                    consumer.get("queue_depth"), path=path, label=f"{role}.queue_depth"
                )
                if int(batch["sum"]) != int(round_["events"]):
                    raise ValueError(f"{path}: {role} batch_size.sum does not equal round events")
                if int(batch["count"]) != int(queue["count"]):
                    raise ValueError(f"{path}: {role} histogram observation counts differ")
                batch_rows.append(
                    {
                        "language": data["language"],
                        "scenario": data["scenario"],
                        "pair_id": data["pair_id"],
                        "fork_index": data["fork_index"],
                        "round_index": round_["index"],
                        "phase": round_["phase"],
                        "ops_per_sec": round_["ops_per_sec"],
                        "role": role,
                        "batch_observations": batch["count"],
                        "batch_events_sum": batch["sum"],
                        "batch_min": batch["min"],
                        "batch_max": batch["max"],
                        "batch_mean": batch["mean"],
                        "batch_log2_bins": json.dumps(batch["log2_bins"], separators=(",", ":")),
                        "queue_min": queue["min"],
                        "queue_max": queue["max"],
                        "queue_mean": queue["mean"],
                        "queue_log2_bins": json.dumps(queue["log2_bins"], separators=(",", ":")),
                    }
                )
            producer = diagnostics.get("producer_backpressure")
            if not isinstance(producer, dict):
                raise ValueError(f"{path}: producer_backpressure must be an object")
            supported = bool(producer.get("supported", False))
            expected_supported = data["scenario"] != "mpsc_batch"
            if supported != expected_supported:
                raise ValueError(f"{path}: producer backpressure support does not match scenario")
            expected_action = (
                "unsupported_multi_producer"
                if not supported
                else "spin_loop"
                if data["language"] == "rust"
                else "park_nanos_1_request"
            )
            if producer.get("iteration_action") != expected_action:
                raise ValueError(f"{path}: unexpected producer backpressure iteration action")
            entries = int(producer.get("entries", 0))
            iterations = int(producer.get("wait_loop_iterations", 0))
            maximum = int(producer.get("max_wait_loop_iterations", 0))
            if min(entries, iterations, maximum) < 0:
                raise ValueError(f"{path}: producer backpressure counters must be non-negative")
            if (entries == 0) != (iterations == 0 and maximum == 0):
                raise ValueError(f"{path}: producer backpressure zero counters are inconsistent")
            if entries > 0 and (iterations < entries or maximum == 0 or maximum > iterations):
                raise ValueError(f"{path}: producer backpressure counters are inconsistent")
            producer_rows.append(
                {
                    "language": data["language"],
                    "scenario": data["scenario"],
                    "pair_id": data["pair_id"],
                    "fork_index": data["fork_index"],
                    "round_index": round_["index"],
                    "phase": round_["phase"],
                    "ops_per_sec": round_["ops_per_sec"],
                    "supported": supported,
                    "iteration_action": expected_action,
                    "entries": entries,
                    "wait_loop_iterations": iterations,
                    "max_wait_loop_iterations": maximum,
                }
            )

    for name, rows in (
        ("round_batch_diagnostics.csv", batch_rows),
        ("round_producer_backpressure.csv", producer_rows),
    ):
        with (results_dir / name).open("w", newline="") as handle:
            writer = csv.DictWriter(handle, fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)
    return len(artifacts)


def render_report(
    results_dir: pathlib.Path,
    pairs: list[Pair],
    summary: dict[str, dict[str, Any]],
    diagnostic_artifacts: int = 0,
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
            "- Per-thread CPU affinity metadata must match within every Rust/Java pair; when requested, each process hard-fails before timing unless every measured role verifies its CPU.\n",
        ]
    )
    if diagnostic_artifacts:
        lines.extend(
            [
                "- **Probe-conditioned run:** per-round handler and producer counters were enabled; throughput here is diagnostic evidence, not a canonical Rust/Java ranking.\n",
                "- Round identity is preserved in `round_batch_diagnostics.csv` and `round_producer_backpressure.csv`; log2 bin `i` covers `[2^i, 2^(i+1)-1]`.\n",
                "- Producer iteration counts describe different actions: Rust `spin_loop` calls versus Java `parkNanos(1L)` requests; compare entries and round correlation, not raw iteration counts as equal-duration work.\n",
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
    diagnostic_artifacts = write_diagnostic_csvs(args.results_dir)
    (args.results_dir / "fork_summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    report = render_report(args.results_dir, pairs, summary, diagnostic_artifacts)
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
