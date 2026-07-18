#!/usr/bin/env bash
# Post-modernization throughput baseline (macOS / Linux).
#
# Usage:
#   ./scripts/run_baseline.sh
#   ./scripts/run_baseline.sh --events 5_000_000
#   ./scripts/run_baseline.sh --full    # 3 runs + print reminder to update medians
#
# Writes:
#   benches/results/baseline_latest.md
#   benches/results/baseline_<stamp>.md
#   target/baseline/metrics_<stamp>.md

set -euo pipefail
cd "$(dirname "$0")/.."

export CARGO_TERM_COLOR=always
export RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}"

EVENTS=2000000
BUFFER=1024
FULL=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --events) EVENTS="${2//_/}"; shift 2 ;;
    --buffer) BUFFER="$2"; shift 2 ;;
    --full) FULL=1; shift ;;
    *) shift ;;
  esac
done

echo "== BadBatch baseline_metrics =="
echo "rustc: $(rustc --version)"
echo "RUSTFLAGS=${RUSTFLAGS}"
echo "events=${EVENTS} buffer=${BUFFER}"

runs=1
if [[ "$FULL" -eq 1 ]]; then
  runs=3
fi

for ((i=1; i<=runs; i++)); do
  echo
  echo "--- run ${i}/${runs} ---"
  cargo run --release --features bench-tools --bin baseline_metrics -- --events "$EVENTS" --buffer "$BUFFER"
done

echo
echo "Latest: benches/results/baseline_latest.md"
if [[ "$FULL" -eq 1 ]]; then
  echo "Update medians in benches/results/BASELINE.md if numbers moved materially."
fi
