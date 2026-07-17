# Verification–Implementation Consistency Audit

This note maps the Rust implementation to the TLA+ models and highlights checks to keep the proofs reliable as the code evolves.

## Scope

- Models: `BadBatchSPMC.tla`, `BadBatchMPMC.tla`, `BadBatchPipeline.tla`, `SimpleSPMC.tla`, `BadBatchRingBuffer.tla` (module instantiated by the others)
- Rust: core under `src/disruptor/*` — `ring_buffer.rs`, `sequencer.rs`, `sequence_barrier.rs`, `sequence.rs`, `event_processor.rs`, `builder.rs`, wait strategies.

## Key Semantics Alignment

- Ring indexing: TLA `IndexOf(sequence) == sequence % Size` matches Rust `index = sequence & (buffer_size-1)` in `RingBuffer`.
- Single-producer publish: TLA `published` is a scalar cursor; Rust `SingleProducerSequencer.publish` sets `cursor` to the last published sequence. `is_available(seq) := seq <= cursor` matches.
- Multi-producer claim vs publish: TLA `next_sequence` is a shared claim counter; writers can publish out of order. Rust `MultiProducerSequencer` cursor tracks highest claimed; publication sets per-slot availability. Barriers compute contiguity via `get_highest_published_sequence`. TLA consumers gate on `IsPublished(next)` which enforces contiguity when advancing — equivalent to barrier-side contiguity convergence.
- Availability encoding: TLA `published[index]` toggles even/odd on each publication; consumers compare with expected round parity. Rust:
  - Legacy path stores parity flag per index (`available_buffer[index] = (seq >> shift) & 1`).
  - Bitmap path flips a bit (`fetch_xor`) with round-flag comparison. Both are semantically equivalent to the TLA parity-toggling model.
- Backpressure: TLA checks `MinReadSequence >= seq - Size` before `BeginWrite`. Rust checks identical `wrap_point > min_sequence` logic in both sequencers.

## Safety/Liveness Coverage

- No data races: TLA `BadBatchRingBuffer.NoDataRaces` prohibits overlapping reader/writer on the same slot and multiple concurrent writers to a slot. This is enforced in Rust by sequencers/gating and exclusive claims prior to writes.
- Ordering: TLA readers always consume `read[r]+1` and require `IsPublished(next)` (MPMC) or `published >= next` (SPMC). This matches Rust barriers returning highest contiguous published sequence; consumers cannot observe gaps.
- Pipeline dependency: TLA `BadBatchPipeline.DependencyOrder` asserts `read2[s2] <= read1[s1]` — Stage2 never overtakes Stage1. Rust enforces this via `ProcessingSequenceBarrier` tracking the previous stage's sequences with `MinStage1 >= next` gating.
- Progress: TLA `Liveness` asserts eventual consumption of all published events under fairness — aligned with wait strategies signaling and sequencer progress in Rust.

## Tooling

- `verification/verify.sh` runs TLC via `tlc` or `tla2tools.jar` with `-deadlock`. Local jar included for offline runs.

## What To Re-check After Code Changes

- If changing cursor semantics or publish mechanics, ensure:
  - Single-producer still sets cursor on publish and `is_available(seq) := seq <= cursor` remains true.
  - Multi-producer still marks per-slot availability with even/odd parity and barriers still compute contiguity via `get_highest_published_sequence`.
- If altering backpressure logic, keep the `wrap_point <= min(gating)` invariant equivalent to the TLA `MinReadSequence` guard.
- If adding batch publish/consume, confirm it decomposes to per-sequence publish steps consistent with parity toggling; otherwise extend the model.
- If introducing new wait strategies or memory ordering changes, the TLA specs remain valid (they abstract from timing), but re-run TLC to catch unintended behaviors.
- If altering the barrier contiguity path (e.g. the `needs_contiguity_check` flag in `ProcessingSequenceBarrier`), ensure single-producer still publishes in order so the available sequence from the wait strategy is already contiguous — matching the TLA SPMC model which uses a scalar cursor (`published >= next`) rather than per-slot `IsPublished`.

## Round-2 Optimization Consistency (2026-06-25)

The following optimizations were verified against the TLA+ models:

1. **Barrier SP contiguity skip** (`ProcessingSequenceBarrier.needs_contiguity_check`):
   - Single-producer sequencers publish in order, so the wait strategy's available sequence is already the contiguous prefix.
   - The TLA SPMC model already uses a scalar cursor guard (`published >= next`) rather than per-slot `IsPublished`, so skipping `get_highest_published_sequence` for single-producer is consistent with the model.
   - Multi-producer still calls `get_highest_published_sequence` — matches TLA MPMC `IsPublished(next)` contiguity enforcement.

2. **BatchEventProcessor DataProvider de-virtualization** (concrete `Arc<RingBuffer<T>>` instead of `Arc<dyn DataProvider<T>>`):
   - Pure implementation change; the access pattern and safety contract are identical. No TLA impact.

All 5 TLA+ models re-verified successfully after these changes.

## Round-3 Modernization Consistency (2026-07-17)

1. **Monomorphized wait strategy / barrier** (`Arc<W>`, `ProcessingSequenceBarrier<W>`):
   - Implementation change only; publish/contiguity/backpressure protocol unchanged.
2. **WorkerPool scheme A** (same-stage parallel consumers, CAS claim on shared work sequence):
   - Not modeled in current TLA specs (they model exclusive sequential consumers / pipeline stages).
   - Safety argument in code: exclusive CAS claim ⇒ single mutator per sequence; gating uses min(worker sequences).
   - Optional model enhancement: add a WorkProcessor claim counter with mutual exclusion on claimed sequences.
3. **Slot padding CacheLine128**: layout-only; no protocol impact.

## Round-4 Consumer Engine + SP Cell (2026-07-17)

1. **Unified `consumer_engine`**: Builder and `BatchEventProcessor` share sequential/work-processor loops — no protocol change.
2. **SP `next_value`/`cached_value` → `UnsafeCell`**: still single-publisher exclusive; cursor/gating remain the cross-thread coordination surface (matches LMAX nextValue/cachedValue thread discipline).
3. **API features**: `lmax-dsl` / `extras` gate secondary surfaces; default keeps them on for continuity.

## Suggested Model Enhancements (Optional)

- Add a simple safety property asserting per-reader monotonic consumption (prefix strictly increasing), though it’s already implied by the state machine and `Liveness`.
- Add a small comment in the MPMC model noting that `next_sequence` models atomic CAS claims; interleaving is covered by independent writers’ `pc` state.

## Current Assessment

- The TLA+ models match the current Rust implementation's core invariants and concurrency semantics for SPMC, MPMC, and Pipeline scenarios.
- The availability/parity mechanism in the models is consistent with both the legacy per-index flag and the bitmap path in Rust.
- The models remain reliable to guard core correctness as the code evolves, provided the checks above are revisited with behavioral changes.
- Last full re-verification: 2026-06-25 (Java 26, all 5 models pass).

