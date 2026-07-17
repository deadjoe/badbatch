//! Read-only fan-out: every same-stage consumer sees every sequence (`&E`).
//! Distinct from WorkerPool work-sharing (CAS claim, one consumer per sequence).

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

    let a = Arc::new(Mutex::new(Vec::with_capacity(TOTAL as usize)));
    let b = Arc::new(Mutex::new(Vec::with_capacity(TOTAL as usize)));
    let ca = Arc::new(AtomicU64::new(0));
    let cb = Arc::new(AtomicU64::new(0));
    let a_h = Arc::clone(&a);
    let b_h = Arc::clone(&b);
    let ca_h = Arc::clone(&ca);
    let cb_h = Arc::clone(&cb);

    let mut d = build_single_producer(BUFFER, Ev::default, BusySpinWaitStrategy)
        .fan_out_events_with(move |e: &Ev, sequence, _| {
            assert_eq!(e.v, sequence);
            a_h.lock().unwrap().push(sequence);
            ca_h.fetch_add(1, Ordering::Relaxed);
        })
        .fan_out_events_with(move |e: &Ev, sequence, _| {
            assert_eq!(e.v, sequence);
            b_h.lock().unwrap().push(sequence);
            cb_h.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    for i in 0..TOTAL {
        d.publish(|e| e.v = i);
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    while (ca.load(Ordering::Acquire) < TOTAL as u64 || cb.load(Ordering::Acquire) < TOTAL as u64)
        && Instant::now() < deadline
    {
        std::hint::spin_loop();
    }
    d.shutdown();

    assert_eq!(ca.load(Ordering::Acquire), TOTAL as u64);
    assert_eq!(cb.load(Ordering::Acquire), TOTAL as u64);

    for list in [&a, &b] {
        let mut vals = list.lock().unwrap().clone();
        vals.sort_unstable();
        let unique: BTreeSet<_> = vals.iter().copied().collect();
        assert_eq!(unique.len(), vals.len());
        assert_eq!(vals.len() as i64, TOTAL);
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
    let a = Arc::new(AtomicU64::new(0));
    let b = Arc::new(AtomicU64::new(0));
    let a_h = Arc::clone(&a);
    let b_h = Arc::clone(&b);
    const TOTAL: u64 = 5_000;

    let mut d = build_single_producer(1024, Ev::default, BusySpinWaitStrategy)
        .handle_events_with(move |_e: &mut Ev, _, _| {
            a_h.fetch_add(1, Ordering::Relaxed);
        })
        .handle_events_with(move |_e: &mut Ev, _, _| {
            b_h.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    for i in 0..TOTAL {
        d.publish(|e| e.v = i as i64);
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    while a.load(Ordering::Acquire) + b.load(Ordering::Acquire) < TOTAL && Instant::now() < deadline
    {
        std::hint::spin_loop();
    }
    d.shutdown();

    let sum = a.load(Ordering::Acquire) + b.load(Ordering::Acquire);
    assert_eq!(sum, TOTAL);
    // Work-sharing: neither side should have seen *all* events (probabilistically
    // for large N; strict: neither equals TOTAL unless one starves entirely).
    assert!(a.load(Ordering::Acquire) > 0);
    assert!(b.load(Ordering::Acquire) > 0);
    assert!(a.load(Ordering::Acquire) < TOTAL);
    assert!(b.load(Ordering::Acquire) < TOTAL);
}
