#!/usr/bin/env bash
# Build and run the frozen six-arm Rust/Java open-loop tail-latency protocol.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 "$ROOT/tools/head_to_head/run_tail_latency.py" "$@"
