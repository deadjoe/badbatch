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
    /// Wait for a sequence to become available
    ///
    /// This is called when a consumer is waiting for new events.
    /// The implementation should decide how to wait (busy spin, yield, sleep, etc.)
    fn wait_for(&self, sequence: i64);
}

/// Busy spin wait strategy - lowest possible latency
///
/// This strategy continuously checks for new events without yielding the CPU.
/// It provides the lowest latency but uses 100% CPU while waiting.
#[derive(Copy, Clone, Debug)]
pub struct BusySpin;

impl SimpleWaitStrategy for BusySpin {
    #[inline]
    fn wait_for(&self, _sequence: i64) {
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
    fn wait_for(&self, _sequence: i64) {
        hint::spin_loop();
    }
}

/// Yielding wait strategy - balanced latency and CPU usage
///
/// This strategy yields the CPU thread after a few busy spin attempts.
/// Good balance between latency and CPU usage.
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
        Self::new(100) // Default to 100 spin tries
    }
}

impl SimpleWaitStrategy for Yielding {
    fn wait_for(&self, _sequence: i64) {
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
    fn wait_for(&self, _sequence: i64) {
        std::thread::sleep(std::time::Duration::from_nanos(self.sleep_nanos));
    }
}

/// Adapter to use SimpleWaitStrategy with existing WaitStrategy interface
///
/// This allows the simplified wait strategies to work with the existing
/// LMAX Disruptor infrastructure while providing the simpler API.
#[derive(Debug)]
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
    fn wait_for(
        &self,
        sequence: i64,
        _cursor: Arc<Sequence>,
        _dependent_sequences: &[Arc<Sequence>],
    ) -> crate::disruptor::Result<i64> {
        self.strategy.wait_for(sequence);
        Ok(sequence)
    }

    fn signal_all_when_blocking(&self) {
        // Simple strategies don't need signaling
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

    #[test]
    fn test_busy_spin_strategy() {
        let strategy = BusySpin;
        // Should not block or panic
        strategy.wait_for(42);
    }

    #[test]
    fn test_busy_spin_with_hint_strategy() {
        let strategy = BusySpinWithHint;
        // Should not block or panic
        strategy.wait_for(42);
    }

    #[test]
    fn test_yielding_strategy() {
        let strategy = Yielding::new(5);
        // Should not block indefinitely
        strategy.wait_for(42);
    }

    #[test]
    fn test_sleeping_strategy() {
        let strategy = Sleeping::new(1000); // 1 microsecond
        let start = std::time::Instant::now();
        strategy.wait_for(42);
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
        use crate::disruptor::{WaitStrategy, Sequence};
        use std::sync::Arc;

        let adapter = busy_spin();
        let cursor = Arc::new(Sequence::new_with_initial_value());
        let dependent_sequences = vec![Arc::new(Sequence::new_with_initial_value())];

        let result = adapter.wait_for(42, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }
}
