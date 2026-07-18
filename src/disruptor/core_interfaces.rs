//! Core Interfaces for the Disruptor Pattern
//!
//! This module re-exports the [`DataProvider`] abstraction used by event
//! processors.
//!
//! The `Cursored`, `Sequenced`, and `EventSink` traits that used to live here
//! were removed in the 2026-07 audit cleanup: they had no production
//! implementations or callers (the concrete `Sequencer` trait and `Producer`
//! APIs cover their roles), and dead trait surface only invites divergent
//! implementations.

// Re-export DataProvider from event_processor to avoid duplication
// This provides a unified DataProvider interface across the codebase
pub use crate::disruptor::event_processor::DataProvider;

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::TryFrom;

    struct TestDataProvider {
        data: Vec<i32>,
    }

    impl DataProvider<i32> for TestDataProvider {
        unsafe fn get(&self, sequence: i64) -> &i32 {
            let len = self.data.len();
            let len_i64 = i64::try_from(len).expect("data length fits in i64");
            let normalized = sequence.rem_euclid(len_i64);
            let index = usize::try_from(normalized).expect("normalized index must fit");
            &self.data[index]
        }

        #[allow(clippy::mut_from_ref)]
        unsafe fn get_mut(&self, sequence: i64) -> &mut i32 {
            // SAFETY: This is only used in tests where we control access
            let len = self.data.len();
            let len_i64 = i64::try_from(len).expect("data length fits in i64");
            let normalized = sequence.rem_euclid(len_i64);
            let index = usize::try_from(normalized).expect("normalized index must fit");
            let ptr = self.data.as_ptr().cast_mut();
            &mut *ptr.add(index)
        }
    }

    #[test]
    fn test_data_provider_trait() {
        let provider = TestDataProvider {
            data: vec![1, 2, 3, 4, 5],
        };

        // SAFETY: single-threaded test, no concurrent writers.
        unsafe {
            assert_eq!(*provider.get(0), 1);
            assert_eq!(*provider.get(2), 3);
            assert_eq!(*provider.get(7), 3); // Wraps around

            *provider.get_mut(7) = 99;

            assert_eq!(*provider.get(2), 99);
        }
    }
}
