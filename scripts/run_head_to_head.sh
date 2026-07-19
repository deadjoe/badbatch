#!/usr/bin/env bash
# Fork-level head-to-head: BadBatch (Rust Builder) vs LMAX Disruptor (Java BEP).
# See tools/head_to_head/README.md for the measurement and interpretation contract.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SCENARIO="all"
MODE="quick"
ORDER="both-orders"
PADDING="none"
RESULTS_DIR=""
EVENTS_OVERRIDE=""
BUFFER_OVERRIDE=""
FORKS_OVERRIDE=""
SEED="1"

usage() {
  cat <<'EOF'
Usage: bash scripts/run_head_to_head.sh [options]

Run paired fresh-process forks of BadBatch and LMAX Disruptor. Every fork has
in-process warmup and contributes exactly one measured sample.

Options:
  --scenario <all|unicast|unicast_batch|mpsc_batch|pipeline>
  --mode <quick|full>          quick uses 2 fork pairs; full uses 7
  --order <rust-first|java-first|both-orders>
                               both-orders balances and randomizes pair order
  --forks <N>                  override paired fork count per scenario
  --seed <N>                   reproducible order-plan seed (default 1)
  --event-padding <none|128>   Rust slot padding only
  --events-total <N>           override events for all scenarios
  --buffer-size <N>            override buffer size
  --results-dir <PATH>         must be absent or empty

Examples:
  bash scripts/run_head_to_head.sh --mode quick
  bash scripts/run_head_to_head.sh --scenario pipeline --mode full --forks 20 --seed 20260719
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --scenario) SCENARIO="${2:?}"; shift 2 ;;
    --mode) MODE="${2:?}"; shift 2 ;;
    --order) ORDER="${2:?}"; shift 2 ;;
    --forks) FORKS_OVERRIDE="${2:?}"; shift 2 ;;
    --seed) SEED="${2:?}"; shift 2 ;;
    --event-padding) PADDING="${2:?}"; shift 2 ;;
    --events-total) EVENTS_OVERRIDE="${2:?}"; shift 2 ;;
    --buffer-size) BUFFER_OVERRIDE="${2:?}"; shift 2 ;;
    --results-dir) RESULTS_DIR="${2:?}"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage >&2; exit 1 ;;
  esac
done

case "$SCENARIO" in all|unicast|unicast_batch|mpsc_batch|pipeline) ;; *)
  echo "Invalid --scenario" >&2; exit 1 ;;
esac
case "$MODE" in quick|full) ;; *) echo "Invalid --mode" >&2; exit 1 ;; esac
case "$ORDER" in rust-first|java-first|both-orders) ;; *)
  echo "Invalid --order" >&2; exit 1 ;;
esac
case "$PADDING" in none|128) ;; *) echo "Invalid --event-padding" >&2; exit 1 ;; esac
[[ "$SEED" =~ ^[0-9]+$ ]] || { echo "--seed must be a non-negative integer" >&2; exit 1; }
if [[ -n "$FORKS_OVERRIDE" ]]; then
  [[ "$FORKS_OVERRIDE" =~ ^[1-9][0-9]*$ ]] || { echo "--forks must be > 0" >&2; exit 1; }
fi

if [[ ! -d examples/disruptor/src/main/java/com/lmax/disruptor ]]; then
  echo "error: examples/disruptor LMAX sources not found." >&2
  exit 1
fi
for command in javac java cargo python3 shasum; do
  command -v "$command" >/dev/null || { echo "error: $command not found" >&2; exit 1; }
done

if [[ -z "$RESULTS_DIR" ]]; then
  RESULTS_DIR="head_to_head_results/$(date +%Y%m%d_%H%M%S)_$$"
fi
if [[ -d "$RESULTS_DIR" ]] && [[ -n "$(find "$RESULTS_DIR" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  echo "error: --results-dir must be absent or empty: $RESULTS_DIR" >&2
  exit 1
fi
mkdir -p "$RESULTS_DIR"

echo "[INFO] Results -> $RESULTS_DIR"
echo "[INFO] Building Rust h2h_rust (release)..."
cargo build --release --features bench-tools --bin h2h_rust

echo "[INFO] Compiling Java HeadToHead + LMAX sources..."
JAVA_CP="$RESULTS_DIR/java_classes"
mkdir -p "$JAVA_CP"
# Java 17+ for modern switch syntax in the harness.
# shellcheck disable=SC2046
javac --release 17 -d "$JAVA_CP" \
  $(find examples/disruptor/src/main/java tools/head_to_head/java -name '*.java')

BADBATCH_REV="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
LMAX_REV="$(git -C examples/disruptor rev-parse HEAD 2>/dev/null || echo unknown)"
if [[ -n "$(git status --porcelain 2>/dev/null)" ]]; then BADBATCH_DIRTY=true; else BADBATCH_DIRTY=false; fi
if [[ -n "$(git -C examples/disruptor status --porcelain 2>/dev/null)" ]]; then LMAX_DIRTY=true; else LMAX_DIRTY=false; fi

tracked_tree_hash() {
  python3 - "$1" <<'PY'
import hashlib, pathlib, subprocess, sys
root = pathlib.Path(sys.argv[1])
paths = subprocess.check_output(
    ["git", "-C", str(root), "ls-files", "-co", "--exclude-standard", "-z"]
).split(b"\0")
digest = hashlib.sha256()
for raw in sorted(path for path in paths if path):
    path = root / raw.decode()
    if not path.is_file():
        continue
    digest.update(raw)
    digest.update(b"\0")
    digest.update(path.read_bytes())
    digest.update(b"\0")
print(digest.hexdigest())
PY
}

directory_hash() {
  python3 - "$1" <<'PY'
import hashlib, pathlib, sys
root = pathlib.Path(sys.argv[1])
digest = hashlib.sha256()
for path in sorted(path for path in root.rglob("*") if path.is_file()):
    relative = path.relative_to(root).as_posix().encode()
    digest.update(relative)
    digest.update(b"\0")
    digest.update(path.read_bytes())
    digest.update(b"\0")
print(digest.hexdigest())
PY
}

BADBATCH_TREE_SHA="$(tracked_tree_hash "$ROOT")"
LMAX_TREE_SHA="$(tracked_tree_hash "$ROOT/examples/disruptor")"
RUST_BINARY_SHA="$(shasum -a 256 target/release/h2h_rust | awk '{print $1}')"
JAVA_CLASSES_SHA="$(directory_hash "$JAVA_CP")"

{
  echo "timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host=$(uname -a)"
  echo "badbatch_git=$BADBATCH_REV"
  echo "badbatch_dirty=$BADBATCH_DIRTY"
  echo "badbatch_tree_sha256=$BADBATCH_TREE_SHA"
  echo "lmax_git=$LMAX_REV"
  echo "lmax_dirty=$LMAX_DIRTY"
  echo "lmax_tree_sha256=$LMAX_TREE_SHA"
  echo "rust_binary_sha256=$RUST_BINARY_SHA"
  echo "java_classes_sha256=$JAVA_CLASSES_SHA"
  echo "rustc=$(rustc --version 2>/dev/null || true)"
  echo "cargo=$(cargo --version 2>/dev/null || true)"
  echo "java=$(java -version 2>&1 | head -1)"
  echo "javac=$(javac -version 2>&1)"
  echo "mode=$MODE order=$ORDER padding=$PADDING scenario=$SCENARIO"
  echo "seed=$SEED forks_override=${FORKS_OVERRIDE:-default}"
  echo "RUSTFLAGS=${RUSTFLAGS:-}"
} >"$RESULTS_DIR/environment.txt"
cp "$RESULTS_DIR/environment.txt" "$RESULTS_DIR/environment.env"

if [[ "$SCENARIO" == "all" ]]; then
  scenario_list=(unicast unicast_batch mpsc_batch pipeline)
else
  scenario_list=("$SCENARIO")
fi

# wait buffer events batch warmup fork-pairs
scenario_defaults() {
  local scenario="$1" mode="$2"
  case "$scenario" in
    unicast)
      if [[ "$mode" == "quick" ]]; then echo "yielding 65536 1000000 1 1 2"
      else echo "yielding 65536 100000000 1 3 7"; fi ;;
    unicast_batch)
      if [[ "$mode" == "quick" ]]; then echo "yielding 65536 1000000 10 1 2"
      else echo "yielding 65536 100000000 10 3 7"; fi ;;
    mpsc_batch)
      if [[ "$mode" == "quick" ]]; then echo "busy-spin 65536 3000000 10 1 2"
      else echo "busy-spin 65536 60000000 10 3 7"; fi ;;
    pipeline)
      if [[ "$mode" == "quick" ]]; then echo "yielding 8192 1000000 1 1 2"
      else echo "yielding 8192 100000000 1 3 7"; fi ;;
  esac
}

planned_orders() {
  python3 - "$ORDER" "$1" "$SEED" "$2" <<'PY'
import hashlib, random, sys
order, count, seed, scenario = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
if order == "rust-first":
    plan = ["rust-then-java"] * count
elif order == "java-first":
    plan = ["java-then-rust"] * count
else:
    plan = ["rust-then-java" if i % 2 == 0 else "java-then-rust" for i in range(count)]
    derived = int.from_bytes(hashlib.sha256(f"{seed}:{scenario}".encode()).digest()[:8], "big")
    random.Random(derived).shuffle(plan)
print("\n".join(plan))
PY
}

run_rust() {
  local scenario="$1" wait="$2" buffer="$3" events="$4" batch="$5" warmup="$6"
  local order_label="$7" fork_index="$8" pair_id="$9" stem="${10}"
  local pad_arg="$PADDING"
  [[ "$scenario" == "mpsc_batch" ]] && pad_arg="none"
  [[ -n "$EVENTS_OVERRIDE" ]] && events="$EVENTS_OVERRIDE"
  [[ -n "$BUFFER_OVERRIDE" ]] && buffer="$BUFFER_OVERRIDE"

  echo "[INFO] Rust $pair_id ($order_label wait=$wait buffer=$buffer events=$events)"
  ./target/release/h2h_rust \
    --scenario "$scenario" --wait-strategy "$wait" --event-padding "$pad_arg" \
    --buffer-size "$buffer" --events-total "$events" --batch-size "$batch" \
    --warmup-rounds "$warmup" --measured-rounds 1 \
    --run-order "$order_label" --pair-id "$pair_id" --fork-index "$fork_index" \
    --harness-rev "$BADBATCH_REV" --implementation-rev "$BADBATCH_REV" \
    --harness-dirty "$BADBATCH_DIRTY" --implementation-dirty "$BADBATCH_DIRTY" \
    --impl-label "badbatch-builder" \
    --output "$RESULTS_DIR/rust_${stem}.json" \
    >"$RESULTS_DIR/rust_${stem}.stdout"
}

run_java() {
  local scenario="$1" wait="$2" buffer="$3" events="$4" batch="$5" warmup="$6"
  local order_label="$7" fork_index="$8" pair_id="$9" stem="${10}"
  [[ -n "$EVENTS_OVERRIDE" ]] && events="$EVENTS_OVERRIDE"
  [[ -n "$BUFFER_OVERRIDE" ]] && buffer="$BUFFER_OVERRIDE"

  echo "[INFO] Java $pair_id ($order_label wait=$wait buffer=$buffer events=$events)"
  java -Xms2g -Xmx2g -XX:+AlwaysPreTouch -cp "$JAVA_CP" \
    com.lmax.disruptor.headtohead.HeadToHead \
    --scenario "$scenario" --wait-strategy "$wait" --event-padding none \
    --buffer-size "$buffer" --events-total "$events" --batch-size "$batch" \
    --warmup-rounds "$warmup" --measured-rounds 1 \
    --run-order "$order_label" --pair-id "$pair_id" --fork-index "$fork_index" \
    --harness-rev "$BADBATCH_REV" --implementation-rev "$LMAX_REV" \
    --harness-dirty "$BADBATCH_DIRTY" --implementation-dirty "$LMAX_DIRTY" \
    --impl-label "lmax-bep" \
    --output "$RESULTS_DIR/java_${stem}.json" \
    >"$RESULTS_DIR/java_${stem}.stdout"
}

echo -e "scenario\tpair_id\tfork_index\trun_order" >"$RESULTS_DIR/fork_plan.tsv"
for scenario in "${scenario_list[@]}"; do
  read -r wait buffer events batch warmup forks <<<"$(scenario_defaults "$scenario" "$MODE")"
  [[ -n "$FORKS_OVERRIDE" ]] && forks="$FORKS_OVERRIDE"
  fork_index=0
  while IFS= read -r order_label; do
    fork_index=$((fork_index + 1))
    fork_tag="$(printf '%03d' "$fork_index")"
    pair_id="${scenario}-${fork_tag}"
    order_slug="${order_label//-/_}"
    stem="${scenario}_fork${fork_tag}_${order_slug}"
    printf '%s\t%s\t%s\t%s\n' "$scenario" "$pair_id" "$fork_index" "$order_label" \
      >>"$RESULTS_DIR/fork_plan.tsv"
    if [[ "$order_label" == "rust-then-java" ]]; then
      run_rust "$scenario" "$wait" "$buffer" "$events" "$batch" "$warmup" "$order_label" "$fork_index" "$pair_id" "$stem"
      run_java "$scenario" "$wait" "$buffer" "$events" "$batch" "$warmup" "$order_label" "$fork_index" "$pair_id" "$stem"
    else
      run_java "$scenario" "$wait" "$buffer" "$events" "$batch" "$warmup" "$order_label" "$fork_index" "$pair_id" "$stem"
      run_rust "$scenario" "$wait" "$buffer" "$events" "$batch" "$warmup" "$order_label" "$fork_index" "$pair_id" "$stem"
    fi
  done < <(planned_orders "$forks" "$scenario")
done

python3 tools/head_to_head/report_forks.py "$RESULTS_DIR"

echo
echo "[INFO] Done. Artifacts in: $RESULTS_DIR"
echo "[INFO] Report: $RESULTS_DIR/REPORT.md"
