#!/usr/bin/env bash
# Functional smoke test for fork planning, artifact preservation, and reporting.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

python3 tools/head_to_head/test_report_forks.py

RESULTS_DIR="$(mktemp -d /private/tmp/badbatch-h2h-smoke.XXXXXX)"
export RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}"

bash scripts/run_head_to_head.sh \
  --scenario unicast \
  --mode quick \
  --order both-orders \
  --forks 2 \
  --seed 42 \
  --events-total 10000 \
  --results-dir "$RESULTS_DIR"

python3 - "$RESULTS_DIR" <<'PY'
import csv, json, pathlib, sys

root = pathlib.Path(sys.argv[1])
rust = sorted(root.glob("rust_*.json"))
java = sorted(root.glob("java_*.json"))
assert len(rust) == 2, rust
assert len(java) == 2, java
assert len({path.name for path in rust + java}) == 4

orders = set()
for path in rust + java:
    data = json.loads(path.read_text())
    measured = [round_ for round_ in data["rounds"] if round_["phase"] == "measured"]
    assert len(measured) == 1, (path, measured)
    assert data["summary"]["checksum_valid_all"], path
    assert data["pair_id"] in {"unicast-001", "unicast-002"}
    assert data["fork_index"] in {1, 2}
    assert data["harness_git_rev"]
    assert data["implementation_rev"]
    orders.add(data["run_order"])

assert orders == {"rust-then-java", "java-then-rust"}, orders
with (root / "fork_samples.csv").open() as handle:
    rows = list(csv.DictReader(handle))
assert len(rows) == 2, rows
assert (root / "fork_summary.json").exists()
assert (root / "REPORT.md").exists()
print(f"h2h smoke artifacts verified: {root}")
PY
