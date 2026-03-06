# BadBatch Benchmarks

This directory contains Criterion benchmarks for the BadBatch Disruptor engine. The goal is to measure steady‑state throughput/latency across common topologies (SPSC/MPSC/pipelines), plus comparisons against channels.

## Running

```bash
# Recommended runner (logs to benchmark_logs/)
./scripts/run_benchmarks.sh quick
./scripts/run_benchmarks.sh all

# Run a single suite directly
cargo bench --bench single_producer_single_consumer
cargo bench --bench multi_producer_single_consumer
```

Notes:
- The script uses `timeout` (Linux) or `gtimeout` (macOS via `coreutils`) when available.
- Benchmarks emit `WARNING:` lines on timeouts; the runner surfaces these counts.

## Benchmark Design Principles

- **Avoid setup/teardown in timed loops**: most suites create the Disruptor once per benchmark case and use `iter_custom` for steady‑state measurement.
- **No sleeps/printing in hot paths**: benchmark warnings are emitted only on errors/timeouts.
- **Multi‑threaded benchmarks use persistent threads** where possible (spawning threads per iteration will dominate measurements).
- **Separate API-path vs peak-path results**: SPSC/MPSC suites include single-event publication cases to reflect common application usage, while raw throughput cases favor batch/range publication to align with LMAX Disruptor and `disruptor-rs`.

## Suites (benches/*.rs)

- `comprehensive_benchmarks.rs`: quick “sanity” suite for CI/dev; small SPSC/throughput/latency checks.
- `single_producer_single_consumer.rs`: SPSC throughput across wait strategies; includes both per-event API-path cases and reference-aligned batch publication cases.
- `multi_producer_single_consumer.rs`: true MPSC publishing with `PRODUCER_COUNT = 3`; producers are persistent threads triggered per Criterion iteration, with both single-event and batch publication cases.
- `pipeline_processing.rs`: 2/3/4 stage handler dependency pipelines with BusySpin/Yielding.
- `latency_comparison.rs`: one-way publish→on_event latency comparison (Disruptor vs std/crossbeam channels) using a fixed batch per iteration.
- `throughput_comparison.rs`: peak steady-state throughput comparison across buffer sizes/wait strategies, favoring batch publication on the Disruptor side to match reference projects more closely.
- `buffer_size_scaling.rs`: explores buffer size/workload interactions; includes a small “buffer utilization” micro‑bench and a lightweight memory/payload probe.

## Interpreting Results

- Throughput is reported as `Kelem/s`, `Melem/s`, or `Gelem/s` (elements/events per second).
- Criterion may print `change:` blocks if it finds prior baselines; those are not additional benchmark cases.
- The runner summary reports the highest-throughput non-baseline row as the `Peak Case`; inspect the full case list before drawing conclusions from a single headline number.
- Numbers vary significantly by CPU power mode, OS scheduling, Rust version, and build flags. Treat results as **relative comparisons** on the same machine and revision.

## Reports

```bash
./scripts/run_benchmarks.sh report
# HTML output: target/criterion/reports/index.html
```

**Last Updated**: 2026-03-06
