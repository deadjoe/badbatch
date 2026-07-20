#!/usr/bin/env bash
# Functional smoke test for fork planning, artifact preservation, and reporting.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

python3 tools/head_to_head/test_report_forks.py
bash scripts/setup_head_to_head_lmax.sh

TMP_ROOT="${TMPDIR:-/tmp}"
RESULTS_DIR="$(mktemp -d "${TMP_ROOT%/}/badbatch-h2h-smoke.XXXXXX")"
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
    assert "round_diagnostics" not in data
    measured = [round_ for round_ in data["rounds"] if round_["phase"] == "measured"]
    assert len(measured) == 1, (path, measured)
    assert data["summary"]["checksum_valid_all"], path
    assert data["pair_id"] in {"unicast-001", "unicast-002"}
    assert data["fork_index"] in {1, 2}
    assert data["harness_git_rev"]
    assert data["implementation_rev"]
    affinity = data["cpu_affinity"]
    assert affinity["requested_cpu_list"] == []
    assert affinity["mode"] == "none"
    assert affinity["verified_all"] is False
    assert affinity["role_cpu_map"] == {}
    provenance = data["fork_provenance"]
    assert provenance["started_at_utc"].endswith("Z")
    assert provenance["ended_at_utc"].endswith("Z")
    assert provenance["ended_unix_ns"] >= provenance["started_unix_ns"]
    assert provenance["orchestrator_elapsed_ns"] >= 0
    assert provenance["process_exit_code"] == 0
    for key in ("loadavg_at_start", "loadavg_at_end"):
        loadavg = provenance[key]
        assert loadavg is None or len(loadavg) == 3
    for key in ("linux_host_at_start", "linux_host_at_end"):
        host = provenance[key]
        if host is not None:
            assert "cpu" in host["cpu_times"]
            assert "steal" in host["cpu_times"]["cpu"]
    orders.add(data["run_order"])

assert orders == {"rust-then-java", "java-then-rust"}, orders
with (root / "fork_samples.csv").open() as handle:
    rows = list(csv.DictReader(handle))
assert len(rows) == 2, rows
assert (root / "fork_summary.json").exists()
assert (root / "REPORT.md").exists()
assert not (root / "round_batch_diagnostics.csv").exists()
assert not (root / "round_producer_backpressure.csv").exists()
print(f"h2h smoke artifacts verified: {root}")
PY

DIAGNOSTIC_RESULTS_DIR="$(mktemp -d "${TMP_ROOT%/}/badbatch-h2h-diagnostic-smoke.XXXXXX")"
bash scripts/run_head_to_head.sh \
  --scenario pipeline \
  --mode quick \
  --order rust-first \
  --forks 1 \
  --events-total 10000 \
  --buffer-size 64 \
  --round-diagnostics \
  --results-dir "$DIAGNOSTIC_RESULTS_DIR"

python3 - "$DIAGNOSTIC_RESULTS_DIR" <<'PY'
import csv, json, pathlib, sys

root = pathlib.Path(sys.argv[1])
for path in (next(root.glob("rust_*.json")), next(root.glob("java_*.json"))):
    data = json.loads(path.read_text())
    assert data["round_diagnostics"] is True
    assert len(data["rounds"]) == 2
    for round_ in data["rounds"]:
        diagnostics = round_["diagnostics"]
        assert [item["role"] for item in diagnostics["batch_processing"]] == [
            "stage_1", "stage_2", "stage_3"
        ]
        for item in diagnostics["batch_processing"]:
            batch = item["batch_size"]
            queue = item["queue_depth"]
            assert batch["sum"] == round_["events"]
            assert sum(batch["log2_bins"]) == batch["count"]
            assert sum(queue["log2_bins"]) == queue["count"]
        producer = diagnostics["producer_backpressure"]
        assert producer["supported"] is True
        assert producer["iteration_action"] in {"spin_loop", "park_nanos_1_request"}

with (root / "round_batch_diagnostics.csv").open() as handle:
    batch_rows = list(csv.DictReader(handle))
with (root / "round_producer_backpressure.csv").open() as handle:
    producer_rows = list(csv.DictReader(handle))
assert len(batch_rows) == 12, len(batch_rows)  # 2 languages x 2 rounds x 3 stages
assert len(producer_rows) == 4, len(producer_rows)
assert {row["round_index"] for row in batch_rows} == {"1", "2"}
assert {row["phase"] for row in batch_rows} == {"warmup", "measured"}
assert "Probe-conditioned run" in (root / "REPORT.md").read_text()
print(f"h2h diagnostic smoke artifacts verified: {root}")
PY
