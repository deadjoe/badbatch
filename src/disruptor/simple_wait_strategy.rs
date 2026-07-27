//! Simplified wait strategies inspired by disruptor-rs
//!
//! This module provides simplified wait strategies that are easier to use
//! and understand compared to the full LMAX Disruptor wait strategies.

use crate::disruptor::Sequence;
use std::hint;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Return the sequence a waiter should observe.
///
/// Mirrors `wait_strategy::wait_available_sequence`: when there are upstream
/// dependent sequences the waiter observes only their minimum; otherwise it
/// observes the producer cursor.
#[inline]
fn wait_available_sequence(cursor: &Sequence, dependent_sequences: &[Arc<Sequence>]) -> i64 {
    if dependent_sequences.is_empty() {
        cursor.get()
    } else {
        Sequence::get_minimum_sequence(dependent_sequences)
    }
}

/// Simplified wait strategy trait inspired by disruptor-rs
///
/// This is much simpler than the full LMAX WaitStrategy interface,
/// focusing on the core waiting behavior without complex dependencies.
pub trait SimpleWaitStrategy: Copy + Send + Sync + std::fmt::Debug {
    /// Idle / backoff while a sequence is not yet available.
    ///
    /// Named `backoff` (not `wait_for`) to avoid colliding with
    /// [`crate::disruptor::WaitStrategy::wait_for`] once these types implement
    /// the full wait-strategy trait.
    ///
    /// # Deprecated
    /// Use [`Self::backoff_with_miss`] instead. `backoff` does not receive the
    /// per-wait miss counter, so stateful strategies (e.g. [`Yielding`]) cannot
    /// align their spin-then-yield schedule with the full LMAX implementations.
    #[deprecated(note = "use backoff_with_miss; removed at 1.0")]
    fn backoff(&self) {
        let mut miss = 0u32;
        self.backoff_with_miss(&mut miss);
    }

    /// Idle / backoff for one miss, with a per-wait miss counter.
    ///
    /// `miss` starts at `0` for each invocation of a wait method and is
    /// incremented by the strategy on every call. Stateless strategies may
    /// ignore it; [`Yielding`] uses it to match the full
    /// [`crate::disruptor::YieldingWaitStrategy`] spin-then-yield schedule.
    ///
    /// The default implementation delegates to [`Self::backoff`] for backward
    /// compatibility with existing implementations that have not yet migrated.
    fn backoff_with_miss(&self, miss: &mut u32) {
        let _ = miss;
        #[allow(deprecated)]
        self.backoff();
    }
}

/// Busy spin wait strategy - lowest possible latency
///
/// This strategy continuously checks for new events without yielding the CPU.
/// It provides the lowest latency but uses 100% CPU while waiting.
#[derive(Copy, Clone, Debug)]
pub struct BusySpin;

impl SimpleWaitStrategy for BusySpin {
    #[inline]
    #[allow(deprecated)]
    fn backoff(&self) {
        // Do nothing, true busy spin for lowest latency
    }

    #[inline]
    fn backoff_with_miss(&self, miss: &mut u32) {
        // True busy spin; just account for the miss so generic loops can observe
        // progress if they need to.
        *miss = miss.saturating_add(1);
    }
}

/// Busy spin with spin loop hint - optimized busy spinning
///
/// This strategy uses the spin_loop hint to allow the processor to optimize
/// its behavior (e.g., saving power or switching hyper threads).
/// Slightly higher latency than pure busy spin but more CPU-friendly.
#[derive(Copy, Clone, Debug)]
pub struct BusySpinWithHint;

impl SimpleWaitStrategy for BusySpinWithHint {
    #[inline]
    #[allow(deprecated)]
    fn backoff(&self) {
        hint::spin_loop();
    }

    #[inline]
    fn backoff_with_miss(&self, miss: &mut u32) {
        hint::spin_loop();
        *miss = miss.saturating_add(1);
    }
}

/// Yielding wait strategy - balanced latency and CPU usage
///
/// Yields the CPU after a short busy-spin window. The default window size
/// matches LMAX / [`crate::disruptor::YieldingWaitStrategy::SPIN_TRIES`]
/// (`100`). Prefer the monomorphized LMAX path
/// ([`crate::disruptor::YieldingWaitStrategy`]) on the Builder hot path;
/// this type is the simplified `SimpleWaitStrategy` surface.
#[derive(Copy, Clone, Debug)]
pub struct Yielding {
    spin_tries: u32,
}

impl Yielding {
    /// Create a new yielding wait strategy
    ///
    /// # Arguments
    /// * `spin_tries` - Number of busy spin attempts before yielding
    pub fn new(spin_tries: u32) -> Self {
        Self { spin_tries }
    }
}

impl Default for Yielding {
    fn default() -> Self {
        // Align with LMAX YieldingWaitStrategy.SPIN_TRIES / core YieldingWaitStrategy.
        Self::new(100)
    }
}

impl SimpleWaitStrategy for Yielding {
    #[allow(deprecated)]
    fn backoff(&self) {
        // Try busy spinning first
        for _ in 0..self.spin_tries {
            hint::spin_loop();
        }
        // Then yield the thread
        std::thread::yield_now();
    }

    fn backoff_with_miss(&self, miss: &mut u32) {
        // Align with full YieldingWaitStrategy: the first `spin_tries` misses
        // each perform one spin-loop hint; subsequent misses yield the thread.
        if *miss < self.spin_tries {
            hint::spin_loop();
        } else {
            std::thread::yield_now();
        }
        *miss = miss.saturating_add(1);
    }
}

/// Sleeping wait strategy - lowest CPU usage
///
/// This strategy sleeps for a short duration when waiting.
/// Highest latency but lowest CPU usage.
#[derive(Copy, Clone, Debug)]
pub struct Sleeping {
    sleep_nanos: u64,
}

impl Sleeping {
    /// Create a new sleeping wait strategy
    ///
    /// # Arguments
    /// * `sleep_nanos` - Nanoseconds to sleep when waiting
    pub fn new(sleep_nanos: u64) -> Self {
        Self { sleep_nanos }
    }
}

impl Default for Sleeping {
    fn default() -> Self {
        Self::new(1000) // Default to 1 microsecond
    }
}

impl SimpleWaitStrategy for Sleeping {
    #[allow(deprecated)]
    fn backoff(&self) {
        std::thread::sleep(std::time::Duration::from_nanos(self.sleep_nanos));
    }

    fn backoff_with_miss(&self, miss: &mut u32) {
        std::thread::sleep(std::time::Duration::from_nanos(self.sleep_nanos));
        *miss = miss.saturating_add(1);
    }
}

/// Adapter to use SimpleWaitStrategy with existing WaitStrategy interface
///
/// This allows the simplified wait strategies to work with the existing
/// LMAX Disruptor infrastructure while providing the simpler API.
#[derive(Debug, Clone)]
pub struct SimpleWaitStrategyAdapter<S>
where
    S: SimpleWaitStrategy,
{
    strategy: S,
}

impl<S> SimpleWaitStrategyAdapter<S>
where
    S: SimpleWaitStrategy,
{
    /// Create a new adapter
    pub fn new(strategy: S) -> Self {
        Self { strategy }
    }
}

impl<S> crate::disruptor::WaitStrategy for SimpleWaitStrategyAdapter<S>
where
    S: SimpleWaitStrategy,
{
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        // Alert has terminal priority: it must win even when the sequence is
        // already available, matching the full LMAX wait strategies.
        let mut miss = 0u32;
        loop {
            if alerted.load(Ordering::Acquire) {
                return Err(crate::disruptor::DisruptorError::Alert);
            }
            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }
            self.strategy.backoff_with_miss(&mut miss);
        }
    }

    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: std::time::Duration,
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        let start = std::time::Instant::now();
        let mut miss = 0u32;
        loop {
            if alerted.load(Ordering::Acquire) {
                return Err(crate::disruptor::DisruptorError::Alert);
            }
            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }
            // Re-check alert after observing a miss, before deciding between
            // timeout and backoff. Full strategies sample alert twice per loop;
            // we match that ordering here.
            if alerted.load(Ordering::Acquire) {
                return Err(crate::disruptor::DisruptorError::Alert);
            }
            if start.elapsed() >= timeout {
                return Err(crate::disruptor::DisruptorError::Timeout);
            }
            self.strategy.backoff_with_miss(&mut miss);
        }
    }

    fn wait_for_with_shutdown_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &std::sync::atomic::AtomicBool,
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        let mut miss = 0u32;
        loop {
            if shutdown_flag.load(Ordering::Acquire) {
                return Err(crate::disruptor::DisruptorError::Alert);
            }
            if alerted.load(Ordering::Acquire) {
                return Err(crate::disruptor::DisruptorError::Alert);
            }
            let available = wait_available_sequence(cursor, dependent_sequences);
            if available >= sequence {
                return Ok(available);
            }
            self.strategy.backoff_with_miss(&mut miss);
        }
    }

    fn signal_all_when_blocking(&self) {
        // Simple strategies don't need signaling
    }

    #[inline]
    fn needs_signal(&self) -> bool {
        false
    }
}

// Direct WaitStrategy impls so simplified ZSTs can be passed to `build_*` without adapters.
impl crate::disruptor::WaitStrategy for BusySpin {
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_alert(
            sequence,
            cursor,
            dependent_sequences,
            alerted,
        )
    }

    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: std::time::Duration,
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_timeout_and_alert(
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
        shutdown_flag: &std::sync::atomic::AtomicBool,
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_shutdown_and_alert(
            sequence,
            cursor,
            dependent_sequences,
            shutdown_flag,
            alerted,
        )
    }

    fn signal_all_when_blocking(&self) {}

    #[inline]
    fn needs_signal(&self) -> bool {
        false
    }
}

impl crate::disruptor::WaitStrategy for BusySpinWithHint {
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_alert(
            sequence,
            cursor,
            dependent_sequences,
            alerted,
        )
    }

    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: std::time::Duration,
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_timeout_and_alert(
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
        shutdown_flag: &std::sync::atomic::AtomicBool,
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_shutdown_and_alert(
            sequence,
            cursor,
            dependent_sequences,
            shutdown_flag,
            alerted,
        )
    }

    fn signal_all_when_blocking(&self) {}

    #[inline]
    fn needs_signal(&self) -> bool {
        false
    }
}

impl crate::disruptor::WaitStrategy for Yielding {
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_alert(
            sequence,
            cursor,
            dependent_sequences,
            alerted,
        )
    }

    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: std::time::Duration,
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_timeout_and_alert(
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
        shutdown_flag: &std::sync::atomic::AtomicBool,
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_shutdown_and_alert(
            sequence,
            cursor,
            dependent_sequences,
            shutdown_flag,
            alerted,
        )
    }

    fn signal_all_when_blocking(&self) {}

    #[inline]
    fn needs_signal(&self) -> bool {
        false
    }
}

impl crate::disruptor::WaitStrategy for Sleeping {
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_alert(
            sequence,
            cursor,
            dependent_sequences,
            alerted,
        )
    }

    fn wait_for_with_timeout_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        timeout: std::time::Duration,
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_timeout_and_alert(
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
        shutdown_flag: &std::sync::atomic::AtomicBool,
        alerted: &std::sync::atomic::AtomicBool,
    ) -> crate::disruptor::Result<i64> {
        SimpleWaitStrategyAdapter::new(*self).wait_for_with_shutdown_and_alert(
            sequence,
            cursor,
            dependent_sequences,
            shutdown_flag,
            alerted,
        )
    }

    fn signal_all_when_blocking(&self) {}

    #[inline]
    fn needs_signal(&self) -> bool {
        false
    }
}

/// Convenience function to create a BusySpin adapter
pub fn busy_spin() -> SimpleWaitStrategyAdapter<BusySpin> {
    SimpleWaitStrategyAdapter::new(BusySpin)
}

/// Convenience function to create a BusySpinWithHint adapter
pub fn busy_spin_with_hint() -> SimpleWaitStrategyAdapter<BusySpinWithHint> {
    SimpleWaitStrategyAdapter::new(BusySpinWithHint)
}

/// Convenience function to create a Yielding adapter
pub fn yielding() -> SimpleWaitStrategyAdapter<Yielding> {
    SimpleWaitStrategyAdapter::new(Yielding::default())
}

/// Convenience function to create a custom Yielding adapter
pub fn yielding_with_tries(spin_tries: u32) -> SimpleWaitStrategyAdapter<Yielding> {
    SimpleWaitStrategyAdapter::new(Yielding::new(spin_tries))
}

/// Convenience function to create a Sleeping adapter
pub fn sleeping() -> SimpleWaitStrategyAdapter<Sleeping> {
    SimpleWaitStrategyAdapter::new(Sleeping::default())
}

/// Convenience function to create a custom Sleeping adapter
pub fn sleeping_with_nanos(sleep_nanos: u64) -> SimpleWaitStrategyAdapter<Sleeping> {
    SimpleWaitStrategyAdapter::new(Sleeping::new(sleep_nanos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{DisruptorError, Sequence, WaitStrategy};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    #[derive(Copy, Clone, Debug)]
    struct AdvanceCursorAndDependency<'a> {
        cursor: &'a Sequence,
        dependency: &'a Sequence,
    }

    impl SimpleWaitStrategy for AdvanceCursorAndDependency<'_> {
        fn backoff_with_miss(&self, _miss: &mut u32) {
            self.cursor.set(100);
            self.dependency.set(100);
        }
    }

    #[test]
    fn test_busy_spin_strategy() {
        let strategy = BusySpin;
        // Should not block or panic
        let mut miss = 0u32;
        strategy.backoff_with_miss(&mut miss);
    }

    #[test]
    fn test_busy_spin_with_hint_strategy() {
        let strategy = BusySpinWithHint;
        // Should not block or panic
        let mut miss = 0u32;
        strategy.backoff_with_miss(&mut miss);
    }

    #[test]
    fn test_yielding_strategy() {
        let strategy = Yielding::new(5);
        // Should not block indefinitely
        let mut miss = 0u32;
        strategy.backoff_with_miss(&mut miss);
    }

    #[test]
    fn test_sleeping_strategy() {
        let strategy = Sleeping::new(1000); // 1 microsecond
        let start = std::time::Instant::now();
        let mut miss = 0u32;
        strategy.backoff_with_miss(&mut miss);
        let elapsed = start.elapsed();
        // Should have slept for at least some time
        assert!(elapsed.as_nanos() >= 1000);
    }

    #[test]
    fn test_adapter_creation() {
        let _adapter1 = busy_spin();
        let _adapter2 = busy_spin_with_hint();
        let _adapter3 = yielding();
        let _adapter4 = yielding_with_tries(50);
        let _adapter5 = sleeping();
        let _adapter6 = sleeping_with_nanos(2000);
    }

    #[test]
    fn test_adapter_functionality() {
        let adapter = busy_spin();
        let cursor = Arc::new(Sequence::new(50)); // Set cursor ahead of requested sequence
        let dependent_sequences = vec![Arc::new(Sequence::new(45))]; // Set dependency ahead too

        let result = adapter.wait_for(42, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        // Should return min(cursor=50, dep_min=45) = 45, but since we requested 42, should return 45
        assert_eq!(result.unwrap(), 45);
    }

    #[test]
    fn test_adapter_times_out_when_dependency_does_not_advance() {
        let adapter = busy_spin_with_hint();
        let cursor = Arc::new(Sequence::new(50));
        let dependent_sequences = vec![Arc::new(Sequence::new(40))];

        let result = adapter.wait_for_with_timeout(
            42,
            &cursor,
            &dependent_sequences,
            Duration::from_millis(5),
        );

        assert!(matches!(result, Err(DisruptorError::Timeout)));
    }

    #[test]
    fn test_adapter_alert_interrupts_wait_immediately() {
        let adapter = yielding_with_tries(1);
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = Vec::new();
        let alerted = AtomicBool::new(true);

        let result = adapter.wait_for_with_alert(1, &cursor, &dependent_sequences, &alerted);
        assert!(matches!(result, Err(DisruptorError::Alert)));
    }

    #[test]
    fn test_adapter_shutdown_interrupts_wait_immediately() {
        let adapter = sleeping_with_nanos(1_000);
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = Vec::new();
        let shutdown = AtomicBool::new(true);
        let alerted = AtomicBool::new(false);

        let result = adapter.wait_for_with_shutdown_and_alert(
            1,
            &cursor,
            &dependent_sequences,
            &shutdown,
            &alerted,
        );

        assert!(matches!(result, Err(DisruptorError::Alert)));
    }

    #[test]
    fn test_adapter_returns_minimum_of_cursor_and_dependencies() {
        let adapter = yielding_with_tries(1);
        let cursor = Arc::new(Sequence::new(15));
        let dependent_sequences = vec![Arc::new(Sequence::new(12)), Arc::new(Sequence::new(18))];

        let result = adapter.wait_for(10, &cursor, &dependent_sequences).unwrap();
        assert_eq!(result, 12);
    }

    #[test]
    fn test_adapter_refreshes_cursor_after_dependencies_advance() {
        let cursor = Arc::new(Sequence::new(1));
        let dependency = Arc::new(Sequence::new(0));
        let adapter = SimpleWaitStrategyAdapter::new(AdvanceCursorAndDependency {
            cursor: &cursor,
            dependency: &dependency,
        });

        let result = adapter
            .wait_for(1, &cursor, &[Arc::clone(&dependency)])
            .unwrap();

        assert_eq!(result, 100);
    }

    #[test]
    fn test_yielding_miss_state_machine_boundary() {
        // `Yielding::backoff_with_miss` must match the full
        // `YieldingWaitStrategy` schedule: the first `spin_tries` misses each
        // perform one spin-loop hint, and every subsequent miss yields.
        //
        // We cannot directly observe `hint::spin_loop` vs `yield_now`, so this
        // test pins the observable counter boundary and relies on the semantic
        // equivalence tests in `wait_strategy_semantic_equivalence.rs` to catch
        // any action-selection divergence.
        let spin_tries = 3u32;
        let strategy = Yielding::new(spin_tries);

        let mut miss = 0u32;
        for expected in 1..=spin_tries {
            strategy.backoff_with_miss(&mut miss);
            assert_eq!(
                miss, expected,
                "miss counter should advance by one on each spin-phase call"
            );
        }

        // First call at the boundary (miss == spin_tries) must take the yield
        // branch and still advance the counter.
        strategy.backoff_with_miss(&mut miss);
        assert_eq!(miss, spin_tries + 1);

        // Subsequent calls stay in the yield branch and keep advancing.
        strategy.backoff_with_miss(&mut miss);
        assert_eq!(miss, spin_tries + 2);
    }
}
