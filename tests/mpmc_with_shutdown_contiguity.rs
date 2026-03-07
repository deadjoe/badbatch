#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! End-to-end test for MPMC contiguity with shutdown-aware barrier
//!
//! These tests avoid scheduler-timing assumptions and instead drive the
//! `MultiProducerSequencer + ProcessingSequenceBarrier` path directly. That gives
//! us precise control over out-of-order publication, gap filling, and
//! `wait_for_with_shutdown` interruption semantics.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use badbatch::disruptor::{
    BusySpinWaitStrategy, DisruptorError, MultiProducerSequencer, ProcessingSequenceBarrier,
    SequenceBarrier, Sequencer, SequencerEnum,
};

fn create_barrier(buffer_size: usize) -> (
    Arc<MultiProducerSequencer>,
    Arc<ProcessingSequenceBarrier>,
) {
    let wait_strategy = Arc::new(BusySpinWaitStrategy::new());
    let sequencer = Arc::new(MultiProducerSequencer::new(buffer_size, wait_strategy.clone()));
    let barrier = Arc::new(ProcessingSequenceBarrier::new(
        sequencer.get_cursor(),
        wait_strategy,
        vec![],
        SequencerEnum::Multi(sequencer.clone()),
    ));
    (sequencer, barrier)
}

fn claim_first_n(sequencer: &Arc<MultiProducerSequencer>, count: usize) -> Vec<i64> {
    (0..count).map(|_| sequencer.next().unwrap()).collect()
}

fn assert_gap_blocks_progress_until_fill(buffer_size: usize) {
    let (sequencer, barrier) = create_barrier(buffer_size);
    let sequences = claim_first_n(&sequencer, 4);
    assert_eq!(sequences, vec![0, 1, 2, 3]);

    sequencer.publish(sequences[0]);
    sequencer.publish(sequences[2]);
    sequencer.publish(sequences[3]);

    assert_eq!(
        barrier.wait_for_with_timeout(0, Duration::from_millis(5)).unwrap(),
        0
    );
    assert_eq!(
        barrier.wait_for_with_timeout(1, Duration::from_millis(5)).unwrap(),
        0,
        "the barrier must not advance past the missing sequence 1"
    );

    sequencer.publish(sequences[1]);
    assert_eq!(
        barrier.wait_for_with_timeout(1, Duration::from_millis(5)).unwrap(),
        3,
        "once the gap is filled the full contiguous prefix must become visible"
    );
}

fn assert_shutdown_interrupts_gap_wait(buffer_size: usize) {
    let (sequencer, barrier) = create_barrier(buffer_size);
    let sequences = claim_first_n(&sequencer, 4);
    assert_eq!(sequences, vec![0, 1, 2, 3]);

    sequencer.publish(sequences[0]);
    sequencer.publish(sequences[2]);
    sequencer.publish(sequences[3]);

    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let barrier_clone = barrier.clone();
    let shutdown_clone = shutdown_flag.clone();
    let (ready_tx, ready_rx) = sync_channel(1);
    let (result_tx, result_rx) = sync_channel(1);

    let waiter = thread::spawn(move || {
        ready_tx.send(()).unwrap();

        // Mirror the consumer loop shape: a gap can cause `wait_for_with_shutdown`
        // to yield a sequence lower than requested, so a real consumer retries
        // until the gap is filled or shutdown is observed.
        loop {
            match barrier_clone.wait_for_with_shutdown(1, &shutdown_clone) {
                Ok(available) if available >= 1 => {
                    result_tx.send(Ok(available)).unwrap();
                    break;
                }
                Ok(_) => {
                    thread::yield_now();
                }
                Err(error) => {
                    result_tx.send(Err(error)).unwrap();
                    break;
                }
            }
        }
    });

    ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    shutdown_flag.store(true, Ordering::Release);

    let result = result_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(result, Err(DisruptorError::Alert)));

    waiter.join().unwrap();
}

#[test]
fn mpmc_contiguity_small_buffer_requires_gap_fill() {
    assert_gap_blocks_progress_until_fill(16);
}

#[test]
fn mpmc_contiguity_bitmap_buffer_requires_gap_fill() {
    assert_gap_blocks_progress_until_fill(64);
}

#[test]
fn mpmc_shutdown_small_buffer_interrupts_gap_wait() {
    assert_shutdown_interrupts_gap_wait(16);
}

#[test]
fn mpmc_shutdown_bitmap_buffer_interrupts_gap_wait() {
    assert_shutdown_interrupts_gap_wait(64);
}
