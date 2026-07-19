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
