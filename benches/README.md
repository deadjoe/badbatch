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
| `src/bin/h2h_tail_latency.rs` | Standalone open-loop tail-latency driver |
| `tools/head_to_head/java/.../TailLatency.java` | Matching LMAX Java open-loop driver |
| `scripts/run_tail_latency_head_to_head.sh` | Six-arm calibration, preflight, run, and validation |
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
cargo bench --bench worker_pool
```

### Open-loop tail latency (non-Criterion)

`h2h_tail_latency` drives a fixed producer schedule and measures each event from
its planned send time through handler completion. Raw samples are mandatory so
tail claims remain independently auditable. The producer never waits for
consumer completion between publications; if bounded-ring backpressure delays a
publish, the original planned timestamp remains in force:

```bash
cargo run --release --features bench-tools --bin h2h_tail_latency -- \
  --rate 100000 \
  --events-total 1000000 \
  --warmup-events 100000 \
  --samples-output target/tail-samples.csv \
  --output target/tail-summary.json
```

Without `--rate` or `--max-rate`, the driver first calibrates throughput and then
runs the configured load percentages (defaults: 50%, 70%, 90%). With multiple
loads, the percentage is appended to each raw-sample filename.

The cross-language F.5 protocol is frozen in
[`docs/f5_tail_latency_protocol.md`](../docs/f5_tail_latency_protocol.md).
Formal orchestration calibrates each language's allocation-free, allocating-W,
and allocating-4W arm independently, takes one global common maximum, and then
passes that value with each arm's own maximum:

```bash
# Phase A: at least three fresh-process artifacts per arm and language;
# the runner conservatively selects the minimum calibrated maximum
target/release/h2h_tail_latency --calibrate-only --handler-mode allocation-free \
  --output target/rust-a-calibration.json

# Phase B: identical absolute target, with arm-specific utilization metadata
target/release/h2h_tail_latency --handler-mode allocating --retention-window 65536 \
  --rate 100000 --max-rate 200000 --own-max 300000 \
  --events-total 1000000 --warmup-events 100000 \
  --samples-output target/rust-bw.csv --output target/rust-bw.json
```

Allocation-free mode keeps the original measurement hot path and reports zero
workload allocations. Allocating mode requires the **measured** region—not
warmup plus measurement—to fill its entire retention window and validates
exactly one 48-byte requested allocation per measured event. The logical
payload remains four `u64` fields (32 bytes); the remaining bytes are explicit
cross-runtime matching padding, not application payload.

Use `--inject-sleep-us-by-load 167,72,19` as a direct coordinated-omission
counterfactual for three configured loads. The run is invalid unless the
**observed** pause keeps the complete drain-amplified affected population in
the protocol's allowed range, the recorded post-pause schedule leaves the
computed drain allowance, and the pause is visible in both p99.9 and max. The
default injection point is 25% into the measured region and can be changed
with `--inject-at-measured-pct`. A run also exits non-zero when achieved
producer rate is below 95% of target. These are harness-validity checks, not
portable performance conclusions; the full control-vs-pause median and tighter
rate signature is checked across repeated artifacts, and formal results still
require controlled hosts, CPU placement, and repeated runs.

The binary embeds its Git revision and dirty state at build time. That
provenance is independent of the directory used to launch the binary; an
unknown revision or dirty build hard-invalidates every load and exits non-zero.

The full Rust/Java protocol has one entry point:

```bash
# Results must be outside both source repositories. Announce an exclusive
# compile/benchmark window on a shared host before starting.
JAVA_HOME=/path/to/jdk-17 \
  ./scripts/run_tail_latency_head_to_head.sh \
  --results-dir /absolute/external/path/f5-tail-run \
  --rust-warmup-events 100000 \
  --java-warmup-events 1000000 \
  --measured-events 1000000 \
  --cpu-list 2,3
```

The runner does not trust source-level payload size or independently chosen
load percentages. It first builds both artifacts with immutable provenance,
checks the A/B bytecode, uses JFR to verify the Java B payload size, calibrates
Rust/Java × A/B-W/B-4W in at least three balanced fresh processes, selects each
arm's minimum, and chooses the minimum of all six conservative maxima. Every
arm then receives the same absolute 50/70/90% targets, three fresh-process
controls, and three fresh-process injected pause counterfactuals. The matched
replicates run in phase-interleaved order
`control-r1 → injected-r1 → control-r2 → injected-r2 → ...`; language order
still alternates inside each block. After calibration but before any tail
result is observed, the runner freezes a separate microsecond pause
for every load. Before calibration it automatically measures Rust and Java
pause-delivery precision under the same CPU/JVM/JFR/GC profile in five fresh
processes per candidate and selects the first host-local duration whose worst
overshoot is at most 1.75x and whose relative full range is at most 10% in both
runtimes. The per-load selector
intersects that independent minimum with the affected bounds, allows at most
`floor(N / 20)` affected samples under the measured 1.75x overshoot ceiling,
and retains `floor(N / 10)` as the final observed hard gate. An empty
intersection fails with the minimum larger `N`; it never falls back to manual
host tuning.
`--inject-sleep-us-by-load 50:U,70:U,90:U` may predeclare
different values, but an out-of-range choice aborts before measurement. Rust
and Java warmup counts are separate; freeze them from runtime-specific
steady-state evidence before a formal run while keeping the measured sample
count common. The validator streams every raw row, checks exact planned
timestamps, requested and observed pauses, direct backlog and drain-amplified
affected counts, pairing, allocation alignment, wall-clock anchors,
provenance, JFR/GC artifact presence, and the predeclared
control-versus-pause tolerances. It writes `validation_report.json` and exits
non-zero on any mismatch.

The default p50 equivalence band is the largest of 2.5%, the full range of the
three control p50 values, and the full range of the three injected p50 values.
Each empirical range can be used only when that side independently satisfies
`full range / abs(median) <= 5%`; instability on either side makes equivalence
inconclusive instead of widening the band. The validator requires the relative
floor to remain strictly below the stability ceiling, which keeps the empirical
branches reachable. Every cell records all three delta components and the
winning source, so a future tolerance override cannot silently change which
term controls the band. The comparison uses the two medians.
The formulas, replicate counts, and limits are written to the manifest before
the first control; override them only before execution. The report also
surfaces signed positive/negative/zero counts, an exact two-sided sign test, and
per-language and per-arm splits. Non-constant load-monotonic language/arm
deltas are shown with their no-ties chance baseline: with three loads, any one
group is monotonic in either direction with probability 1/3 under an
independent continuous null. Integer-nanosecond ties and cross-load dependence
can change that baseline. Decided cells are the primary signed population;
inconclusive cells retain their status and signed delta rather than being
treated as zero effect. For post-run analysis, pass
`--prior-validation-report PATH` to the validator to record per-cell old/new
status and delta, same-direction residuals, and exact nonzero reproductions.
Its primary reproduction signal excludes zeros and requires the same sign plus
an absolute old/new magnitude ratio within `[1/2, 2]`. Both reports' schema,
relative tolerance, and stability limit are printed. Near the 1 ns integer
quantization floor, the ratio rule has little discrimination beyond the same
nonzero sign; the report states that limit without inventing an unregistered
absolute cutoff. When gate contexts differ, status changes are explicitly
non-attributable to measurement alone, while signed deltas remain
gate-independent. This comparison is descriptive and cannot alter acceptance.
These are residual observations, not extra pass/fail gates. More inconclusive
Java cells can reflect larger JIT/safepoint/scheduling dispersion and are not,
by themselves, evidence that Java is slower. A cell with sustained-delay-scale
control medians plus achieved-rate loss is classified as unable to maintain
that offered load during the measured window, rather than as a tail-latency
result.

The defaults pin G1 with a fixed 2 GiB heap and preserve JFR plus timestamped
GC/safepoint logs. Override JVM flags only before execution and keep them with
the resulting manifest. The runner produces validity evidence, not a portable
performance conclusion; host controls, order, repeated runs, and the protocol's
attribution limits still govern any claim.

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

### `worker_pool.rs`

**Role:** same-stage parallel consumers (WorkerPool scheme A) throughput.

Single producer, `BusySpinWaitStrategy`, 1/2/4/8 workers. Each worker owns its own `CachePadded<AtomicI64>` counter; the producer waits for the per-iteration target. The 1-worker case is a negative control; ≥2 workers exercise the CAS-claim path in `consumer_engine.rs`.

Use this suite to validate changes that touch the work-processor loop (e.g. `4460db6` C.3). The 1-worker arm is a negative control: because single-worker `current` advances every round, the conditional store is equivalent to an unconditional store, so it should show no measurable difference. **If 1 worker shows a measurable difference, the round is contaminated by cross-build artifact and the whole A/B should be discarded.** A measurable difference at ≥2 workers (with a clean 1-worker control) indicates the benchmark can see hot-path changes; it does not establish the direction of C.3's effect.

> **Note:** that difference verifies sensitivity only. Its *direction* is not an established performance result — cross-build comparisons on this crate carry a demonstrated code-layout confound (see F.3), so any claim about C.3's real effect requires the F.2 paired methodology (interleaved pairs, fresh processes, bootstrap CI).

### `worker_pool_break_even.rs`

**Role:** find the handler-cost inflection point where WorkerPool scheme A stops being a net loss.

Scans worker counts 1/2/4/8 against handler costs trivial / ~50ns / ~100ns / ~200ns / ~400ns / ~800ns / ~10µs, all within a single bench binary. Each tier is self-calibrated inside the bench binary (isolated single-threaded handler cost, ≥3 repetitions, median + range). A fan-out arm (`fan_out_events_with`) with the same thread counts and handler costs serves as a control to isolate shared-claim contention; it is **not** a direct throughput comparison because each fan-out consumer processes every event.

Use this suite to decide whether `also_partition_with` is appropriate for a given handler. On this host (Mac16,11: 10 P-core + 4 E-core), the 8-worker point is reported only as a reference value because 9 busy-spin threads risk heterogeneous-core scheduling.

> **Note:** the self-calibrated ns/event is measured in isolation (single thread, hot cache, direct call). It is the definition users can replicate for their own handlers, but it is not the in-situ cost inside a running Disruptor.

#### Measured inflection point (Mac16,11, single binary)

All numbers come from one `cargo bench --bench worker_pool_break_even` run; the different worker counts and handler costs are runtime parameters, so the comparison stays inside the same binary and avoids the cross-build layout confounds that affect A/B tests on this crate.

The handler-cost column uses **derived in-situ cost**: the single-worker (1w) per-event total time minus an ~8.5 ns overhead estimated from the high-cost tiers, where isolated self-calibration and in-situ measurement agree. The low-cost tiers (trivial / 050ns / 100ns) fall in a short-loop region where isolated self-calibration does not transfer reliably to the Disruptor path, so their derived costs are the authoritative values for this table.

| derived handler cost (ns/event) | wp 1 (Melem/s) | wp 2 speedup | wp 4 speedup | wp 8 speedup | fan-out 2 (M inv/s) | fan-out 4 (M inv/s) | fan-out 8 (M inv/s) |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 8.2 (trivial) | 59.69 | 0.17 | 0.11 | 0.05 | 78.1 | 106.1 | 133.6 |
| 13.2 (050ns) | 46.16 | 0.21 | 0.13 | 0.07 | 98.5 | 161.8 | 273.7 |
| 34.6 (100ns) | 23.21 | 0.39 | 0.27 | 0.14 | 49.0 | 94.5 | 178.8 |
| 197.2 (200ns) | 4.86 | 1.32 | 1.57 | 0.80 | 9.5 | 18.7 | 36.5 |
| 420.7 (400ns) | 2.33 | 1.66 | 2.93 | 1.64 | 4.7 | 9.2 | 17.9 |
| 851.1 (800ns) | 1.16 | 1.83 | 3.56 | 5.87 | 2.3 | 4.6 | 8.9 |
| 11,054.5 (010us) | 0.09 | 1.96 | 3.87 | 7.60 | 0.18 | 0.35 | 0.68 |

Interpolated crossing where WorkerPool N becomes faster than a single worker:

- **2 workers:** ~140 ns/event
- **4 workers:** ~125 ns/event

Given the wide spacing between low-cost tiers and the approximate nature of the overhead subtraction, both points are best summarized as **"on the order of 100–150 ns/event"** on this host.

Practical guidance:

- If your isolated handler cost is **below ~150 ns/event**, `also_partition_with` is a net loss on this machine. Prefer a single consumer or `fan_out_events_with`.
- If your handler cost is **above ~150 ns/event**, WorkerPool starts to win; at ~200 ns/event 2 workers already deliver a 1.3× speedup and 4 workers deliver ~1.6×.
- The 8-worker speedup is a strong function of handler cost: it is near zero below ~200 ns/event and only approaches linear scaling at high costs (~800 ns+). On this host it is **not** a reliable expansion point because 8 busy-spin workers + 1 producer approach the 10 P-core limit and may be scheduled onto E-cores.

The fan-out control confirms that the collapse at low handler costs is not a general multi-threading overhead. Expressing both arms as aggregate handler invocations per second makes the mechanism explicit:

| derived handler cost (ns/event) | WorkerPool 2w (M inv/s) | fan-out 2 (M inv/s) | fan-out / WorkerPool |
|---:|---:|---:|---:|
| 8.2 (trivial) | 10.15 | 78.10 | 7.70× |
| 34.6 (100ns) | 9.05 | 49.00 | 5.41× |
| 197.2 (200ns) | 6.42 | 9.50 | 1.48× |
| 851.1 (800ns) | 2.12 | 2.30 | 1.08× |
| 11,054.5 (010us) | 0.18 | 0.18 | 1.02× |

The same two threads and the same handler cost are used in both arms; the only difference is whether the workers share a single `work_sequence` claim (WorkerPool) or each consume every event independently (fan-out). The ratio falls from ~7.7× down to ~1× as handler cost rises, which is exactly the signature of claim-contention dominance: when the handler is cheap, the CAS claim is the bottleneck; when the handler is expensive, the handler itself dominates and the two arms converge.

The bottleneck is therefore the shared `work_sequence` CAS claim that WorkerPool uses to partition events, which dominates when the per-event handler work is small.

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
