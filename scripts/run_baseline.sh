#!/usr/bin/env bash
# Capture a quick, reproducible performance baseline on the local machine.
# Prefer running on an otherwise idle system with fixed power settings.
#
# Usage:
#   ./scripts/run_baseline.sh
#   ./scripts/run_baseline.sh --full   # longer criterion sample
#
# Writes a markdown summary under docs/bench-baseline-latest.md (gitignored path
# under docs/ may require -f if committing; results are local by default).

set -euo pipefail
cd "$(dirname "$0")/.."

FULL="${1:-}"
export CARGO_TERM_COLOR=always
export RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}"

OUT_DIR="target/baseline"
mkdir -p "$OUT_DIR"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT_MD="${OUT_DIR}/baseline_${STAMP}.md"
LATEST="docs/bench-baseline-latest.md"

echo "== BadBatch baseline @ ${STAMP} =="
echo "host: $(uname -a)"
echo "rustc: $(rustc --version)"
echo "RUSTFLAGS=${RUSTFLAGS}"

{
  echo "# BadBatch performance baseline"
  echo
  echo "- UTC: \`${STAMP}\`"
  echo "- Host: \`$(uname -a)\`"
  echo "- rustc: \`$(rustc --version)\`"
  echo "- RUSTFLAGS: \`${RUSTFLAGS}\`"
  echo
  echo "## Criterion (SPSC BusySpin sample)"
  echo
  echo '```'
} >"$OUT_MD"

# Focused SPSC microbench — primary modernization metric
if [[ "$FULL" == "--full" ]]; then
  cargo bench --bench single_producer_single_consumer -- --sample-size 100 \
    BusySpin 2>&1 | tee -a "$OUT_MD" | tail -40
else
  # Quick sample for local iteration
  cargo bench --bench single_producer_single_consumer -- --sample-size 20 --warm-up-time 1 --measurement-time 2 \
    BusySpin 2>&1 | tee -a "$OUT_MD" | tail -40
fi

{
  echo '```'
  echo
  echo "## Notes"
  echo
  echo "- Prefer pinning to performance cores when comparing runs."
  echo "- Compare \`SlotPadding::None\` vs \`CacheLine128\` with the padded bench variants."
  echo "- WorkerPool / MPSC: \`cargo bench --bench multi_producer_single_consumer\`."
  echo "- This file is a local artifact; paste key numbers into git only when intentional."
} >>"$OUT_MD"

# Mirror into docs if possible (may be ignored by .gitignore)
mkdir -p docs
cp "$OUT_MD" "$LATEST" 2>/dev/null || cp "$OUT_MD" "target/baseline/bench-baseline-latest.md"

echo
echo "Wrote ${OUT_MD}"
echo "Done."
