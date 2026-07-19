#!/usr/bin/env python3
"""Capture repeatable perf stat and perf c2c evidence for representative matrix arms."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import pathlib
import random
import statistics
import subprocess
import sys
from dataclasses import dataclass


@dataclass(frozen=True)
class Arm:
    label: str
    claim: str
    handler: str


ARMS = (
    Arm("locked-r", "locked", "r"),
    Arm("bypass-r", "bypass-unsafe", "r"),
    Arm("locked-w3", "locked", "w3"),
    Arm("bypass-w3", "bypass-unsafe", "w3"),
    Arm("locked-sb", "locked", "sb"),
)

EVENT_GROUPS = {
    "execution": ("cycles", "instructions", "branches", "branch-misses"),
    "cache": ("cache-references", "cache-misses", "l1d.replacement", "l2_rqsts.all_rfo"),
    "retired_memory": (
        "mem_inst_retired.all_loads",
        "mem_inst_retired.all_stores",
        "mem_load_retired.l1_miss",
        "mem_load_retired.l2_miss",
    ),
    "coherence": (
        "mem_load_retired.l3_miss",
        "mem_load_l3_hit_retired.xsnp_hitm",
        "l2_rqsts.rfo_miss",
        "offcore_response.demand_rfo.l3_hit.snoop_hitm",
    ),
    "scheduler": ("context-switches", "cpu-migrations", "page-faults", "task-clock"),
}


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="microseconds")


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def h2h_command(
    binary: pathlib.Path,
    output: pathlib.Path,
    arm: Arm,
    events: int,
    warmup: int,
    cpu_list: str,
    run_order: str,
) -> list[str]:
    return [
        str(binary),
        "--scenario",
        "unicast",
        "--wait-strategy",
        "busy-spin",
        "--event-padding",
        "none",
        "--buffer-size",
        "8192",
        "--events-total",
        str(events),
        "--batch-size",
        "1",
        "--warmup-rounds",
        str(warmup),
        "--measured-rounds",
        "1",
        "--run-order",
        run_order,
        "--pair-id",
        run_order,
        "--fork-index",
        "1",
        "--cpu-list",
        cpu_list,
        "--handler-mode",
        arm.handler,
        "--impl-label",
        arm.label,
        "--output",
        str(output),
    ]


def arm_env(arm: Arm) -> dict[str, str]:
    env = os.environ.copy()
    env["BB_H2H_CLAIM_MODE"] = arm.claim
    env["BB_H2H_PRODUCER_BACKOFF"] = "none"
    return env


def parse_perf_stat(path: pathlib.Path) -> dict[str, dict[str, object]]:
    result: dict[str, dict[str, object]] = {}
    for line in path.read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) < 3:
            continue
        raw_value, unit, event = fields[:3]
        if raw_value.startswith("<"):
            result[event] = {"value": None, "unit": unit, "raw": line}
            continue
        try:
            value = float(raw_value)
        except ValueError:
            continue
        result[event] = {"value": value, "unit": unit, "raw": line}
    return result


def capture_stat(
    binary: pathlib.Path,
    output_dir: pathlib.Path,
    events_total: int,
    warmup: int,
    cpu_list: str,
    repeats: int,
    seed: int,
) -> dict[str, object]:
    rng = random.Random(seed)
    observations: dict[str, dict[str, list[float]]] = {
        arm.label: {} for arm in ARMS
    }
    for group_name, perf_events in EVENT_GROUPS.items():
        for repeat in range(1, repeats + 1):
            order = list(ARMS)
            rng.shuffle(order)
            for position, arm in enumerate(order, start=1):
                stem = f"stat_{group_name}_rep{repeat:02d}_pos{position:02d}_{arm.label}"
                stat_path = output_dir / f"{stem}.tsv"
                result_path = output_dir / f"{stem}.json"
                stdout_path = output_dir / f"{stem}.stdout"
                command = h2h_command(
                    binary,
                    result_path,
                    arm,
                    events_total,
                    warmup,
                    cpu_list,
                    stem,
                )
                perf_command = [
                    "perf",
                    "stat",
                    "-x",
                    "\t",
                    "-o",
                    str(stat_path),
                    "-e",
                    ",".join(perf_events),
                    "--",
                    *command,
                ]
                completed = subprocess.run(
                    perf_command, env=arm_env(arm), text=True, capture_output=True
                )
                stdout_path.write_text(completed.stdout + completed.stderr)
                if completed.returncode != 0 or not result_path.is_file():
                    raise RuntimeError(f"perf stat failed: {stdout_path}")
                result = json.loads(result_path.read_text())
                if not result["summary"]["checksum_valid_all"]:
                    raise RuntimeError(f"checksum failed: {result_path}")
                parsed = parse_perf_stat(stat_path)
                missing = [event for event in perf_events if parsed.get(event, {}).get("value") is None]
                if missing:
                    raise RuntimeError(f"unsupported events {missing}: {stat_path}")
                for event in perf_events:
                    observations[arm.label].setdefault(event, []).append(
                        float(parsed[event]["value"])
                    )
                print(
                    f"[stat] {group_name} rep {repeat}/{repeats} pos {position}/{len(ARMS)} "
                    f"{arm.label}: {result['summary']['median_ops_per_sec'] / 1e6:.2f} Melem/s",
                    flush=True,
                )

    operations = events_total * (warmup + 1)
    arm_summary: dict[str, object] = {}
    for arm in ARMS:
        medians = {
            event: statistics.median(values)
            for event, values in observations[arm.label].items()
        }
        arm_summary[arm.label] = {
            "median_raw": medians,
            "median_per_event": {
                event: value / operations
                for event, value in medians.items()
                if event != "task-clock"
            },
            "ipc": medians["instructions"] / medians["cycles"],
            "raw_observations": observations[arm.label],
        }
    return {
        "events_total_per_round": events_total,
        "warmup_rounds": warmup,
        "operations_observed_per_process": operations,
        "repeats": repeats,
        "event_groups": EVENT_GROUPS,
        "arms": arm_summary,
    }


def capture_c2c(
    binary: pathlib.Path,
    output_dir: pathlib.Path,
    events_total: int,
    cpu_list: str,
) -> dict[str, object]:
    result: dict[str, object] = {}
    for arm in ARMS:
        stem = f"c2c_{arm.label}"
        data_path = output_dir / f"{stem}.data"
        result_path = output_dir / f"{stem}.json"
        record_stdout = output_dir / f"{stem}.record.stdout"
        command = h2h_command(
            binary,
            result_path,
            arm,
            events_total,
            0,
            cpu_list,
            stem,
        )
        record = subprocess.run(
            ["perf", "c2c", "record", "-o", str(data_path), "--", *command],
            env=arm_env(arm),
            text=True,
            capture_output=True,
        )
        record_stdout.write_text(record.stdout + record.stderr)
        if record.returncode != 0 or not data_path.is_file():
            raise RuntimeError(f"perf c2c record failed: {record_stdout}")
        captured = json.loads(result_path.read_text())
        if not captured["summary"]["checksum_valid_all"]:
            raise RuntimeError(f"checksum failed: {result_path}")
        stats = subprocess.run(
            ["perf", "c2c", "report", "--stats", "-i", str(data_path)],
            text=True,
            capture_output=True,
        )
        full = subprocess.run(
            [
                "perf",
                "c2c",
                "report",
                "--stdio",
                "--show-all",
                "--full-symbols",
                "--no-source",
                "-g",
                "none",
                "-i",
                str(data_path),
            ],
            text=True,
            capture_output=True,
        )
        (output_dir / f"{stem}.stats.txt").write_text(stats.stdout + stats.stderr)
        (output_dir / f"{stem}.report.txt").write_text(full.stdout + full.stderr)
        if stats.returncode != 0 or full.returncode != 0:
            raise RuntimeError(f"perf c2c report failed for {arm.label}")
        result[arm.label] = {
            "throughput_melem_s": captured["summary"]["median_ops_per_sec"] / 1e6,
            "perf_data_sha256": sha256(data_path),
            "perf_data_bytes": data_path.stat().st_size,
        }
        print(
            f"[c2c] {arm.label}: {result[arm.label]['throughput_melem_s']:.2f} Melem/s",
            flush=True,
        )
    return result


def write_report(output_dir: pathlib.Path, stat: dict[str, object], c2c: dict[str, object]) -> None:
    lines = [
        "# Linux PMU and perf c2c capture",
        "",
        "## PMU medians",
        "",
        "Each cell except IPC is the median hardware count per published event across repeats.",
        "",
        "| arm | cycles/event | instructions/event | IPC | stores/event | L1 misses/event | L2 misses/event | L3 misses/event | load HITM/event | RFO HITM/event |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for label, row in stat["arms"].items():
        per_event = row["median_per_event"]
        lines.append(
            f"| {label} | {per_event['cycles']:.3f} | {per_event['instructions']:.3f} | "
            f"{row['ipc']:.3f} | {per_event['mem_inst_retired.all_stores']:.3f} | "
            f"{per_event['mem_load_retired.l1_miss']:.6f} | "
            f"{per_event['mem_load_retired.l2_miss']:.6f} | "
            f"{per_event['mem_load_retired.l3_miss']:.6f} | "
            f"{per_event['mem_load_l3_hit_retired.xsnp_hitm']:.6f} | "
            f"{per_event['offcore_response.demand_rfo.l3_hit.snoop_hitm']:.6f} |"
        )
    lines.extend(
        [
            "",
            "## c2c captures",
            "",
            "| arm | throughput Melem/s | perf.data bytes |",
            "|---|---:|---:|",
        ]
    )
    for label, row in c2c.items():
        lines.append(
            f"| {label} | {row['throughput_melem_s']:.2f} | {row['perf_data_bytes']} |"
        )
    lines.extend(
        [
            "",
            "Full cache-line tables are in `c2c_*.report.txt`; summary counters are in "
            "`c2c_*.stats.txt`; raw samples remain in `c2c_*.data`.",
            "",
        ]
    )
    (output_dir / "REPORT.md").write_text("\n".join(lines))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--output-dir", type=pathlib.Path, required=True)
    parser.add_argument("--events", type=int, default=30_000_000)
    parser.add_argument("--c2c-events", type=int, default=50_000_000)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--seed", type=int, default=20260722)
    parser.add_argument("--cpu-list", default="2,3")
    args = parser.parse_args()
    binary = args.binary.resolve()
    output_dir = args.output_dir.resolve()
    if not binary.is_file():
        parser.error(f"binary not found: {binary}")
    output_dir.mkdir(parents=True, exist_ok=False)
    metadata = {
        "started_utc": utc_now(),
        "binary": str(binary),
        "binary_sha256": sha256(binary),
        "perf_version": subprocess.check_output(["perf", "--version"], text=True).strip(),
        "events": args.events,
        "c2c_events": args.c2c_events,
        "warmup": args.warmup,
        "repeats": args.repeats,
        "seed": args.seed,
        "cpu_list": args.cpu_list,
        "arms": [arm.__dict__ for arm in ARMS],
    }
    (output_dir / "environment.json").write_text(json.dumps(metadata, indent=2) + "\n")
    try:
        stat = capture_stat(
            binary,
            output_dir,
            args.events,
            args.warmup,
            args.cpu_list,
            args.repeats,
            args.seed,
        )
        c2c = capture_c2c(binary, output_dir, args.c2c_events, args.cpu_list)
    except RuntimeError as error:
        print(str(error), file=sys.stderr)
        return 1
    summary = {"metadata": metadata, "perf_stat": stat, "perf_c2c": c2c, "completed_utc": utc_now()}
    (output_dir / "SUMMARY.json").write_text(json.dumps(summary, indent=2) + "\n")
    write_report(output_dir, stat, c2c)
    checksum_paths = sorted(
        path for path in output_dir.iterdir() if path.is_file() and path.name != "SHA256SUMS"
    )
    (output_dir / "SHA256SUMS").write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in checksum_paths)
    )
    print(f"complete: {output_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
