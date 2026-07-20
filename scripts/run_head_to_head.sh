#!/usr/bin/env bash
# Fork-level head-to-head: BadBatch (Rust Builder) vs LMAX Disruptor (Java BEP).
# See tools/head_to_head/README.md for the measurement and interpretation contract.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# shellcheck source=../tools/head_to_head/lmax.env
source tools/head_to_head/lmax.env

SCENARIO="all"
MODE="quick"
ORDER="both-orders"
PADDING="none"
RESULTS_DIR=""
EVENTS_OVERRIDE=""
BUFFER_OVERRIDE=""
FORKS_OVERRIDE=""
SEED="1"
CPU_LIST=""
ROUND_DIAGNOSTICS=false

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
  --cpu-list <N,N,...>         Linux logical CPUs; pins every measured worker role
  --round-diagnostics          opt-in per-round batch/backpressure probe
  --results-dir <PATH>         must be absent or empty

Examples:
  bash scripts/run_head_to_head.sh --mode quick
  bash scripts/run_head_to_head.sh --scenario pipeline --mode full --forks 20 \
    --seed 20260719 --cpu-list 2,3,4,5
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
    --cpu-list) CPU_LIST="${2:?}"; shift 2 ;;
    --round-diagnostics) ROUND_DIAGNOSTICS=true; shift ;;
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

bash scripts/setup_head_to_head_lmax.sh --check
LMAX_HEAD="$(git -C examples/disruptor rev-parse HEAD)"
[[ "$LMAX_HEAD" == "$LMAX_GIT_REV" ]] || {
  echo "error: expected LMAX $LMAX_GIT_REV, found $LMAX_HEAD" >&2
  exit 1
}

AFFINITY_PREFIX=(env)
AFFINITY_ARGS=(--cpu-list "")
if [[ -n "$CPU_LIST" ]]; then
  command -v taskset >/dev/null || { echo "error: --cpu-list requires taskset" >&2; exit 1; }
  CPU_LIST="$(python3 - "$CPU_LIST" "$SCENARIO" <<'PY'
import sys

raw, scenario = sys.argv[1:]
try:
    cpus = [int(value) for value in raw.split(",")]
except ValueError as error:
    raise SystemExit(f"--cpu-list must contain comma-separated non-negative integers: {error}")
if not cpus or any(cpu < 0 for cpu in cpus) or len(set(cpus)) != len(cpus):
    raise SystemExit("--cpu-list must contain unique non-negative integers")
required = 4 if scenario in {"all", "mpsc_batch", "pipeline"} else 2
if len(cpus) < required:
    raise SystemExit(f"--cpu-list requires at least {required} CPUs for scenario {scenario}")
print(",".join(map(str, cpus)))
PY
)"
  taskset -c "$CPU_LIST" true
  AFFINITY_PREFIX=(taskset -c "$CPU_LIST")
  AFFINITY_ARGS=(--cpu-list "$CPU_LIST")
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
RUST_FEATURES="bench-tools"
ROUND_DIAGNOSTIC_ARG=""
if [[ "$ROUND_DIAGNOSTICS" == true ]]; then
  RUST_FEATURES="bench-tools,bench-round-diagnostics"
  ROUND_DIAGNOSTIC_ARG="--round-diagnostics"
fi
cargo build --release --features "$RUST_FEATURES" --bin h2h_rust

echo "[INFO] Compiling Java HeadToHead + LMAX sources..."
JAVA_CP="$RESULTS_DIR/java_classes"
mkdir -p "$JAVA_CP"
# Java 17+ for modern switch syntax in the harness.
JAVA_SOURCES=()
JAVA_INSTRUMENTED_SOURCE_SHA="none"
if [[ "$ROUND_DIAGNOSTICS" == true ]]; then
  JAVA_INSTRUMENTED_SOURCE="$RESULTS_DIR/java_instrumented_sources/com/lmax/disruptor/SingleProducerSequencer.java"
  python3 tools/head_to_head/instrument_lmax.py \
    examples/disruptor/src/main/java/com/lmax/disruptor/SingleProducerSequencer.java \
    "$JAVA_INSTRUMENTED_SOURCE"
  while IFS= read -r -d '' java_source; do
    JAVA_SOURCES+=("$java_source")
  done < <(find examples/disruptor/src/main/java -name '*.java' \
    ! -path '*/com/lmax/disruptor/SingleProducerSequencer.java' -print0)
  JAVA_SOURCES+=("$JAVA_INSTRUMENTED_SOURCE")
  JAVA_INSTRUMENTED_SOURCE_SHA="$(shasum -a 256 "$JAVA_INSTRUMENTED_SOURCE" | awk '{print $1}')"
else
  while IFS= read -r -d '' java_source; do
    JAVA_SOURCES+=("$java_source")
  done < <(find examples/disruptor/src/main/java -name '*.java' -print0)
fi
while IFS= read -r -d '' java_source; do
  JAVA_SOURCES+=("$java_source")
done < <(find tools/head_to_head/java -name '*.java' -print0)
javac --release 17 -d "$JAVA_CP" "${JAVA_SOURCES[@]}"

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
  echo "round_diagnostics=$ROUND_DIAGNOSTICS"
  echo "java_instrumented_single_producer_sha256=$JAVA_INSTRUMENTED_SOURCE_SHA"
  echo "rustc=$(rustc --version 2>/dev/null || true)"
  echo "cargo=$(cargo --version 2>/dev/null || true)"
  echo "java=$(java -version 2>&1 | head -1)"
  echo "javac=$(javac -version 2>&1)"
  echo "mode=$MODE order=$ORDER padding=$PADDING scenario=$SCENARIO"
  echo "seed=$SEED forks_override=${FORKS_OVERRIDE:-default}"
  echo "cpu_list=${CPU_LIST:-none}"
  echo "cpu_affinity_mode=$([[ -n "$CPU_LIST" ]] && echo per-thread-verified || echo none)"
  if [[ -r /proc/sys/kernel/nmi_watchdog ]]; then
    echo "nmi_watchdog=$(< /proc/sys/kernel/nmi_watchdog)"
    echo "perf_event_paranoid=$(< /proc/sys/kernel/perf_event_paranoid)"
    echo "clocksource=$(< /sys/devices/system/clocksource/clocksource0/current_clocksource)"
    echo "smt_control=$(< /sys/devices/system/cpu/smt/control)"
    echo "transparent_hugepage=$(< /sys/kernel/mm/transparent_hugepage/enabled)"
    echo "numa_balancing=$(< /proc/sys/kernel/numa_balancing)"
    if [[ -r /sys/devices/system/cpu/intel_pstate/no_turbo ]]; then
      echo "intel_pstate_no_turbo=$(< /sys/devices/system/cpu/intel_pstate/no_turbo)"
    fi
    echo "ext4lazyinit=$([[ -n "$(pgrep -x ext4lazyinit 2>/dev/null || true)" ]] && echo active || echo finished)"
    for cpu in ${CPU_LIST//,/ }; do
      [[ -z "$cpu" ]] && continue
      governor_path="/sys/devices/system/cpu/cpu${cpu}/cpufreq/scaling_governor"
      sibling_path="/sys/devices/system/cpu/cpu${cpu}/topology/thread_siblings_list"
      [[ -r "$governor_path" ]] && echo "cpu${cpu}_governor=$(< "$governor_path")"
      [[ -r "$sibling_path" ]] && echo "cpu${cpu}_thread_siblings=$(< "$sibling_path")"
    done
  fi
  if command -v lscpu >/dev/null; then
    echo "cpu_topology_begin"
    lscpu -e=CPU,CORE,SOCKET,NODE,ONLINE
    echo "cpu_topology_end"
  fi
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

run_fork_process() {
  local output_path="$1" stdout_path="$2"
  shift 2
  local started ended process_status
  started="$(python3 tools/head_to_head/record_fork.py snapshot)"
  if "$@" >"$stdout_path"; then
    process_status=0
  else
    process_status=$?
  fi
  ended="$(python3 tools/head_to_head/record_fork.py snapshot)"
  if [[ -f "$output_path" ]]; then
    python3 tools/head_to_head/record_fork.py annotate \
      "$output_path" "$started" "$ended" "$process_status"
  fi
  return "$process_status"
}

run_rust() {
  local scenario="$1" wait="$2" buffer="$3" events="$4" batch="$5" warmup="$6"
  local order_label="$7" fork_index="$8" pair_id="$9" stem="${10}"
  local pad_arg="$PADDING"
  [[ "$scenario" == "mpsc_batch" ]] && pad_arg="none"
  [[ -n "$EVENTS_OVERRIDE" ]] && events="$EVENTS_OVERRIDE"
  [[ -n "$BUFFER_OVERRIDE" ]] && buffer="$BUFFER_OVERRIDE"

  echo "[INFO] Rust $pair_id ($order_label wait=$wait buffer=$buffer events=$events)"
  local output_path="$RESULTS_DIR/rust_${stem}.json"
  run_fork_process "$output_path" "$RESULTS_DIR/rust_${stem}.stdout" \
    "${AFFINITY_PREFIX[@]}" ./target/release/h2h_rust \
    --scenario "$scenario" --wait-strategy "$wait" --event-padding "$pad_arg" \
    --buffer-size "$buffer" --events-total "$events" --batch-size "$batch" \
    --warmup-rounds "$warmup" --measured-rounds 1 \
    --run-order "$order_label" --pair-id "$pair_id" --fork-index "$fork_index" \
    --harness-rev "$BADBATCH_REV" --implementation-rev "$BADBATCH_REV" \
    --harness-dirty "$BADBATCH_DIRTY" --implementation-dirty "$BADBATCH_DIRTY" \
    "${AFFINITY_ARGS[@]}" \
    ${ROUND_DIAGNOSTIC_ARG:+"$ROUND_DIAGNOSTIC_ARG"} \
    --impl-label "badbatch-builder" \
    --output "$output_path"
}

run_java() {
  local scenario="$1" wait="$2" buffer="$3" events="$4" batch="$5" warmup="$6"
  local order_label="$7" fork_index="$8" pair_id="$9" stem="${10}"
  [[ -n "$EVENTS_OVERRIDE" ]] && events="$EVENTS_OVERRIDE"
  [[ -n "$BUFFER_OVERRIDE" ]] && buffer="$BUFFER_OVERRIDE"

  echo "[INFO] Java $pair_id ($order_label wait=$wait buffer=$buffer events=$events)"
  local output_path="$RESULTS_DIR/java_${stem}.json"
  run_fork_process "$output_path" "$RESULTS_DIR/java_${stem}.stdout" \
    "${AFFINITY_PREFIX[@]}" java -Xms2g -Xmx2g -XX:+AlwaysPreTouch -cp "$JAVA_CP" \
    com.lmax.disruptor.headtohead.HeadToHead \
    --scenario "$scenario" --wait-strategy "$wait" --event-padding none \
    --buffer-size "$buffer" --events-total "$events" --batch-size "$batch" \
    --warmup-rounds "$warmup" --measured-rounds 1 \
    --run-order "$order_label" --pair-id "$pair_id" --fork-index "$fork_index" \
    --harness-rev "$BADBATCH_REV" --implementation-rev "$LMAX_REV" \
    --harness-dirty "$BADBATCH_DIRTY" --implementation-dirty "$LMAX_DIRTY" \
    "${AFFINITY_ARGS[@]}" \
    ${ROUND_DIAGNOSTIC_ARG:+"$ROUND_DIAGNOSTIC_ARG"} \
    --impl-label "lmax-bep" \
    --output "$output_path"
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
