use super::super::*;
use super::common::{test_event_factory, wait_until, TestEvent};
use crate::disruptor::producer::Producer;
use crate::disruptor::wait_strategy::{BusySpinWaitStrategy, YieldingWaitStrategy};
use crate::disruptor::EventHandler;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

#[test]
fn test_parallel_stage_then_waits_for_all_upstream_handlers() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // LMAX WorkerPool scheme A: parallel same-stage handlers CAS-claim sequences
    // exclusively (work-sharing). Downstream waits on min(worker sequences).
    let upstream_claims = Arc::new(AtomicUsize::new(0));
    let upstream_claims_clone = Arc::clone(&upstream_claims);
    let upstream_claims_clone2 = Arc::clone(&upstream_claims);
    let downstream_processed = Arc::new(AtomicUsize::new(0));
    let downstream_processed_clone = Arc::clone(&downstream_processed);

    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("parallel-slow")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                std::thread::sleep(Duration::from_millis(25));
                event.data.push_str("|worker");
                upstream_claims_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .thread_name("parallel-fast")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                event.data.push_str("|worker");
                upstream_claims_clone2.fetch_add(1, Ordering::SeqCst);
            },
        )
        .and_then()
        .thread_name("downstream")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                assert!(
                    event.data.contains("|worker"),
                    "downstream must observe the exclusive upstream work-claim mutation"
                );
                downstream_processed_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    // Publish several events so both workers can claim some work
    for i in 0..4 {
        let _ = disruptor_handle.publish(|event| {
            event.value = i;
            event.data.clear();
        });
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    while downstream_processed.load(Ordering::SeqCst) < 4 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }

    disruptor_handle.shutdown();
    assert_eq!(upstream_claims.load(Ordering::SeqCst), 4);
    assert_eq!(downstream_processed.load(Ordering::SeqCst), 4);
}

#[test]
fn test_parallel_consumers_in_dependent_stage_share_upstream_barrier() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // WorkerPool scheme A on a dependent stage: workers share the upstream
    // barrier and CAS-claim sequences for exclusive processing.
    let mid_claims = Arc::new(AtomicUsize::new(0));
    let mid_a = Arc::clone(&mid_claims);
    let mid_b = Arc::clone(&mid_claims);
    let final_stage_processed = Arc::new(AtomicUsize::new(0));
    let final_stage_processed_clone = Arc::clone(&final_stage_processed);

    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("root")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                event.data.push_str("|root");
            },
        )
        .and_then()
        .thread_name("left")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                assert!(
                    event.data.contains("|root"),
                    "work-claim handlers must wait for the full upstream stage"
                );
                std::thread::sleep(Duration::from_millis(5));
                event.data.push_str("|mid");
                mid_a.fetch_add(1, Ordering::SeqCst);
            },
        )
        .thread_name("right")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                assert!(
                    event.data.contains("|root"),
                    "parallel workers share the upstream barrier"
                );
                event.data.push_str("|mid");
                mid_b.fetch_add(1, Ordering::SeqCst);
            },
        )
        .and_then()
        .thread_name("final")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                assert!(event.data.contains("|root"));
                assert!(
                    event.data.contains("|mid"),
                    "final stage sees exclusive mid-stage mutation"
                );
                final_stage_processed_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    for i in 0..4 {
        let _ = disruptor_handle.publish(|event| {
            event.value = i;
            event.data.clear();
        });
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    while final_stage_processed.load(Ordering::SeqCst) < 4 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }

    disruptor_handle.shutdown();
    assert_eq!(mid_claims.load(Ordering::SeqCst), 4);
    assert_eq!(final_stage_processed.load(Ordering::SeqCst), 4);
}

#[test]
fn test_multi_producer_and_then_pipeline() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let stage1 = Arc::new(AtomicUsize::new(0));
    let stage2 = Arc::new(AtomicUsize::new(0));
    let s1 = Arc::clone(&stage1);
    let s2 = Arc::clone(&stage2);

    let mut handle = build_multi_producer(32, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(move |_e: &mut TestEvent, _, _| {
            s1.fetch_add(1, Ordering::SeqCst);
        })
        .and_then()
        .handle_events_with(move |_e: &mut TestEvent, _, _| {
            s2.fetch_add(1, Ordering::SeqCst);
        })
        .build();

    assert_eq!(handle.consumer_count(), 2);

    for i in 0..8 {
        let _ = handle.publish(|event| {
            event.value = i;
        });
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    while stage2.load(Ordering::SeqCst) < 8 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    handle.shutdown();

    assert_eq!(stage1.load(Ordering::SeqCst), 8);
    assert_eq!(stage2.load(Ordering::SeqCst), 8);
}
