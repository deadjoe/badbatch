#!/usr/bin/env python3
"""Capture and persist orchestrator-side provenance for one benchmark process."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import time
from typing import Any


def _linux_host_snapshot() -> dict[str, Any] | None:
    proc_stat = pathlib.Path("/proc/stat")
    if not proc_stat.exists():
        return None

    result: dict[str, Any] = {"cpu_times": {}}
    cpu_fields = ["user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"]
    for line in proc_stat.read_text().splitlines():
        parts = line.split()
        if not parts:
            continue
        if parts[0] == "cpu" or (parts[0].startswith("cpu") and parts[0][3:].isdigit()):
            values = [int(value) for value in parts[1:9]]
            result["cpu_times"][parts[0]] = dict(zip(cpu_fields, values, strict=True))
        elif parts[0] in {"ctxt", "intr", "processes", "procs_running", "procs_blocked"}:
            result[parts[0]] = int(parts[1])

    interrupts_path = pathlib.Path("/proc/interrupts")
    if interrupts_path.exists():
        lines = interrupts_path.read_text().splitlines()
        cpu_headers = lines[0].split() if lines else []
        totals = [0] * len(cpu_headers)
        for line in lines[1:]:
            parts = line.split()
            if len(parts) < len(cpu_headers) + 1:
                continue
            try:
                counts = [int(value) for value in parts[1 : len(cpu_headers) + 1]]
            except ValueError:
                continue
            totals = [total + count for total, count in zip(totals, counts, strict=True)]
        result["interrupts_per_cpu"] = dict(zip(cpu_headers, totals, strict=True))

    frequencies: dict[str, int] = {}
    for path in sorted(pathlib.Path("/sys/devices/system/cpu").glob("cpu[0-9]*/cpufreq/scaling_cur_freq")):
        try:
            frequencies[path.parts[-3]] = int(path.read_text().strip())
        except (OSError, ValueError):
            continue
    result["scaling_cur_freq_khz"] = frequencies
    return result


def snapshot() -> dict[str, Any]:
    try:
        loadavg = list(os.getloadavg())
    except OSError:
        loadavg = None
    return {
        "timestamp_utc": dt.datetime.now(dt.timezone.utc)
        .isoformat(timespec="microseconds")
        .replace("+00:00", "Z"),
        "unix_ns": time.time_ns(),
        "loadavg": loadavg,
        "linux_host": _linux_host_snapshot(),
    }


def annotate(
    artifact: pathlib.Path,
    started: dict[str, Any],
    ended: dict[str, Any],
    process_exit_code: int,
) -> None:
    data = json.loads(artifact.read_text())
    start_ns = int(started["unix_ns"])
    end_ns = int(ended["unix_ns"])
    if end_ns < start_ns:
        raise ValueError("fork end timestamp precedes start timestamp")
    data["fork_provenance"] = {
        "started_at_utc": started["timestamp_utc"],
        "ended_at_utc": ended["timestamp_utc"],
        "started_unix_ns": start_ns,
        "ended_unix_ns": end_ns,
        "orchestrator_elapsed_ns": end_ns - start_ns,
        "loadavg_at_start": started["loadavg"],
        "loadavg_at_end": ended["loadavg"],
        "linux_host_at_start": started.get("linux_host"),
        "linux_host_at_end": ended.get("linux_host"),
        "process_exit_code": process_exit_code,
    }
    temporary = artifact.with_name(f".{artifact.name}.tmp")
    temporary.write_text(json.dumps(data, indent=2) + "\n")
    os.replace(temporary, artifact)


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("snapshot")
    annotate_parser = subparsers.add_parser("annotate")
    annotate_parser.add_argument("artifact", type=pathlib.Path)
    annotate_parser.add_argument("started_json")
    annotate_parser.add_argument("ended_json")
    annotate_parser.add_argument("process_exit_code", type=int)
    args = parser.parse_args()

    if args.command == "snapshot":
        print(json.dumps(snapshot(), separators=(",", ":")))
        return
    annotate(
        args.artifact,
        json.loads(args.started_json),
        json.loads(args.ended_json),
        args.process_exit_code,
    )


if __name__ == "__main__":
    main()
