//! Stress-test exclusive CAS claims for `WorkerPool` scheme A (no hang loops).
//!
//! Multiple threads race on the shared work cursor; claimed sequences must be a
//! unique permutation of `0..N`.

use badbatch::disruptor::consumer_engine::new_work_sequence;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

#[test]
fn parallel_cas_claims_are_unique_and_complete() {
    const N: usize = 10_000;
    const WORKERS: usize = 4;
    let n_i64 = i64::try_from(N).expect("N fits i64");

    let work = new_work_sequence();
    let done = Arc::new(AtomicBool::new(false));
    let claimed = Arc::new(Mutex::new(Vec::with_capacity(N)));

    let mut handles = Vec::new();
    for _ in 0..WORKERS {
        let work = Arc::clone(&work);
        let claimed = Arc::clone(&claimed);
        let done = Arc::clone(&done);
        handles.push(thread::spawn(move || loop {
            if done.load(Ordering::Acquire) {
                break;
            }
            let current = work.load(Ordering::Acquire);
            if current + 1 >= n_i64 {
                break;
            }
            let next = current + 1;
            if work
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                claimed.lock().unwrap().push(next);
                if next + 1 >= n_i64 {
                    done.store(true, Ordering::Release);
                    break;
                }
            } else {
                std::hint::spin_loop();
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let mut vals = claimed.lock().unwrap().clone();
    vals.sort_unstable();
    let unique: BTreeSet<_> = vals.iter().copied().collect();
    assert_eq!(unique.len(), vals.len(), "duplicate claims detected");
    assert_eq!(vals.len(), N);
    assert_eq!(vals[0], 0);
    assert_eq!(*vals.last().unwrap(), n_i64 - 1);
}
