# Head-to-head: BadBatch vs LMAX Disruptor

Same-machine comparison harness. **Not** a substitute for Criterion benches, and **not** a claim of portable absolute performance.

Accepted macOS/Linux evidence and the current engineering interpretation are
summarized in [`docs/PERFORMANCE.md`](../../docs/PERFORMANCE.md); this document
defines the runner and artifact contract.

## Why this exists

Official LMAX `perftest` / JMH numbers are hard to map 1:1 onto BadBatch’s Builder API and Criterion suites (different event models, harnesses, and metrics). This tool runs **matched scenarios** on one host:

| Side | Implementation under test |
|------|---------------------------|
| Rust | BadBatch **public Builder** (`build_single_producer` / `build_multi_producer`) |
| Java | Idiomatic LMAX **`RingBuffer` + `BatchEventProcessor`** |

Both sides share:

- scenarios: `unicast`, `unicast_batch`, `mpsc_batch` (3 producers), `pipeline` (3 stages)
- payload fields and arithmetic **checksums**
- batch sizes, buffer sizes, event totals
- fresh process forks with in-process warmup and exactly **one measured sample per fork**
- paired Rust/Java fork order, balanced and reproducibly randomized by default
- wall time: first publish → consumer has processed all events
- terminal checksum/progress publication exactly once at the final sequence
- JSON fork metadata (`pair_id`, `fork_index`, implementation revisions, dirty state)
- orchestrator-side per-fork start/end UTC timestamps and load-average snapshots

## Prerequisites

- Rust stable (repo `rust-toolchain.toml`)
- JDK 17+ (`javac` / `java`)
- LMAX sources provisioned at the pinned revision declared in
  `tools/head_to_head/lmax.env`:

  ```bash
  bash scripts/setup_head_to_head_lmax.sh
  ```
- Optional: `RUSTFLAGS="-C target-cpu=native"` for fair native codegen on this machine

## Run

```bash
# Smoke (2 paired forks per scenario, small event counts)
bash scripts/run_head_to_head.sh --mode quick

# Full event counts (can take a long time)
bash scripts/run_head_to_head.sh --mode full

# Serious fork-level study (20 paired forks, reproducible balanced order)
bash scripts/run_head_to_head.sh --scenario pipeline --mode full \
  --order both-orders --forks 20 --seed 20260719 \
  --cpu-list 2,3,4,5

# Rust 128B slot padding (Java still heap objects — interpret carefully)
bash scripts/run_head_to_head.sh --scenario unicast --event-padding 128 --mode quick

# Probe every warmup/measured round (diagnostic throughput, not canonical ranking)
bash scripts/run_head_to_head.sh --scenario pipeline --mode full --forks 20 \
  --seed 20260719 --cpu-list 2,3,4,5 --round-diagnostics
```

Outputs go to `head_to_head_results/<timestamp>/`:

| File | Content |
|------|---------|
| `environment.txt` | host, both source revisions/dirty state/tree hashes, binary hashes, tools |
| `fork_plan.tsv` | precomputed pair IDs and Rust/Java order |
| `rust_<scenario>_fork…json` / `java_<scenario>_fork…json` | one fresh-process fork, including `fork_provenance` timestamps/loadavg |
| `fork_samples.csv` | paired fork scatter input |
| `fork_summary.json` | aggregate fork statistics |
| `REPORT.md` | fork summary, every pair, and interpretation boundary |
| `round_batch_diagnostics.csv` | opt-in per-round, per-stage batch-size/queue-depth histograms |
| `round_producer_backpressure.csv` | opt-in per-round producer capacity-wait counters |

## Per-round diagnostic mode

`--round-diagnostics` is deliberately separate from the canonical benchmark:

- Rust is rebuilt with `bench-round-diagnostics`; normal and `bench-tools`-only
  builds contain no hot-path probe fields, counters or branches.
- Java compiles a verified temporary copy of LMAX `SingleProducerSequencer.java`
  under the results directory. The pinned `examples/disruptor` checkout is never
  edited. Its generated-source SHA256 is recorded in `environment.txt`.
- Every warmup and measured round retains its own consumer-stage batch-size and
  queue-depth histograms, plus producer backpressure entries, total wait-loop
  iterations, and maximum iterations for one entry. Nothing is collapsed to a
  process-level mode before it reaches raw JSON/CSV.
- Histogram bin `i` is floor-log2: `[2^i, 2^(i+1)-1]` (64 counters).
- Rust iterations count `spin_loop()` calls; Java iterations count
  `parkNanos(1L)` requests. Those are different physical actions, so compare
  per-round correlation and entry incidence, not the raw counts as equal time.
- Probe-enabled throughput is conditioned on instrumentation overhead. Use it
  to distinguish batch/backpressure hypotheses, not as the headline H2H ratio.

## How to answer “what’s the gap?”

1. Run `quick` first; confirm all **checksum OK** and both orders are preserved.
2. Use `--forks 20` or more for a serious modal-frequency/ranking study.
3. Inspect every pair in `fork_samples.csv` before reading aggregate medians.
4. Compare scenarios:
   - **unicast** — single-event publish tax  
   - **unicast_batch** — bulk publish (often the fairest “throughput” comparison)  
   - **mpsc_batch** — multi-writer contention  
   - **pipeline** — multi-stage barriers  
5. Treat fork-level multimodality as a first-class result; a low CV inside one process is not evidence of cross-process stability.

## Fork protocol

- The script builds Rust and Java once, then launches a fresh process for every sample.
- The same shell/Python wrapper snapshots UTC time and system load average immediately before and after every child process; these observations are outside the measured interval and are diagnostic rather than cross-platform performance metrics.
- On Linux, `--cpu-list` applies and verifies per-thread affinity before timing. The
  role map is identical on both sides: unicast uses publisher/consumer; MPSC uses
  three producers plus the consumer; pipeline uses publisher plus stages 1–3.
  Every JSON fork artifact records the requested CPUs, role mapping, and
  `verified_all`; the process fails before timing if any pin cannot be verified.
- Each process performs the configured warmup rounds and exactly one measured round.
- A pair is one Rust fork plus one Java fork with the same scenario/configuration.
- `--order both-orders` creates a balanced order plan and shuffles it with `--seed`.
- Output names include scenario, fork index, and order, so no order can overwrite another.
- The report deliberately does not infer fast/slow modes. Preserve and inspect the raw scatter.

`quick` defaults to 2 pairs and `full` to 7. Those defaults are functional/exploratory;
they are not publication-grade sample counts. Use at least 20 paired forks when mode
frequency or implementation ranking matters.

## What is *not* comparable

- JVM GC and heap object layout vs Rust inline ring slots  
- Builder orchestration cost vs bare LMAX BEP (intentional for “product API vs idiomatic LMAX”)  
- Criterion HTML benches vs this harness (different metrics)  
- Cross-machine absolute ops/s without pinning and power control
- Results produced without `--cpu-list` when CPU migration would affect the claim
- A portable ranking inferred from one host or a visibly mixed fork distribution

## Layout

```
src/bin/h2h_rust.rs                 # Rust harness
tools/head_to_head/java/.../HeadToHead.java
scripts/run_head_to_head.sh
tools/head_to_head/report_forks.py
examples/disruptor/                 # LMAX sources (required to compile Java side)
```

## After library changes

Re-run at least:

```bash
bash scripts/run_head_to_head.sh --mode quick --scenario all
```

Validate the harness machinery itself with:

```bash
bash scripts/test_head_to_head.sh
```

If the public Builder API or default wait/batch semantics change, update **both** harnesses and this README in the same PR.
