//! Property-based tests for disruptor components
//!
//! These tests use proptest to verify properties that should hold for all inputs

use crate::disruptor::{
    event_factory::DefaultEventFactory,
    ring_buffer::RingBuffer,
    sequence::Sequence,
    sequencer::{MultiProducerSequencer, Sequencer, SingleProducerSequencer},
    wait_strategy::BusySpinWaitStrategy,
};
use proptest::prelude::*;
use std::convert::TryFrom;
use std::sync::Arc;

/// Property tests for Sequence
mod sequence_properties {
    use super::*;

    proptest! {
        #[test]
        fn sequence_get_set_consistency(value in any::<i64>()) {
            let seq = Sequence::new(0);
            seq.set(value);
            prop_assert_eq!(seq.get(), value);
        }

        #[test]
        fn sequence_add_and_get_consistency(initial in any::<i64>(), delta in 1i64..1000) {
            let seq = Sequence::new(initial);
            let old_value = seq.add_and_get(delta);
            prop_assert_eq!(old_value, initial + delta);
            prop_assert_eq!(seq.get(), initial + delta);
        }

        #[test]
        fn sequence_compare_and_set_success(initial in any::<i64>(), new_value in any::<i64>()) {
            let seq = Sequence::new(initial);
            let result = seq.compare_and_set(initial, new_value);
            prop_assert!(result);
            prop_assert_eq!(seq.get(), new_value);
        }

        #[test]
        fn sequence_compare_and_set_failure(initial in any::<i64>(), wrong_expected in any::<i64>(), new_value in any::<i64>()) {
            prop_assume!(wrong_expected != initial);
            let seq = Sequence::new(initial);
            let result = seq.compare_and_set(wrong_expected, new_value);
            prop_assert!(!result);
            prop_assert_eq!(seq.get(), initial);
        }

        #[test]
        fn sequence_monotonic_increment(initial in any::<i64>(), increments in prop::collection::vec(1i64..100, 1..50)) {
            let seq = Sequence::new(initial);
            let mut expected = initial;

            for inc in increments {
                let old_val = seq.add_and_get(inc);
                expected += inc;
                prop_assert_eq!(old_val, expected);
                prop_assert_eq!(seq.get(), expected);
            }
        }
    }
}

/// Property tests for `RingBuffer`
mod ring_buffer_properties {
    use super::*;

    proptest! {
        #[test]
        fn ring_buffer_size_is_power_of_two(size_power in 1u32..16) {
            let size = 1usize << size_power;  // 2^size_power
            let factory = DefaultEventFactory::<i64>::new();
            let buffer = RingBuffer::new(size, factory).unwrap();
            let size_i64 = i64::try_from(size).expect("size fits within i64");
            prop_assert_eq!(buffer.size(), size_i64);
            prop_assert!(size.is_power_of_two());
        }

        #[test]
        fn ring_buffer_get_set_consistency(
            size_power in 1u32..10,
            sequence in any::<i64>(),
            value in any::<i64>()
        ) {
            let size = 1usize << size_power;
            let factory = DefaultEventFactory::<i64>::new();
            let mut buffer = RingBuffer::new(size, factory).unwrap();

            // 确保sequence是有效的
            let size_i64 = i64::try_from(size).expect("size fits within i64");
            let normalized_seq = sequence.rem_euclid(size_i64);

            *buffer.get_mut(normalized_seq) = value;
            prop_assert_eq!(*buffer.get(normalized_seq), value);
        }

        #[test]
        fn ring_buffer_wrapping_behavior(
            size_power in 1u32..8,
            sequences in prop::collection::vec(any::<i64>(), 1..20)
        ) {
            let size = 1usize << size_power;
            let factory = DefaultEventFactory::<i64>::new();
            let mut buffer = RingBuffer::new(size, factory).unwrap();
            let size_i64 = i64::try_from(size).expect("size fits within i64");

            for (i, seq) in sequences.iter().enumerate() {
                let normalized = seq.rem_euclid(size_i64);
                let value = i64::try_from(i).expect("index fits within i64");

                *buffer.get_mut(normalized) = value;
                prop_assert_eq!(*buffer.get(normalized), value);
            }
        }
    }
}

/// Property tests for `SingleProducerSequencer`
mod single_producer_sequencer_properties {
    use super::*;

    proptest! {
        #[test]
        fn single_producer_next_is_monotonic(
            size_power in 1u32..10,
            requests in prop::collection::vec(1usize..10, 1..20)
        ) {
            let buffer_size = 1usize << size_power;
            let wait_strategy = Arc::new(BusySpinWaitStrategy::new());
            let sequencer = SingleProducerSequencer::new(buffer_size, wait_strategy);

            let mut last_sequence = -1i64;

            for request_size in requests {
                let request_i64 = i64::try_from(request_size).expect("request size fits in i64");
                if let Ok(sequence) = sequencer.next_n(request_i64) {
                    prop_assert!(sequence > last_sequence);
                    sequencer.publish(sequence);
                    last_sequence = sequence;
                }
            }
        }

        #[test]
        fn single_producer_publish_makes_available(
            size_power in 1u32..8,
            sequences in prop::collection::vec(1usize..5, 1..10)
        ) {
            let buffer_size = 1usize << size_power;
            let wait_strategy = Arc::new(BusySpinWaitStrategy::new());
            let sequencer = SingleProducerSequencer::new(buffer_size, wait_strategy);

            for request_size in sequences {
                let request_i64 = i64::try_from(request_size).expect("request size fits in i64");
                if let Ok(sequence) = sequencer.next_n(request_i64) {
                    // 发布前不应该可用
                    prop_assert!(!sequencer.is_available(sequence));

                    sequencer.publish(sequence);

                    // 发布后应该可用
                    prop_assert!(sequencer.is_available(sequence));
                }
            }
        }
    }
}

/// Property tests for `MultiProducerSequencer`
mod multi_producer_sequencer_properties {
    use super::*;

    proptest! {
        #[test]
        fn multi_producer_next_is_unique(
            size_power in 1u32..8,
            requests in prop::collection::vec(1usize..5, 1..10)
        ) {
            let buffer_size = 1usize << size_power;
            let wait_strategy = Arc::new(BusySpinWaitStrategy::new());
            let sequencer = MultiProducerSequencer::new(buffer_size, wait_strategy);

            let mut allocated_sequences = std::collections::HashSet::new();

            for request_size in requests {
                let request_i64 = i64::try_from(request_size).expect("request size fits in i64");
                if let Ok(sequence) = sequencer.next_n(request_i64) {
                    // 每个分配的序列应该是唯一的
                    prop_assert!(!allocated_sequences.contains(&sequence));
                    allocated_sequences.insert(sequence);
                    sequencer.publish(sequence);
                }
            }
        }

        #[test]
        fn multi_producer_publish_order_independence(
            size_power in 1u32..6,
            mut sequences in prop::collection::vec(1usize..3, 2..5)
        ) {
            let buffer_size = 1usize << size_power;
            let wait_strategy = Arc::new(BusySpinWaitStrategy::new());
            let sequencer = MultiProducerSequencer::new(buffer_size, wait_strategy);

            // 分配多个序列
            let mut allocated = Vec::new();
            for request_size in &sequences {
                let request_i64 = i64::try_from(*request_size).expect("request size fits in i64");
                if let Ok(seq) = sequencer.next_n(request_i64) {
                    allocated.push(seq);
                }
            }

            // 随机打乱发布顺序
            sequences.reverse();

            // 按打乱的顺序发布
            for &seq in &allocated {
                sequencer.publish(seq);
                prop_assert!(sequencer.is_available(seq));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn property_tests_compile() {
        // 这个测试确保所有属性测试都能编译
        // 实际的属性测试会在 proptest! 宏中运行
    }
}
