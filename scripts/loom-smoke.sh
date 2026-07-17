#!/usr/bin/env bash
# Exhaustive concurrency smoke for WorkerPool claim (loom).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TERM_COLOR=always
# Optional: export LOOM_MAX_PREEMPTIONS=3 for faster local iteration
cargo test --test loom_work_claim --release --locked "$@"
echo "loom smoke ok"
