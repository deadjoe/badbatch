#!/usr/bin/env bash
# Provision the exact LMAX Disruptor source revision used by the H2H harness.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# shellcheck source=../tools/head_to_head/lmax.env
source tools/head_to_head/lmax.env

TARGET="examples/disruptor"
MODE="ensure"
if [[ "${1:-}" == "--check" ]]; then
  MODE="check"
elif [[ $# -gt 0 ]]; then
  echo "Usage: bash scripts/setup_head_to_head_lmax.sh [--check]" >&2
  exit 2
fi

check_checkout() {
  [[ -d "$TARGET/src/main/java/com/lmax/disruptor" ]] || return 1
  [[ "$(git -C "$TARGET" rev-parse HEAD 2>/dev/null)" == "$LMAX_GIT_REV" ]] || return 1
  [[ -z "$(git -C "$TARGET" status --porcelain)" ]] || return 1
}

if check_checkout; then
  echo "[INFO] LMAX Disruptor ready: $LMAX_GIT_REV"
  exit 0
fi

if [[ "$MODE" == "check" ]]; then
  echo "error: LMAX Disruptor is missing, dirty, or not pinned to $LMAX_GIT_REV" >&2
  echo "run: bash scripts/setup_head_to_head_lmax.sh" >&2
  exit 1
fi

if [[ -e "$TARGET" && ! -d "$TARGET/.git" ]]; then
  echo "error: $TARGET exists but is not a Git checkout; refusing to replace it" >&2
  exit 1
fi

if [[ ! -d "$TARGET/.git" ]]; then
  mkdir -p "$(dirname "$TARGET")"
  git clone --no-checkout "$LMAX_GIT_URL" "$TARGET"
elif [[ -n "$(git -C "$TARGET" status --porcelain)" ]]; then
  echo "error: $TARGET has local changes; refusing to overwrite them" >&2
  exit 1
fi

if ! git -C "$TARGET" cat-file -e "$LMAX_GIT_REV^{commit}" 2>/dev/null; then
  git -C "$TARGET" fetch --depth 1 "$LMAX_GIT_URL" "$LMAX_GIT_REV"
fi
git -C "$TARGET" checkout --detach "$LMAX_GIT_REV"

check_checkout || {
  echo "error: failed to provision clean LMAX revision $LMAX_GIT_REV" >&2
  exit 1
}
echo "[INFO] LMAX Disruptor ready: $LMAX_GIT_REV"
