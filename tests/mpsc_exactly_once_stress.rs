//! Multi-producer / single-consumer: every sequence is observed exactly once.
//!
//! Exercises multi-producer claim + availability tracking + contiguity barrier under load.

use badbatch::disruptor::{build_multi_producer, BusySpinWaitStrategy, Producer};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Default, Clone, Copy)]
struct Ev {
    producer_id: u32,
    n: u32,
}

#[test]
fn mpsc_sequences_exactly_once_and_contiguous() {
    const PRODUCERS: usize = 4;
    const PER_PRODUCER: u64 = 20_000;
    const TOTAL: u64 = PRODUCERS as u64 * PER_PRODUCER;
    const BUFFER: usize = 1024;

    let seen = Arc::new(Mutex::new(Vec::with_capacity(
        usize::try_from(TOTAL).expect("TOTAL fits usize"),
    )));
    let count = Arc::new(AtomicU64::new(0));
    let seen_h = Arc::clone(&seen);
    let count_h = Arc::clone(&count);

    let mut d = build_multi_producer(BUFFER, Ev::default, BusySpinWaitStrategy)
        .handle_events_with(move |_e: &mut Ev, sequence, _| {
            seen_h.lock().unwrap().push(sequence);
            count_h.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    let mut handles = Vec::new();
    for p in 0..PRODUCERS {
        let mut prod = d.create_producer();
        handles.push(thread::spawn(move || {
            for n in 0..PER_PRODUCER {
                loop {
                    match prod.try_publish(|e| {
                        e.producer_id = u32::try_from(p).unwrap_or(0);
                        e.n = u32::try_from(n).unwrap_or(0);
                    }) {
                        Ok(_) => break,
                        Err(_) => std::hint::spin_loop(),
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    while count.load(Ordering::Acquire) < TOTAL && Instant::now() < deadline {
        std::hint::spin_loop();
    }
    assert_eq!(
        count.load(Ordering::Acquire),
        TOTAL,
        "timeout waiting for consumers"
    );
    d.shutdown();

    let mut vals = seen.lock().unwrap().clone();
    vals.sort_unstable();
    let unique: BTreeSet<_> = vals.iter().copied().collect();
    assert_eq!(unique.len(), vals.len(), "duplicate sequences consumed");
    assert_eq!(u64::try_from(vals.len()).unwrap(), TOTAL);
    assert_eq!(vals[0], 0);
    assert_eq!(*vals.last().unwrap(), i64::try_from(TOTAL).unwrap() - 1);
    for (i, s) in vals.iter().enumerate() {
        assert_eq!(*s, i64::try_from(i).unwrap(), "gap in consumed sequences");
    }
}

#[test]
fn spsc_pipeline_dependency_order_holds() {
    // Single producer pipeline: stage2 never sees a sequence stage1 has not finished.
    use badbatch::disruptor::build_single_producer;
    use std::sync::atomic::AtomicI64;

    const TOTAL: i64 = 80_000;
    const BUFFER: usize = 1024;

    let stage1_seq = Arc::new(AtomicI64::new(-1));
    let stage2_count = Arc::new(AtomicU64::new(0));
    let violations = Arc::new(AtomicU64::new(0));
    let s1_prod = Arc::clone(&stage1_seq);
    let s1_cons = Arc::clone(&stage1_seq);
    let s2 = Arc::clone(&stage2_count);
    let viol = Arc::clone(&violations);

    let mut d = build_single_producer(BUFFER, Ev::default, BusySpinWaitStrategy)
        .handle_events_with(move |_e: &mut Ev, sequence, _| {
            s1_prod.store(sequence, Ordering::Release);
        })
        .and_then()
        .handle_events_with(move |_e: &mut Ev, sequence, _| {
            // Gating already enforces order; check store visibility of stage1 progress.
            let mut seen = s1_cons.load(Ordering::Acquire);
            if seen < sequence {
                for _ in 0..256 {
                    seen = s1_cons.load(Ordering::Acquire);
                    if seen >= sequence {
                        break;
                    }
                    std::hint::spin_loop();
                }
                if seen < sequence {
                    viol.fetch_add(1, Ordering::Relaxed);
                }
            }
            s2.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    for n in 0..TOTAL {
        d.publish(|e| {
            e.n = u32::try_from(n).unwrap_or(0);
        });
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    while stage2_count.load(Ordering::Acquire) < TOTAL as u64 && Instant::now() < deadline {
        std::hint::spin_loop();
    }
    d.shutdown();

    assert_eq!(stage2_count.load(Ordering::Acquire), TOTAL as u64);
    assert_eq!(violations.load(Ordering::Acquire), 0);
}
