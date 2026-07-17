//! Loom models for `WorkerPool` scheme A work-sequence CAS claim.
//!
//! Explores interleavings of the **pure claim algorithm** used by
//! `consumer_engine` (CAS on a shared work cursor starting at -1).
//!
//! Keep models tiny: loom state space grows exponentially with steps/threads.
//!
//! ```bash
//! cargo test --test loom_work_claim --release
//! # optional: LOOM_MAX_PREEMPTIONS=3 cargo test --test loom_work_claim --release
//! ```

use loom::sync::atomic::{AtomicI64, Ordering};
use loom::sync::{Arc, Mutex};
use loom::thread;
use std::collections::BTreeSet;

const INITIAL: i64 = -1;

/// Claim next sequence if `next < max_exclusive` (sequences in `0..max_exclusive`).
fn try_claim(work: &AtomicI64, max_exclusive: i64) -> Option<i64> {
    let current = work.load(Ordering::Acquire);
    let next = current + 1;
    if next >= max_exclusive {
        return None;
    }
    work.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| next)
}

fn run_partition(workers: usize, max_exclusive: i64) {
    loom::model(move || {
        let work = Arc::new(AtomicI64::new(INITIAL));
        let log = Arc::new(Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for _ in 0..workers {
            let work = Arc::clone(&work);
            let log = Arc::clone(&log);
            handles.push(thread::spawn(move || {
                while let Some(claimed) = try_claim(&work, max_exclusive) {
                    log.lock().unwrap().push(claimed);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let mut vals = log.lock().unwrap().clone();
        vals.sort_unstable();
        let unique: BTreeSet<_> = vals.iter().copied().collect();
        assert_eq!(
            unique.len(),
            vals.len(),
            "duplicate claim under interleaving"
        );
        let expected: Vec<i64> = (0..max_exclusive).collect();
        assert_eq!(vals, expected);
    });
}

// Debug builds: ignored so `cargo test` stays fast. CI/scripts run --release.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "loom models: cargo test --test loom_work_claim --release"
)]
fn two_workers_two_claims() {
    // Minimal: sequences 0,1 — essential exclusive-claim property.
    run_partition(2, 2);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "loom models: cargo test --test loom_work_claim --release"
)]
fn two_workers_three_claims() {
    run_partition(2, 3);
}

/// Multi-producer next()-style claim is the same CAS pattern on a shared counter.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "loom models: cargo test --test loom_work_claim --release"
)]
fn multi_producer_style_two_claims() {
    run_partition(2, 2);
}
