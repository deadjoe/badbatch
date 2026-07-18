# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
for the crate version in `Cargo.toml`.

**MSRV policy:** `package.rust-version` tracks a recent stable (currently **1.97**).
Bumps happen when the project adopts language/stdlib features that require a newer
compiler, or when CI stabilizes on a new stable series. Patch toolchain updates
(e.g. 1.97.0 → 1.97.1) do not require a crate version bump.

## [0.2.0] — 2026-07

### Soundness (2026-07-18 cross-audit — breaking)

Four independent safe-API undefined-behavior holes, each reproduced under Miri
from `#![forbid(unsafe_code)]` callers, were closed. The safe surface shrank;
low-level composition now requires `unsafe` with documented contracts.

- **Producer capability typing.** `SimpleProducer` no longer implements
  `Clone`; `DisruptorHandle<E, W, M>` carries a `SingleProducerMode` /
  `MultiProducerMode` marker and only multi-mode handles have
  `create_producer`. `SimpleProducer::new`, `CloneableProducer::new`,
  `DisruptorCore` and `create_disruptor_core` are crate-private.
  `SingleProducerSequencer::new` is `unsafe fn` (single claim-driver
  contract). Cloning a single-mode producer used to race on the sequencer's
  non-atomic `UnsafeCell` claim state.
- **Ring buffer read access.** `RingBuffer::get` and `DataProvider::get` are
  `unsafe fn`: a safe `&T` could previously be held across a producer write to
  the same slot (Stacked Borrows violation). `Disruptor::get_ring_buffer` is
  removed. `EventPoller::new` and `BatchEventProcessor::new` are `unsafe fn`
  with topology contracts; use `open_single_producer_poller`, the Builder, or
  the DSL, which discharge them internally.
- **Handler exclusivity.** `BatchEventProcessor` keeps its handler behind
  `parking_lot::Mutex` (locked once per `run()`, `try_lock` on cold lifecycle
  methods) instead of `UnsafeCell` + hand-written `Sync`; concurrent
  `on_start`/`try_run_once` used to alias `&mut H`.
- **DSL publish serialization.** `Disruptor` (lmax-dsl) is `Sync` with
  `publish_event(&self)`; in `ProducerType::Single` mode a shared
  `Arc<Disruptor>` could drive the unsynchronized claim path from two
  threads. Single-mode DSL publishing now serializes claim access behind an
  internal mutex (multi mode takes no lock).
- **Claim bounds.** `try_next_n` (single and multi) rejects `n < 1` and
  `n > buffer_size`, and producer-side capacity checks clamp the gating
  minimum to the producer position (LMAX `Util.getMinimumSequence(seqs, min)`
  semantics) instead of treating an empty gating list as `i64::MAX`. A
  capacity-8 sequencer used to accept `try_next_n(1000)` and alias `&mut`
  slots within one batch.

Regression coverage: `tests/soundness_regression.rs`, two `compile_fail`
doctests (producer clone, single-mode `create_producer`), and an extended
Miri CI subset (claim bounds, concurrent lifecycle).

### License

- Project license changed from **AGPL-3.0** to **Apache-2.0** (see `LICENSE`, `NOTICE`).

### Added

- Monomorphized hot path (`WaitStrategy` / barrier / handler generics; no hot-path `dyn`).
- Unified `consumer_engine` loops shared by Builder and BatchEventProcessor.
- WorkerPool CAS work-claim for same-stage parallel mutable consumers.
- Read-only fan-out (`fan_out_events_with`) distinct from WorkerPool.
- EventPoller for user-owned consumer threads.
- 128-byte slot padding option (`CacheLine128` / `with_cache_line_padding`).
- Feature gates: `lmax-dsl`, `extras` (default on); core path works with `--no-default-features`.
- Concurrency verification: loom claim models, MPSC exactly-once stress, Miri CI subset.
- `rust-toolchain.toml` (stable) for local/CI alignment.

### Changed

- Builder module split (`builder/{core,consumer,fluent,handle,entry}`).
- Stage access model: `ConsumerAccess` + `RunMode` (no readonly bool / Option work magic).
- Single `ClosureEventHandler`; `StopFlag` stop model for builder vs BEP.
- MSRV declared as `1.97`.

### Removed / clarified

- Dishonest `Clone` on consumer join-handle ownership paths.
- Dual fluent `ClosureEventHandler` and static stop conventions.
- **`SharedRingBuffer` and feature `shared-ring-buffer`** — was a global `RwLock`
  over the ring (not LMAX protocol). Share via Builder / `CloneableProducer` /
  `EventPoller` instead.

## [0.1.x] — prior

Historical pre-modernization releases; see git history for detail.
