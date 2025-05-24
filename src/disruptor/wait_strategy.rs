//! Wait strategies for the Disruptor
//!
//! Different wait strategies provide different trade-offs between latency,
//! CPU usage, and throughput. Choose the appropriate strategy based on your
//! deployment environment and performance requirements.

use crate::disruptor::{Result, Sequence};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Trait for different waiting strategies
pub trait WaitStrategy: Send + Sync {
    /// Wait for the given sequence to become available
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    /// * `cursor` - The current cursor position
    /// * `dependent_sequences` - Sequences that this consumer depends on
    ///
    /// # Returns
    /// The sequence that became available, or an error if timeout/shutdown
    fn wait_for(
        &self,
        sequence: i64,
        cursor: &Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64>;

    /// Signal that new data is available
    fn signal_all_when_blocking(&self);
}

/// Blocking wait strategy using condition variables
///
/// This is the most conservative strategy with respect to CPU usage.
/// It uses a lock and condition variable to park the consumer thread
/// when no new events are available.
pub struct BlockingWaitStrategy {
    mutex: std::sync::Mutex<()>,
    condition: std::sync::Condvar,
}

impl BlockingWaitStrategy {
    pub fn new() -> Self {
        Self {
            mutex: std::sync::Mutex::new(()),
            condition: std::sync::Condvar::new(),
        }
    }
}

impl Default for BlockingWaitStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitStrategy for BlockingWaitStrategy {
    fn wait_for(
        &self,
        sequence: i64,
        cursor: &Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64> {
        let mut available_sequence = cursor.get();

        if available_sequence < sequence {
            let mut guard = self.mutex.lock().unwrap();

            while {
                available_sequence = cursor.get();
                available_sequence < sequence
            } {
                guard = self.condition.wait(guard).unwrap();
            }
        }

        // Check dependent sequences
        while !dependent_sequences.is_empty() {
            let min_sequence = dependent_sequences
                .iter()
                .map(|seq| seq.get())
                .min()
                .unwrap_or(i64::MAX);

            if min_sequence >= sequence {
                break;
            }

            // Brief pause before checking again
            thread::yield_now();
        }

        Ok(available_sequence)
    }

    fn signal_all_when_blocking(&self) {
        let _guard = self.mutex.lock().unwrap();
        self.condition.notify_all();
    }
}

/// Yielding wait strategy
///
/// This strategy will busy spin and then yield the thread to other threads.
/// It provides good performance when the number of consumer threads is less
/// than the number of logical cores.
pub struct YieldingWaitStrategy;

impl YieldingWaitStrategy {
    pub fn new() -> Self {
        Self
    }
}

impl Default for YieldingWaitStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitStrategy for YieldingWaitStrategy {
    fn wait_for(
        &self,
        sequence: i64,
        cursor: &Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64> {
        let mut counter = 0;
        let mut available_sequence;

        loop {
            available_sequence = cursor.get();

            if available_sequence >= sequence {
                break;
            }

            counter += 1;
            if counter > 100 {
                thread::yield_now();
                counter = 0;
            }
        }

        // Check dependent sequences
        while !dependent_sequences.is_empty() {
            let min_sequence = dependent_sequences
                .iter()
                .map(|seq| seq.get())
                .min()
                .unwrap_or(i64::MAX);

            if min_sequence >= sequence {
                break;
            }

            thread::yield_now();
        }

        Ok(available_sequence)
    }

    fn signal_all_when_blocking(&self) {
        // No-op for yielding strategy
    }
}

/// Busy spin wait strategy
///
/// This strategy will busy spin without yielding. It provides the lowest
/// latency but uses 100% CPU. Should only be used when the number of
/// consumer threads is less than the number of physical cores.
pub struct BusySpinWaitStrategy;

impl BusySpinWaitStrategy {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BusySpinWaitStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitStrategy for BusySpinWaitStrategy {
    fn wait_for(
        &self,
        sequence: i64,
        cursor: &Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64> {
        let mut available_sequence;

        loop {
            available_sequence = cursor.get();

            if available_sequence >= sequence {
                break;
            }

            // Busy spin - no yielding
            std::hint::spin_loop();
        }

        // Check dependent sequences
        while !dependent_sequences.is_empty() {
            let min_sequence = dependent_sequences
                .iter()
                .map(|seq| seq.get())
                .min()
                .unwrap_or(i64::MAX);

            if min_sequence >= sequence {
                break;
            }

            std::hint::spin_loop();
        }

        Ok(available_sequence)
    }

    fn signal_all_when_blocking(&self) {
        // No-op for busy spin strategy
    }
}

/// Sleeping wait strategy
///
/// This strategy will busy spin briefly, then sleep for a short period.
/// It provides a balance between latency and CPU usage, suitable for
/// scenarios where low latency is not critical.
pub struct SleepingWaitStrategy {
    sleep_duration: Duration,
}

impl SleepingWaitStrategy {
    pub fn new() -> Self {
        Self {
            sleep_duration: Duration::from_nanos(1),
        }
    }

    pub fn with_sleep_duration(sleep_duration: Duration) -> Self {
        Self { sleep_duration }
    }
}

impl Default for SleepingWaitStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitStrategy for SleepingWaitStrategy {
    fn wait_for(
        &self,
        sequence: i64,
        cursor: &Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64> {
        let mut counter = 0;
        let mut available_sequence;

        loop {
            available_sequence = cursor.get();

            if available_sequence >= sequence {
                break;
            }

            counter += 1;
            if counter > 100 {
                thread::sleep(self.sleep_duration);
                counter = 0;
            }
        }

        // Check dependent sequences
        while !dependent_sequences.is_empty() {
            let min_sequence = dependent_sequences
                .iter()
                .map(|seq| seq.get())
                .min()
                .unwrap_or(i64::MAX);

            if min_sequence >= sequence {
                break;
            }

            thread::sleep(self.sleep_duration);
        }

        Ok(available_sequence)
    }

    fn signal_all_when_blocking(&self) {
        // No-op for sleeping strategy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn test_blocking_wait_strategy() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        // Should return immediately since cursor is already >= sequence
        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_yielding_wait_strategy() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_busy_spin_wait_strategy() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_sleeping_wait_strategy() {
        let strategy = SleepingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_wait_strategy_with_dependencies() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(8));
        let dep2 = Arc::new(Sequence::new(9));
        let dependent_sequences = vec![dep1.clone(), dep2.clone()];

        // Should wait for dependencies
        let start = Instant::now();

        // Update dependencies in another thread
        let dep1_clone = dep1.clone();
        let dep2_clone = dep2.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            dep1_clone.set(10);
            dep2_clone.set(10);
        });

        let result = strategy.wait_for(9, &cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert!(start.elapsed() >= Duration::from_millis(5));
    }
}
