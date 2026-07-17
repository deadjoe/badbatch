# Head-to-head: BadBatch vs LMAX Disruptor

Same-machine comparison harness. **Not** a substitute for Criterion benches, and **not** a claim of portable absolute performance.

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
- warmup + measured rounds; **median ops/s** over measured rounds only
- wall time: first publish → consumer has processed all events
- JSON schema (`summary.median_ops_per_sec`, `checksum_valid_all`, …)

## Prerequisites

- Rust stable (repo `rust-toolchain.toml`)
- JDK 17+ (`javac` / `java`)
- LMAX sources at `examples/disruptor/src/main/java/com/lmax/disruptor/…`
- Optional: `RUSTFLAGS="-C target-cpu=native"` for fair native codegen on this machine

## Run

```bash
# Smoke (≈ minutes, small event counts)
bash scripts/run_head_to_head.sh --mode quick

# Full event counts (can take a long time)
bash scripts/run_head_to_head.sh --mode full

# One scenario, both run orders (detect order bias)
bash scripts/run_head_to_head.sh --scenario unicast_batch --mode quick --order both-orders

# Rust 128B slot padding (Java still heap objects — interpret carefully)
bash scripts/run_head_to_head.sh --scenario unicast --event-padding 128 --mode quick
```

Outputs go to `head_to_head_results/<timestamp>/`:

| File | Content |
|------|---------|
| `environment.txt` | host, git, rustc, java |
| `rust_<scenario>.json` / `java_<scenario>.json` | full rounds + summary |
| `REPORT.md` | table + interpretation notes |

## How to answer “what’s the gap?”

1. Run `quick` first; confirm all **checksum OK**.
2. Run `full` (or raise `--events-total`) for stable medians.
3. Read **rust/java** ratio in `REPORT.md`.
4. Compare scenarios:
   - **unicast** — single-event publish tax  
   - **unicast_batch** — bulk publish (often the fairest “throughput” comparison)  
   - **mpsc_batch** — multi-writer contention  
   - **pipeline** — multi-stage barriers  
5. If ratios flip with `--order both-orders`, treat as noise / cache / thermal effects and re-run cold.

## What is *not* comparable

- JVM GC and heap object layout vs Rust inline ring slots  
- Builder orchestration cost vs bare LMAX BEP (intentional for “product API vs idiomatic LMAX”)  
- Criterion HTML benches vs this harness (different metrics)  
- Cross-machine absolute ops/s without pinning and power control  

## Layout

```
src/bin/h2h_rust.rs                 # Rust harness
tools/head_to_head/java/.../HeadToHead.java
scripts/run_head_to_head.sh
examples/disruptor/                 # LMAX sources (required to compile Java side)
```

## After library changes

Re-run at least:

```bash
bash scripts/run_head_to_head.sh --mode quick --scenario all
```

If the public Builder API or default wait/batch semantics change, update **both** harnesses and this README in the same PR.
