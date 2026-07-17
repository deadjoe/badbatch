#!/usr/bin/env bash
# Head-to-head: BadBatch (Rust Builder) vs LMAX Disruptor (Java BEP) on one machine.
# See tools/head_to_head/README.md for methodology and how to interpret results.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

SCENARIO="all"
MODE="quick"
ORDER="rust-first"
PADDING="none"
RESULTS_DIR=""
EVENTS_OVERRIDE=""
BUFFER_OVERRIDE=""

usage() {
  cat <<'EOF'
Usage: bash scripts/run_head_to_head.sh [options]

Compare BadBatch (public Builder API) to native LMAX Disruptor (RingBuffer +
BatchEventProcessor) with matched scenarios, event counts, checksums, and
median ops/s over measured rounds.

Options:
  --scenario <all|unicast|unicast_batch|mpsc_batch|pipeline>
  --mode <quick|full>          quick ≈ 1–3M events / few rounds; full ≈ 60–100M
  --order <rust-first|java-first|both-orders>
  --event-padding <none|128>   Rust slot padding only (Java stays heap objects)
  --events-total <N>           override events for all scenarios
  --buffer-size <N>            override buffer size
  --results-dir <PATH>

Examples:
  bash scripts/run_head_to_head.sh --mode quick
  bash scripts/run_head_to_head.sh --scenario unicast_batch --mode full --order both-orders
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --scenario) SCENARIO="${2:?}"; shift 2 ;;
    --mode) MODE="${2:?}"; shift 2 ;;
    --order) ORDER="${2:?}"; shift 2 ;;
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

if [[ ! -d examples/disruptor/src/main/java/com/lmax/disruptor ]]; then
  echo "error: examples/disruptor LMAX sources not found." >&2
  echo "Clone/copy LMAX Disruptor sources into examples/disruptor (src/main/java/...)." >&2
  exit 1
fi
command -v javac >/dev/null || { echo "error: javac not found" >&2; exit 1; }
command -v java >/dev/null || { echo "error: java not found" >&2; exit 1; }
command -v cargo >/dev/null || { echo "error: cargo not found" >&2; exit 1; }
command -v python3 >/dev/null || { echo "error: python3 not found" >&2; exit 1; }

if [[ -z "$RESULTS_DIR" ]]; then
  RESULTS_DIR="head_to_head_results/$(date +%Y%m%d_%H%M%S)_$$"
fi
mkdir -p "$RESULTS_DIR"

# --- environment capture ----------------------------------------------------------
{
  echo "timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host=$(uname -a)"
  echo "git=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo "rustc=$(rustc --version 2>/dev/null || true)"
  echo "cargo=$(cargo --version 2>/dev/null || true)"
  echo "java=$(java -version 2>&1 | head -1)"
  echo "javac=$(javac -version 2>&1)"
  echo "mode=$MODE order=$ORDER padding=$PADDING scenario=$SCENARIO"
  echo "RUSTFLAGS=${RUSTFLAGS:-}"
} >"$RESULTS_DIR/environment.txt"
cp "$RESULTS_DIR/environment.txt" "$RESULTS_DIR/environment.env"

echo "[INFO] Results → $RESULTS_DIR"
echo "[INFO] Building Rust h2h_rust (release)..."
cargo build --release --bin h2h_rust

echo "[INFO] Compiling Java HeadToHead + LMAX sources..."
JAVA_CP="$RESULTS_DIR/java_classes"
rm -rf "$JAVA_CP"
mkdir -p "$JAVA_CP"
# Java 17+ for modern switch syntax in harness; LMAX sources compile with --release 17.
# shellcheck disable=SC2046
javac --release 17 -d "$JAVA_CP" \
  $(find examples/disruptor/src/main/java tools/head_to_head/java -name '*.java')

scenario_list=()
if [[ "$SCENARIO" == "all" ]]; then
  scenario_list=(unicast unicast_batch mpsc_batch pipeline)
else
  scenario_list=("$SCENARIO")
fi

# defaults: wait buffer events batch warmup measured
scenario_defaults() {
  local scenario="$1" mode="$2"
  case "$scenario" in
    unicast)
      if [[ "$mode" == "quick" ]]; then echo "yielding 65536 1000000 1 1 2"
      else echo "yielding 65536 100000000 1 3 7"; fi
      ;;
    unicast_batch)
      if [[ "$mode" == "quick" ]]; then echo "yielding 65536 1000000 10 1 2"
      else echo "yielding 65536 100000000 10 3 7"; fi
      ;;
    mpsc_batch)
      if [[ "$mode" == "quick" ]]; then echo "busy-spin 65536 3000000 10 1 2"
      else echo "busy-spin 65536 60000000 10 3 7"; fi
      ;;
    pipeline)
      if [[ "$mode" == "quick" ]]; then echo "yielding 8192 1000000 1 1 2"
      else echo "yielding 8192 100000000 1 3 7"; fi
      ;;
  esac
}

run_rust() {
  local scenario="$1" wait="$2" buffer="$3" events="$4" batch="$5" warmup="$6" measured="$7" order_label="$8"
  local out="$RESULTS_DIR/rust_${scenario}.json"
  local pad_arg="$PADDING"
  if [[ "$scenario" == "mpsc_batch" ]]; then
    pad_arg="none"
  fi
  local -a extra=()
  [[ -n "$EVENTS_OVERRIDE" ]] && events="$EVENTS_OVERRIDE"
  [[ -n "$BUFFER_OVERRIDE" ]] && buffer="$BUFFER_OVERRIDE"

  echo "[INFO] Rust  $scenario (wait=$wait buffer=$buffer events=$events batch=$batch pad=$pad_arg)"
  ./target/release/h2h_rust \
    --scenario "$scenario" \
    --wait-strategy "$wait" \
    --event-padding "$pad_arg" \
    --buffer-size "$buffer" \
    --events-total "$events" \
    --batch-size "$batch" \
    --warmup-rounds "$warmup" \
    --measured-rounds "$measured" \
    --run-order "$order_label" \
    --impl-label "badbatch-builder" \
    --output "$out" \
    >"$RESULTS_DIR/rust_${scenario}.stdout"
}

run_java() {
  local scenario="$1" wait="$2" buffer="$3" events="$4" batch="$5" warmup="$6" measured="$7" order_label="$8"
  local out="$RESULTS_DIR/java_${scenario}.json"
  [[ -n "$EVENTS_OVERRIDE" ]] && events="$EVENTS_OVERRIDE"
  [[ -n "$BUFFER_OVERRIDE" ]] && buffer="$BUFFER_OVERRIDE"

  echo "[INFO] Java  $scenario (wait=$wait buffer=$buffer events=$events batch=$batch)"
  java \
    -Xms2g -Xmx2g -XX:+AlwaysPreTouch \
    -cp "$JAVA_CP" \
    com.lmax.disruptor.headtohead.HeadToHead \
    --scenario "$scenario" \
    --wait-strategy "$wait" \
    --event-padding none \
    --buffer-size "$buffer" \
    --events-total "$events" \
    --batch-size "$batch" \
    --warmup-rounds "$warmup" \
    --measured-rounds "$measured" \
    --run-order "$order_label" \
    --impl-label "lmax-bep" \
    --output "$out" \
    >"$RESULTS_DIR/java_${scenario}.stdout"
}

orders=()
case "$ORDER" in
  rust-first) orders=("rust-then-java") ;;
  java-first) orders=("java-then-rust") ;;
  both-orders) orders=("rust-then-java" "java-then-rust") ;;
esac

for order_label in "${orders[@]}"; do
  for scenario in "${scenario_list[@]}"; do
    read -r wait buffer events batch warmup measured \
      <<<"$(scenario_defaults "$scenario" "$MODE")"

    if [[ "$order_label" == "rust-then-java" ]]; then
      run_rust "$scenario" "$wait" "$buffer" "$events" "$batch" "$warmup" "$measured" "$order_label"
      run_java "$scenario" "$wait" "$buffer" "$events" "$batch" "$warmup" "$measured" "$order_label"
    else
      run_java "$scenario" "$wait" "$buffer" "$events" "$batch" "$warmup" "$measured" "$order_label"
      run_rust "$scenario" "$wait" "$buffer" "$events" "$batch" "$warmup" "$measured" "$order_label"
    fi
  done
done

python3 - "$RESULTS_DIR" <<'PY'
import json, pathlib, sys

results_dir = pathlib.Path(sys.argv[1])
order = ["unicast", "unicast_batch", "mpsc_batch", "pipeline"]
found = {}
for rust_path in results_dir.glob("rust_*.json"):
    scenario = rust_path.stem[len("rust_"):]
    java_path = results_dir / f"java_{scenario}.json"
    if not java_path.exists():
        continue
    rust = json.loads(rust_path.read_text())
    java = json.loads(java_path.read_text())
    rm = rust["summary"]["median_ops_per_sec"]
    jm = java["summary"]["median_ops_per_sec"]
    ratio = (rm / jm) if jm else 0.0
    valid = rust["summary"]["checksum_valid_all"] and java["summary"]["checksum_valid_all"]
    rcv = rust["summary"].get("cv", 0.0)
    jcv = java["summary"].get("cv", 0.0)
    pad = rust.get("event_padding", "none")
    label = scenario if pad == "none" else f"{scenario}[pad={pad}]"
    found[scenario] = (label, rm, jm, ratio, rcv, jcv, valid,
                       rust.get("wait_strategy"), rust.get("buffer_size"), rust.get("events_total"))

rows = [found[s] for s in order if s in found]
rows += [found[s] for s in sorted(found) if s not in order]

if not rows:
    print("[WARN] No paired rust_*/java_* JSON results found.")
    sys.exit(0)

def m(x):
    return x / 1_000_000.0

print()
print("Head-to-Head Summary (median ops/s over measured rounds)")
print("scenario              rust Melem/s   java Melem/s   rust/java   rust_cv  java_cv  ok")
print("-------------------- ------------- ------------- --------- -------- -------- ---")
for label, rm, jm, ratio, rcv, jcv, valid, *_ in rows:
    print(f"{label:<20} {m(rm):>13.2f} {m(jm):>13.2f} {ratio:>9.4f} {rcv:>8.3f} {jcv:>8.3f} {str(valid):>3}")
    if jcv > 0.25 or rcv > 0.25:
        print(f"  ^ note: high variance (CV>0.25) — treat median cautiously; re-run or use busy-spin / more rounds")

md = []
md.append("# Head-to-head report\n")
env = (results_dir / "environment.txt").read_text() if (results_dir / "environment.txt").exists() else ""
md.append("## Environment\n\n```\n" + env + "```\n")
md.append("## Results\n\n")
md.append("| scenario | rust median Melem/s | java median Melem/s | rust/java | rust CV | java CV | checksum OK |\n")
md.append("|----------|--------------------:|--------------------:|----------:|--------:|--------:|:-----------:|\n")
for label, rm, jm, ratio, rcv, jcv, valid, *_ in rows:
    note = " ⚠️ high CV" if (jcv > 0.25 or rcv > 0.25) else ""
    md.append(f"| {label}{note} | {m(rm):.2f} | {m(jm):.2f} | {ratio:.4f} | {rcv:.3f} | {jcv:.3f} | {valid} |\n")

md.append("\n## How to read this\n\n")
md.append("- **Comparable:** same scenario topology, event count, batch size, wait-strategy name, ")
md.append("warmup/measured rounds, completion = all events consumed, checksum of payload arithmetic.\n")
md.append("- **Not identical runtimes:** Rust Builder API vs Java `RingBuffer`+`BatchEventProcessor`; ")
md.append("JVM heap objects vs Rust inline slots; GC vs no GC; different OS thread scheduling.\n")
md.append("- **ratio > 1** → BadBatch higher median ops/s on this machine/run; **< 1** → LMAX higher.\n")
md.append("- Prefer **CV** (coefficient of variation) small on both sides; re-run with `--order both-orders` if order effects matter.\n")
md.append("- Use `quick` for smoke; `full` for publication-scale event counts.\n")
md.append("- If CV > 0.25, the median is noisy — do not treat a single full run as definitive for that scenario.\n")

md.append("\n## Likely drivers of differences (investigate next)\n\n")
md.append("1. **Publication API cost** — single-event vs batch (`unicast` vs `unicast_batch`).\n")
md.append("2. **Multi-producer contention** — `mpsc_batch` CAS/bitmap paths.\n")
md.append("3. **Pipeline barriers** — dependent stages in `pipeline`.\n")
md.append("4. **Memory layout** — Rust padding (`--event-padding 128`) changes working set; Java side has no equivalent slot pad.\n")
md.append("5. **Wait strategy** — BusySpin vs Yielding interaction with OS scheduler.\n")

(results_dir / "REPORT.md").write_text("".join(md))
print(f"\n[INFO] Wrote {results_dir / 'REPORT.md'}")
PY

echo
echo "[INFO] Done. Artifacts in: $RESULTS_DIR"
echo "[INFO] Open $RESULTS_DIR/REPORT.md for the written summary."
