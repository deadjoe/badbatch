#!/usr/bin/env bash
# Build the Java tail-latency harness with immutable, generated provenance.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LMAX_ROOT="$ROOT/examples/disruptor"
CLASSES_DIR=""
GENERATED_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --classes-dir)
      CLASSES_DIR="$2"
      shift 2
      ;;
    --generated-dir)
      GENERATED_DIR="$2"
      shift 2
      ;;
    --lmax-root)
      LMAX_ROOT="$2"
      shift 2
      ;;
    --help|-h)
      echo "usage: $0 --classes-dir DIR --generated-dir DIR [--lmax-root DIR]"
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$CLASSES_DIR" || -z "$GENERATED_DIR" ]]; then
  echo "error: --classes-dir and --generated-dir are required" >&2
  exit 2
fi
if [[ ! -d "$LMAX_ROOT/src/main/java" ]]; then
  echo "error: LMAX sources missing: $LMAX_ROOT/src/main/java" >&2
  exit 1
fi
if [[ -z "${JAVA_HOME:-}" || ! -x "$JAVA_HOME/bin/javac" ]]; then
  echo "error: JAVA_HOME must name a JDK 17+ with bin/javac" >&2
  exit 1
fi

mkdir -p "$CLASSES_DIR" "$GENERATED_DIR"
CLASSES_DIR="$(cd "$CLASSES_DIR" && pwd -P)"
GENERATED_DIR="$(cd "$GENERATED_DIR" && pwd -P)"
LMAX_ROOT="$(cd "$LMAX_ROOT" && pwd -P)"

case "$CLASSES_DIR/" in
  "$ROOT/"*|"$LMAX_ROOT/"*)
    echo "error: classes-dir must be outside both source repositories" >&2
    exit 1
    ;;
esac
case "$GENERATED_DIR/" in
  "$ROOT/"*|"$LMAX_ROOT/"*)
    echo "error: generated-dir must be outside both source repositories" >&2
    exit 1
    ;;
esac

GENERATED_SOURCE="$GENERATED_DIR/com/lmax/disruptor/headtohead/TailBuildProvenance.java"
PROVENANCE_MANIFEST="$GENERATED_DIR/provenance.json"
python3 "$ROOT/tools/head_to_head/generate_tail_provenance.py" \
  --badbatch-root "$ROOT" \
  --lmax-root "$LMAX_ROOT" \
  --output "$GENERATED_SOURCE" \
  --manifest "$PROVENANCE_MANIFEST"

JAVA_SOURCES=()
while IFS= read -r -d '' source; do
  JAVA_SOURCES+=("$source")
done < <(find "$LMAX_ROOT/src/main/java" -name '*.java' ! -name 'module-info.java' -print0)
JAVA_SOURCES+=(
  "$ROOT/tools/head_to_head/java/com/lmax/disruptor/headtohead/TailLatency.java"
  "$GENERATED_SOURCE"
)

"$JAVA_HOME/bin/javac" --release 17 -d "$CLASSES_DIR" "${JAVA_SOURCES[@]}"
python3 "$ROOT/tools/head_to_head/generate_tail_provenance.py" \
  --manifest "$PROVENANCE_MANIFEST" \
  --verify

echo "Java tail harness compiled: $CLASSES_DIR"
echo "Build provenance manifest: $PROVENANCE_MANIFEST"
