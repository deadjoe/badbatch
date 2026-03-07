#!/usr/bin/env bash
set -euo pipefail

SCENARIO="all"
MODE="full"
ORDER="rust-first"
RESULTS_DIR=""

usage() {
    cat <<'EOF'
Usage: bash scripts/run_head_to_head.sh [options]

Options:
  --scenario <all|unicast|unicast_batch|mpsc_batch|pipeline>
  --mode <full|quick>
  --order <rust-first|java-first>
  --results-dir <PATH>
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --scenario)
            SCENARIO="${2:?missing value for --scenario}"
            shift 2
            ;;
        --mode)
            MODE="${2:?missing value for --mode}"
            shift 2
            ;;
        --order)
            ORDER="${2:?missing value for --order}"
            shift 2
            ;;
        --results-dir)
            RESULTS_DIR="${2:?missing value for --results-dir}"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unsupported argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ "$SCENARIO" != "all" && "$SCENARIO" != "unicast" && "$SCENARIO" != "unicast_batch" && "$SCENARIO" != "mpsc_batch" && "$SCENARIO" != "pipeline" ]]; then
    echo "Unsupported scenario: $SCENARIO" >&2
    exit 1
fi

if [[ "$MODE" != "full" && "$MODE" != "quick" ]]; then
    echo "Unsupported mode: $MODE" >&2
    exit 1
fi

if [[ "$ORDER" != "rust-first" && "$ORDER" != "java-first" ]]; then
    echo "Unsupported order: $ORDER" >&2
    exit 1
fi

if [[ -z "$RESULTS_DIR" ]]; then
    RESULTS_DIR="head_to_head_results/$(date +%Y%m%d_%H%M%S)"
fi

mkdir -p "$RESULTS_DIR"

echo "[INFO] Building Rust head-to-head harness..."
cargo build --release --bin h2h_rust

echo "[INFO] Building Java head-to-head harness..."
JAVA_MODE="javac-classes"
JAVA_CP="$RESULTS_DIR/java_classes"
rm -rf "$JAVA_CP"
mkdir -p "$JAVA_CP"
javac --release 11 -d "$JAVA_CP" $(find examples/disruptor/src/main/java tools/head_to_head/java -name '*.java' | tr '\n' ' ')

case "$ORDER" in
    rust-first)
        RUN_ORDER_VALUE="rust-then-java"
        ;;
    java-first)
        RUN_ORDER_VALUE="java-then-rust"
        ;;
esac

scenario_list=()
if [[ "$SCENARIO" == "all" ]]; then
    scenario_list=("unicast" "unicast_batch" "mpsc_batch" "pipeline")
else
    scenario_list=("$SCENARIO")
fi

scenario_defaults() {
    local scenario="$1"
    local mode="$2"

    case "$scenario" in
        unicast)
            if [[ "$mode" == "quick" ]]; then
                echo "yielding 65536 1000000 1 1 2"
            else
                echo "yielding 65536 100000000 1 3 7"
            fi
            ;;
        unicast_batch)
            if [[ "$mode" == "quick" ]]; then
                echo "yielding 65536 1000000 10 1 2"
            else
                echo "yielding 65536 100000000 10 3 7"
            fi
            ;;
        mpsc_batch)
            if [[ "$mode" == "quick" ]]; then
                echo "busy-spin 65536 3000000 10 1 2"
            else
                echo "busy-spin 65536 60000000 10 3 7"
            fi
            ;;
        pipeline)
            if [[ "$mode" == "quick" ]]; then
                echo "yielding 8192 1000000 1 1 2"
            else
                echo "yielding 8192 100000000 1 3 7"
            fi
            ;;
        *)
            echo "Unknown scenario: $scenario" >&2
            exit 1
            ;;
    esac
}

run_rust() {
    local scenario="$1"
    local wait_strategy="$2"
    local buffer_size="$3"
    local events_total="$4"
    local batch_size="$5"
    local warmup_rounds="$6"
    local measured_rounds="$7"
    local output_path="$RESULTS_DIR/rust_${scenario}.json"

    ./target/release/h2h_rust \
        --scenario "$scenario" \
        --wait-strategy "$wait_strategy" \
        --buffer-size "$buffer_size" \
        --events-total "$events_total" \
        --batch-size "$batch_size" \
        --warmup-rounds "$warmup_rounds" \
        --measured-rounds "$measured_rounds" \
        --run-order "$RUN_ORDER_VALUE" \
        --output "$output_path" \
        >/dev/null
}

run_java() {
    local scenario="$1"
    local wait_strategy="$2"
    local buffer_size="$3"
    local events_total="$4"
    local batch_size="$5"
    local warmup_rounds="$6"
    local measured_rounds="$7"
    local output_path="$RESULTS_DIR/java_${scenario}.json"

    java \
        -Xms2g -Xmx2g -XX:+AlwaysPreTouch \
        -cp "$JAVA_CP" \
        com.lmax.disruptor.headtohead.HeadToHead \
        --scenario "$scenario" \
        --wait-strategy "$wait_strategy" \
        --buffer-size "$buffer_size" \
        --events-total "$events_total" \
        --batch-size "$batch_size" \
        --warmup-rounds "$warmup_rounds" \
        --measured-rounds "$measured_rounds" \
        --run-order "$RUN_ORDER_VALUE" \
        --output "$output_path" \
        >/dev/null
}

for scenario in "${scenario_list[@]}"; do
    read -r wait_strategy buffer_size events_total batch_size warmup_rounds measured_rounds \
        <<<"$(scenario_defaults "$scenario" "$MODE")"

    echo "[INFO] Running scenario: $scenario"

    if [[ "$ORDER" == "rust-first" ]]; then
        run_rust "$scenario" "$wait_strategy" "$buffer_size" "$events_total" "$batch_size" "$warmup_rounds" "$measured_rounds"
        run_java "$scenario" "$wait_strategy" "$buffer_size" "$events_total" "$batch_size" "$warmup_rounds" "$measured_rounds"
    else
        run_java "$scenario" "$wait_strategy" "$buffer_size" "$events_total" "$batch_size" "$warmup_rounds" "$measured_rounds"
        run_rust "$scenario" "$wait_strategy" "$buffer_size" "$events_total" "$batch_size" "$warmup_rounds" "$measured_rounds"
    fi
done

echo "[INFO] Java build mode: $JAVA_MODE"

python3 - "$RESULTS_DIR" <<'PY'
import json
import pathlib
import sys

results_dir = pathlib.Path(sys.argv[1])
scenarios = ["unicast", "unicast_batch", "mpsc_batch", "pipeline"]
rows = []

for scenario in scenarios:
    rust_path = results_dir / f"rust_{scenario}.json"
    java_path = results_dir / f"java_{scenario}.json"
    if not rust_path.exists() or not java_path.exists():
        continue

    with rust_path.open() as fh:
        rust = json.load(fh)
    with java_path.open() as fh:
        java = json.load(fh)

    rust_median = rust["summary"]["median_ops_per_sec"]
    java_median = java["summary"]["median_ops_per_sec"]
    ratio = (rust_median / java_median) if java_median else 0.0
    valid = rust["summary"]["checksum_valid_all"] and java["summary"]["checksum_valid_all"]
    rows.append((scenario, rust_median, java_median, ratio, valid))

if not rows:
    print("[WARN] No comparable JSON results were found.")
    sys.exit(0)

print()
print("Head-to-Head Summary")
print("scenario        rust_median_ops/s    java_median_ops/s    rust/java    valid")
print("--------------- -------------------- -------------------- ------------ -----")
for scenario, rust_median, java_median, ratio, valid in rows:
    print(
        f"{scenario:<15} "
        f"{rust_median:>20.2f} "
        f"{java_median:>20.2f} "
        f"{ratio:>12.4f} "
        f"{str(valid):>5}"
    )
PY

echo
echo "[INFO] Results written to: $RESULTS_DIR"
