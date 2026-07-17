# Benchmark guide

Authoritative notes for the BadBatch Criterion suites under `benches/`, the runner
`scripts/run_benchmarks.sh`, and the log formatter `scripts/result_formatter.sh`.

If this file disagrees with older notes, **prefer this file and the current code**.

## Purpose

These benchmarks help you:

- Catch large regressions on a given machine and revision
- Observe steady-state behavior across topologies
- Compare wait strategies, batch vs single-event publish, pipelines, and buffer sizes
- Support limited, carefully scoped comparisons with LMAX Java perftests under `examples/disruptor/`

They are **not**:

- Portable absolute performance guarantees for releases
- A fully isolated lab harness (no automatic CPU pinning / core isolation)
- A single metric that ranks “the project” against every other implementation

Prefer same-machine, same-environment, same-revision-family comparisons.

## Layout

| Path | Role |
|------|------|
| `benches/*.rs` | Criterion benchmarks and custom latency stats |
| `scripts/run_benchmarks.sh` | Suite runner (timeouts, logs, summaries) |
| `scripts/result_formatter.sh` | Parses Criterion / latency logs into summaries |
| `benchmark_logs/` | Per-suite stdout/stderr from the runner |
| `target/criterion/` | Criterion HTML reports |
| `benches/results/BASELINE.md` | Checked-in median baseline (Apple Silicon; machine-specific) |
| `examples/disruptor/` | Upstream LMAX Java reference (when present) |

## How to run

### Recommended: suite runner

```bash
# Smoke / CI-style check
./scripts/run_benchmarks.sh quick

# Short harness debug (comprehensive_benchmarks with tiny sample/warmup)
./scripts/run_benchmarks.sh minimal

# Individual suites
./scripts/run_benchmarks.sh spsc
./scripts/run_benchmarks.sh mpsc
./scripts/run_benchmarks.sh pipeline
./scripts/run_benchmarks.sh latency
./scripts/run_benchmarks.sh throughput
./scripts/run_benchmarks.sh scaling

# Full sequence (quick + formal suites)
./scripts/run_benchmarks.sh all
```

### Direct Criterion

```bash
cargo bench --bench comprehensive_benchmarks
cargo bench --bench single_producer_single_consumer
cargo bench --bench multi_producer_single_consumer
cargo bench --bench pipeline_processing
cargo bench --bench latency_comparison
cargo bench --bench throughput_comparison
cargo bench --bench buffer_size_scaling
```

### Other runner modes

```bash
./scripts/run_benchmarks.sh compile     # compile only
./scripts/run_benchmarks.sh regression  # re-runs comprehensive_benchmarks (not auto-diff vs history)
./scripts/run_benchmarks.sh report      # refresh HTML via comprehensive_benchmarks only
```

Notes:

- `minimal` ≈ `cargo bench --bench comprehensive_benchmarks -- --sample-size 10 --warm-up-time 1 --measurement-time 1`
- `regression` does **not** automatically diff against a historical baseline
- `report` does **not** re-run every suite

## Runner behavior

`scripts/run_benchmarks.sh`:

- Runs each suite as `cargo bench --bench <suite>`
- Writes full output to `benchmark_logs/<suite>.log`
- In `all` mode, continues after a suite failure
- `all` order: `quick`, `spsc`, `mpsc`, `pipeline`, `latency`, `throughput`, `scaling`
- After each suite, invokes the formatter for a short summary; prints a unified summary at the end

Timeouts (when GNU `timeout` or macOS `gtimeout` is available):

- Default: `TIMEOUT_SECONDS=600`
- `single_producer_single_consumer`: `SPSC_TIMEOUT=900`
- `buffer_size_scaling`: `SCALING_TIMEOUT=900`
- Without a timeout binary, the runner still works but has no outer time limit

## Formatter behavior

`scripts/result_formatter.sh`:

1. Extracts Criterion median `time` / `thrpt` estimates
2. Extracts custom latency lines (`mean` / `median` / `p95` / `p99` / `max`) from `latency_comparison`
3. Picks a headline “Peak Case” per suite
4. Counts `WARNING:` lines in logs

### Peak Case selection

Throughput-oriented suites:

- Prefer non-`baseline` cases
- If both `pause:0ms` and non-zero pause exist, prefer `pause:0ms`
- Choose maximum throughput among candidates; if none, first non-baseline

`latency_comparison`:

- Does not use Criterion thrpt for the headline
- Chooses the implementation with the lowest custom **mean** latency

`Samples` / `Iterations` come from the Collecting line of the **selected** peak case (not the first baseline).

`Highest Reported Throughput` in the global summary is a convenience headline only: it compares throughput suites and skips `comprehensive_benchmarks` and `latency_comparison`. It is **not** an overall project score.

Treat any suite log containing `WARNING:` as unclean for performance conclusions, even if `cargo bench` exited zero.

## Design principles (benchmark code)

- Construct disruptors / handlers / worker threads outside timed sections when possible
- Prefer Criterion `iter_custom` on hot paths
- Completion uses a **monotonically increasing counter + per-iteration target** (not “reset to zero and wait for a fixed total,” which can confuse iteration boundaries)
- BusySpin / Yielding waiters spin or yield; Blocking / Sleeping paths avoid harness-level `sleep(1ms)` fake slowness
- Suites may apply internal timeouts to avoid hangs
- `quick` is smoke / sanity, not a primary KPI suite

## Suites

### `comprehensive_benchmarks.rs`

**Role:** light smoke / CI-style check.

Includes Safe_SPSC (BusySpin, bursts 100/1000), Safe_Throughput (buffers 256/1024), Safe_Latency (single publish→consume), and a `std::sync::mpsc` Channel_Baseline (channel/thread setup cost per iteration — not steady-state comparable to Disruptor).

Typical Criterion knobs: measurement ~5s, warmup ~2s, sample_size ~15 (some groups shorter).

**Use for:** “Did this change break the basic path?” — not primary KPI.

### `single_producer_single_consumer.rs`

**Role:** SPSC throughput; wait strategies; single-event vs batch publish; optional cache-line padding.

Matrix (current intent): buffer 1024; bursts 100/1000; pause 0ms; cases BusySpin, Yielding, Blocking, Sleeping, BatchBusySpin, BatchYielding, BusySpinPadded, YieldingPadded, baseline.

- Non-batch: closer to common single-event APIs  
- Batch*: closer to high-throughput batch publication  
- *Padded: `with_cache_line_padding(true)` — on Apple Silicon, padding often **slows** tiny events (see `results/BASELINE.md`)

Criterion: measurement ~10s, warmup ~3s, sample_size ~20.

### `multi_producer_single_consumer.rs`

**Role:** real concurrent MPSC (multiple producer threads).

`PRODUCER_COUNT = 3`; buffer 1024; bursts 10/100/500; pause 0ms/1ms (burst 500 only pause 0ms). Persistent producers; generation counter per burst. Cases include single-event try_publish and batch_publish under BusySpin.

Criterion: measurement ~15s, warmup ~5s, sample_size ~10.

Prefer this suite over `throughput_comparison` `Batch_MS_*` for true multi-producer contention.

### `pipeline_processing.rs`

**Role:** multi-stage dependent pipelines (Two/Three/Four stage; BusySpin/Yielding).

Buffer 2048; bursts 50/200/1000. Stages do fixed arithmetic only; completion is last-stage count.

More stages → lower end-to-end throughput is expected. On macOS / hybrid cores, BusySpin does not always beat Yielding.

### `latency_comparison.rs`

**Role:** one-way publish → `on_event` latency vs `std::sync::mpsc::sync_channel` and `crossbeam::channel::bounded`.

Buffer 1024; 500 events per Criterion iteration. Implementations: Disruptor/BusySpin, StdMpsc, Crossbeam.

- Criterion `time` / `thrpt` = **batch wall time for 500 events**
- Single-hop latency headline = custom **Latency Statistics** (mean/median/p95/p99/max)

Not round-trip ping-pong latency.

### `throughput_comparison.rs`

**Role:** steady-state throughput across buffer sizes, wait strategies, and channel baselines.

Fixed 10_000 events per iteration; buffers 256/1024/4096; batch publish with chunk `min(buffer_size, 256)`.

**Important:** `Batch_MS_*` means multi-producer **sequencer** with a **single publishing thread** — not concurrent MPSC. For real MPSC, use `multi_producer_single_consumer.rs`.

### `buffer_size_scaling.rs`

**Role:** buffer size vs processing cost / payload probes (not a full factorial matrix).

Includes Fast/Medium/Slow processing points, MemoryUsage (payload/allocation probe, not a precise profiler), BufferUtil (try_publish + artificial backpressure). Use for sizing intuition, not max-throughput bragging rights.

## Interpreting output

Per suite: first ~10 cases (not sorted by speed), outlier notes, short assessment. For latency suite, read Latency Statistics first.

Summary: Peak Case, Peak Throughput/Time, Samples/Iterations (for the peak case), or lowest mean latency for the latency suite.

Suggested primary KPIs:

| Question | Suite |
|----------|--------|
| One-hop latency | `latency_comparison` (custom stats) |
| SPSC | `single_producer_single_consumer` |
| True MPSC | `multi_producer_single_consumer` |
| Pipeline | `pipeline_processing` |
| Steady-state data-plane | `throughput_comparison` (with MS caveat) |

Do not treat as final headline: Quick suite, Highest Reported Throughput alone, or `Batch_MS_*` as “real MPSC”.

## Common mistakes

1. **Quick is high** → does not imply main suites improved.  
2. **Criterion thrpt on latency suite** → batch of 500, not single-event latency.  
3. **`Batch_MS_*`** → multi-producer sequencer, single publisher thread.  
4. **Ignoring `WARNING:`** → result is not clean.  
5. **Cross-machine absolute numbers** → environment noise dominates without pinning and power control.

## Environment

No automatic CPU pinning or isolated cores. Results vary with frequency scaling, background load, power state, OS scheduling, hybrid cores, and toolchain/`RUSTFLAGS`.

Expect roughly 5–15% throughput noise; latency more. For release-grade claims, use a controlled server (e.g. pinned x86_64 or ARM Linux), fixed toolchain, and stable power/governor settings.

`RUSTFLAGS="-C target-cpu=native"` (and LSE where relevant) can matter on multi-producer atomic paths.

## Suggested workflows

```bash
# Daily
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
./scripts/run_benchmarks.sh minimal

# Hot path / concurrency changes
./scripts/run_benchmarks.sh spsc
./scripts/run_benchmarks.sh mpsc
./scripts/run_benchmarks.sh throughput

# Barrier / pipeline / wait strategy
./scripts/run_benchmarks.sh pipeline
./scripts/run_benchmarks.sh latency

# Broader check
./scripts/run_benchmarks.sh all
./scripts/run_benchmarks.sh report
```

Review checklist: all suites OK → logs free of `WARNING:` → correct KPI per suite → latency custom stats vs Criterion batch → true MPSC vs `Batch_MS_*` → same machine/toolchain when comparing.

## Comparison with native LMAX Disruptor

For **same-machine, matched-scenario** medians (recommended), use the head-to-head harness:

```bash
bash scripts/run_head_to_head.sh --mode quick
# see tools/head_to_head/README.md
```

That tool compares BadBatch Builder vs LMAX `RingBuffer`+`BatchEventProcessor` with shared event counts, checksums, and JSON summaries. Official LMAX **perftest** / JMH remain useful but are harder to map 1:1 onto BadBatch’s API.

Prefer `examples/disruptor` **perftest** jars over ad-hoc JMH cross-plots when using Java-only microbenches:

| BadBatch suite / case | Closest LMAX perftest (when available) |
|-----------------------|----------------------------------------|
| SPSC BusySpin/Yielding | `OneToOneSequencedThroughputTest` |
| SPSC BatchBusySpin/BatchYielding | `OneToOneSequencedBatchThroughputTest` |
| MPSC Batch BusySpin | `ThreeToOneSequencedBatchThroughputTest` |
| Pipeline ThreeStage | `OneToThreePipelineSequencedThroughputTest` |

Do not equate BadBatch one-way latency with LMAX ping-pong latency tests.

Example (if the Java tree is checked out):

```bash
cd examples/disruptor
./gradlew perfJar
java -cp build/libs/disruptor-perf-*.jar com.lmax.disruptor.sequenced.OneToOneSequencedThroughputTest
```

JMH is better for Java-internal microbenchmarks than for 1:1 matching of BadBatch end-to-end suites.
