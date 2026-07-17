<div align="center">
  <img src="badbatch.png" alt="BadBatch logo" width="200"/>

  # BadBatch

  [![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
  [![Tests](https://github.com/deadjoe/badbatch/workflows/Tests/badge.svg)](https://github.com/deadjoe/badbatch/actions)
</div>

A Rust implementation of the [LMAX Disruptor](https://github.com/LMAX-Exchange/disruptor) pattern: a pre-allocated ring buffer with sequence-based coordination for high-throughput, low-latency event processing between threads.

**Preferred API:** monomorphized Builder (`build_single_producer` / `build_multi_producer`), `Producer`, and `EventPoller`. An LMAX-style DSL (`Disruptor`) and `ElegantConsumer` are available behind optional features (enabled by default).

---

## What this crate provides

- **Ring buffer** — power-of-two capacity, pre-allocated slots, access coordinated by sequencers (not by locking the buffer)
- **Single- and multi-producer sequencers** — multi-producer uses an availability structure (bitmap path for larger buffers; legacy path for smaller ones)
- **Wait strategies** — Blocking, BusySpin, Yielding, Sleeping
- **Consumers** — sequential handlers, same-stage WorkerPool (CAS work-claim), read-only fan-out (`fan_out_events_with`), pipeline stages via `and_then`
- **Event handlers / factories / translators** — LMAX-style extension points
- **Optional surfaces** — feature `lmax-dsl` (classic `Disruptor` DSL), feature `extras` (`ElegantConsumer`)

Cross-thread use is **protocol-based** (claim / publish / barriers / consumer sequences). There is no mutex-wrapped “shared ring buffer” API.

**Platforms:** macOS and Linux. Windows is not a target.

**MSRV:** Rust **1.97** (`rust-version` in `Cargo.toml`). Prefer current stable via `rust-toolchain.toml`.

---

## Quick start (Builder — recommended)

```rust
use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};

#[derive(Debug, Default)]
struct MyEvent {
    value: i64,
}

fn main() {
    let mut handle = build_single_producer(1024, MyEvent::default, BusySpinWaitStrategy)
        .handle_events_with(|event, sequence, _end_of_batch| {
            // process event at `sequence`
            let _ = (event, sequence);
        })
        .build();

    handle.publish(|event| event.value = 42);

    handle.batch_publish(5, |batch| {
        for (i, event) in batch.enumerate() {
            event.value = i as i64;
        }
    });

    handle.shutdown();
}
```

### Same-stage consumers (do not mix modes on one stage)

| API | Behavior |
|-----|----------|
| `handle_events_with` (2+ handlers) | **WorkerPool** — CAS claim; each sequence handled by one mutable consumer |
| `fan_out_events_with` | **Fan-out** — every consumer observes every sequence via `&E` |

Pipeline stages: `.and_then()` starts a dependent stage that waits on the previous stage’s sequences.

Multi-producer:

```rust
use badbatch::disruptor::{build_multi_producer, BusySpinWaitStrategy};

#[derive(Default)]
struct MyEvent { value: i64 }

let handle = build_multi_producer(1024, MyEvent::default, BusySpinWaitStrategy)
    .handle_events_with(|_e, _s, _b| {})
    .build();
// handle.create_producer() / CloneableProducer for additional publishers
```

### EventPoller (user-owned thread)

```rust
use badbatch::disruptor::{open_single_producer_poller, BusySpinWaitStrategy};

#[derive(Default)]
struct MyEvent { value: i64 }

let (mut poller, mut producer) =
    open_single_producer_poller(1024, MyEvent::default, BusySpinWaitStrategy).unwrap();
// producer.publish(...); poller.poll() / take() on the consumer thread
```

### Features

| Feature | Default | Contents |
|---------|---------|----------|
| (always on) | — | Builder, sequencers, `consumer_engine`, EventPoller, wait strategies |
| `lmax-dsl` | yes | Classic `Disruptor` DSL / `BatchEventProcessor`-oriented API |
| `extras` | yes | `ElegantConsumer` and related helpers |
| `full-benchmarks` | no | Extra benchmark surface |

Core-only: `cargo test --lib --no-default-features`.

Engineering notes: [`docs/MODERNIZATION.md`](docs/MODERNIZATION.md), [`docs/README.md`](docs/README.md).

---

## LMAX-style DSL (feature `lmax-dsl`)

Requires a monomorphized wait strategy (not `Box<dyn WaitStrategy>`):

```rust
use badbatch::disruptor::{
    BlockingWaitStrategy, DefaultEventFactory, Disruptor, EventHandler, EventTranslator,
    ProducerType,
};

#[derive(Debug, Default)]
struct MyEvent {
    value: i64,
    message: String,
}

struct MyEventHandler;
impl EventHandler<MyEvent> for MyEventHandler {
    fn on_event(
        &mut self,
        event: &mut MyEvent,
        sequence: i64,
        end_of_batch: bool,
    ) -> badbatch::disruptor::Result<()> {
        let _ = (event, sequence, end_of_batch);
        Ok(())
    }
}

struct MyEventTranslator {
    value: i64,
    message: String,
}
impl EventTranslator<MyEvent> for MyEventTranslator {
    fn translate_to(&self, event: &mut MyEvent, _sequence: i64) {
        event.value = self.value;
        event.message = self.message.clone();
    }
}

fn main() {
    let factory = DefaultEventFactory::<MyEvent>::new();
    let mut disruptor = Disruptor::new(
        factory,
        1024,
        ProducerType::Single,
        BlockingWaitStrategy::new(),
    )
    .unwrap()
    .handle_events_with(MyEventHandler)
    .build();

    disruptor.start().unwrap();
    disruptor
        .publish_event(MyEventTranslator {
            value: 42,
            message: "hello".to_string(),
        })
        .unwrap();
    disruptor.shutdown().unwrap();
}
```

---

## Development

### Build and test

```bash
git clone https://github.com/deadjoe/badbatch.git
cd badbatch
cargo build --release

cargo test
bash scripts/test-all.sh          # fmt, clippy, tests, audit/deny (as configured)

cargo test --lib
cargo test --test '*'
cargo test --doc
```

### Benchmarks and baseline

Criterion benches live under `benches/` (SPSC, MPSC, pipeline, latency, throughput, buffer scaling, comprehensive).

```bash
bash scripts/run_benchmarks.sh quick   # shorter
bash scripts/run_benchmarks.sh all     # full suite (long)

# Checked-in median baseline (Apple Silicon, specific commits — not a portable guarantee):
# benches/results/BASELINE.md
RUSTFLAGS="-C target-cpu=native" ./scripts/run_baseline.sh --full
```

On AArch64 with LSE (e.g. Apple Silicon), building with `target-cpu=native` or `+lse` can reduce cost of contended atomics on multi-producer paths.

### Slot padding

Events sit inline in the ring. Small events may share a cache line with neighbors. Optional slot padding:

- `with_cache_line_padding(true)` → 128-byte slots  
- or `with_slot_padding(SlotPadding::CacheLine64 | CacheLine128)`  

Default is **no** padding; measure on your hardware (on Apple Silicon, padding can hurt tiny-event SPSC throughput — see `BASELINE.md`).

### Repository notes

- **`Cargo.lock` is tracked** for reproducible CI (`--locked`). Library dependents do not inherit this lockfile.
- **`proptest-regressions/`** holds proptest failure seeds; keep under version control.
- **Changelog / MSRV policy:** [`CHANGELOG.md`](CHANGELOG.md).

### Quality tooling

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo audit
cargo deny check
cargo doc --no-deps
```

CI (GitHub Actions): stable tests, `--no-default-features`, loom claim tests, Miri on selected modules (nightly).

---

## Implementation notes (accurate scope)

| Topic | Reality in this repo |
|-------|----------------------|
| Lock-free core | Ring + sequencers + consumer loops use atomics/CAS; **Blocking** wait uses a mutex/condvar by design |
| Monomorphization | Preferred Builder path is generic over wait strategy and handlers |
| Multi-producer availability | Bitmap-style path for larger buffers; fallback for smaller sizes (see sequencer) |
| Gating sequences | `arc-swap` for producer-side gating list reads |
| Affinity | Optional `pin_at_core` / `ThreadBuilder` — not a full NUMA runtime |
| Formal methods | TLA+ **model checking** under `verification/` (not a complete proof of the Rust binary) |
| Performance claims | No portable “N Mops” guarantee; use benches + `BASELINE.md` as machine-specific data |

Design background: [`DESIGN.md`](DESIGN.md). Consistency notes: [`verification/CONSISTENCY.md`](verification/CONSISTENCY.md).

### TLA+ models

Models and scripts live in `verification/`:

- `BadBatchSPMC.tla`, `BadBatchMPMC.tla`, `BadBatchPipeline.tla`, `BadBatchRingBuffer.tla`, `SimpleSPMC.tla`
- Runner: `cd verification && ./verify.sh` (see `verification/README.md` for configs and state counts)

---

## Contributing

1. Fork and branch  
2. Keep changes focused; add tests for behavioral changes  
3. Run `bash scripts/test-all.sh` (or at least `cargo test` + clippy)  
4. Open a pull request  

This project follows the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct).

---

## License

Licensed under the **Apache License, Version 2.0** ([LICENSE](LICENSE)).

```
Copyright 2025–2026 Joe <smartjoe@gmail.com>
```

---

## Acknowledgments

- [LMAX Disruptor](https://github.com/LMAX-Exchange/disruptor) — original design  
- [disruptor-rs](https://github.com/nicholassm/disruptor-rs) — API ideas for the Builder-style surface  
- [crossbeam](https://github.com/crossbeam-rs/crossbeam) — `CachePadded` and related utilities  
- [TLA+](https://lamport.azurewebsites.net/tla/tla.html) — specification language used in `verification/`

## Links

- [Issues](https://github.com/deadjoe/badbatch/issues)  
- [DESIGN.md](DESIGN.md)  
- [CHANGELOG.md](CHANGELOG.md)  
- [verification/](verification/)  
- [benches/](benches/)  
