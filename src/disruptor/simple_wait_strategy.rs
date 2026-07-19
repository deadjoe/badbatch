//! Simplified wait strategies inspired by disruptor-rs
//!
//! This module provides simplified wait strategies that are easier to use
//! and understand compared to the full LMAX Disruptor wait strategies.

use crate::disruptor::Sequence;
use std::hint;
use std::sync::Arc;

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
    fn backoff(&self);
}

/// Busy spin wait strategy - lowest possible latency
///
/// This strategy continuously checks for new events without yielding the CPU.
/// It provides the lowest latency but uses 100% CPU while waiting.
#[derive(Copy, Clone, Debug)]
pub struct BusySpin;

impl SimpleWaitStrategy for BusySpin {
    #[inline]
    fn backoff(&self) {
        // Do nothing, true busy spin for lowest latency
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
    fn backoff(&self) {
        hint::spin_loop();
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
    fn backoff(&self) {
        // Try busy spinning first
        for _ in 0..self.spin_tries {
            hint::spin_loop();
        }
        // Then yield the thread
        std::thread::yield_now();
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
    fn backoff(&self) {
        std::thread::sleep(std::time::Duration::from_nanos(self.sleep_nanos));
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
        // Conservatively wait until the producer cursor reaches the requested sequence
        // and all dependent sequences reach it too, avoiding spurious availability.
        let mut available = cursor.get();
        while available < sequence {
            if alerted.load(std::sync::atomic::Ordering::Acquire) {
                return Err(crate::disruptor::DisruptorError::Alert);
            }
            self.strategy.backoff();
            available = cursor.get();
        }

        if !dependent_sequences.is_empty() {
            loop {
                if alerted.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(crate::disruptor::DisruptorError::Alert);
                }
                let min_dep = Sequence::get_minimum_sequence(dependent_sequences);
                if min_dep >= sequence {
                    break;
                }
                self.strategy.backoff();
            }
        }

        // Return the visible availability upper bound (consistent with Blocking/Yielding).
        let dep_min = if dependent_sequences.is_empty() {
            available
        } else {
            Sequence::get_minimum_sequence(dependent_sequences)
        };
        Ok(std::cmp::min(available, dep_min))
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
        loop {
            if alerted.load(std::sync::atomic::Ordering::Acquire) {
                return Err(crate::disruptor::DisruptorError::Alert);
            }
            let cursor_val = cursor.get();
            let dep_min = if dependent_sequences.is_empty() {
                cursor_val
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };
            let available = std::cmp::min(cursor_val, dep_min);
            if available >= sequence {
                return Ok(available);
            }
            if start.elapsed() >= timeout {
                return Err(crate::disruptor::DisruptorError::Timeout);
            }
            // Backoff according to strategy
            self.strategy.backoff();
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
        loop {
            if shutdown_flag.load(std::sync::atomic::Ordering::Acquire) {
                return Err(crate::disruptor::DisruptorError::Alert);
            }
            if alerted.load(std::sync::atomic::Ordering::Acquire) {
                return Err(crate::disruptor::DisruptorError::Alert);
            }
            let cursor_val = cursor.get();
            let dep_min = if dependent_sequences.is_empty() {
                cursor_val
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };
            let available = std::cmp::min(cursor_val, dep_min);
            if available >= sequence {
                return Ok(available);
            }
            self.strategy.backoff();
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

    #[test]
    fn test_busy_spin_strategy() {
        let strategy = BusySpin;
        // Should not block or panic
        strategy.backoff();
    }

    #[test]
    fn test_busy_spin_with_hint_strategy() {
        let strategy = BusySpinWithHint;
        // Should not block or panic
        strategy.backoff();
    }

    #[test]
    fn test_yielding_strategy() {
        let strategy = Yielding::new(5);
        // Should not block indefinitely
        strategy.backoff();
    }

    #[test]
    fn test_sleeping_strategy() {
        let strategy = Sleeping::new(1000); // 1 microsecond
        let start = std::time::Instant::now();
        strategy.backoff();
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
}
