//! Sequencer Implementation
//!
//! This module provides sequencer implementations for coordinating access to the ring buffer.
//! Sequencers manage the allocation of sequence numbers and ensure that producers don't
//! overwrite events that haven't been consumed yet.

use crate::disruptor::sequence_barrier::ProcessingSequenceBarrier;
use crate::disruptor::{
    is_power_of_two, DisruptorError, FailureRecord, Result, Sequence, WaitStrategy,
};
use arc_swap::ArcSwap;
use crossbeam_utils::CachePadded;
use parking_lot::Mutex;
use std::cell::UnsafeCell;
use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

#[cfg(feature = "bench-round-diagnostics")]
static BENCH_ROUND_DIAGNOSTICS_ENABLED: AtomicBool = AtomicBool::new(false);

/// Benchmark-only snapshot of single-producer capacity-wait activity.
///
/// Available only with `bench-round-diagnostics`; it is not a production
/// metrics API. One iteration means one `spin_loop()` call on the Rust path.
#[cfg(feature = "bench-round-diagnostics")]
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BenchProducerBackpressureSnapshot {
    /// Number of claims that entered the capacity-wait loop.
    pub entries: u64,
    /// Total capacity-wait loop iterations across those entries.
    pub wait_loop_iterations: u64,
    /// Largest capacity-wait loop iteration count for one claim.
    pub max_wait_loop_iterations: u64,
}

/// Enable or disable benchmark-only per-round capacity-wait counters.
///
/// Call this before starting producer threads. It exists solely for the H2H
/// harness and is compiled out of normal and `bench-tools`-only builds.
#[cfg(feature = "bench-round-diagnostics")]
#[doc(hidden)]
pub fn configure_bench_round_diagnostics(enabled: bool) {
    BENCH_ROUND_DIAGNOSTICS_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Trait for sequencers that coordinate access to the ring buffer
///
/// This trait defines the interface for sequencers, which are responsible
/// for allocating sequence numbers and coordinating between producers and consumers.
/// This follows the exact design from the original LMAX Disruptor Sequencer interface.
pub trait Sequencer: Send + Sync + std::fmt::Debug {
    /// Get the current cursor position
    ///
    /// # Returns
    /// The current cursor sequence
    fn get_cursor(&self) -> Arc<Sequence>;

    /// Get the buffer size
    ///
    /// # Returns
    /// The size of the ring buffer
    fn get_buffer_size(&self) -> usize;

    /// Claim the next sequence number
    ///
    /// # Returns
    /// The next available sequence number
    ///
    /// # Errors
    /// Returns an error if the buffer is full
    fn next(&self) -> Result<i64>;

    /// Claim the next n sequence numbers
    ///
    /// # Arguments
    /// * `n` - The number of sequence numbers to claim
    ///
    /// # Returns
    /// The highest sequence number claimed
    ///
    /// # Errors
    /// Returns an error if there isn't enough capacity
    fn next_n(&self, n: i64) -> Result<i64>;

    /// Try to claim the next sequence number without blocking
    ///
    /// # Returns
    /// The next available sequence number, or None if not available
    fn try_next(&self) -> Option<i64>;

    /// Try to claim the next n sequence numbers without blocking
    ///
    /// # Arguments
    /// * `n` - The number of sequence numbers to claim
    ///
    /// # Returns
    /// The highest sequence number claimed, or None if not enough capacity
    fn try_next_n(&self, n: i64) -> Option<i64>;

    /// Publish a sequence number
    ///
    /// # Arguments
    /// * `sequence` - The sequence number to publish
    fn publish(&self, sequence: i64);

    /// Publish a range of sequence numbers
    ///
    /// # Arguments
    /// * `low` - The lowest sequence number to publish
    /// * `high` - The highest sequence number to publish
    fn publish_range(&self, low: i64, high: i64);

    /// Check if a sequence is available
    ///
    /// # Arguments
    /// * `sequence` - The sequence to check
    ///
    /// # Returns
    /// True if the sequence is available for consumption
    fn is_available(&self, sequence: i64) -> bool;

    /// Get the highest published sequence
    ///
    /// # Arguments
    /// * `next_sequence` - The next sequence to check from
    /// * `available_sequence` - The highest known available sequence
    ///
    /// # Returns
    /// The highest published sequence
    fn get_highest_published_sequence(&self, next_sequence: i64, available_sequence: i64) -> i64;

    /// Add gating sequences that this sequencer must not overtake
    ///
    /// # Arguments
    /// * `gating_sequences` - The sequences that gate this sequencer
    fn add_gating_sequences(&self, gating_sequences: &[Arc<Sequence>]);

    /// Remove gating sequences
    ///
    /// # Arguments
    /// * `sequence` - The sequence to remove
    ///
    /// # Returns
    /// True if the sequence was removed
    fn remove_gating_sequence(&self, sequence: Arc<Sequence>) -> bool;

    /// Get the minimum sequence from gating sequences
    ///
    /// # Returns
    /// The minimum sequence that gates this sequencer
    fn get_minimum_sequence(&self) -> i64;

    /// Get the remaining capacity
    ///
    /// # Returns
    /// The remaining capacity in the buffer
    fn remaining_capacity(&self) -> i64;

    /// Check if there's available capacity for the required number of sequences
    ///
    /// # Arguments
    /// * `required_capacity` - The number of sequences needed
    ///
    /// # Returns
    /// True if the buffer has capacity, false otherwise
    fn has_available_capacity(&self, required_capacity: usize) -> bool;

    /// Poison the pipeline after a fatal consumer/producer failure. Blocking
    /// claim calls return [`crate::disruptor::DisruptorError::Poisoned`] and
    /// try-claims fail, instead of spinning on a gating sequence that will
    /// never advance (or exposing a never-written slot).
    ///
    /// Default is a no-op so external `Sequencer` implementations keep
    /// compiling; the built-in sequencers override it.
    fn poison(&self) {}

    /// Whether [`Sequencer::poison`] has been called.
    fn is_poisoned(&self) -> bool {
        false
    }

    /// Retain a structured pipeline failure if no earlier failure was recorded.
    ///
    /// The default is a no-op so external `Sequencer` implementations keep
    /// compiling. Built-in sequencers use first-failure-wins semantics.
    fn record_failure(&self, _failure: &FailureRecord) {}

    /// Return the first structured pipeline failure, when the sequencer stores one.
    fn first_failure(&self) -> Option<FailureRecord> {
        None
    }

    /// Record a failure before poisoning the pipeline.
    ///
    /// Recording first ensures a thread that observes the poisoned flag can
    /// immediately retrieve the causal context through [`Sequencer::first_failure`].
    fn poison_with_failure(&self, failure: &FailureRecord) {
        self.record_failure(failure);
        self.poison();
    }

    /// Close the claim path after a clean shutdown/halt: further `next` /
    /// `try_next` calls fail with [`crate::disruptor::DisruptorError::Shutdown`]
    /// (or `None` for try) so producers cannot keep writing into a halted ring.
    ///
    /// Default is a no-op so external `Sequencer` implementations keep compiling.
    fn close(&self) {}

    /// Whether [`Sequencer::close`] has been called.
    fn is_closed(&self) -> bool {
        false
    }

    /// Re-open the claim path after a prior [`Sequencer::close`] (e.g. DSL restart).
    /// Default is a no-op.
    fn reopen(&self) {}
}

/// Enum-based sequencer dispatch that eliminates vtable indirection.
///
/// Only two concrete sequencer types exist, so enum dispatch provides
/// the same runtime polymorphism as `Arc<dyn Sequencer>` but with
/// inline match dispatch instead of vtable lookups on every call.
#[derive(Debug)]
pub enum SequencerEnum<W>
where
    W: WaitStrategy + 'static,
{
    /// Single-producer sequencer variant
    Single(Arc<SingleProducerSequencer<W>>),
    /// Multi-producer sequencer variant
    Multi(Arc<MultiProducerSequencer<W>>),
}

impl<W> Clone for SequencerEnum<W>
where
    W: WaitStrategy + 'static,
{
    fn clone(&self) -> Self {
        match self {
            SequencerEnum::Single(s) => SequencerEnum::Single(Arc::clone(s)),
            SequencerEnum::Multi(m) => SequencerEnum::Multi(Arc::clone(m)),
        }
    }
}

impl<W> SequencerEnum<W>
where
    W: WaitStrategy + 'static,
{
    /// Create a monomorphized sequence barrier using this sequencer.
    ///
    /// Returns a concrete `ProcessingSequenceBarrier<W>` so the wait path
    /// has no vtable indirection on the hot consumer loop.
    pub fn new_barrier(
        &self,
        sequences_to_track: Vec<Arc<Sequence>>,
    ) -> Arc<ProcessingSequenceBarrier<W>> {
        match self {
            SequencerEnum::Single(s) => s.new_barrier_typed(sequences_to_track),
            SequencerEnum::Multi(m) => m.new_barrier_typed(sequences_to_track),
        }
    }

    #[cfg(feature = "bench-round-diagnostics")]
    pub(crate) fn bench_producer_backpressure_snapshot(
        &self,
    ) -> Option<BenchProducerBackpressureSnapshot> {
        match self {
            Self::Single(sequencer) => Some(sequencer.bench_backpressure_snapshot()),
            Self::Multi(_) => None,
        }
    }
}

/// Macro to delegate all Sequencer trait methods via match dispatch.
macro_rules! dispatch_sequencer {
    ($self:ident, $method:ident $(, $arg:expr)*) => {
        match $self {
            SequencerEnum::Single(s) => s.$method($($arg),*),
            SequencerEnum::Multi(m) => m.$method($($arg),*),
        }
    };
}

impl<W> Sequencer for SequencerEnum<W>
where
    W: WaitStrategy + 'static,
{
    #[inline]
    fn get_cursor(&self) -> Arc<Sequence> {
        dispatch_sequencer!(self, get_cursor)
    }

    #[inline]
    fn get_buffer_size(&self) -> usize {
        dispatch_sequencer!(self, get_buffer_size)
    }

    #[inline]
    fn next(&self) -> Result<i64> {
        dispatch_sequencer!(self, next)
    }

    #[inline]
    fn next_n(&self, n: i64) -> Result<i64> {
        dispatch_sequencer!(self, next_n, n)
    }

    #[inline]
    fn try_next(&self) -> Option<i64> {
        dispatch_sequencer!(self, try_next)
    }

    #[inline]
    fn try_next_n(&self, n: i64) -> Option<i64> {
        dispatch_sequencer!(self, try_next_n, n)
    }

    #[inline]
    fn publish(&self, sequence: i64) {
        dispatch_sequencer!(self, publish, sequence);
    }

    #[inline]
    fn publish_range(&self, low: i64, high: i64) {
        dispatch_sequencer!(self, publish_range, low, high);
    }

    #[inline]
    fn is_available(&self, sequence: i64) -> bool {
        dispatch_sequencer!(self, is_available, sequence)
    }

    #[inline]
    fn get_highest_published_sequence(&self, next_sequence: i64, available_sequence: i64) -> i64 {
        dispatch_sequencer!(
            self,
            get_highest_published_sequence,
            next_sequence,
            available_sequence
        )
    }

    #[inline]
    fn add_gating_sequences(&self, gating_sequences: &[Arc<Sequence>]) {
        dispatch_sequencer!(self, add_gating_sequences, gating_sequences);
    }

    #[inline]
    fn remove_gating_sequence(&self, sequence: Arc<Sequence>) -> bool {
        dispatch_sequencer!(self, remove_gating_sequence, sequence)
    }

    #[inline]
    fn get_minimum_sequence(&self) -> i64 {
        dispatch_sequencer!(self, get_minimum_sequence)
    }

    #[inline]
    fn remaining_capacity(&self) -> i64 {
        dispatch_sequencer!(self, remaining_capacity)
    }

    #[inline]
    fn has_available_capacity(&self, required_capacity: usize) -> bool {
        dispatch_sequencer!(self, has_available_capacity, required_capacity)
    }

    #[inline]
    fn poison(&self) {
        dispatch_sequencer!(self, poison);
    }

    #[inline]
    fn is_poisoned(&self) -> bool {
        dispatch_sequencer!(self, is_poisoned)
    }

    #[inline]
    fn record_failure(&self, failure: &FailureRecord) {
        dispatch_sequencer!(self, record_failure, failure);
    }

    #[inline]
    fn first_failure(&self) -> Option<FailureRecord> {
        dispatch_sequencer!(self, first_failure)
    }

    #[inline]
    fn poison_with_failure(&self, failure: &FailureRecord) {
        dispatch_sequencer!(self, poison_with_failure, failure);
    }

    #[inline]
    fn close(&self) {
        dispatch_sequencer!(self, close);
    }

    #[inline]
    fn is_closed(&self) -> bool {
        dispatch_sequencer!(self, is_closed)
    }

    #[inline]
    fn reopen(&self) {
        dispatch_sequencer!(self, reopen);
    }
}

fn add_gating_sequences_rcu(
    gating_sequences: &ArcSwap<Vec<Arc<Sequence>>>,
    to_add: &[Arc<Sequence>],
) {
    if to_add.is_empty() {
        return;
    }

    // Treat gating sequences as a set keyed by pointer identity (`Arc::ptr_eq`).
    // This matches the intended Disruptor model (each consumer has a unique `Sequence`) and
    // avoids accidental duplicates increasing the minimum-sequence scan cost.
    //
    // The closure is side-effect-free and can re-run if `rcu()` retries due to concurrent writers.
    gating_sequences.rcu(|current| {
        let mut updated = Vec::with_capacity(current.len() + to_add.len());

        // Keep existing sequences, but drop any accidental duplicates.
        for seq in current.iter() {
            if !updated.iter().any(|existing| Arc::ptr_eq(existing, seq)) {
                updated.push(Arc::clone(seq));
            }
        }

        // Add new sequences if not already present.
        for seq in to_add {
            if !updated.iter().any(|existing| Arc::ptr_eq(existing, seq)) {
                updated.push(Arc::clone(seq));
            }
        }

        updated
    });
}

fn remove_gating_sequence_cas(
    gating_sequences: &ArcSwap<Vec<Arc<Sequence>>>,
    sequence: &Arc<Sequence>,
) -> bool {
    let mut cur = gating_sequences.load();
    loop {
        // Fast-path: don't allocate/clone if it's not present in the currently observed snapshot.
        if !cur.iter().any(|s| Arc::ptr_eq(s, sequence)) {
            return false;
        }

        // Remove all matches (duplicates should not happen, but this keeps behavior robust).
        let updated = cur
            .iter()
            .filter(|s| !Arc::ptr_eq(s, sequence))
            .cloned()
            .collect::<Vec<_>>();

        let prev = gating_sequences.compare_and_swap(&*cur, updated.into());
        if Arc::ptr_eq(&*prev, &*cur) {
            return true;
        }
        cur = prev;
    }
}

/// Single producer sequencer
///
/// This sequencer is optimized for scenarios where only one thread will be
/// publishing events. It provides the best performance when you can guarantee
/// single-threaded publishing.
///
/// Key optimizations from LMAX Disruptor:
/// - nextValue: tracks the next sequence to be claimed (not published yet)
/// - cachedValue: cached minimum gating sequence for performance
/// - No atomic operations needed since only one producer thread
#[derive(Debug)]
pub struct SingleProducerSequencer<W>
where
    W: WaitStrategy + 'static,
{
    buffer_size: usize,
    buffer_size_i64: i64,
    wait_strategy: Arc<W>,
    needs_signal: bool,
    cursor: Arc<Sequence>,
    /// Gating sequences that this sequencer must not overtake.
    ///
    /// Uses `ArcSwap` for lock-free reads on the producer hot path (backpressure spin loop),
    /// matching LMAX Java's `volatile Sequence[]` + `AtomicReferenceFieldUpdater` design.
    /// Writes (add/remove) use `rcu()` with CAS retry — acceptable since they only occur
    /// during setup/shutdown, not on the hot path.
    gating_sequences: ArcSwap<Vec<Arc<Sequence>>>,
    /// Next sequence value to be claimed (equivalent to LMAX nextValue).
    ///
    /// Exclusive to the single publishing thread. Stored in `UnsafeCell` (not atomics)
    /// because LMAX guarantees single-threaded access; `SimpleProducer` is `!Sync` to
    /// reinforce that discipline at the handle layer. Concurrent access is rejected
    /// by [`Self::claim_lock`] (see [`crate::disruptor::DisruptorError::ConcurrentClaimDriver`]).
    next_value: UnsafeCell<i64>,
    /// Cached minimum gating sequence (equivalent to LMAX cachedValue).
    /// Same single-publisher exclusivity as `next_value`.
    cached_value: UnsafeCell<i64>,
    /// Runtime mutual exclusion for claim-state cells (`next_value` / `cached_value`).
    ///
    /// High-level APIs already prevent dual handles; this closes the residual where
    /// a caller uses `unsafe { SingleProducerSequencer::new }` and then drives
    /// claim methods from two threads via a shared `Arc` (safe-looking after the
    /// unsafe constructor). Concurrent claimants get
    /// [`crate::disruptor::DisruptorError::ConcurrentClaimDriver`] / `None` instead
    /// of a data race on the `UnsafeCell`s.
    claim_lock: AtomicBool,
    #[cfg(feature = "bench-round-diagnostics")]
    bench_backpressure_entries: AtomicU64,
    #[cfg(feature = "bench-round-diagnostics")]
    bench_backpressure_iterations: AtomicU64,
    #[cfg(feature = "bench-round-diagnostics")]
    bench_backpressure_max_iterations: AtomicU64,
    /// Set after a fatal consumer/producer failure. Claim methods fail fast
    /// instead of spinning on a dead gating sequence.
    poisoned: AtomicBool,
    /// Set on clean halt/shutdown: further claims return Shutdown instead of
    /// writing into a stopped pipeline (audit residual: publish-after-halt).
    closed: AtomicBool,
    /// First causal failure. Accessed only on failure/diagnostic paths and kept
    /// after the hot poison/closed flags to avoid separating them in the layout.
    first_failure: Mutex<Option<FailureRecord>>,
}

// SAFETY: `next_value` / `cached_value` are only read/written while `claim_lock`
// is held. Other threads may access `cursor`, gating sequences, wait strategy,
// and the lock itself. Concurrent claim attempts fail without touching the cells.
unsafe impl<W: WaitStrategy + 'static> Sync for SingleProducerSequencer<W> {}

/// Releases [`SingleProducerSequencer::claim_lock`] on every exit path (including panic).
struct SingleClaimGuard<'a>(&'a AtomicBool);

impl Drop for SingleClaimGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl<W> SingleProducerSequencer<W>
where
    W: WaitStrategy + 'static,
{
    /// Create a new single producer sequencer
    ///
    /// # Arguments
    /// * `buffer_size` - The size of the ring buffer
    /// * `wait_strategy` - The wait strategy to use
    ///
    /// # Returns
    /// A new SingleProducerSequencer instance
    ///
    /// # Safety
    /// The claim state (`next_value`/`cached_value`) is non-atomic. Callers must
    /// drive claim methods (`next`, `next_n`, `try_next`, `try_next_n`,
    /// `has_available_capacity`, `remaining_capacity`) from **one thread at a
    /// time**. A runtime claim lock rejects concurrent drivers with
    /// [`crate::disruptor::DisruptorError::ConcurrentClaimDriver`] (or `None`
    /// on try paths) so a violated contract fails closed instead of data-racing,
    /// but overlapping multi-publisher use is still a protocol error — prefer
    /// the Builder / `SimpleProducer` surface which enforces single ownership
    /// in the type system. Read-side methods used by barriers/consumers
    /// (`is_available`, cursor access, gating management) are unrestricted.
    pub unsafe fn new(buffer_size: usize, wait_strategy: Arc<W>) -> Self {
        let buffer_size_i64 = i64::try_from(buffer_size).expect("buffer size must fit into i64");
        let needs_signal = wait_strategy.needs_signal();
        Self {
            buffer_size,
            buffer_size_i64,
            wait_strategy,
            needs_signal,
            cursor: Arc::new(Sequence::new_with_initial_value()),
            gating_sequences: ArcSwap::from_pointee(Vec::new()),
            next_value: UnsafeCell::new(-1),
            cached_value: UnsafeCell::new(-1),
            claim_lock: AtomicBool::new(false),
            #[cfg(feature = "bench-round-diagnostics")]
            bench_backpressure_entries: AtomicU64::new(0),
            #[cfg(feature = "bench-round-diagnostics")]
            bench_backpressure_iterations: AtomicU64::new(0),
            #[cfg(feature = "bench-round-diagnostics")]
            bench_backpressure_max_iterations: AtomicU64::new(0),
            poisoned: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            first_failure: Mutex::new(None),
        }
    }

    /// Acquire exclusive access to claim-state cells, or return the concurrent-driver error.
    #[inline]
    fn acquire_claim(&self) -> Result<SingleClaimGuard<'_>> {
        // Swap true: if it was already true, another driver holds the lock.
        // Failed claimants never touch next_value/cached_value → no data race.
        if self.claim_lock.swap(true, Ordering::Acquire) {
            return Err(DisruptorError::ConcurrentClaimDriver);
        }
        Ok(SingleClaimGuard(&self.claim_lock))
    }

    /// Non-blocking claim lock for try paths (`None` = busy or unavailable).
    #[inline]
    fn try_acquire_claim(&self) -> Option<SingleClaimGuard<'_>> {
        if self.claim_lock.swap(true, Ordering::Acquire) {
            return None;
        }
        Some(SingleClaimGuard(&self.claim_lock))
    }

    #[inline]
    unsafe fn next_value(&self) -> i64 {
        *self.next_value.get()
    }

    #[inline]
    unsafe fn set_next_value(&self, v: i64) {
        *self.next_value.get() = v;
    }

    #[inline]
    unsafe fn cached_value(&self) -> i64 {
        *self.cached_value.get()
    }

    #[inline]
    unsafe fn set_cached_value(&self, v: i64) {
        *self.cached_value.get() = v;
    }

    /// Create a barrier that holds a `SequencerEnum::Single` reference back to this sequencer.
    ///
    /// This avoids the unsafe `SequencerWrapper` by passing the concrete `Arc` directly.
    pub fn new_barrier_typed(
        self: &Arc<Self>,
        sequences_to_track: Vec<Arc<Sequence>>,
    ) -> Arc<ProcessingSequenceBarrier<W>> {
        Arc::new(ProcessingSequenceBarrier::new(
            self.cursor.clone(),
            Arc::clone(&self.wait_strategy),
            sequences_to_track,
            SequencerEnum::Single(Arc::clone(self)),
        ))
    }

    /// Minimum of the gating sequences, clamped to at most `minimum`.
    ///
    /// LMAX `Util.getMinimumSequence(gatingSequences, minimum)`: with no gating
    /// sequences registered the producer gates on its own position, so wrap-point
    /// checks stay meaningful instead of comparing against `i64::MAX`.
    #[inline]
    fn gating_minimum(&self, minimum: i64) -> i64 {
        let guard = self.gating_sequences.load();
        Sequence::get_minimum_sequence_with_default(&guard, minimum)
    }

    #[cfg(feature = "bench-round-diagnostics")]
    #[inline]
    fn record_bench_backpressure(&self, iterations: u64) {
        if iterations == 0 {
            return;
        }
        self.bench_backpressure_entries
            .fetch_add(1, Ordering::Relaxed);
        self.bench_backpressure_iterations
            .fetch_add(iterations, Ordering::Relaxed);
        self.bench_backpressure_max_iterations
            .fetch_max(iterations, Ordering::Relaxed);
    }

    #[cfg(feature = "bench-round-diagnostics")]
    fn bench_backpressure_snapshot(&self) -> BenchProducerBackpressureSnapshot {
        BenchProducerBackpressureSnapshot {
            entries: self.bench_backpressure_entries.load(Ordering::Relaxed),
            wait_loop_iterations: self.bench_backpressure_iterations.load(Ordering::Relaxed),
            max_wait_loop_iterations: self
                .bench_backpressure_max_iterations
                .load(Ordering::Relaxed),
        }
    }

    /// Check if there's available capacity for the required number of sequences
    /// This matches the LMAX Disruptor SingleProducerSequencer.hasAvailableCapacity method
    fn has_available_capacity_internal(&self, required_capacity: usize, do_store: bool) -> bool {
        // SAFETY: single-publisher exclusive access to next/cached.
        let next_value = unsafe { self.next_value() };
        let required_capacity_i64 =
            i64::try_from(required_capacity).expect("required capacity must fit into i64");
        let wrap_point = (next_value + required_capacity_i64) - self.buffer_size_i64;
        let cached_gating_sequence = unsafe { self.cached_value() };

        if wrap_point > cached_gating_sequence || cached_gating_sequence > next_value {
            if do_store {
                // StoreLoad fence (equivalent to cursor.setVolatile in LMAX)
                self.cursor.set_volatile(next_value);
            }

            let min_sequence = self.gating_minimum(next_value);
            unsafe { self.set_cached_value(min_sequence) };

            if wrap_point > min_sequence {
                return false;
            }
        }

        true
    }
}

impl<W> Sequencer for SingleProducerSequencer<W>
where
    W: WaitStrategy + 'static,
{
    #[inline]
    fn get_cursor(&self) -> Arc<Sequence> {
        Arc::clone(&self.cursor)
    }

    #[inline]
    fn get_buffer_size(&self) -> usize {
        self.buffer_size
    }

    #[inline]
    fn next(&self) -> Result<i64> {
        self.next_n(1)
    }

    #[inline]
    fn next_n(&self, n: i64) -> Result<i64> {
        if n < 1 || n > self.buffer_size_i64 {
            return Err(DisruptorError::InvalidSequence(n));
        }
        // Relaxed: closed/poisoned are terminal monotonic flags; coherence
        // guarantees eventual visibility. An Acquire load here pairs a
        // per-publish STLR (cursor Release store) with an LDAR, creating a
        // StoreLoad barrier per event on ARM that cost ~5x single-event
        // throughput (P3 bisection, 2026-07-19).
        if self.closed.load(Ordering::Relaxed) {
            return Err(DisruptorError::Shutdown);
        }
        if self.poisoned.load(Ordering::Relaxed) {
            return Err(DisruptorError::Poisoned);
        }

        // Serializes claim-state access so concurrent Arc drivers fail closed
        // instead of racing on next_value/cached_value (residual 2026-07-19).
        let _claim = self.acquire_claim()?;

        // This follows the exact LMAX Disruptor SingleProducerSequencer.next(int n) logic.
        // SAFETY: claim_lock held — exclusive access to next/cached cells.
        let next_value = unsafe { self.next_value() };
        let next_sequence = next_value + n;
        let wrap_point = next_sequence - self.buffer_size_i64;
        let cached_gating_sequence = unsafe { self.cached_value() };

        if wrap_point > cached_gating_sequence || cached_gating_sequence > next_value {
            // Set cursor with volatile semantics (equivalent to cursor.setVolatile in LMAX)
            self.cursor.set_volatile(next_value);

            // Wait for consumers to catch up.
            // Use spin_loop() instead of yield_now() to avoid syscall overhead
            // (~1-5μs per sched_yield). PAUSE (x86) / YIELD (ARM) stays on-core.
            let mut min_sequence;
            #[cfg(feature = "bench-round-diagnostics")]
            let diagnostics_enabled = BENCH_ROUND_DIAGNOSTICS_ENABLED.load(Ordering::Relaxed);
            #[cfg(feature = "bench-round-diagnostics")]
            let mut backpressure_iterations = 0_u64;
            while {
                min_sequence = self.gating_minimum(next_value);
                wrap_point > min_sequence
            } {
                // A dead consumer never advances its gating sequence; fail fast
                // instead of spinning forever. Relaxed: see entry check above.
                if self.closed.load(Ordering::Relaxed) {
                    return Err(DisruptorError::Shutdown);
                }
                if self.poisoned.load(Ordering::Relaxed) {
                    return Err(DisruptorError::Poisoned);
                }
                #[cfg(feature = "bench-round-diagnostics")]
                if diagnostics_enabled {
                    backpressure_iterations = backpressure_iterations.saturating_add(1);
                }
                std::hint::spin_loop();
            }
            #[cfg(feature = "bench-round-diagnostics")]
            if diagnostics_enabled {
                self.record_bench_backpressure(backpressure_iterations);
            }

            unsafe { self.set_cached_value(min_sequence) };
        }

        // Update next_value (equivalent to this.nextValue = nextSequence in LMAX)
        unsafe { self.set_next_value(next_sequence) };

        Ok(next_sequence)
    }

    fn try_next(&self) -> Option<i64> {
        self.try_next_n(1)
    }

    #[inline]
    fn try_next_n(&self, n: i64) -> Option<i64> {
        // Same bounds as next_n: a claim larger than the buffer can never succeed
        // and would alias ring slots within a single batch if allowed through.
        if n < 1 || n > self.buffer_size_i64 {
            return None;
        }
        // Relaxed: terminal monotonic flags (see next_n).
        if self.closed.load(Ordering::Relaxed) || self.poisoned.load(Ordering::Relaxed) {
            return None;
        }

        let _claim = self.try_acquire_claim()?;

        // This follows the exact LMAX Disruptor SingleProducerSequencer.tryNext(int n) logic
        let Ok(required) = usize::try_from(n) else {
            return None;
        };

        if !self.has_available_capacity_internal(required, true) {
            return None; // Insufficient capacity
        }

        // Update next_value and return the sequence (equivalent to this.nextValue += n)
        // SAFETY: claim_lock held — exclusive access to next/cached cells.
        let next_sequence = unsafe { self.next_value() + n };
        unsafe { self.set_next_value(next_sequence) };

        Some(next_sequence)
    }

    #[inline]
    fn publish(&self, sequence: i64) {
        self.cursor.set(sequence);
        if self.needs_signal {
            self.wait_strategy.signal_all_when_blocking();
        }
    }

    #[inline]
    fn publish_range(&self, _low: i64, high: i64) {
        self.publish(high);
    }

    #[inline]
    fn is_available(&self, sequence: i64) -> bool {
        sequence <= self.cursor.get()
    }

    #[inline]
    fn get_highest_published_sequence(&self, _next_sequence: i64, available_sequence: i64) -> i64 {
        available_sequence
    }

    fn add_gating_sequences(&self, gating_sequences: &[Arc<Sequence>]) {
        add_gating_sequences_rcu(&self.gating_sequences, gating_sequences);
    }

    fn remove_gating_sequence(&self, sequence: Arc<Sequence>) -> bool {
        remove_gating_sequence_cas(&self.gating_sequences, &sequence)
    }

    fn get_minimum_sequence(&self) -> i64 {
        let guard = self.gating_sequences.load();
        Sequence::get_minimum_sequence(&guard)
    }

    fn remaining_capacity(&self) -> i64 {
        // This follows the exact LMAX Disruptor SingleProducerSequencer.remainingCapacity logic.
        // If another thread holds the claim lock, report 0 (conservative "full") rather
        // than racing on next_value — remaining_capacity is advisory for try-path
        // classification and never authorizes a claim by itself.
        let Some(_claim) = self.try_acquire_claim() else {
            return 0;
        };
        // SAFETY: claim_lock held.
        let next_value = unsafe { self.next_value() };
        let consumed = self.gating_minimum(next_value);
        let used_capacity = next_value.saturating_sub(consumed);
        self.buffer_size_i64.saturating_sub(used_capacity)
    }

    fn has_available_capacity(&self, required_capacity: usize) -> bool {
        let Some(_claim) = self.try_acquire_claim() else {
            return false;
        };
        self.has_available_capacity_internal(required_capacity, false)
    }

    fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
        // Wake any consumers blocked in a wait strategy so they observe the
        // failure promptly (producers spin and check the flag themselves).
        self.wait_strategy.signal_all_when_blocking();
    }

    fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    fn record_failure(&self, failure: &FailureRecord) {
        let mut first_failure = self.first_failure.lock();
        if first_failure.is_none() {
            *first_failure = Some(failure.clone());
        }
    }

    fn first_failure(&self) -> Option<FailureRecord> {
        self.first_failure.lock().clone()
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.wait_strategy.signal_all_when_blocking();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn reopen(&self) {
        self.closed.store(false, Ordering::Release);
    }
}

/// Multi producer sequencer with bitmap optimization
///
/// This sequencer supports multiple threads publishing events concurrently.
/// It uses optimized algorithms inspired by both LMAX Disruptor and disruptor-rs,
/// combining the best of both approaches for maximum performance.
///
/// ## Cursor Semantics
///
/// **Important**: This implementation uses a different cursor semantic than the original LMAX Disruptor:
/// - **Our cursor**: Tracks the highest **claimed** sequence across all producers
/// - **LMAX cursor**: Tracks the highest **published contiguous** sequence
///
/// This design choice prioritizes performance by avoiding cursor updates during publish operations,
/// instead relying on sequence barriers to perform contiguity convergence via `get_highest_published_sequence()`.
/// This approach is safe but requires proper barrier implementation for correctness.
///
/// Key features:
/// - Bitmap-based availability tracking (inspired by disruptor-rs)
/// - CAS-based coordination between producers (LMAX Disruptor)
/// - Optimized batch publishing support
/// - Cache-friendly memory layout
/// - ABA problem prevention
#[derive(Debug)]
pub struct MultiProducerSequencer<W>
where
    W: WaitStrategy + 'static,
{
    buffer_size: usize,
    buffer_size_i64: i64,
    wait_strategy: Arc<W>,
    needs_signal: bool,
    /// Cursor tracks the highest **claimed** sequence number by any producer.
    ///
    /// **Important**: Unlike LMAX Disruptor's cursor which represents the "highest published
    /// contiguous sequence", our cursor represents the "highest claimed sequence" across all
    /// producers. This design choice requires barrier-side contiguity convergence via
    /// `get_highest_published_sequence()` to ensure correctness in MPMC scenarios.
    ///
    /// The publish operation only marks availability bits without advancing the cursor,
    /// relying on consumers to perform contiguity checking through sequence barriers.
    cursor: Arc<Sequence>,
    /// Gating sequences that this sequencer must not overtake.
    /// See `SingleProducerSequencer::gating_sequences` for design rationale.
    gating_sequences: ArcSwap<Vec<Arc<Sequence>>>,
    /// Bitmap tracking availability of slots using AtomicU64 arrays
    /// Each bit represents whether a slot was published in an even or odd round
    /// This is inspired by disruptor-rs's innovative bitmap approach
    available_bitmap: Box<[AtomicU64]>,
    /// Index mask for fast modulo operations (buffer_size - 1)
    index_mask: usize,
    /// Bit shift for calculating round flags: `buffer_size.trailing_zeros()`
    /// Replaces expensive div_euclid with `sequence >> index_shift`
    index_shift: u32,

    /// Cached minimum gating sequence to reduce lock contention
    /// This is an optimization from the LMAX Disruptor to avoid frequent reads of gating sequences
    cached_gating_sequence: CachePadded<AtomicI64>,
    /// Legacy available buffer for backward compatibility with small buffers (< 64)
    available_buffer: Vec<AtomicI32>,
    /// Set after a fatal consumer/producer failure. Claim methods fail fast
    /// instead of spinning on a dead gating sequence.
    poisoned: AtomicBool,
    /// Set on clean halt/shutdown: further claims return Shutdown.
    closed: AtomicBool,
    /// First causal failure. Accessed only on failure/diagnostic paths and kept
    /// after the hot poison/closed flags to avoid separating them in the layout.
    first_failure: Mutex<Option<FailureRecord>>,
}

impl<W> MultiProducerSequencer<W>
where
    W: WaitStrategy + 'static,
{
    /// Create a new multi producer sequencer with optional bitmap optimization
    ///
    /// # Arguments
    /// * `buffer_size` - The size of the ring buffer (must be a power of 2)
    /// * `wait_strategy` - The wait strategy to use
    ///
    /// # Returns
    /// A new MultiProducerSequencer instance
    ///
    /// # Panics
    /// Panics if `buffer_size` is not a power of 2. Callers should validate
    /// buffer size before construction (e.g., via `Disruptor::new` or builders).
    pub fn new(buffer_size: usize, wait_strategy: Arc<W>) -> Self {
        assert!(
            is_power_of_two(buffer_size),
            "Buffer size must be a power of 2"
        );
        let needs_signal = wait_strategy.needs_signal();

        // Initialize bitmap for availability tracking if buffer is large enough
        // For smaller buffers, we'll use an empty bitmap and fall back to legacy method
        let available_bitmap = if buffer_size >= 64 {
            let bitmap_size = buffer_size / 64; // buffer_size is always power-of-2 and >= 64
            let bitmap: Box<[AtomicU64]> = (0..bitmap_size)
                .map(|_| AtomicU64::new(!0_u64)) // Initialize with all 1's (nothing published initially, matching disruptor-rs)
                .collect();
            bitmap
        } else {
            // For small buffers, use empty bitmap and fall back to legacy method
            Box::new([])
        };

        // Only allocate legacy available_buffer for small buffers (< 64);
        // large buffers use the bitmap exclusively.
        let available_buffer: Vec<AtomicI32> = if buffer_size < 64 {
            (0..buffer_size).map(|_| AtomicI32::new(-1)).collect()
        } else {
            Vec::new()
        };

        let buffer_size_i64 = i64::try_from(buffer_size).expect("buffer size must fit into i64");

        Self {
            buffer_size,
            buffer_size_i64,
            wait_strategy,
            needs_signal,
            cursor: Arc::new(Sequence::new_with_initial_value()),
            gating_sequences: ArcSwap::from_pointee(Vec::new()),
            available_bitmap,
            index_mask: buffer_size - 1,
            index_shift: buffer_size.trailing_zeros(),
            cached_gating_sequence: CachePadded::new(AtomicI64::new(-1)),
            available_buffer,
            poisoned: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            first_failure: Mutex::new(None),
        }
    }

    /// Create a barrier that holds a `SequencerEnum::Multi` reference back to this sequencer.
    ///
    /// This avoids the unsafe `SequencerWrapper` by passing the concrete `Arc` directly.
    pub fn new_barrier_typed(
        self: &Arc<Self>,
        sequences_to_track: Vec<Arc<Sequence>>,
    ) -> Arc<ProcessingSequenceBarrier<W>> {
        Arc::new(ProcessingSequenceBarrier::new(
            self.cursor.clone(),
            Arc::clone(&self.wait_strategy),
            sequences_to_track,
            SequencerEnum::Multi(Arc::clone(self)),
        ))
    }

    /// Calculate the index in the available buffer for a given sequence
    /// Uses bitmask for O(1) modulo on power-of-2 buffer sizes
    #[inline]
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    fn calculate_index(&self, sequence: i64) -> usize {
        // Sign loss is intentional: bitmask discards upper bits and sign.
        // Truncation is safe: index_mask is at most usize::MAX.
        (sequence as usize) & self.index_mask
    }

    /// Calculate the availability flag for a given sequence
    /// Uses bit shift for O(1) round calculation on power-of-2 buffer sizes
    /// Returns 0 for even rounds, 1 for odd rounds
    #[inline]
    fn calculate_availability_flag(&self, sequence: i64) -> i32 {
        ((sequence >> self.index_shift) & 1) as i32
    }

    /// Calculate the round flag for bitmap availability checking (inspired by disruptor-rs)
    /// Uses bit shift for O(1) round calculation on power-of-2 buffer sizes
    /// Returns 0 for even rounds, 1 for odd rounds
    #[inline]
    #[allow(clippy::cast_sign_loss)]
    fn calculate_round_flag(&self, sequence: i64) -> u64 {
        // Sign loss is intentional: `& 1` guarantees result is 0 or 1.
        ((sequence >> self.index_shift) & 1) as u64
    }

    /// Calculate availability indices for bitmap (inspired by disruptor-rs)
    /// Uses bitmask for O(1) slot calculation on power-of-2 buffer sizes
    #[inline]
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    fn calculate_availability_indices(&self, sequence: i64) -> (usize, usize) {
        // Sign loss is intentional: bitmask discards upper bits and sign.
        let slot_index = (sequence as usize) & self.index_mask;
        let availability_index = slot_index >> 6; // Divide by 64
        let bit_index = slot_index & 63; // Modulo 64
        (availability_index, bit_index)
    }

    /// Get availability bitmap at index (inspired by disruptor-rs)
    fn availability_at(&self, index: usize) -> &AtomicU64 {
        // SAFETY: Index is always calculated with calculate_availability_indices and is within bounds
        unsafe { self.available_bitmap.get_unchecked(index) }
    }

    /// Set a sequence as available for consumption
    /// Uses bitmap for large buffers (>= 64), legacy available_buffer for small buffers
    fn set_available(&self, sequence: i64) {
        if self.available_bitmap.is_empty() {
            // Legacy LMAX method for small buffers
            let index = self.calculate_index(sequence);
            let flag = self.calculate_availability_flag(sequence);
            self.available_buffer[index].store(flag, Ordering::Release);
        } else {
            // Bitmap method for large buffers (>= 64)
            self.publish_bitmap(sequence);
        }
    }

    /// Publish using bitmap method (inspired by disruptor-rs)
    /// Uses XOR operation to flip the bit, encoding even/odd round publication
    fn publish_bitmap(&self, sequence: i64) {
        let (availability_index, bit_index) = self.calculate_availability_indices(sequence);
        if availability_index < self.available_bitmap.len() {
            let availability = self.availability_at(availability_index);
            let mask = 1_u64 << bit_index;
            // XOR operation flips the bit to encode even/odd round publication
            availability.fetch_xor(mask, Ordering::Release);
        }
    }

    /// Check if a sequence is available using bitmap method (inspired by disruptor-rs)
    /// Compares the bit value with the expected round flag, handling bitmap wraparound
    fn is_bitmap_available(&self, sequence: i64) -> bool {
        let (availability_index, bit_index) = self.calculate_availability_indices(sequence);
        if availability_index < self.available_bitmap.len() {
            let availability = self.availability_at(availability_index);
            let current_value = availability.load(Ordering::Acquire);

            // Calculate expected flag for this sequence
            let expected_flag = self.calculate_round_flag(sequence);

            let actual_flag = (current_value >> bit_index) & 1;
            // Check if the bit matches the expected round flag
            actual_flag == expected_flag
        } else {
            false
        }
    }

    /// Get highest published sequence using bitmap method (inspired by disruptor-rs)
    /// Scans from `next_sequence` to find the highest contiguous published sequence.
    /// Uses bulk XOR + trailing_zeros for O(1) per 64-bit word instead of per-bit checking.
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    fn get_highest_published_sequence_bitmap(
        &self,
        next_sequence: i64,
        available_sequence: i64,
    ) -> i64 {
        if next_sequence > available_sequence {
            return available_sequence;
        }

        let mut sequence = next_sequence;

        while sequence <= available_sequence {
            let (word_index, start_bit) = self.calculate_availability_indices(sequence);
            if word_index >= self.available_bitmap.len() {
                return sequence - 1;
            }

            // Load the word once for all bits in this word
            let word = self.availability_at(word_index).load(Ordering::Acquire);

            // How many bits can we check in this word starting from start_bit?
            let bits_remaining_in_word = 64 - start_bit;
            let sequences_remaining = (available_sequence - sequence + 1) as usize;
            let bits_to_check = bits_remaining_in_word.min(sequences_remaining);

            // All sequences within one bitmap word share the same round flag
            // (buffer_size >= 64, so each 64-slot word maps to one round parity).
            let expected_flag = self.calculate_round_flag(sequence);
            // Build the expected word pattern: all 0s or all 1s depending on round.
            let expected_word = if expected_flag == 0 { 0_u64 } else { !0_u64 };

            // Build mask covering only the bits we need to check:
            // bits [start_bit, start_bit + bits_to_check)
            let range_mask = if bits_to_check >= 64 {
                !0_u64
            } else {
                ((1_u64 << bits_to_check) - 1) << start_bit
            };

            // XOR with expected pattern: mismatch bits become 1.
            // Mask to only the range we care about.
            let mismatches = (word ^ expected_word) & range_mask;

            if mismatches != 0 {
                // First mismatch bit position (absolute bit index in the word)
                let first_mismatch = mismatches.trailing_zeros() as usize;
                let offset = first_mismatch - start_bit;
                return sequence + offset as i64 - 1;
            }

            sequence += bits_to_check as i64;
        }

        available_sequence
    }

    /// Get highest published sequence using legacy LMAX Disruptor algorithm
    fn get_highest_published_sequence_legacy(
        &self,
        next_sequence: i64,
        available_sequence: i64,
    ) -> i64 {
        // This is the core algorithm from LMAX Disruptor for finding the highest
        // contiguous published sequence in a multi-producer environment

        // Start from the next sequence we're looking for
        let mut sequence = next_sequence;

        // Scan through the available buffer to find the highest contiguous sequence
        while sequence <= available_sequence {
            if !self.is_available_internal(sequence) {
                // Found a gap, return the sequence before this gap
                return sequence - 1;
            }
            sequence += 1;
        }

        // All sequences up to available_sequence are published
        available_sequence
    }

    /// Check if a sequence is available for consumption
    /// This matches the LMAX Disruptor isAvailable method with proper flag checking
    fn is_available_internal(&self, sequence: i64) -> bool {
        // Bitmap is allocated iff buffer_size >= 64; one check suffices.
        if self.available_bitmap.is_empty() {
            let index = self.calculate_index(sequence);
            let flag = self.calculate_availability_flag(sequence);
            self.available_buffer[index].load(Ordering::Acquire) == flag
        } else {
            self.is_bitmap_available(sequence)
        }
    }

    /// Check if there's available capacity for the required number of sequences
    /// This matches the LMAX Disruptor hasAvailableCapacity method
    fn has_available_capacity_internal(
        &self,
        gating_sequences: &[Arc<Sequence>],
        required_capacity: usize,
        cursor_value: i64,
    ) -> bool {
        let required_capacity_i64 =
            i64::try_from(required_capacity).expect("required capacity must fit into i64");
        let wrap_point = (cursor_value + required_capacity_i64) - self.buffer_size_i64;
        let cached_gating_sequence = self.cached_gating_sequence.load(Ordering::Acquire);

        if wrap_point > cached_gating_sequence || cached_gating_sequence > cursor_value {
            // LMAX Util.getMinimumSequence(gatingSequences, cursor): with no gating
            // sequences the producer gates on the cursor, keeping wrap checks meaningful.
            let min_sequence =
                Sequence::get_minimum_sequence_with_default(gating_sequences, cursor_value);
            self.cached_gating_sequence
                .store(min_sequence, Ordering::Release);

            if wrap_point > min_sequence {
                return false;
            }
        }

        true
    }
}

impl<W> Sequencer for MultiProducerSequencer<W>
where
    W: WaitStrategy + 'static,
{
    #[inline]
    fn get_cursor(&self) -> Arc<Sequence> {
        Arc::clone(&self.cursor)
    }

    #[inline]
    fn get_buffer_size(&self) -> usize {
        self.buffer_size
    }

    fn next(&self) -> Result<i64> {
        self.next_n(1)
    }

    #[inline]
    fn next_n(&self, n: i64) -> Result<i64> {
        if n < 1 || n > self.buffer_size_i64 {
            return Err(DisruptorError::InvalidSequence(n));
        }

        // Following LMAX Disruptor design: Use CAS loop instead of getAndAdd
        // This ensures sequence allocation is contiguous
        let mut current;
        let mut next;

        // Try claiming the sequence using CAS
        loop {
            // Closed/poisoned are terminal monotonic flags. Relaxed: see Single
            // next_n for the ARM StoreLoad rationale.
            if self.closed.load(Ordering::Relaxed) {
                return Err(DisruptorError::Shutdown);
            }
            if self.poisoned.load(Ordering::Relaxed) {
                return Err(DisruptorError::Poisoned);
            }

            current = self.cursor.get();
            next = current + n;

            // Check if we have available capacity
            let wrap_point = next - self.buffer_size_i64;
            let cached_gating_sequence = self.cached_gating_sequence.load(Ordering::Acquire);

            if wrap_point > cached_gating_sequence || cached_gating_sequence > current {
                // Gating minimum clamped to the current cursor (LMAX Util semantics):
                // empty gating gates on the cursor instead of i64::MAX.
                let guard = self.gating_sequences.load();
                let min_sequence = Sequence::get_minimum_sequence_with_default(&guard, current);

                // If we don't have enough capacity, wait until we do
                if wrap_point > min_sequence {
                    // spin_loop() avoids syscall overhead of yield_now() (~1-5μs).
                    // PAUSE (x86) / YIELD (ARM) keeps the thread on-core.
                    std::hint::spin_loop();
                    continue;
                }

                // Update the cached gating sequence
                self.cached_gating_sequence
                    .store(min_sequence, Ordering::Release);
            }

            // Try to claim the sequences using CAS
            if self.cursor.compare_and_set(current, next) {
                break;
            }
            // If CAS failed, another producer claimed this sequence, try again with new current
        }

        Ok(next)
    }

    fn try_next(&self) -> Option<i64> {
        self.try_next_n(1)
    }

    #[inline]
    fn try_next_n(&self, n: i64) -> Option<i64> {
        // Same bounds as next_n: a claim larger than the buffer can never succeed
        // and would alias ring slots within a single batch if allowed through.
        if n < 1 || n > self.buffer_size_i64 {
            return None;
        }
        // Relaxed: terminal monotonic flags (see next_n).
        if self.closed.load(Ordering::Relaxed) || self.poisoned.load(Ordering::Relaxed) {
            return None;
        }

        let current = self.cursor.get();
        let next = current + n;

        // Check if we have available capacity
        let Ok(required) = usize::try_from(n) else {
            return None;
        };

        let guard = self.gating_sequences.load();
        if !self.has_available_capacity_internal(&guard, required, current) {
            return None; // Insufficient capacity
        }

        // Single CAS attempt - if it fails, return None (try semantics)
        if self.cursor.compare_and_set(current, next) {
            Some(next)
        } else {
            None // Another producer claimed this sequence
        }
    }

    #[inline]
    fn publish(&self, sequence: i64) {
        // Use the proper LMAX Disruptor setAvailable method
        self.set_available(sequence);

        // Signal waiting consumers
        if self.needs_signal {
            self.wait_strategy.signal_all_when_blocking();
        }
    }

    /// Publish a contiguous range of sequences `[low, high]`.
    ///
    /// For bitmap mode, each 64-slot word is XOR-flipped in a single `fetch_xor`.
    /// Ranges spanning multiple words result in multiple atomic ops (not a single
    /// atomic batch), which is consistent with LMAX Disruptor and disruptor-rs.
    /// Consumers always scan for contiguity via `get_highest_published_sequence`,
    /// so a partially-visible range simply means fewer events are consumed in that
    /// barrier wait round — correctness is preserved.
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    fn publish_range(&self, low: i64, high: i64) {
        if low > high {
            return;
        }

        if self.available_bitmap.is_empty() {
            // Legacy mode: per-slot store
            for sequence in low..=high {
                let index = self.calculate_index(sequence);
                let flag = self.calculate_availability_flag(sequence);
                self.available_buffer[index].store(flag, Ordering::Release);
            }
        } else {
            // Bitmap mode: batch XOR per word
            let mut sequence = low;
            while sequence <= high {
                let (word_index, start_bit) = self.calculate_availability_indices(sequence);
                let bits_remaining_in_word = 64 - start_bit;
                let sequences_remaining = (high - sequence + 1) as usize;
                let bits_to_set = bits_remaining_in_word.min(sequences_remaining);

                // Build a contiguous bitmask in O(1) instead of a loop
                let mask = if bits_to_set >= 64 {
                    !0_u64
                } else {
                    ((1_u64 << bits_to_set) - 1) << start_bit
                };

                if word_index < self.available_bitmap.len() {
                    self.availability_at(word_index)
                        .fetch_xor(mask, Ordering::Release);
                }

                sequence += bits_to_set as i64;
            }
        }

        if self.needs_signal {
            self.wait_strategy.signal_all_when_blocking();
        }
    }

    #[inline]
    fn is_available(&self, sequence: i64) -> bool {
        // Use the proper LMAX Disruptor isAvailable method with flag checking
        self.is_available_internal(sequence)
    }

    #[inline]
    fn get_highest_published_sequence(&self, next_sequence: i64, available_sequence: i64) -> i64 {
        // Bitmap is allocated iff buffer_size >= 64; one check suffices.
        if self.available_bitmap.is_empty() {
            self.get_highest_published_sequence_legacy(next_sequence, available_sequence)
        } else {
            self.get_highest_published_sequence_bitmap(next_sequence, available_sequence)
        }
    }

    fn add_gating_sequences(&self, gating_sequences: &[Arc<Sequence>]) {
        add_gating_sequences_rcu(&self.gating_sequences, gating_sequences);
    }

    fn remove_gating_sequence(&self, sequence: Arc<Sequence>) -> bool {
        remove_gating_sequence_cas(&self.gating_sequences, &sequence)
    }

    fn get_minimum_sequence(&self) -> i64 {
        let guard = self.gating_sequences.load();
        Sequence::get_minimum_sequence(&guard)
    }

    fn remaining_capacity(&self) -> i64 {
        let next_value = self.cursor.get();
        let guard = self.gating_sequences.load();
        let consumed = Sequence::get_minimum_sequence_with_default(&guard, next_value);
        let used_capacity = next_value.saturating_sub(consumed);
        self.buffer_size_i64.saturating_sub(used_capacity)
    }

    fn has_available_capacity(&self, required_capacity: usize) -> bool {
        let guard = self.gating_sequences.load();
        let cursor_value = self.cursor.get();
        self.has_available_capacity_internal(&guard, required_capacity, cursor_value)
    }

    fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
        // Wake any consumers blocked in a wait strategy so they observe the
        // failure promptly (producers spin and check the flag themselves).
        self.wait_strategy.signal_all_when_blocking();
    }

    fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    fn record_failure(&self, failure: &FailureRecord) {
        let mut first_failure = self.first_failure.lock();
        if first_failure.is_none() {
            *first_failure = Some(failure.clone());
        }
    }

    fn first_failure(&self) -> Option<FailureRecord> {
        self.first_failure.lock().clone()
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.wait_strategy.signal_all_when_blocking();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn reopen(&self) {
        self.closed.store(false, Ordering::Release);
    }
}

// SequenceBarrier is already imported above

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::sequence_barrier::SequenceBarrier;
    use crate::disruptor::{BlockingWaitStrategy, BusySpinWaitStrategy};

    #[test]
    fn test_multi_producer_sequencer_creation() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(1024, wait_strategy);

        assert_eq!(sequencer.buffer_size, 1024);
        assert_eq!(sequencer.index_mask, 1023);
        // buffer_size >= 64: bitmap used, legacy buffer empty
        assert!(sequencer.available_buffer.is_empty());
        assert_eq!(sequencer.available_bitmap.len(), 1024 / 64);
    }

    #[test]
    fn test_multi_producer_sequencer_next() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Test basic sequence claiming
        let seq1 = sequencer.next().unwrap();
        assert_eq!(seq1, 0);

        let seq2 = sequencer.next().unwrap();
        assert_eq!(seq2, 1);
    }

    #[test]
    fn test_multi_producer_sequencer_publish_and_availability() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Claim and publish a sequence
        let sequence = sequencer.next().unwrap();
        assert!(!sequencer.is_available(sequence)); // Not available until published

        sequencer.publish(sequence);
        assert!(sequencer.is_available(sequence)); // Now available
    }

    #[test]
    fn test_multi_producer_highest_published_sequence() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Publish sequences 0, 1, 2
        for _i in 0..3 {
            let seq = sequencer.next().unwrap();
            sequencer.publish(seq);
        }

        // All sequences should be available
        let highest = sequencer.get_highest_published_sequence(0, 2);
        assert_eq!(highest, 2);
    }

    #[test]
    fn test_single_producer_sequencer_creation() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = unsafe { SingleProducerSequencer::new(1024, wait_strategy) };

        assert_eq!(sequencer.buffer_size, 1024);
    }

    #[test]
    fn test_single_producer_sequencer_next() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = unsafe { SingleProducerSequencer::new(8, wait_strategy) };

        // Test basic sequence claiming
        let seq1 = sequencer.next().unwrap();
        assert_eq!(seq1, 0);

        let seq2 = sequencer.next().unwrap();
        assert_eq!(seq2, 1);
    }

    #[test]
    fn test_single_producer_sequencer_next_n() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = unsafe { SingleProducerSequencer::new(8, wait_strategy) };

        // Test claiming multiple sequences
        let seq = sequencer.next_n(3).unwrap();
        assert_eq!(seq, 2); // Returns highest sequence (0, 1, 2)

        // Test invalid sequence count
        let result = sequencer.next_n(0);
        assert!(result.is_err());

        let result = sequencer.next_n(-1);
        assert!(result.is_err());

        // Test claiming more than buffer size
        let result = sequencer.next_n(10);
        assert!(result.is_err());
    }

    #[test]
    fn test_single_producer_sequencer_try_next() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = unsafe { SingleProducerSequencer::new(8, wait_strategy) };

        // Test try_next success
        let seq = sequencer.try_next().unwrap();
        assert_eq!(seq, 0);

        // Test try_next_n success
        let seq = sequencer.try_next_n(2).unwrap();
        assert_eq!(seq, 2);

        // Test try_next_n with invalid count
        let result = sequencer.try_next_n(0);
        assert!(result.is_none());

        let result = sequencer.try_next_n(-1);
        assert!(result.is_none());
    }

    #[test]
    fn test_single_producer_sequencer_publish() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = unsafe { SingleProducerSequencer::new(8, wait_strategy) };

        let seq = sequencer.next().unwrap();
        assert!(!sequencer.is_available(seq));

        sequencer.publish(seq);
        assert!(sequencer.is_available(seq));

        // Test publish_range
        let seq2 = sequencer.next_n(3).unwrap();
        sequencer.publish_range(seq + 1, seq2);
        assert!(sequencer.is_available(seq2));
    }

    #[test]
    fn test_single_producer_sequencer_gating_sequences() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = unsafe { SingleProducerSequencer::new(8, wait_strategy) };

        let gating_seq1 = Arc::new(Sequence::new(5));
        let gating_seq2 = Arc::new(Sequence::new(3));

        // Add gating sequences
        sequencer.add_gating_sequences(&[gating_seq1.clone(), gating_seq2.clone()]);
        // Adding the same sequences again should be idempotent (no duplicates)
        sequencer.add_gating_sequences(&[gating_seq1.clone(), gating_seq2.clone()]);
        assert_eq!(sequencer.gating_sequences.load().len(), 2);

        // Test minimum sequence calculation
        let min_seq = sequencer.get_minimum_sequence();
        assert_eq!(min_seq, 3); // Should be minimum of gating sequences

        // Test remove gating sequence
        let removed = sequencer.remove_gating_sequence(gating_seq2.clone());
        assert!(removed);
        // Removing again should report false
        assert!(!sequencer.remove_gating_sequence(gating_seq2.clone()));

        let min_seq = sequencer.get_minimum_sequence();
        assert_eq!(min_seq, 5); // Should be remaining gating sequence

        // Test removing non-existent sequence
        let removed = sequencer.remove_gating_sequence(Arc::new(Sequence::new(100)));
        assert!(!removed);
    }

    #[test]
    fn test_single_producer_sequencer_remove_gating_sequence_concurrent() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = Arc::new(unsafe { SingleProducerSequencer::new(8, wait_strategy) });

        let gating_seq = Arc::new(Sequence::new(3));
        sequencer.add_gating_sequences(std::slice::from_ref(&gating_seq));
        assert_eq!(sequencer.gating_sequences.load().len(), 1);

        let barrier = Arc::new(std::sync::Barrier::new(3));

        let t1_sequencer = Arc::clone(&sequencer);
        let t1_seq = Arc::clone(&gating_seq);
        let t1_barrier = Arc::clone(&barrier);
        let t1 = std::thread::spawn(move || {
            t1_barrier.wait();
            t1_sequencer.remove_gating_sequence(t1_seq)
        });

        let t2_sequencer = Arc::clone(&sequencer);
        let t2_seq = Arc::clone(&gating_seq);
        let t2_barrier = Arc::clone(&barrier);
        let t2 = std::thread::spawn(move || {
            t2_barrier.wait();
            t2_sequencer.remove_gating_sequence(t2_seq)
        });

        barrier.wait();

        let r1 = t1.join().expect("thread 1 panicked");
        let r2 = t2.join().expect("thread 2 panicked");

        assert!(
            r1 ^ r2,
            "expected exactly one successful remove, got r1={r1} r2={r2}"
        );
        assert!(sequencer.gating_sequences.load().is_empty());
    }

    #[test]
    fn test_single_producer_sequencer_capacity() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = unsafe { SingleProducerSequencer::new(8, wait_strategy) };

        // Test remaining capacity with no consumers
        let capacity = sequencer.remaining_capacity();
        assert_eq!(capacity, 8);

        // Test has_available_capacity
        assert!(sequencer.has_available_capacity(4));
        assert!(sequencer.has_available_capacity(8));

        // Add a consumer sequence
        let consumer_seq = Arc::new(Sequence::new(-1));
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_seq));

        // Claim some sequences
        let _seq1 = sequencer.next_n(4).unwrap();

        // Test remaining capacity with consumer
        let capacity = sequencer.remaining_capacity();
        assert!(capacity <= 8);

        // Update consumer sequence and test again
        consumer_seq.set(2);
        let capacity = sequencer.remaining_capacity();
        assert!(capacity > 0);
    }

    #[test]
    fn test_single_producer_sequencer_barrier() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = Arc::new(unsafe { SingleProducerSequencer::new(8, wait_strategy) });

        let dep_seq = Arc::new(Sequence::new(0));
        let barrier = sequencer.new_barrier_typed(vec![dep_seq]);

        assert_eq!(barrier.get_cursor().get(), -1);
        assert!(!barrier.is_alerted());
    }

    #[test]
    fn test_multi_producer_sequencer_next_n() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Test claiming multiple sequences
        let seq = sequencer.next_n(3).unwrap();
        assert_eq!(seq, 2); // Returns highest sequence (0, 1, 2)

        // Test invalid sequence count
        let result = sequencer.next_n(0);
        assert!(result.is_err());

        let result = sequencer.next_n(-1);
        assert!(result.is_err());

        // Test claiming more than buffer size
        let result = sequencer.next_n(10);
        assert!(result.is_err());
    }

    #[test]
    fn test_multi_producer_sequencer_try_next() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Test try_next success
        let seq = sequencer.try_next().unwrap();
        assert_eq!(seq, 0);

        // Test try_next_n success
        let seq = sequencer.try_next_n(2).unwrap();
        assert_eq!(seq, 2);

        // Test try_next_n with invalid count
        let result = sequencer.try_next_n(0);
        assert!(result.is_none());

        let result = sequencer.try_next_n(-1);
        assert!(result.is_none());
    }

    #[test]
    fn test_multi_producer_sequencer_publish_range() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Claim sequences
        let seq1 = sequencer.next().unwrap();
        let seq2 = sequencer.next().unwrap();
        let seq3 = sequencer.next().unwrap();

        // Publish range
        sequencer.publish_range(seq1, seq3);

        // All sequences in range should be available
        assert!(sequencer.is_available(seq1));
        assert!(sequencer.is_available(seq2));
        assert!(sequencer.is_available(seq3));

        // Test invalid range
        sequencer.publish_range(10, 5); // high < low should be handled gracefully
    }

    #[test]
    fn test_multi_producer_sequencer_gating_sequences() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        let gating_seq1 = Arc::new(Sequence::new(5));
        let gating_seq2 = Arc::new(Sequence::new(3));

        // Add gating sequences
        sequencer.add_gating_sequences(&[gating_seq1.clone(), gating_seq2.clone()]);
        // Adding the same sequences again should be idempotent (no duplicates)
        sequencer.add_gating_sequences(&[gating_seq1.clone(), gating_seq2.clone()]);
        assert_eq!(sequencer.gating_sequences.load().len(), 2);

        // Test minimum sequence calculation
        let min_seq = sequencer.get_minimum_sequence();
        assert_eq!(min_seq, 3); // Should be minimum of gating sequences

        // Test remove gating sequence
        let removed = sequencer.remove_gating_sequence(gating_seq2.clone());
        assert!(removed);
        // Removing again should report false
        assert!(!sequencer.remove_gating_sequence(gating_seq2.clone()));

        let min_seq = sequencer.get_minimum_sequence();
        assert_eq!(min_seq, 5); // Should be remaining gating sequence

        // Test removing non-existent sequence
        let removed = sequencer.remove_gating_sequence(Arc::new(Sequence::new(100)));
        assert!(!removed);
    }

    #[test]
    fn test_multi_producer_sequencer_remove_gating_sequence_concurrent() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = Arc::new(MultiProducerSequencer::new(8, wait_strategy));

        let gating_seq = Arc::new(Sequence::new(3));
        sequencer.add_gating_sequences(std::slice::from_ref(&gating_seq));
        assert_eq!(sequencer.gating_sequences.load().len(), 1);

        let barrier = Arc::new(std::sync::Barrier::new(3));

        let t1_sequencer = Arc::clone(&sequencer);
        let t1_seq = Arc::clone(&gating_seq);
        let t1_barrier = Arc::clone(&barrier);
        let t1 = std::thread::spawn(move || {
            t1_barrier.wait();
            t1_sequencer.remove_gating_sequence(t1_seq)
        });

        let t2_sequencer = Arc::clone(&sequencer);
        let t2_seq = Arc::clone(&gating_seq);
        let t2_barrier = Arc::clone(&barrier);
        let t2 = std::thread::spawn(move || {
            t2_barrier.wait();
            t2_sequencer.remove_gating_sequence(t2_seq)
        });

        barrier.wait();

        let r1 = t1.join().expect("thread 1 panicked");
        let r2 = t2.join().expect("thread 2 panicked");

        assert!(
            r1 ^ r2,
            "expected exactly one successful remove, got r1={r1} r2={r2}"
        );
        assert!(sequencer.gating_sequences.load().is_empty());
    }

    #[test]
    fn test_multi_producer_sequencer_capacity() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Test remaining capacity with no consumers
        assert_eq!(sequencer.remaining_capacity(), 8);

        // Test has_available_capacity
        assert!(sequencer.has_available_capacity(4));
        assert!(sequencer.has_available_capacity(8));

        // Add a consumer sequence
        let consumer_seq = Arc::new(Sequence::new(-1));
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_seq));

        // Claim some sequences
        let _seq1 = sequencer.next_n(4).unwrap();

        // With 4 sequences claimed and none consumed, remaining capacity should drop by 4
        assert_eq!(sequencer.remaining_capacity(), 4);

        // Update consumer sequence and test again
        consumer_seq.set(2);
        assert_eq!(sequencer.remaining_capacity(), 7);
    }

    #[test]
    fn test_multi_producer_sequencer_barrier() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = Arc::new(MultiProducerSequencer::new(8, wait_strategy));

        let dep_seq = Arc::new(Sequence::new(0));
        let barrier = sequencer.new_barrier_typed(vec![dep_seq]);

        assert_eq!(barrier.get_cursor().get(), -1);
        assert!(!barrier.is_alerted());
    }

    #[test]
    fn test_multi_producer_sequencer_highest_published_with_gaps() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Claim sequences 0, 1, 2, 3
        let seq0 = sequencer.next().unwrap();
        let seq1 = sequencer.next().unwrap();
        let seq2 = sequencer.next().unwrap();
        let seq3 = sequencer.next().unwrap();

        // Publish 0, 2, 3 but leave gap at 1
        sequencer.publish(seq0);
        sequencer.publish(seq2);
        sequencer.publish(seq3);

        // Should only return 0 due to gap at 1
        let highest = sequencer.get_highest_published_sequence(0, 3);
        assert_eq!(highest, 0);

        // Now publish sequence 1
        sequencer.publish(seq1);

        // Should now return 3 as all sequences are published
        let highest = sequencer.get_highest_published_sequence(0, 3);
        assert_eq!(highest, 3);
    }

    #[test]
    fn test_multi_producer_bitmap_functionality() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy); // Large enough for bitmap

        // Test bitmap functionality
        let seq = sequencer.next().unwrap();
        assert!(!sequencer.is_available(seq));

        sequencer.publish(seq);
        assert!(sequencer.is_available(seq));

        // Test that bitmap is used for large buffers
        assert!(!sequencer.available_bitmap.is_empty());
    }

    #[test]
    fn test_multi_producer_small_buffer_fallback() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy); // Small buffer

        // Test that bitmap is empty for small buffers (falls back to legacy method)
        assert!(sequencer.available_bitmap.is_empty());

        // Test that legacy method still works
        let seq = sequencer.next().unwrap();
        assert!(!sequencer.is_available(seq));

        sequencer.publish(seq);
        assert!(sequencer.is_available(seq));
    }

    #[test]
    fn test_bitmap_basic_functionality() {
        let wait_strategy = Arc::new(crate::disruptor::BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        // Initially, sequence 0 should not be available
        assert!(
            !sequencer.is_bitmap_available(0),
            "Sequence 0 should not be available initially"
        );

        // Publish sequence 0
        sequencer.publish_bitmap(0);

        // Now sequence 0 should be available
        assert!(
            sequencer.is_bitmap_available(0),
            "Sequence 0 should be available after publishing"
        );

        // Sequence 1 should still not be available
        assert!(
            !sequencer.is_bitmap_available(1),
            "Sequence 1 should not be available"
        );

        // Publish sequence 1
        sequencer.publish_bitmap(1);

        // Now sequence 1 should be available
        assert!(
            sequencer.is_bitmap_available(1),
            "Sequence 1 should be available after publishing"
        );

        // Test round wraparound - sequence 64 should use different round flag
        sequencer.publish_bitmap(64);
        assert!(
            sequencer.is_bitmap_available(64),
            "Sequence 64 should be available after publishing"
        );
    }

    #[test]
    fn test_bitmap_flags_calculation() {
        let wait_strategy = Arc::new(crate::disruptor::BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        // Test round flag calculation
        assert_eq!(
            sequencer.calculate_round_flag(0),
            0,
            "Sequence 0 should be round 0 (even)"
        );
        assert_eq!(
            sequencer.calculate_round_flag(63),
            0,
            "Sequence 63 should be round 0 (even)"
        );
        assert_eq!(
            sequencer.calculate_round_flag(64),
            1,
            "Sequence 64 should be round 1 (odd)"
        );
        assert_eq!(
            sequencer.calculate_round_flag(127),
            1,
            "Sequence 127 should be round 1 (odd)"
        );
        assert_eq!(
            sequencer.calculate_round_flag(128),
            0,
            "Sequence 128 should be round 0 (even)"
        );

        // Test availability indices calculation
        let (avail_idx, bit_idx) = sequencer.calculate_availability_indices(0);
        assert_eq!(avail_idx, 0);
        assert_eq!(bit_idx, 0);

        let (avail_idx, bit_idx) = sequencer.calculate_availability_indices(63);
        assert_eq!(avail_idx, 0);
        assert_eq!(bit_idx, 63);

        let (avail_idx, bit_idx) = sequencer.calculate_availability_indices(64);
        assert_eq!(
            avail_idx, 0,
            "Sequence 64 should map to availability_index 0 (same as sequence 0)"
        );
        assert_eq!(
            bit_idx, 0,
            "Sequence 64 should map to bit_index 0 (same as sequence 0)"
        );
    }

    #[test]
    fn test_bitmap_get_highest_published() {
        let wait_strategy = Arc::new(crate::disruptor::BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        // No sequences published yet
        let highest = sequencer.get_highest_published_sequence_bitmap(0, 5);
        assert_eq!(
            highest, -1,
            "Should return -1 when no sequences are published"
        );

        // Publish sequence 0
        sequencer.publish_bitmap(0);
        let highest = sequencer.get_highest_published_sequence_bitmap(0, 5);
        assert_eq!(
            highest, 0,
            "Should return 0 when only sequence 0 is published"
        );

        // Publish sequences 0, 1, 2
        sequencer.publish_bitmap(1);
        sequencer.publish_bitmap(2);
        let highest = sequencer.get_highest_published_sequence_bitmap(0, 5);
        assert_eq!(
            highest, 2,
            "Should return 2 when sequences 0,1,2 are published"
        );

        // Gap test: publish 0,1,2,4 (skip 3)
        sequencer.publish_bitmap(4);
        let highest = sequencer.get_highest_published_sequence_bitmap(0, 5);
        assert_eq!(
            highest, 2,
            "Should return 2 when there's a gap at sequence 3"
        );

        // Fill the gap
        sequencer.publish_bitmap(3);
        let highest = sequencer.get_highest_published_sequence_bitmap(0, 5);
        assert_eq!(
            highest, 4,
            "Should return 4 when all sequences 0-4 are published"
        );
    }

    #[test]
    fn test_multi_producer_availability_calculations() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        // Test index calculation
        let index = sequencer.calculate_index(65);
        assert_eq!(index, 1); // 65 & 63 = 1

        // Test availability flag calculation
        let flag = sequencer.calculate_availability_flag(64);
        assert_eq!(flag, 1); // 64 >> 6 = 1

        // Test availability indices calculation
        let (availability_index, bit_index) = sequencer.calculate_availability_indices(70);
        assert_eq!(availability_index, 0); // 6 >> 6 = 0
        assert_eq!(bit_index, 6); // 6 - 0 * 64 = 6
    }

    #[test]
    #[should_panic(expected = "Buffer size must be a power of 2")]
    fn test_multi_producer_invalid_buffer_size() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        MultiProducerSequencer::new(7, wait_strategy); // Not a power of 2
    }

    #[test]
    fn test_bitmap_wraparound_correctness() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        // Add a gating sequence so we can wrap around
        let consumer_seq = Arc::new(Sequence::new(-1));
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_seq));

        // Publish the entire first round (sequences 0..63)
        for i in 0..64 {
            let seq = sequencer.next().unwrap();
            assert_eq!(seq, i);
            sequencer.publish(seq);
        }

        // Verify all first-round sequences are available
        for i in 0..64 {
            assert!(
                sequencer.is_available(i),
                "Sequence {i} should be available"
            );
        }
        let highest = sequencer.get_highest_published_sequence(0, 63);
        assert_eq!(highest, 63);

        // Advance consumer to allow wraparound
        consumer_seq.set(63);

        // Publish the second round (sequences 64..127)
        for i in 64..128 {
            let seq = sequencer.next().unwrap();
            assert_eq!(seq, i);
            sequencer.publish(seq);
        }

        // Verify second-round sequences are available
        for i in 64..128 {
            assert!(
                sequencer.is_available(i),
                "Sequence {i} should be available after wraparound"
            );
        }
        let highest = sequencer.get_highest_published_sequence(64, 127);
        assert_eq!(highest, 127);
    }

    #[test]
    fn test_bitmap_wraparound_with_gaps() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        let consumer_seq = Arc::new(Sequence::new(-1));
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_seq));

        // Publish first round fully
        for _ in 0..64 {
            let seq = sequencer.next().unwrap();
            sequencer.publish(seq);
        }
        consumer_seq.set(63);

        // Second round: claim 0..3 but publish with gap at seq 66
        let seq64 = sequencer.next().unwrap();
        let seq65 = sequencer.next().unwrap();
        let seq66 = sequencer.next().unwrap();
        let seq67 = sequencer.next().unwrap();

        sequencer.publish(seq64);
        sequencer.publish(seq65);
        // Skip seq66
        sequencer.publish(seq67);

        // Highest contiguous from 64 should be 65 (gap at 66)
        let highest = sequencer.get_highest_published_sequence(64, 67);
        assert_eq!(highest, 65, "Should stop at gap at sequence 66");

        // Fill the gap
        sequencer.publish(seq66);
        let highest = sequencer.get_highest_published_sequence(64, 67);
        assert_eq!(highest, 67, "All sequences should be contiguous now");
    }

    #[test]
    fn test_bitmap_index_shift_field() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let seq64 = MultiProducerSequencer::new(64, wait_strategy.clone());
        assert_eq!(seq64.index_shift, 6); // 64 = 2^6

        let seq128 = MultiProducerSequencer::new(128, wait_strategy.clone());
        assert_eq!(seq128.index_shift, 7); // 128 = 2^7

        let seq1024 = MultiProducerSequencer::new(1024, wait_strategy);
        assert_eq!(seq1024.index_shift, 10); // 1024 = 2^10
    }

    #[test]
    fn test_no_dual_write_in_set_available() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        // For buffer_size >= 64, legacy buffer should not even be allocated
        assert!(
            sequencer.available_buffer.is_empty(),
            "Legacy buffer should not be allocated for bitmap-enabled sequencers"
        );

        // Bitmap should work correctly
        let seq = sequencer.next().unwrap();
        sequencer.set_available(seq);
        assert!(
            sequencer.is_bitmap_available(seq),
            "Bitmap should reflect the published sequence"
        );
    }

    // Soundness regression (P0-4, audit 2026-07-18): try_next_n must reject claims
    // larger than the buffer even with no gating sequences registered, otherwise a
    // single batch maps two sequences onto the same ring slot (aliased &mut).

    #[test]
    fn test_single_try_next_n_rejects_over_capacity_without_gating() {
        let sequencer = unsafe { SingleProducerSequencer::new(8, Arc::new(BusySpinWaitStrategy)) };

        assert_eq!(sequencer.try_next_n(9), None);
        assert_eq!(sequencer.try_next_n(1000), None);
        assert_eq!(sequencer.try_next_n(0), None);

        // Full-buffer claims stay legal, and with no gating sequences the
        // producer may wrap freely (LMAX semantics) — but never over-claim.
        assert_eq!(sequencer.try_next_n(8), Some(7));
        assert_eq!(sequencer.try_next_n(8), Some(15));
        assert_eq!(sequencer.try_next_n(9), None);
    }

    #[test]
    fn test_multi_try_next_n_rejects_over_capacity_without_gating() {
        let sequencer = MultiProducerSequencer::new(8, Arc::new(BusySpinWaitStrategy));

        assert_eq!(sequencer.try_next_n(9), None);
        assert_eq!(sequencer.try_next_n(1000), None);
        assert_eq!(sequencer.try_next_n(0), None);

        assert_eq!(sequencer.try_next_n(8), Some(7));
        assert_eq!(sequencer.try_next_n(8), Some(15));
        assert_eq!(sequencer.try_next_n(9), None);
    }

    #[test]
    fn test_single_try_next_n_respects_gating_backpressure() {
        let sequencer = unsafe { SingleProducerSequencer::new(8, Arc::new(BusySpinWaitStrategy)) };
        let consumer = Arc::new(Sequence::new(-1));
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer));

        // Fill the buffer, then further claims must fail until the consumer moves.
        assert_eq!(sequencer.try_next_n(8), Some(7));
        assert_eq!(sequencer.try_next(), None);

        consumer.set(3);
        assert_eq!(sequencer.try_next_n(4), Some(11));
        assert_eq!(sequencer.try_next(), None);
    }

    #[test]
    fn test_multi_try_next_n_respects_gating_backpressure() {
        let sequencer = MultiProducerSequencer::new(8, Arc::new(BusySpinWaitStrategy));
        let consumer = Arc::new(Sequence::new(-1));
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer));

        assert_eq!(sequencer.try_next_n(8), Some(7));
        assert_eq!(sequencer.try_next(), None);

        consumer.set(3);
        assert_eq!(sequencer.try_next_n(4), Some(11));
        assert_eq!(sequencer.try_next(), None);
    }

    #[test]
    fn test_remaining_capacity_without_gating_stays_bounded() {
        let single = unsafe { SingleProducerSequencer::new(8, Arc::new(BusySpinWaitStrategy)) };
        assert_eq!(single.remaining_capacity(), 8);
        single.try_next_n(4);
        // No gating sequences: producer gates on itself, capacity stays full.
        assert_eq!(single.remaining_capacity(), 8);

        let multi = MultiProducerSequencer::new(8, Arc::new(BusySpinWaitStrategy));
        assert_eq!(multi.remaining_capacity(), 8);
        multi.try_next_n(4);
        assert_eq!(multi.remaining_capacity(), 8);
    }

    /// Residual: two threads sharing an Arc after `unsafe { SingleProducerSequencer::new }`
    /// used to data-race on next_value. Concurrent drivers must now fail closed.
    #[test]
    fn test_single_concurrent_claim_driver_rejected() {
        use std::time::Duration;

        let sequencer =
            Arc::new(unsafe { SingleProducerSequencer::new(8, Arc::new(BusySpinWaitStrategy)) });
        let consumer = Arc::new(Sequence::new(-1));
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer));

        // Fill the ring so a subsequent next() blocks waiting on the consumer,
        // holding the claim lock for the whole spin.
        assert_eq!(sequencer.try_next_n(8), Some(7));

        let blocked = Arc::clone(&sequencer);
        let joiner = std::thread::spawn(move || blocked.next());

        // Give the blocked thread time to enter next() and hold claim_lock.
        std::thread::sleep(Duration::from_millis(50));

        match sequencer.next() {
            Err(DisruptorError::ConcurrentClaimDriver) => {}
            other => panic!("expected ConcurrentClaimDriver, got {other:?}"),
        }
        assert!(
            sequencer.try_next().is_none(),
            "try path must also refuse a concurrent driver"
        );

        // Unblock the first claimer and ensure it completes cleanly.
        consumer.set(7);
        let claimed = joiner.join().expect("blocked claimer must not panic");
        assert_eq!(claimed.unwrap(), 8);
    }
}
