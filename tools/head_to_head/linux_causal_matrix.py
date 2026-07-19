#!/usr/bin/env python3
"""Run randomized fork-level BadBatch causal matrices on pinned Linux cores."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import math
import os
import pathlib
import platform
import random
import statistics
import subprocess
import sys
from dataclasses import dataclass


@dataclass(frozen=True)
class Arm:
    label: str
    claim: str
    backoff: str
    handler: str


LOCK_ARMS = (
    Arm("locked-none", "locked", "none", "value"),
    Arm("bypass-none", "bypass-unsafe", "none", "value"),
    Arm("bypass-private-atomic", "bypass-unsafe", "private-atomic", "value"),
    Arm("bypass-spin1", "bypass-unsafe", "spin1", "value"),
    Arm("bypass-spin4", "bypass-unsafe", "spin4", "value"),
    Arm("bypass-adaptive", "bypass-unsafe", "adaptive", "value"),
    Arm("locked-adaptive", "locked", "adaptive", "value"),
)

GRADIENT_ARMS = tuple(
    Arm(f"{claim_label}-{backoff}-{handler}", claim, backoff, handler)
    for claim_label, claim in (("locked", "locked"), ("bypass", "bypass-unsafe"))
    for backoff in ("none", "adaptive")
    for handler in ("r", "w1", "w3", "sb")
)


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="microseconds")


def proc_snapshot() -> dict[str, object]:
    if pathlib.Path("/proc/loadavg").is_file():
        loadavg = pathlib.Path("/proc/loadavg").read_text().strip().split()
        loads = tuple(float(value) for value in loadavg[:3])
        runnable = loadavg[3]
        cpu = pathlib.Path("/proc/stat").read_text().splitlines()[0].split()[1:]
        values = [int(value) for value in cpu]
    else:
        loads = os.getloadavg()
        runnable = "unavailable"
        values = []
    return {
        "timestamp_utc": utc_now(),
        "loadavg_1m": loads[0],
        "loadavg_5m": loads[1],
        "loadavg_15m": loads[2],
        "runnable": runnable,
        "cpu_total_ticks": sum(values),
        "cpu_steal_ticks": values[7] if len(values) > 7 else 0,
    }


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sign_test_p(ratios: list[float]) -> float:
    wins = sum(value > 1.0 for value in ratios)
    losses = sum(value < 1.0 for value in ratios)
    n = wins + losses
    if n == 0:
        return 1.0
    tail = min(wins, losses)
    return min(1.0, 2.0 * sum(math.comb(n, k) for k in range(tail + 1)) / (2**n))


def bootstrap_median_ci(values: list[float], seed: int) -> list[float]:
    if not values:
        return [0.0, 0.0]
    rng = random.Random(seed)
    medians = [
        statistics.median(rng.choices(values, k=len(values))) for _ in range(10_000)
    ]
    medians.sort()
    return [medians[249], medians[9749]]


def paired_summary(
    samples: dict[str, dict[int, float]], numerator: str, denominator: str, seed: int
) -> dict[str, object]:
    blocks = sorted(set(samples[numerator]) & set(samples[denominator]))
    ratios = [samples[numerator][block] / samples[denominator][block] for block in blocks]
    return {
        "numerator": numerator,
        "denominator": denominator,
        "pairs": len(ratios),
        "median_ratio": statistics.median(ratios),
        "p10": sorted(ratios)[max(0, int(len(ratios) * 0.10) - 1)],
        "p90": sorted(ratios)[min(len(ratios) - 1, int(len(ratios) * 0.90))],
        "bootstrap_median_95": bootstrap_median_ci(ratios, seed),
        "numerator_wins": sum(value > 1.0 for value in ratios),
        "two_sided_sign_test_p": sign_test_p(ratios),
        "ratios": ratios,
    }


def build_contrasts(phase: str, arms: tuple[Arm, ...]) -> list[tuple[str, str]]:
    labels = {arm.label for arm in arms}
    if phase == "lock":
        return [(arm.label, "locked-none") for arm in arms if arm.label != "locked-none"]

    contrasts: list[tuple[str, str]] = []
    for claim in ("locked", "bypass"):
        for backoff in ("none", "adaptive"):
            for handler in ("w1", "w3", "sb"):
                contrasts.append(
                    (f"{claim}-{backoff}-{handler}", f"{claim}-{backoff}-r")
                )
    for claim in ("locked", "bypass"):
        for handler in ("r", "w1", "w3", "sb"):
            contrasts.append(
                (f"{claim}-adaptive-{handler}", f"{claim}-none-{handler}")
            )
    for backoff in ("none", "adaptive"):
        for handler in ("r", "w1", "w3", "sb"):
            contrasts.append(
                (f"bypass-{backoff}-{handler}", f"locked-{backoff}-{handler}")
            )
    return [pair for pair in contrasts if pair[0] in labels and pair[1] in labels]


def write_report(
    output_dir: pathlib.Path,
    phase: str,
    arm_rows: dict[str, dict[str, object]],
    contrasts: list[dict[str, object]],
) -> None:
    lines = [
        f"# Linux causal matrix: {phase}",
        "",
        "Each sample is one fresh process with one warmup and one measured round. ",
        "Every block contains every arm once in randomized order.",
        "",
        "## Arm summary",
        "",
        "| arm | samples | median Melem/s | fork CV |",
        "|---|---:|---:|---:|",
    ]
    for label, row in arm_rows.items():
        lines.append(
            f"| {label} | {row['samples']} | {row['median_melem_s']:.2f} | {row['cv']:.3f} |"
        )
    lines.extend(
        [
            "",
            "## Paired contrasts",
            "",
            "| numerator / denominator | pairs | median ratio | 95% bootstrap CI | wins | sign-test p |",
            "|---|---:|---:|---:|---:|---:|",
        ]
    )
    for row in contrasts:
        low, high = row["bootstrap_median_95"]
        lines.append(
            f"| {row['numerator']} / {row['denominator']} | {row['pairs']} | "
            f"{row['median_ratio']:.4f}x | {low:.4f}-{high:.4f} | "
            f"{row['numerator_wins']}/{row['pairs']} | {row['two_sided_sign_test_p']:.8f} |"
        )
    lines.extend(
        [
            "",
            "## Safety boundary",
            "",
            "`bypass-unsafe` is valid only because this harness owns exactly one producer handle. ",
            "It is unsound for concurrent raw Arc claim drivers and must not ship.",
            "",
        ]
    )
    (output_dir / "REPORT.md").write_text("\n".join(lines))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--phase", choices=("lock", "gradient"), required=True)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--output-dir", type=pathlib.Path, required=True)
    parser.add_argument("--forks", type=int, default=20)
    parser.add_argument("--events", type=int, default=30_000_000)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument("--seed", type=int, default=20260720)
    parser.add_argument("--cpu-list", default="2,3")
    args = parser.parse_args()

    if args.forks < 1 or args.events < 1 or args.warmup < 0:
        parser.error("forks/events must be positive and warmup non-negative")
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"binary not found: {binary}")

    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=False)
    arms = LOCK_ARMS if args.phase == "lock" else GRADIENT_ARMS
    wait = "yielding" if args.phase == "lock" else "busy-spin"
    buffer_size = 65_536 if args.phase == "lock" else 8_192
    experiment_rev = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], text=True
    ).strip()
    environment = {
        "phase": args.phase,
        "started_utc": utc_now(),
        "host": platform.uname()._asdict(),
        "experiment_rev": experiment_rev,
        "experiment_dirty": bool(
            subprocess.check_output(["git", "status", "--porcelain"], text=True).strip()
        ),
        "binary": str(binary),
        "binary_sha256": sha256(binary),
        "forks": args.forks,
        "events": args.events,
        "warmup": args.warmup,
        "seed": args.seed,
        "cpu_list": args.cpu_list,
        "wait_strategy": wait,
        "buffer_size": buffer_size,
        "arms": [arm.__dict__ for arm in arms],
    }
    (output_dir / "environment.json").write_text(json.dumps(environment, indent=2) + "\n")

    plan_rows = ["block\tposition\tarm\tclaim\tbackoff\thandler"]
    samples: dict[str, dict[int, float]] = {arm.label: {} for arm in arms}
    rng = random.Random(args.seed)
    for block in range(1, args.forks + 1):
        order = list(arms)
        rng.shuffle(order)
        for position, arm in enumerate(order, start=1):
            plan_rows.append(
                f"{block}\t{position}\t{arm.label}\t{arm.claim}\t{arm.backoff}\t{arm.handler}"
            )
            stem = f"block{block:03d}_pos{position:02d}_{arm.label}"
            result_path = output_dir / f"{stem}.json"
            stdout_path = output_dir / f"{stem}.stdout"
            command = [
                str(binary),
                "--scenario",
                "unicast",
                "--wait-strategy",
                wait,
                "--event-padding",
                "none",
                "--buffer-size",
                str(buffer_size),
                "--events-total",
                str(args.events),
                "--batch-size",
                "1",
                "--warmup-rounds",
                str(args.warmup),
                "--measured-rounds",
                "1",
                "--run-order",
                f"block-{block}-position-{position}",
                "--pair-id",
                f"{args.phase}-{block:03d}",
                "--fork-index",
                str(block),
                "--harness-rev",
                experiment_rev,
                "--implementation-rev",
                experiment_rev,
                "--harness-dirty",
                "false",
                "--implementation-dirty",
                "false",
                "--cpu-list",
                args.cpu_list,
                "--handler-mode",
                arm.handler,
                "--impl-label",
                arm.label,
                "--output",
                str(result_path),
            ]
            env = os.environ.copy()
            env["BB_H2H_CLAIM_MODE"] = arm.claim
            env["BB_H2H_PRODUCER_BACKOFF"] = arm.backoff
            before = proc_snapshot()
            completed = subprocess.run(command, env=env, text=True, capture_output=True)
            after = proc_snapshot()
            stdout_path.write_text(completed.stdout + completed.stderr)
            if completed.returncode != 0 or not result_path.is_file():
                print(f"failed arm={arm.label} block={block}: {stdout_path}", file=sys.stderr)
                return completed.returncode or 1
            result = json.loads(result_path.read_text())
            result["matrix"] = {
                "phase": args.phase,
                "block": block,
                "position": position,
                "arm": arm.__dict__,
                "command": command,
                "host_before": before,
                "host_after": after,
                "steal_delta_ticks": after["cpu_steal_ticks"] - before["cpu_steal_ticks"],
            }
            result_path.write_text(json.dumps(result, indent=2) + "\n")
            if not result["summary"]["checksum_valid_all"]:
                print(f"checksum failed: {result_path}", file=sys.stderr)
                return 2
            samples[arm.label][block] = result["summary"]["median_ops_per_sec"]
            print(
                f"[{args.phase}] block {block:02d}/{args.forks} pos {position:02d}/{len(arms)} "
                f"{arm.label}: {samples[arm.label][block] / 1e6:.2f} Melem/s",
                flush=True,
            )

    (output_dir / "fork_plan.tsv").write_text("\n".join(plan_rows) + "\n")
    arm_rows: dict[str, dict[str, object]] = {}
    for arm in arms:
        values = list(samples[arm.label].values())
        mean = statistics.mean(values)
        arm_rows[arm.label] = {
            "samples": len(values),
            "median_melem_s": statistics.median(values) / 1e6,
            "mean_melem_s": mean / 1e6,
            "cv": statistics.pstdev(values) / mean,
            "values_ops_s": values,
        }
    contrast_rows = [
        paired_summary(samples, numerator, denominator, args.seed + index)
        for index, (numerator, denominator) in enumerate(
            build_contrasts(args.phase, arms), start=1
        )
    ]
    summary = {
        "environment": environment,
        "arms": arm_rows,
        "contrasts": contrast_rows,
        "completed_utc": utc_now(),
    }
    (output_dir / "SUMMARY.json").write_text(json.dumps(summary, indent=2) + "\n")
    write_report(output_dir, args.phase, arm_rows, contrast_rows)

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
