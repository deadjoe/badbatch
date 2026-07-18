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

### Lifecycle residual closure (post-audit)

Closes the four secondary residuals left after P0–P2:

- **`try_run_once` fatal `on_batch_start`.** Matches the sequential engine:
  failure freezes the sequence, poisons producers, and stops the processor.
- **DSL drain shutdown.** `Disruptor::shutdown` / `shutdown_timeout` now close
  claims, drain the published backlog, then halt; `halt()` is the abrupt path.
  Post-stop publishes return `DisruptorError::Shutdown`.
- **Sequencer close flag.** `Sequencer::close` freezes claims on clean
  halt/shutdown (Builder and DSL). Distinct from poison (failure path).
- **ElegantConsumer panic hooks.** Handler panics are caught; optional
  `*_with_panic_hook` constructors run a hook (e.g. sequencer poison) before
  re-raising. Default constructors still catch+stop without a hook.
- **Verification depth.** Integration tests for publish-after-halt and DSL
  drain; Miri CI expanded with poller prefix-ack, try_run_once fatal, and
  immediate DSL start/shutdown.

### Surface & validation cleanup (2026-07-18 audit P2 round)

- **Dead surface removed.** `Cursored`, `Sequenced`, and `EventSink` traits
  (no implementations or callers) are gone; `DataProvider` is re-exported
  from `disruptor::`. The empty `full-benchmarks` feature is removed.
- **Feature hygiene.** parking_lot's deadlock detector is opt-in via the new
  `deadlock-detection` feature (was unconditional). The four diagnostic /
  benchmark binaries (~4.3K lines) require the new `bench-tools` feature.
  `panic = "abort"` is removed from the release profile — the P1 poisoning
  design depends on unwinding.
- **README is now executable.** Every Rust block compiles and runs as a
  doctest (`#[doc = include_str!]`); the poller example's three
  audit-confirmed errors are fixed, and all examples use the current
  fallible-publish API.
- **CI hardening.** New macOS ARM64 job (clippy + full tests + core-only);
  `cargo audit` / `cargo deny` failures are hard failures under CI
  (previously warnings).
- **Docs accuracy.** Stale `# Panics` sections on the ring-buffer accessors
  removed (mask+cast cannot panic); the DSL is documented as the
  Java-compat surface and `ElegantConsumer` as legacy with its poisoning
  gap called out; the DSL shutdown's fixed 1 ms sleep and redundant SeqCst
  fence are removed.

### Failure & lifecycle semantics (2026-07-18 audit P1 round — breaking)

Silent failure modes identified by the audit are now explicit, delivered
results. Five changes, each breaking for callers of the old signatures:

- **Fallible publishing.** `Producer::publish` / `batch_publish` (and the
  handle/`CloneableProducer` delegates) return `Result<i64>` with the
  published sequence. Failed claims were previously logged via
  `internal_error!` (silent in release without `BADBATCH_LOG`) and dropped —
  `batch_publish(n > capacity)` lost the whole batch invisibly.
- **Panic poisoning** (`DisruptorError::Poisoned`). A panicking consumer
  thread (builder consumers, `BatchEventProcessor::run`) poisons the
  producers: blocking claims fail fast — including from inside the
  backpressure spin loop — instead of spinning forever on a dead gating
  sequence. A panicking producer update closure (or DSL translator) poisons
  the pipeline instead of exposing a never-written slot on the next publish.
  `ElegantConsumer` (extras/legacy) is not yet covered.
- **Handler error policy.** `ExceptionHandler::handle_event_exception`
  returns an `ErrorDecision` (`Stop` / `Continue`). `DefaultExceptionHandler`
  now stops the processor (LMAX `FatalExceptionHandler` semantics) and
  poisons the producers; the old skip-and-continue behavior is opt-in via
  `IgnoreExceptionHandler` or the new DSL entry point
  `handle_events_with_exception_handler`. `on_batch_start` errors are fatal
  instead of discarded.
- **Draining shutdown.** `DisruptorHandle::shutdown()` drains the published
  backlog before stopping (LMAX `shutdown()`), aborting early when the
  pipeline is poisoned or a consumer already died; `shutdown_timeout(d)`
  bounds the drain and reports Timeout/Poisoned/ShutdownError; `halt()` is
  the explicit abrupt stop (old behavior). `Drop` performs a 2s bounded
  drain instead of discarding the backlog.
- **Poller partial commit.** Dropping an `EventBatch` acknowledges only the
  events actually taken via `next_mut`; untaken events are redelivered.
  `ack_all()` is the explicit full-batch acknowledgement (the old drop
  behavior).

Coverage: `tests/failure_semantics.rs`, `tests/shutdown_semantics.rs`,
poller partial-commit unit tests, and reworked exception-policy integration
tests.

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
