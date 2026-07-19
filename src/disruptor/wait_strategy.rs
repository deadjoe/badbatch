//! Wait Strategy Implementation
//!
//! This module provides different wait strategies for the Disruptor pattern.
//! Wait strategies determine how consumers wait for new events to become available.
//! This follows the exact design from the original LMAX Disruptor WaitStrategy interface.

use crate::disruptor::{DisruptorError, Result, Sequence};
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

static NEVER_ALERTED: AtomicBool = AtomicBool::new(false);

/// Sequence a wait strategy should poll (LMAX `dependentSequence` contract).
///
/// Java wait strategies only observe the barrier's dependent sequence group:
/// - no upstream consumers → that group is the **cursor** (publish visibility)
/// - otherwise → **upstream consumer sequences only** (not re-min'd with cursor)
///
/// Downstream stages therefore do not reload the producer cursor on every spin.
/// Multi-producer publication contiguity is resolved *after* the wait by
/// [`crate::disruptor::sequence_barrier::ProcessingSequenceBarrier`].
#[inline]
fn wait_available_sequence(cursor: &Sequence, dependent_sequences: &[Arc<Sequence>]) -> i64 {
    if dependent_sequences.is_empty() {
        cursor.get()
    } else {
        Sequence::get_minimum_sequence(dependent_sequences)
    }
}

/// Strategy for waiting for events to become available
///
/// This trait defines how consumers wait for new events in the ring buffer.
/// Different strategies provide different trade-offs between CPU usage,
/// latency, and throughput.
pub trait WaitStrategy: Send + Sync + std::fmt::Debug {
    /// Wait for the given sequence to become available
    ///
    /// This method blocks until the specified sequence is available or
    /// until an alert/timeout occurs.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    /// * `cursor` - The current cursor position
    /// * `dependent_sequences` - Sequences that this consumer depends on
    ///
    /// # Returns
    /// The actual available sequence (may be higher than requested)
    ///
    /// # Errors
    /// Returns an error if waiting is interrupted or times out
    fn wait_for(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64> {
        self.wait_for_with_alert(sequence, cursor, dependent_sequences, &NEVER_ALERTED)
    }

    /// Wait for the given sequence while also observing an external barrier alert.
    ///
    /// The extra alert flag allows `SequenceBarrier::alert()` to interrupt any
    /// in-flight waits without requiring wait strategies to own barrier-local state.
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &AtomicBool,
    ) -> Result<i64>;

    /// Wait for the given sequence with timeout
    ///
    /// This method blocks until the specified sequence is available or
    /// until the timeout expires.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    /// * `cursor` - The current cursor position
    /// * `dependent_sequences` - Sequences that this consumer depends on
    /// * `timeout` - Maximum time to wait
    ///
    /// # Returns
    /// The actual available sequence (may be higher than requested)
    ///
    /// # Errors
    /// Returns an error if waiting is interrupted or times out
    fn wait_for_with_timeout(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: Duration,
    ) -> Result<i64> {
        self.wait_for_with_timeout_and_alert(
            sequence,
            cursor,
            dependent_sequences,
            timeout,
            &NEVER_ALERTED,
        )
    }

    /// Wait for the given sequence with timeout while also observing a barrier alert.
    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: Duration,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        // Default implementation: poll with timeout check
        let start = std::time::Instant::now();
        loop {
            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }

            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            if start.elapsed() >= timeout {
                return Err(DisruptorError::Timeout);
            }
            std::thread::yield_now();
        }
    }

    /// Wait for the given sequence with external shutdown signal
    ///
    /// This method blocks until the specified sequence is available or
    /// until an external shutdown signal is received.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    /// * `cursor` - The current cursor position
    /// * `dependent_sequences` - Sequences that this consumer depends on
    /// * `shutdown_flag` - External shutdown signal to check
    ///
    /// # Returns
    /// The actual available sequence (may be higher than requested)
    ///
    /// # Errors
    /// Returns `DisruptorError::Alert` if shutdown is signaled
    fn wait_for_with_shutdown(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &AtomicBool,
    ) -> Result<i64> {
        self.wait_for_with_shutdown_and_alert(
            sequence,
            cursor,
            dependent_sequences,
            shutdown_flag,
            &NEVER_ALERTED,
        )
    }

    /// Wait for the given sequence with shutdown and barrier alert support.
    fn wait_for_with_shutdown_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &AtomicBool,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        // Default implementation with periodic shutdown checks
        let check_interval = Duration::from_millis(1);

        loop {
            // Check shutdown flag first
            if shutdown_flag.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            // Try to get the sequence with a short timeout
            match self.wait_for_with_timeout_and_alert(
                sequence,
                cursor,
                dependent_sequences,
                check_interval,
                alerted,
            ) {
                Ok(available_sequence) => return Ok(available_sequence),
                Err(DisruptorError::Timeout) => {
                    // Timeout indicates we should re-check shutdown flag and retry.
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Signal all waiting threads to wake up
    ///
    /// This is used when shutting down the Disruptor to wake up
    /// any threads that are waiting for events.
    fn signal_all_when_blocking(&self);

    /// Whether producers should call `signal_all_when_blocking()` after publishing.
    ///
    /// This is used to avoid a vtable dispatch on every publish for non-blocking
    /// strategies (e.g., BusySpin/Yielding/Sleeping), while keeping correct
    /// behavior for blocking/custom strategies.
    #[inline]
    fn needs_signal(&self) -> bool {
        true
    }
}

/// Blocking wait strategy using parking/unparking
///
/// This strategy uses thread parking to wait for events. It provides
/// good CPU efficiency but may have higher latency than busy-wait strategies.
/// This is equivalent to the BlockingWaitStrategy in the original LMAX Disruptor.
#[derive(Debug, Default)]
pub struct BlockingWaitStrategy {
    // Use a condvar for proper blocking/signaling
    condvar: Condvar,
    mutex: Mutex<BlockingState>,
}

impl Clone for BlockingWaitStrategy {
    fn clone(&self) -> Self {
        // Create a new instance with default state
        Self::new()
    }
}

/// Internal state for blocking wait strategy
#[derive(Debug, Default)]
struct BlockingState {
    /// Whether the strategy has been alerted (for shutdown)
    alerted: bool,
}

impl BlockingWaitStrategy {
    /// Create a new blocking wait strategy
    pub fn new() -> Self {
        Self {
            condvar: Condvar::new(),
            mutex: Mutex::new(BlockingState::default()),
        }
    }

    /// Alert the wait strategy to wake up all waiting threads
    /// This is used during shutdown to interrupt waiting threads
    pub fn alert(&self) {
        let mut state = self.mutex.lock();
        state.alerted = true;
        self.condvar.notify_all();
    }

    /// Clear the alert state
    pub fn clear_alert(&self) {
        self.mutex.lock().alerted = false;
    }

    /// Check if the strategy is currently alerted
    pub fn is_alerted(&self) -> bool {
        self.mutex.lock().alerted
    }

    #[inline]
    fn is_alerted_with(&self, alerted: &AtomicBool) -> bool {
        self.is_alerted() || alerted.load(Ordering::Acquire)
    }
}

impl WaitStrategy for BlockingWaitStrategy {
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &AtomicBool,
    ) -> Result<i64> {
        // This follows the exact LMAX Disruptor BlockingWaitStrategy.waitFor logic
        let mut available_sequence;

        // First check: wait for cursor to advance if needed
        if cursor.get() < sequence {
            let mut guard = self.mutex.lock();

            while cursor.get() < sequence {
                // Check for alert state (equivalent to barrier.checkAlert() in LMAX)
                if guard.alerted || alerted.load(Ordering::Acquire) {
                    return Err(DisruptorError::Alert);
                }

                // Wait for signal from producers (equivalent to mutex.wait() in LMAX)
                self.condvar.wait(&mut guard);
            }
        }

        // Second check: spin wait for dependent sequences (equivalent to LMAX spin wait)
        loop {
            available_sequence = if dependent_sequences.is_empty() {
                cursor.get()
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };

            if available_sequence >= sequence {
                break;
            }

            // Check for alert state again
            if self.is_alerted_with(alerted) {
                return Err(DisruptorError::Alert);
            }

            // Equivalent to Thread.onSpinWait() in LMAX
            std::hint::spin_loop();
        }

        Ok(available_sequence)
    }

    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: Duration,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        let start_time = std::time::Instant::now();

        // First check: wait for cursor to advance if needed (with timeout)
        if cursor.get() < sequence {
            let mut guard = self.mutex.lock();

            while cursor.get() < sequence {
                // Check for alert state first so barrier alert wins over timeout.
                if guard.alerted || alerted.load(Ordering::Acquire) {
                    return Err(DisruptorError::Alert);
                }

                // Check for timeout
                if start_time.elapsed() >= timeout {
                    return Err(DisruptorError::Timeout);
                }

                // Calculate remaining timeout
                let remaining = timeout.saturating_sub(start_time.elapsed());
                if remaining.is_zero() {
                    return Err(DisruptorError::Timeout);
                }

                // Wait with timeout (parking_lot returns WaitTimeoutResult directly)
                let timeout_result = self.condvar.wait_for(&mut guard, remaining);

                if timeout_result.timed_out() {
                    if guard.alerted || alerted.load(Ordering::Acquire) {
                        return Err(DisruptorError::Alert);
                    }
                    return Err(DisruptorError::Timeout);
                }
            }
        }

        // Second check: spin wait for dependent sequences (with timeout)
        loop {
            let available_sequence = if dependent_sequences.is_empty() {
                cursor.get()
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };

            if available_sequence >= sequence {
                return Ok(available_sequence);
            }

            // Check for alert state first so barrier alert wins over timeout.
            if self.is_alerted_with(alerted) {
                return Err(DisruptorError::Alert);
            }

            // Check for timeout
            if start_time.elapsed() >= timeout {
                return Err(DisruptorError::Timeout);
            }

            std::hint::spin_loop();
        }
    }

    fn signal_all_when_blocking(&self) {
        // Coordinate with the same mutex used by waiters so a producer cannot
        // notify in the narrow window between the consumer's cursor check and
        // its transition into the condvar wait state.
        let _guard = self.mutex.lock();
        self.condvar.notify_all();
    }
}

/// Yielding wait strategy
///
/// LMAX-aligned **spin-then-yield** policy (see Java
/// `com.lmax.disruptor.YieldingWaitStrategy`):
///
/// 1. Busy-poll for [`Self::SPIN_TRIES`] unsuccessful observations of the
///    cursor / dependent sequences (with `spin_loop` hints).
/// 2. Then call `thread::yield_now()` on every subsequent miss until the
///    sequence is available.
///
/// **Protocol:** unchanged relative to other non-blocking strategies — returns
/// only when `available >= sequence`, or with `Alert` / `Timeout` as documented
/// on [`WaitStrategy`]. Claim, publish, barrier contiguity, and memory ordering
/// on sequence loads are not altered; only the idle action between polls is.
///
/// **CPU:** the spin window can briefly use a full core (same trade-off as LMAX).
/// After the window is exhausted, the thread yields so other work can run.
/// Prefer [`BusySpinWaitStrategy`] only when cores can be dedicated.
#[derive(Debug, Default, Clone)]
pub struct YieldingWaitStrategy;

impl YieldingWaitStrategy {
    /// Unsuccessful polls before the first `yield_now` in each wait call.
    ///
    /// Matches LMAX `YieldingWaitStrategy.SPIN_TRIES` (`100`). The counter is
    /// local to each `wait_for*` invocation and is not shared across calls.
    pub const SPIN_TRIES: i32 = 100;

    /// Create a new yielding wait strategy.
    pub fn new() -> Self {
        Self
    }

    /// LMAX `applyWaitMethod`: decrement spin counter, then yield.
    ///
    /// Terminal-flag checks remain in the caller so every poll still observes
    /// alert/shutdown; only the idle action changes.
    #[inline]
    fn apply_wait_method(counter: &mut i32) {
        if *counter == 0 {
            thread::yield_now();
        } else {
            *counter -= 1;
            // LMAX's spin phase is an empty decrement; `spin_loop` is the
            // portable hint for that tight re-poll (PAUSE / YIELD).
            std::hint::spin_loop();
        }
    }
}

impl WaitStrategy for YieldingWaitStrategy {
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &AtomicBool,
    ) -> Result<i64> {
        let mut counter = Self::SPIN_TRIES;
        loop {
            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }
            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }
            Self::apply_wait_method(&mut counter);
        }
    }

    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: Duration,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        let start_time = std::time::Instant::now();
        let mut counter = Self::SPIN_TRIES;

        loop {
            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }

            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            if start_time.elapsed() >= timeout {
                return Err(DisruptorError::Timeout);
            }

            Self::apply_wait_method(&mut counter);
        }
    }

    fn wait_for_with_shutdown_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &AtomicBool,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        let mut counter = Self::SPIN_TRIES;
        loop {
            if shutdown_flag.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }
            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }
            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }
            Self::apply_wait_method(&mut counter);
        }
    }

    fn signal_all_when_blocking(&self) {
        // Non-blocking strategy: nothing to wake.
    }

    #[inline]
    fn needs_signal(&self) -> bool {
        false
    }
}

/// Busy-spin wait strategy
///
/// This strategy continuously polls for events without yielding the CPU.
/// It provides the lowest latency but uses 100% CPU while waiting.
/// Use this only when you can dedicate CPU cores to the Disruptor.
#[derive(Debug, Default, Clone)]
pub struct BusySpinWaitStrategy;

impl BusySpinWaitStrategy {
    /// Create a new busy-spin wait strategy
    pub fn new() -> Self {
        Self
    }
}

impl WaitStrategy for BusySpinWaitStrategy {
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &AtomicBool,
    ) -> Result<i64> {
        loop {
            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }
            std::hint::spin_loop();
        }
    }

    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: Duration,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        let start_time = std::time::Instant::now();

        loop {
            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }

            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            if start_time.elapsed() >= timeout {
                return Err(DisruptorError::Timeout);
            }

            std::hint::spin_loop();
        }
    }

    fn wait_for_with_shutdown_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &AtomicBool,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        loop {
            if shutdown_flag.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }
            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }
            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }
            std::hint::spin_loop();
        }
    }

    fn signal_all_when_blocking(&self) {
        // Busy spin strategy doesn't block, so no signaling needed
    }

    #[inline]
    fn needs_signal(&self) -> bool {
        false
    }
}

/// Sleeping wait strategy
///
/// This strategy sleeps for a short duration while waiting.
/// It provides good CPU efficiency but may have variable latency
/// depending on the sleep duration.
#[derive(Debug, Clone)]
pub struct SleepingWaitStrategy {
    sleep_duration: Duration,
}

impl SleepingWaitStrategy {
    /// Create a new sleeping wait strategy with default sleep duration
    pub fn new() -> Self {
        Self {
            sleep_duration: Duration::from_millis(1),
        }
    }

    /// Create a new sleeping wait strategy with custom sleep duration
    ///
    /// # Arguments
    /// * `sleep_duration` - How long to sleep between checks
    pub fn new_with_duration(sleep_duration: Duration) -> Self {
        Self { sleep_duration }
    }
}

impl Default for SleepingWaitStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitStrategy for SleepingWaitStrategy {
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &AtomicBool,
    ) -> Result<i64> {
        loop {
            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }
            thread::sleep(self.sleep_duration);
        }
    }

    fn wait_for_with_shutdown_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &AtomicBool,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        loop {
            if shutdown_flag.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }
            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }
            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }
            thread::sleep(self.sleep_duration);
        }
    }

    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: Duration,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        let start_time = std::time::Instant::now();

        loop {
            if alerted.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            // Check timeout first
            if start_time.elapsed() >= timeout {
                return Err(DisruptorError::Timeout);
            }

            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }
            thread::sleep(self.sleep_duration);
        }
    }

    fn signal_all_when_blocking(&self) {
        // Sleeping strategy doesn't use complex blocking, so no signaling needed
    }

    #[inline]
    fn needs_signal(&self) -> bool {
        false
    }
}

/// Allow dynamic wait-strategy selection at the DSL boundary.
///
/// Prefer concrete monomorphized `W: WaitStrategy` on the hot path. This impl
/// exists for benches and call sites that choose a strategy by name at runtime.
impl WaitStrategy for Box<dyn WaitStrategy> {
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &AtomicBool,
    ) -> Result<i64> {
        (**self).wait_for_with_alert(sequence, cursor, dependent_sequences, alerted)
    }

    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: Duration,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        (**self).wait_for_with_timeout_and_alert(
            sequence,
            cursor,
            dependent_sequences,
            timeout,
            alerted,
        )
    }

    fn wait_for_with_shutdown_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &AtomicBool,
        alerted: &AtomicBool,
    ) -> Result<i64> {
        (**self).wait_for_with_shutdown_and_alert(
            sequence,
            cursor,
            dependent_sequences,
            shutdown_flag,
            alerted,
        )
    }

    fn signal_all_when_blocking(&self) {
        (**self).signal_all_when_blocking();
    }

    fn needs_signal(&self) -> bool {
        (**self).needs_signal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;
    use std::thread;
    use std::time::Instant;

    #[test]
    fn test_blocking_wait_strategy_creation() {
        let strategy = BlockingWaitStrategy::new();
        assert!(!strategy.is_alerted());

        // Test default creation
        let default_strategy = BlockingWaitStrategy::default();
        assert!(!default_strategy.is_alerted());
    }

    #[test]
    fn test_blocking_wait_strategy_immediate_return() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        // Should return immediately if sequence is already available
        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_blocking_wait_strategy_with_dependent_sequences() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(8));
        let dep2 = Arc::new(Sequence::new(6));
        let dependent_sequences = vec![dep1, dep2];

        // Should return minimum of dependent sequences
        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 6); // Minimum of dependent sequences
    }

    #[test]
    fn test_blocking_wait_strategy_alert_functionality() {
        let strategy = BlockingWaitStrategy::new();

        // Test alert and clear
        assert!(!strategy.is_alerted());
        strategy.alert();
        assert!(strategy.is_alerted());
        strategy.clear_alert();
        assert!(!strategy.is_alerted());
    }

    #[test]
    fn test_blocking_wait_strategy_alert_during_wait() {
        let strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(0)); // Start with low sequence
        let dependent_sequences = vec![];
        let guard = strategy.mutex.lock();
        let (ready_tx, ready_rx) = sync_channel(1);

        let strategy_clone = strategy.clone();
        let cursor_clone = cursor.clone();

        // Spawn a thread that will wait for a high sequence
        let handle = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            strategy_clone.wait_for(100, &cursor_clone, &dependent_sequences)
        });

        ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(guard);

        // Alert the strategy
        strategy.alert();

        // The waiting thread should return with an alert error
        let result = handle.join().unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));
    }

    #[test]
    fn test_blocking_wait_strategy_external_alert_interrupts_wait() {
        use std::sync::atomic::AtomicBool;

        let strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];
        let barrier_alert = Arc::new(AtomicBool::new(false));
        let guard = strategy.mutex.lock();
        let (ready_tx, ready_rx) = sync_channel(1);

        let strategy_clone = strategy.clone();
        let cursor_clone = cursor.clone();
        let barrier_alert_clone = barrier_alert.clone();

        let handle = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            strategy_clone.wait_for_with_timeout_and_alert(
                100,
                &cursor_clone,
                &dependent_sequences,
                Duration::from_millis(250),
                &barrier_alert_clone,
            )
        });

        ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(guard);
        barrier_alert.store(true, Ordering::Release);
        strategy.signal_all_when_blocking();

        let result = handle.join().unwrap();
        assert!(matches!(result, Err(DisruptorError::Alert)));
    }

    #[test]
    fn test_blocking_wait_strategy_timeout() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];
        let timeout = Duration::from_millis(50);

        let start = Instant::now();
        let result = strategy.wait_for_with_timeout(100, &cursor, &dependent_sequences, timeout);
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
        // Condvar timeouts must not return before the requested duration. Do
        // not assert an upper wall-clock bound: once Timeout is returned, such
        // a bound measures CI scheduler delay rather than wait correctness.
        assert!(
            elapsed >= timeout,
            "timeout returned early: elapsed={elapsed:?} timeout={timeout:?}"
        );
    }

    #[test]
    fn test_blocking_wait_strategy_timeout_immediate_return() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];
        let timeout = Duration::from_millis(100);

        // The available cursor must bypass the timeout path.
        let result = strategy.wait_for_with_timeout(5, &cursor, &dependent_sequences, timeout);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_blocking_wait_strategy_signal_all() {
        let strategy = BlockingWaitStrategy::new();

        // Test that signal_all_when_blocking doesn't panic
        strategy.signal_all_when_blocking();
    }

    #[test]
    fn test_blocking_wait_strategy_signal_coordinates_with_waiter_mutex() {
        use std::sync::mpsc::sync_channel;

        let strategy = Arc::new(BlockingWaitStrategy::new());
        let mut guard = strategy.mutex.lock();
        let (ready_tx, ready_rx) = sync_channel(1);
        let completed = Arc::new(AtomicBool::new(false));

        let strategy_clone = Arc::clone(&strategy);
        let completed_clone = Arc::clone(&completed);
        let handle = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            strategy_clone.signal_all_when_blocking();
            completed_clone.store(true, Ordering::Release);
        });

        ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        // `signal_all_when_blocking()` must synchronize with the same mutex used by
        // waiters; otherwise the notify can be emitted before a waiter actually
        // transitions into the condvar wait state, leading to lost wakeups.
        assert!(
            !completed.load(Ordering::Acquire),
            "signal_all_when_blocking returned before the waiter released the mutex"
        );

        let timeout_result = strategy
            .condvar
            .wait_for(&mut guard, Duration::from_millis(100));
        assert!(!timeout_result.timed_out());

        handle.join().unwrap();
        assert!(completed.load(Ordering::Acquire));
    }

    #[test]
    fn test_yielding_wait_strategy_creation() {
        let _strategy = YieldingWaitStrategy::new();

        // Test default creation
        let _default_strategy = YieldingWaitStrategy::new();

        // Both should work (no state to compare)
    }

    #[test]
    fn test_yielding_wait_strategy_immediate_return() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_yielding_wait_strategy_with_dependent_sequences() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(8));
        let dep2 = Arc::new(Sequence::new(6));
        let dependent_sequences = vec![dep1, dep2];

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 6); // Minimum of dependent sequences
    }

    #[test]
    fn test_yielding_wait_strategy_with_low_dependent_sequence() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(3)); // Cursor lower than requested
        let dep1 = Arc::new(Sequence::new(2)); // Dependent sequence even lower
        let dependent_sequences = vec![dep1];

        // With unified semantics, use timeout variant to avoid hanging
        let result = strategy.wait_for_with_timeout(
            5,
            &cursor,
            &dependent_sequences,
            Duration::from_millis(10),
        );

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
    }

    #[test]
    fn test_yielding_wait_strategy_signal_all() {
        let strategy = YieldingWaitStrategy::new();

        // Test that signal_all_when_blocking doesn't panic
        strategy.signal_all_when_blocking();
    }

    #[test]
    fn test_busy_spin_wait_strategy_creation() {
        let _strategy = BusySpinWaitStrategy::new();

        // Test default creation
        let _default_strategy = BusySpinWaitStrategy::new();

        // Both should work (no state to compare)
    }

    #[test]
    fn test_busy_spin_wait_strategy_immediate_return() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_busy_spin_wait_strategy_with_dependent_sequences() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(8));
        let dep2 = Arc::new(Sequence::new(6));
        let dependent_sequences = vec![dep1, dep2];

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 6); // Minimum of dependent sequences
    }

    #[test]
    fn test_busy_spin_wait_strategy_with_low_dependent_sequence() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(3)); // Cursor lower than requested
        let dep1 = Arc::new(Sequence::new(2)); // Dependent sequence even lower
        let dependent_sequences = vec![dep1];

        let result = strategy.wait_for_with_timeout(
            5,
            &cursor,
            &dependent_sequences,
            Duration::from_millis(10),
        );

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
    }

    #[test]
    fn test_busy_spin_wait_strategy_signal_all() {
        let strategy = BusySpinWaitStrategy::new();

        // Test that signal_all_when_blocking doesn't panic
        strategy.signal_all_when_blocking();
    }

    #[test]
    fn test_sleeping_wait_strategy_creation() {
        let strategy = SleepingWaitStrategy::new();
        assert_eq!(strategy.sleep_duration, Duration::from_millis(1));

        // Test default creation
        let default_strategy = SleepingWaitStrategy::default();
        assert_eq!(default_strategy.sleep_duration, Duration::from_millis(1));

        // Test custom duration
        let custom_duration = Duration::from_micros(500);
        let custom_strategy = SleepingWaitStrategy::new_with_duration(custom_duration);
        assert_eq!(custom_strategy.sleep_duration, custom_duration);
    }

    #[test]
    fn test_sleeping_wait_strategy_immediate_return() {
        let strategy = SleepingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);

        // Test custom duration
        let custom_strategy = SleepingWaitStrategy::new_with_duration(Duration::from_micros(100));
        let result = custom_strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sleeping_wait_strategy_with_dependent_sequences() {
        let strategy = SleepingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(8));
        let dep2 = Arc::new(Sequence::new(6));
        let dependent_sequences = vec![dep1, dep2];

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 6); // Minimum of dependent sequences
    }

    #[test]
    fn test_sleeping_wait_strategy_with_low_dependent_sequence() {
        let strategy = SleepingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(3)); // Cursor lower than requested
        let dep1 = Arc::new(Sequence::new(2)); // Dependent sequence even lower
        let dependent_sequences = vec![dep1];

        let result = strategy.wait_for_with_timeout(
            5,
            &cursor,
            &dependent_sequences,
            Duration::from_millis(10),
        );

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
    }

    #[test]
    fn test_sleeping_wait_strategy_signal_all() {
        let strategy = SleepingWaitStrategy::new();

        // Test that signal_all_when_blocking doesn't panic
        strategy.signal_all_when_blocking();
    }

    // Advanced tests for complex scenarios

    #[test]
    fn test_blocking_wait_strategy_concurrent_access() {
        let strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(5));
        let dependent_sequences = vec![];

        let mut handles = vec![];

        // Spawn multiple threads that wait for different sequences
        for i in 0..3 {
            let strategy_clone = strategy.clone();
            let cursor_clone = cursor.clone();
            let deps_clone = dependent_sequences.clone();

            let handle =
                thread::spawn(move || strategy_clone.wait_for(i, &cursor_clone, &deps_clone));
            handles.push(handle);
        }

        // All threads should complete successfully since cursor is at 5
        for handle in handles {
            let result = handle.join().unwrap();
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 5);
        }
    }

    #[test]
    fn test_blocking_wait_strategy_timeout_with_dependent_sequences() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(2)); // This will cause timeout
        let dependent_sequences = vec![dep1];
        let timeout = Duration::from_millis(50);

        let start = Instant::now();
        let result = strategy.wait_for_with_timeout(5, &cursor, &dependent_sequences, timeout);
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
        assert!(elapsed >= timeout);
    }

    #[test]
    fn test_blocking_wait_strategy_zero_timeout() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];
        let timeout = Duration::from_millis(0);

        let result = strategy.wait_for_with_timeout(100, &cursor, &dependent_sequences, timeout);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
    }

    #[test]
    fn test_yielding_wait_strategy_actual_yielding() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];

        // Spawn a thread that will advance the cursor after a short delay
        let cursor_clone = cursor.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            cursor_clone.set(10);
        });

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert_eq!(result.unwrap(), 10);
        handle.join().unwrap();
    }

    #[test]
    fn test_busy_spin_wait_strategy_actual_spinning() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];

        // Spawn a thread that will advance the cursor after a short delay
        let cursor_clone = cursor.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            cursor_clone.set(10);
        });

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert_eq!(result.unwrap(), 10);
        handle.join().unwrap();
    }

    #[test]
    fn test_sleeping_wait_strategy_actual_sleeping() {
        let strategy = SleepingWaitStrategy::new_with_duration(Duration::from_millis(5));
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];

        // Spawn a thread that will advance the cursor after a short delay
        let cursor_clone = cursor.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            cursor_clone.set(10);
        });

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert_eq!(result.unwrap(), 10);
        handle.join().unwrap();
    }

    #[test]
    fn test_wait_strategy_trait_default_timeout_implementation() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];
        let timeout = Duration::from_millis(100);

        // Test that default implementation delegates to wait_for
        let result = strategy.wait_for_with_timeout(5, &cursor, &dependent_sequences, timeout);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_blocking_wait_strategy_mutex_poisoning_recovery() {
        let strategy = BlockingWaitStrategy::new();

        // Test that the strategy can handle mutex operations gracefully
        // Even if there were poisoning issues, the methods should not panic
        strategy.alert();
        strategy.clear_alert();
        assert!(!strategy.is_alerted());
    }

    #[test]
    fn test_all_strategies_debug_format() {
        let blocking = BlockingWaitStrategy::new();
        let yielding = YieldingWaitStrategy::new();
        let busy_spin = BusySpinWaitStrategy::new();
        let sleeping = SleepingWaitStrategy::new();

        // Test that all strategies implement Debug properly
        let blocking_debug = format!("{blocking:?}");
        let yielding_debug = format!("{yielding:?}");
        let busy_spin_debug = format!("{busy_spin:?}");
        let sleeping_debug = format!("{sleeping:?}");

        assert!(blocking_debug.contains("BlockingWaitStrategy"));
        assert!(yielding_debug.contains("YieldingWaitStrategy"));
        assert!(busy_spin_debug.contains("BusySpinWaitStrategy"));
        assert!(sleeping_debug.contains("SleepingWaitStrategy"));
    }

    #[test]
    fn test_blocking_wait_strategy_edge_cases() {
        let strategy = BlockingWaitStrategy::new();

        // Test with sequence equal to cursor
        let cursor = Arc::new(Sequence::new(5));
        let result = strategy.wait_for(5, &cursor, &[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);

        // Test with sequence lower than cursor - should return immediately
        let result = strategy.wait_for(3, &cursor, &[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);

        // Test with dependent sequence higher than cursor
        let dep = Arc::new(Sequence::new(10));
        let result = strategy.wait_for(3, &cursor, &[dep]); // Request sequence 3, which is available
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10); // Should return dependent sequence value since it's the minimum available
    }

    #[test]
    fn test_multiple_dependent_sequences_minimum() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(20));

        // Create multiple dependent sequences with different values
        let dep1 = Arc::new(Sequence::new(15));
        let dep2 = Arc::new(Sequence::new(12));
        let dep3 = Arc::new(Sequence::new(18));
        let dependent_sequences = vec![dep1, dep2, dep3];

        let result = strategy.wait_for(10, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 12); // Should return minimum of dependent sequences
    }

    #[test]
    fn test_sleeping_wait_strategy_very_short_duration() {
        let strategy = SleepingWaitStrategy::new_with_duration(Duration::from_nanos(1));
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_blocking_wait_strategy_alert_before_wait() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];

        // Alert before waiting
        strategy.alert();

        let result = strategy.wait_for(100, &cursor, &dependent_sequences);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));
    }

    // Tests inspired by LMAX Disruptor test patterns

    #[test]
    fn test_blocking_wait_strategy_with_delayed_sequence_update() {
        let strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(-1)); // Start with no events available
        let guard = strategy.mutex.lock();
        let (ready_tx, ready_rx) = sync_channel(1);
        let (release_tx, release_rx) = sync_channel(1);

        // Spawn a thread that will update the cursor once the waiter has entered the test flow.
        let cursor_clone = cursor.clone();
        let strategy_clone = strategy.clone();
        let updater = thread::spawn(move || {
            release_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            cursor_clone.set(5); // Make sequences 0-5 available
            strategy_clone.signal_all_when_blocking();
        });

        let strategy_clone = strategy.clone();
        let cursor_clone = cursor.clone();
        let handle = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            strategy_clone.wait_for(3, &cursor_clone, &[])
        });

        ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(guard);
        release_tx.send(()).unwrap();
        let result = handle.join().unwrap();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);

        updater.join().unwrap();
    }

    #[test]
    fn test_yielding_wait_strategy_with_delayed_sequence_update() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(-1));

        // Spawn a thread that will update the cursor after a delay
        let cursor_clone = cursor.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            cursor_clone.set(5);
        });

        let result = strategy.wait_for(3, &cursor, &[]);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);

        handle.join().unwrap();
    }

    #[test]
    fn test_yielding_wait_strategy_spin_tries_matches_lmax() {
        // Contract lock: keep the public constant aligned with LMAX Java
        // `YieldingWaitStrategy.SPIN_TRIES` (and simple_wait_strategy default).
        assert_eq!(YieldingWaitStrategy::SPIN_TRIES, 100);
    }

    #[test]
    fn test_wait_available_polls_dependents_not_cursor_min() {
        // LMAX Yielding/BusySpin only poll `dependentSequence`. With upstream
        // deps present, the cursor is not re-min'd into availability (stage1
        // already waited for publish before advancing).
        let cursor = Sequence::new(3);
        let dep = Arc::new(Sequence::new(10));
        let available = wait_available_sequence(&cursor, &[dep]);
        assert_eq!(available, 10);

        let cursor_only = wait_available_sequence(&Sequence::new(7), &[]);
        assert_eq!(cursor_only, 7);
    }

    #[test]
    fn test_yielding_apply_wait_method_counts_down_then_stays_at_zero() {
        let mut counter = 3;
        YieldingWaitStrategy::apply_wait_method(&mut counter);
        assert_eq!(counter, 2);
        YieldingWaitStrategy::apply_wait_method(&mut counter);
        assert_eq!(counter, 1);
        YieldingWaitStrategy::apply_wait_method(&mut counter);
        assert_eq!(counter, 0);
        // At zero: yield path; counter remains 0 (subsequent waits always yield).
        YieldingWaitStrategy::apply_wait_method(&mut counter);
        assert_eq!(counter, 0);
    }

    #[test]
    fn test_busy_spin_wait_strategy_with_delayed_sequence_update() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(-1));

        // Spawn a thread that will update the cursor after a delay
        let cursor_clone = cursor.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            cursor_clone.set(5);
        });

        let result = strategy.wait_for(3, &cursor, &[]);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);

        handle.join().unwrap();
    }

    #[test]
    fn test_sleeping_wait_strategy_with_delayed_sequence_update() {
        let strategy = SleepingWaitStrategy::new_with_duration(Duration::from_millis(5));
        let cursor = Arc::new(Sequence::new(-1));

        // Spawn a thread that will update the cursor after a delay
        let cursor_clone = cursor.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            cursor_clone.set(5);
        });

        let result = strategy.wait_for(3, &cursor, &[]);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);

        handle.join().unwrap();
    }

    #[test]
    fn test_wait_strategy_with_multiple_dependent_sequences_realistic() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));

        // Create multiple consumer sequences at different positions
        let consumer1 = Arc::new(Sequence::new(8)); // Slowest consumer
        let consumer2 = Arc::new(Sequence::new(9)); // Faster consumer
        let consumer3 = Arc::new(Sequence::new(7)); // Even slower consumer

        let dependent_sequences = vec![consumer1, consumer2, consumer3];

        // Should return the minimum of all dependent sequences
        let result = strategy.wait_for(6, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 7); // Minimum of dependent sequences
    }

    #[test]
    fn test_wait_strategy_performance_characteristics() {
        // This test verifies that different strategies have expected performance characteristics
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        // All strategies must take the already-available fast path. The return
        // value proves that path without a scheduler-sensitive timing bound.
        let strategies: Vec<Box<dyn WaitStrategy>> = vec![
            Box::new(BusySpinWaitStrategy::new()),
            Box::new(YieldingWaitStrategy::new()),
            Box::new(SleepingWaitStrategy::new()),
            Box::new(BlockingWaitStrategy::new()),
        ];

        for strategy in strategies {
            let result = strategy.wait_for(5, &cursor, &dependent_sequences);

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 10);
        }
    }
}
