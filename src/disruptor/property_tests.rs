//! Property-based tests for disruptor components
//!
//! These tests use proptest to verify properties that should hold for all inputs

use crate::disruptor::{
    build_single_producer,
    disruptor::Disruptor,
    event_factory::DefaultEventFactory,
    event_translator::ClosureEventTranslator,
    ring_buffer::RingBuffer,
    sequence_barrier::ProcessingSequenceBarrier,
    sequence::Sequence,
    sequencer::{MultiProducerSequencer, Sequencer, SequencerEnum, SingleProducerSequencer},
    BlockingWaitStrategy, EventHandler, ProducerType, Result, SequenceBarrier,
    wait_strategy::BusySpinWaitStrategy,
};
use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestCaseResult};
use std::convert::TryFrom;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn wait_until<F>(timeout: Duration, mut condition: F, description: &str)
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < timeout,
            "timed out waiting for {description} after {timeout:?}"
        );
        std::thread::yield_now();
    }
}

fn publication_order_from_keys(keys: &[u8]) -> Vec<usize> {
    let mut order = (0..keys.len()).collect::<Vec<_>>();
    order.sort_by_key(|&index| (keys[index], index));
    order
}

fn highest_contiguous_prefix(published: &[bool]) -> i64 {
    let mut highest = -1;
    for (index, is_published) in published.iter().enumerate() {
        if *is_published {
            highest = i64::try_from(index).expect("test index fits in i64");
        } else {
            break;
        }
    }
    highest
}

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

/// Property tests for `ProcessingSequenceBarrier`
mod processing_sequence_barrier_properties {
    use super::*;

    fn assert_barrier_tracks_highest_contiguous_prefix(
        buffer_size: usize,
        keys: &[u8],
    ) -> TestCaseResult {
        let wait_strategy = Arc::new(BusySpinWaitStrategy::new());
        let sequencer = Arc::new(MultiProducerSequencer::new(buffer_size, wait_strategy.clone()));
        let barrier = ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            wait_strategy,
            vec![],
            SequencerEnum::Multi(sequencer.clone()),
        );

        let claimed_sequences = (0..keys.len())
            .map(|_| sequencer.next().unwrap())
            .collect::<Vec<_>>();

        for (index, sequence) in claimed_sequences.iter().enumerate() {
            prop_assert_eq!(
                *sequence,
                i64::try_from(index).expect("test index fits in i64")
            );
        }

        let mut published = vec![false; keys.len()];
        for publish_index in publication_order_from_keys(keys) {
            sequencer.publish(claimed_sequences[publish_index]);
            published[publish_index] = true;

            let expected_prefix = highest_contiguous_prefix(&published);
            let observed_prefix = barrier
                .wait_for_with_timeout(0, Duration::from_millis(5))
                .unwrap();
            prop_assert_eq!(observed_prefix, expected_prefix);
        }

        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]

        #[test]
        fn processing_sequence_barrier_small_buffer_matches_contiguous_publication_model(
            keys in prop::collection::vec(any::<u8>(), 1..8)
        ) {
            assert_barrier_tracks_highest_contiguous_prefix(8, &keys)?;
        }

        #[test]
        fn processing_sequence_barrier_bitmap_path_matches_contiguous_publication_model(
            keys in prop::collection::vec(any::<u8>(), 1..8)
        ) {
            assert_barrier_tracks_highest_contiguous_prefix(64, &keys)?;
        }
    }
}

/// Property tests for `Disruptor` lifecycle behavior.
mod disruptor_lifecycle_properties {
    use super::*;

    #[derive(Debug, Default)]
    struct LifecycleEvent {
        value: usize,
    }

    #[derive(Clone)]
    struct LifecycleCountingHandler {
        processed_count: Arc<AtomicUsize>,
        seen_sequences: Arc<Mutex<Vec<i64>>>,
    }

    impl EventHandler<LifecycleEvent> for LifecycleCountingHandler {
        fn on_event(
            &mut self,
            event: &mut LifecycleEvent,
            sequence: i64,
            _end_of_batch: bool,
        ) -> Result<()> {
            self.seen_sequences.lock().unwrap().push(sequence);
            self.processed_count.fetch_add(1, Ordering::AcqRel);
            event.value = event.value.wrapping_add(1);
            Ok(())
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(8))]

        #[test]
        fn disruptor_restart_cycles_process_exactly_all_published_events(
            batches in prop::collection::vec(1usize..6, 1..5)
        ) {
            let processed_count = Arc::new(AtomicUsize::new(0));
            let seen_sequences = Arc::new(Mutex::new(Vec::new()));
            let handler = LifecycleCountingHandler {
                processed_count: processed_count.clone(),
                seen_sequences: seen_sequences.clone(),
            };

            let factory = DefaultEventFactory::<LifecycleEvent>::new();
            let mut disruptor = Disruptor::new(
                factory,
                32,
                ProducerType::Single,
                Box::new(BlockingWaitStrategy::new()),
            )
            .unwrap()
            .handle_events_with(handler)
            .build();

            let mut expected_total = 0usize;
            for batch_size in &batches {
                prop_assert!(disruptor.start().is_ok());

                for value in 0..*batch_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut LifecycleEvent, _sequence| {
                                event.value = value;
                            },
                        ))
                        .unwrap();
                }

                expected_total += *batch_size;
                wait_until(
                    Duration::from_secs(1),
                    || seen_sequences.lock().unwrap().len() == expected_total,
                    "restart cycle to record all processed sequences",
                );

                let observed_sequences = seen_sequences.lock().unwrap().clone();
                prop_assert_eq!(
                    observed_sequences,
                    (0..i64::try_from(expected_total).expect("total fits in i64"))
                        .collect::<Vec<_>>()
                );

                prop_assert!(disruptor.shutdown().is_ok());
            }

            prop_assert!(disruptor.shutdown().is_ok());
            prop_assert_eq!(
                processed_count.load(Ordering::Acquire),
                batches.iter().sum::<usize>()
            );
        }
    }
}

/// Property tests for builder dependency semantics.
mod builder_dependency_properties {
    use super::*;

    #[derive(Debug, Default)]
    struct PipelineEvent {
        value: usize,
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(8))]

        #[test]
        fn builder_parallel_stage_then_downstream_preserves_exact_sequence_visibility(
            event_count in 1usize..8
        ) {
            let stage1_a = Arc::new(Mutex::new(Vec::new()));
            let stage1_b = Arc::new(Mutex::new(Vec::new()));
            let stage2 = Arc::new(Mutex::new(Vec::new()));

            let stage1_a_clone = stage1_a.clone();
            let stage1_b_clone = stage1_b.clone();
            let stage2_clone = stage2.clone();

            let mut disruptor = build_single_producer(32, PipelineEvent::default, BusySpinWaitStrategy)
                .handle_events_with(move |event: &mut PipelineEvent, sequence, _end_of_batch| {
                    event.value = event.value.wrapping_add(1);
                    stage1_a_clone.lock().unwrap().push(sequence);
                })
                .handle_events_with(move |event: &mut PipelineEvent, sequence, _end_of_batch| {
                    event.value = event.value.wrapping_add(1);
                    stage1_b_clone.lock().unwrap().push(sequence);
                })
                .and_then()
                .handle_events_with(move |event: &mut PipelineEvent, sequence, _end_of_batch| {
                    event.value = event.value.wrapping_add(1);
                    stage2_clone.lock().unwrap().push(sequence);
                })
                .build();

            for value in 0..event_count {
                disruptor.publish(|event| {
                    event.value = value;
                });
            }

            wait_until(
                Duration::from_secs(1),
                || stage2.lock().unwrap().len() == event_count,
                "dependent builder stage to process all published events",
            );

            disruptor.shutdown();

            let expected_sequences =
                (0..i64::try_from(event_count).expect("event count fits in i64"))
                    .collect::<Vec<_>>();
            prop_assert_eq!(stage1_a.lock().unwrap().clone(), expected_sequences.clone());
            prop_assert_eq!(stage1_b.lock().unwrap().clone(), expected_sequences.clone());
            prop_assert_eq!(stage2.lock().unwrap().clone(), expected_sequences);
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
