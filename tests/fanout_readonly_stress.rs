//! Read-only fan-out: every same-stage consumer sees every sequence (`&E`).
//! Distinct from `WorkerPool` work-sharing (CAS claim, one consumer per sequence).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::items_after_statements,
    clippy::similar_names
)]

use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default, Clone, Copy)]
struct Ev {
    v: i64,
}

#[test]
fn fan_out_both_consumers_see_all_sequences() {
    const TOTAL: i64 = 10_000;
    const BUFFER: usize = 1024;

    let seqs_a = Arc::new(Mutex::new(Vec::with_capacity(TOTAL as usize)));
    let seqs_b = Arc::new(Mutex::new(Vec::with_capacity(TOTAL as usize)));
    let count_a = Arc::new(AtomicU64::new(0));
    let count_b = Arc::new(AtomicU64::new(0));
    let seqs_a_h = Arc::clone(&seqs_a);
    let seqs_b_h = Arc::clone(&seqs_b);
    let count_a_h = Arc::clone(&count_a);
    let count_b_h = Arc::clone(&count_b);

    let mut d = build_single_producer(BUFFER, Ev::default, BusySpinWaitStrategy)
        .fan_out_events_with(move |e: &Ev, sequence, _| {
            assert_eq!(e.v, sequence);
            seqs_a_h.lock().unwrap().push(sequence);
            count_a_h.fetch_add(1, Ordering::Relaxed);
        })
        .fan_out_events_with(move |e: &Ev, sequence, _| {
            assert_eq!(e.v, sequence);
            seqs_b_h.lock().unwrap().push(sequence);
            count_b_h.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    for i in 0..TOTAL {
        d.publish(|e| e.v = i);
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    while (count_a.load(Ordering::Acquire) < TOTAL as u64
        || count_b.load(Ordering::Acquire) < TOTAL as u64)
        && Instant::now() < deadline
    {
        std::hint::spin_loop();
    }
    d.shutdown();

    assert_eq!(count_a.load(Ordering::Acquire), TOTAL as u64);
    assert_eq!(count_b.load(Ordering::Acquire), TOTAL as u64);

    for list in [&seqs_a, &seqs_b] {
        let mut vals = list.lock().unwrap().clone();
        vals.sort_unstable();
        let unique: BTreeSet<_> = vals.iter().copied().collect();
        assert_eq!(unique.len(), vals.len());
        assert_eq!(vals.len(), TOTAL as usize);
        assert_eq!(vals[0], 0);
        assert_eq!(*vals.last().unwrap(), TOTAL - 1);
    }
}

#[test]
#[should_panic(expected = "cannot mix mutable handlers")]
fn cannot_mix_fanout_and_mutable_on_same_stage() {
    let _ = build_single_producer(8, Ev::default, BusySpinWaitStrategy)
        .fan_out_events_with(|_e: &Ev, _, _| {})
        .handle_events_with(|_e: &mut Ev, _, _| {})
        .build();
}

#[test]
fn worker_pool_still_partitions_not_fanout() {
    // Control: two mutable handlers must NOT both see all events.
    const TOTAL: u64 = 5_000;

    let count_a = Arc::new(AtomicU64::new(0));
    let count_b = Arc::new(AtomicU64::new(0));
    let count_a_h = Arc::clone(&count_a);
    let count_b_h = Arc::clone(&count_b);

    let mut d = build_single_producer(1024, Ev::default, BusySpinWaitStrategy)
        .handle_events_with(move |_e: &mut Ev, _, _| {
            count_a_h.fetch_add(1, Ordering::Relaxed);
        })
        .handle_events_with(move |_e: &mut Ev, _, _| {
            count_b_h.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    for i in 0..TOTAL {
        d.publish(|e| e.v = i.cast_signed());
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    while count_a.load(Ordering::Acquire) + count_b.load(Ordering::Acquire) < TOTAL
        && Instant::now() < deadline
    {
        std::hint::spin_loop();
    }
    d.shutdown();

    let sum = count_a.load(Ordering::Acquire) + count_b.load(Ordering::Acquire);
    assert_eq!(sum, TOTAL);
    // Work-sharing: neither side should have seen *all* events (probabilistically
    // for large N; strict: neither equals TOTAL unless one starves entirely).
    assert!(count_a.load(Ordering::Acquire) > 0);
    assert!(count_b.load(Ordering::Acquire) > 0);
    assert!(count_a.load(Ordering::Acquire) < TOTAL);
    assert!(count_b.load(Ordering::Acquire) < TOTAL);
}
