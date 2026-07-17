# BadBatch Modernization Plan (active)

Internal alignment doc for agent/developer work. Reflects **current intent**, not marketing.

## Goals (priority order)

1. Maximize Rust advantages: monomorphization, type-system exclusivity, precise memory layout, lock-free protocols.
2. Keep LMAX Disruptor **protocol** correct (sequence claim/publish, contiguity, backpressure, barriers).
3. Prefer lock-free designs only — no “documented lock-free / coded with Mutex” drift.
4. Target platforms: **macOS + Linux**. Windows is out of scope.
5. Toolchain: **latest stable Rust** for build/test; **nightly only** where required (Miri).
6. No crates.io / license / external-user compatibility constraints.

## Phase status

| Phase | Scope | Status |
|-------|--------|--------|
| 0 | Trust: honest docs/CI, Miri job, latest stable MSRV | done (MSRV 1.97 / v0.2.0; honest CI + Miri job) |
| 1 | API convergence, WaitStrategy merge, module split, quarantine SharedRingBuffer | done (`lmax-dsl` / `extras` features; preferred Builder+Poller; SharedRingBuffer honesty) |
| 2 | Monomorphize hot path (`W`/`Barrier`/`Handler`), remove hot-path `dyn` | done |
| 3 | LMAX WorkerPool / CAS work-sequence for same-stage parallel consumers (no slot Mutex) | done |
| 4 | Slot padding Align128; unify with Sequence 128B padding model | done |
| 5 | Verification: Miri, loom (as useful), TLA consistency, bench notes | done (Miri CI; claim stress test; baseline script; loom optional deferred) |
| 6 | EventPoller / read-only fan-out | EventPoller done; read-only fan-out deferred |
| N1 | Unified consumer engine | done (`consumer_engine`; Builder + BatchEventProcessor share loops) |
| N2 | macOS performance baseline harness | done (`baseline_metrics` + `benches/results/BASELINE.md` medians) |
| N3 | WorkerPool claim stress | done (`tests/worker_pool_claim_stress.rs`) |
| P0 | Performance evidence closed loop | done (3-run medians on Apple Silicon; padding/batch/contention conclusions) |
| P1 | builder split + feature/docs tighten | done (`builder/{core,consumer,handle,fluent,entry}`; core-only CI job) |

## Architecture decisions

### AD-1: Protocol vs API

- **Protocol layer** stays LMAX-faithful (cursor, gating, multi-producer availability bitmap, barriers).
- **API/implementation layer** is free to be idiomatic Rust (generics, type-state, `!Sync` single producer).

### AD-2: Primary public API

- **Preferred path**: monomorphized Builder (`build_single_producer` / `build_multi_producer`).
- LMAX-style `Disruptor` DSL remains as a compatibility/learning surface until folded under a feature or thin wrapper.
- `ElegantConsumer` / dual wait-strategy story must converge so users are not misled.

### AD-3: Parallel same-stage consumers = WorkerPool (scheme A)

- **Not** per-slot `Mutex`.
- Workers CAS-claim sequences from a shared work cursor; processing follows LMAX WorkProcessor spirit.
- Mutable fan-out is only sound if claim grants exclusive sequence ownership (one worker per sequence).

### AD-4: SharedRingBuffer

- Feature-gated, **not** part of the lock-free core story.
- Documented as convenience / non-lock-free; candidate for removal or redesign (e.g. true shared-memory later).

### AD-5: Padding

- Sequence already uses `CachePadded` → **128-byte** alignment on x86_64/aarch64.
- Slot padding must offer **Align128** (not only 64) for false-sharing control on modern CPUs.
- Default remains `None` (inline); padding is opt-in after measurement.

### AD-6: CI truth

- Claim only what CI runs.
- macOS: primary local dev; Linux: CI (and later local Linux box).
- Miri: separate nightly job; not claimed for stable until it actually runs.

## Non-goals (for this modernization)

- Windows support
- Downstream API stability / semver anxiety
- Chasing stars / crates.io adoption packaging polish beyond correctness


## Implementation notes (phases 2–4)

### Monomorphization (phase 2)

- `SingleProducerSequencer<W>`, `MultiProducerSequencer<W>`, `SequencerEnum<W>`
- `ProcessingSequenceBarrier<W>` stores `Arc<W>` (no `dyn WaitStrategy` on hot path)
- Builder consumer loops use `Arc<ProcessingSequenceBarrier<W>>`
- `SimpleProducer<T, W>`, `DisruptorCore<E, W>`, `DisruptorHandle<E, W>`, `Disruptor<T, W>`
- `BatchEventProcessor<T, H, W>` owns `H` via `UnsafeCell` (no Mutex on hot path); `dyn EventProcessor` only at DSL boundary
- `Sequencer::new_barrier` removed from trait; concrete `SequencerEnum::new_barrier` returns monomorphized barrier

### WorkerPool scheme A (phase 3)

Same-stage parallel consumers (`stage width > 1`) share a `CachePadded<AtomicI64>` work cursor (init `INITIAL_CURSOR_VALUE` / -1). Workers CAS-claim the next sequence, wait on the barrier, process exclusively, then update their own gating sequence. Minimum of worker sequences provides producer backpressure. No per-slot `Mutex`.

### Slot padding (phase 4)

`SlotPadding::{None, CacheLine64, CacheLine128}`. Builder `with_cache_line_padding(true)` maps to **CacheLine128**. Explicit `with_slot_padding` retained for 64-byte experiments.
