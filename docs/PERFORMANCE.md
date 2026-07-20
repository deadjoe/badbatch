# Performance evidence and benchmarking

BadBatch performance numbers are measurements of named binaries under named
conditions, not portable guarantees. CPU topology, affinity, wait strategy,
event layout, buffer size, compiler flags and workload shape can all change the
result materially.

This page separates the checked-in measurement methods from conclusions that
are justified by the current macOS and Linux evidence.

## Measurement surfaces

| Surface | Purpose | Evidence boundary |
|---|---|---|
| Criterion suites under `benches/` | Regression and workload-specific microbenchmarks | Compare like-for-like runs on one machine |
| `baseline_metrics` / `benches/results/BASELINE.md` | Small repeatable macOS baseline | Three-run medians; some scenarios remain noisy |
| `scripts/run_head_to_head.sh` | Same-machine BadBatch Builder vs LMAX Java BEP | Fresh paired processes, matched scenario/configuration and checksums |
| `--round-diagnostics` | Batch formation and producer-backpressure diagnosis | Probe-conditioned throughput; never a replacement for the canonical H2H ratio |

The head-to-head harness records revisions, dirty state, binary/tree hashes,
per-fork timing and host snapshots. On Linux, `--cpu-list` pins and verifies each
measured role before timing. See [`tools/head_to_head/README.md`](../tools/head_to_head/README.md)
for the complete protocol.

## Controlled Linux bare-metal study — 2026-07-20

The accepted baseline ran on one Intel Xeon E-2286G bare-metal host with Ubuntu
24.04. Each scenario used 20 balanced, reproducibly randomized Rust/Java pairs;
each fresh process used three warmup rounds and exactly one measured round.
Publisher/consumer roles were pinned to physical cores, every checksum and
affinity check passed, and every observed steal-time delta was zero.

The BadBatch baseline was commit `887ef84`; LMAX Disruptor was pinned to
`c871ca49826a6be7ada6957f6fbafcfecf7b1f87`. Rust used
`RUSTFLAGS=-C target-cpu=native`; Java used a fixed 2 GiB heap with
`AlwaysPreTouch`. Results are medians of the captured forks:

| Scenario | BadBatch Melem/s | LMAX Java Melem/s | Median paired Rust/Java | Rust fork CV | Java fork CV |
|---|---:|---:|---:|---:|---:|
| unicast | 32.47 | 47.28 | 0.6833x | 0.058 | 0.270 |
| unicast batch | 424.09 | 156.95 | 2.5917x | 0.060 | 0.221 |
| MPSC batch, 3 producers | 140.51 | 165.35 | 0.8582x | 0.013 | 0.180 |
| pipeline, 3 stages | 23.95 | 113.33 | 0.2094x | 0.049 | 0.363 |

These are scenario-specific observations, not a global implementation ranking.
In particular:

- batch publishing amortizes per-claim cost and is BadBatch's strongest bulk
  throughput path in this study;
- single-event unicast and the three-stage pipeline retain substantial fixed
  per-event costs;
- MPSC is a separate multi-producer problem and cannot be improved by a
  single-producer specialization;
- Java fork dispersion is material. Seven of the 20 Java pipeline processes
  crossed both 50 and 100 Melem/s during their own four consecutive rounds
  (three warmups plus one measurement). That rules out treating the observed
  values as stable per-process modes;
- only Java unicast showed a clearly overlapping low cluster with the Rust
  range. Unicast-batch did not overlap, MPSC formed a broad continuum, and
  pipeline was merely in the same order of magnitude at its low samples.

The last two points are why medians must be read with raw round scatter rather
than converted into a “fast batching attractor” claim.

## Causal findings and safety boundary

A measurement-only experiment bypassed the checked single-producer claim lock.
It was intentionally unsafe for concurrent raw `Arc<SingleProducerSequencer>`
claim drivers and is not product code.

Across 20 paired forks per scenario, the unsafe bypass changed throughput by:

| Scenario | Paired bypass/checked median | Bootstrap 95% CI | Bypass wins |
|---|---:|---:|---:|
| unicast | 2.6038x | 2.3793–3.0655 | 20/20 |
| unicast batch | 1.4354x | 1.3712–1.5768 | 20/20 |
| pipeline | 1.5880x | 1.4970–1.6929 | 20/20 |

A producer-private padded atomic performing the same swap/store reproduced the
checked path (`0.9634x` paired median versus checked). PMU measurements also
showed the checked read-only arm at 294.8 cycles/event and IPC 0.475 versus
166.5 cycles/event and IPC 0.750 for the bypass. Together, these results support
uncontended atomic-RMW serialization as the main direct mechanism on this CPU,
not contention on the lock's original cache line.

The safe optimization target is therefore narrow:

1. retain the checked lock on all public/raw sequencer claim paths;
2. share one claim algorithm implementation;
3. add a crate-private lock-free entry reachable only through the Builder's
   uniquely owned, `&mut self`-driven `SimpleProducer` capability;
4. preserve concurrent raw-driver, nested publish, poison and shutdown tests;
5. measure the safe candidate against the unchanged checked path before making
   a product claim.

The unsafe ratios are approximate upper bounds for tested single-producer arms,
not promised gains. They do not apply to MPSC.

## Findings that still require qualification

### Pipeline batching and pacing

The old baseline records per-round throughput but not batch-size, queue-depth or
producer backpressure counts. The unicast lock/backoff matrix also does not
establish whether the capacity-wait path was materially exercised. It validly
rejects those backoff arms as product wins in that unicast configuration, but
does not evaluate pipeline-only pacing or hysteresis.

Use `--round-diagnostics` before changing pacing. It preserves every round and
records per-stage batch/queue histograms plus producer capacity-wait entries and
iterations. Rust iterations count `spin_loop()` calls; Java iterations count
`parkNanos(1L)` requests, so their raw totals are not equal-duration units.

### Ring-slot writes and padding

In an unpadded 32-byte event layout, writing one or three derived fields back to
the shared ring slot reduced throughput by roughly 6–17% and 11–14%,
respectively. PMU/c2c data supports cache-line ownership traffic as a mechanism,
but two adjacent events also share one 64-byte line in that experiment.
Therefore the measured penalty may combine same-slot ownership transfer with
adjacent-slot false sharing.

Do not assume padding helps: on the Apple Silicon tiny-event baseline, 128-byte
padding was approximately 5–6x slower because the larger working set dominated.
The Linux padding × handler-write cross has not yet been measured, so its sign
is deliberately left open.

## Reproduction

Canonical exploratory run:

```bash
RUSTFLAGS="-C target-cpu=native" \
  bash scripts/run_head_to_head.sh --scenario all --mode quick
```

Controlled Linux baseline shape:

```bash
RUSTFLAGS="-C target-cpu=native" \
  bash scripts/run_head_to_head.sh \
    --scenario all --mode full --order both-orders \
    --forks 20 --seed 20260720 --cpu-list 2,3,4,5 \
    --results-dir /path/to/empty/results-directory
```

Per-round pipeline diagnosis:

```bash
RUSTFLAGS="-C target-cpu=native" \
  bash scripts/run_head_to_head.sh \
    --scenario pipeline --mode full --order both-orders \
    --forks 20 --seed 20260720 --cpu-list 2,3,4,5 \
    --round-diagnostics \
    --results-dir /path/to/empty/diagnostic-results-directory
```

The results directories contain raw fork JSON, flattened per-round diagnostics,
environment hashes and an interpretation-boundary report. Raw captures are
intentionally not committed to Git.
