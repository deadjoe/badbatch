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
