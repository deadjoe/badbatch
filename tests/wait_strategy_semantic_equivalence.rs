//! Semantic equivalence between full LMAX wait strategies and their simple
//! counterparts observed through the [`WaitStrategy`] trait.
//!
//! These tests are intentionally strict: any observable behavioral divergence
//! between a full strategy and the corresponding simple strategy routed through
//! [`SimpleWaitStrategyAdapter`] is a failure. They should all pass once A.6
//! converges the two families.
//!
//! Historical record — the state when this file was introduced, one commit
//! *before* the A.6 body landed. Kept so each test's purpose stays legible.
//! All of them pass now; a failure here is a regression, not an expectation.
//! - Alert-priority tests were RED: the simple adapter returned `Ok` when data
//!   was already available and the barrier was alerted, whereas the full
//!   strategies returned `Err(Alert)`.
//! - Timeout-path alert-priority test was GREEN: the simple adapter's timeout
//!   loop already checked alert before availability.
//! - Trait-default test was GREEN: the default
//!   `WaitStrategy::wait_for_with_timeout_and_alert` was already
//!   availability-first.

use badbatch::disruptor::simple_wait_strategy::{
    busy_spin, busy_spin_with_hint, sleeping, yielding as simple_yielding,
};
use badbatch::disruptor::{
    BusySpinWaitStrategy, DisruptorError, Sequence, SleepingWaitStrategy, WaitStrategy,
    YieldingWaitStrategy,
};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Terminal-flag priority: alert must win over availability on every strategy.
// Was RED before the A.6 body: the adapter did not check alert before its
// first availability observation, so it returned Ok where full returned Alert.
// ---------------------------------------------------------------------------

#[test]
fn alert_wins_over_availability_for_busy_spin() {
    assert_alert_wins_over_availability(
        "full BusySpin",
        &BusySpinWaitStrategy::new(),
        "simple BusySpin",
        &busy_spin(),
    );
}

#[test]
fn alert_wins_over_availability_for_busy_spin_with_hint() {
    // Full LMAX family has no BusySpinWithHint variant; BusySpin is the closest
    // canonical counterpart for this alert-priority comparison.
    assert_alert_wins_over_availability(
        "full BusySpin",
        &BusySpinWaitStrategy::new(),
        "simple BusySpinWithHint",
        &busy_spin_with_hint(),
    );
}

#[test]
fn alert_wins_over_availability_for_yielding() {
    assert_alert_wins_over_availability(
        "full Yielding",
        &YieldingWaitStrategy::new(),
        "simple Yielding",
        &simple_yielding(),
    );
}

#[test]
fn alert_wins_over_availability_for_sleeping() {
    assert_alert_wins_over_availability(
        "full Sleeping",
        &SleepingWaitStrategy::new(),
        "simple Sleeping",
        &sleeping(),
    );
}

fn assert_alert_wins_over_availability(
    full_name: &str,
    full: &dyn WaitStrategy,
    simple_name: &str,
    simple: &dyn WaitStrategy,
) {
    let cursor = Arc::new(Sequence::new(10));
    let alerted = AtomicBool::new(true);

    let full_result = full.wait_for_with_alert(5, &cursor, &[], &alerted);
    let simple_result = simple.wait_for_with_alert(5, &cursor, &[], &alerted);

    assert!(
        matches!(full_result, Err(DisruptorError::Alert)),
        "{full_name} should return Alert when already alerted"
    );
    assert!(
        matches!(simple_result, Err(DisruptorError::Alert)),
        "{simple_name} should return Alert when already alerted (must match {full_name})"
    );
}

// ---------------------------------------------------------------------------
// Control: when the barrier is not alerted and the sequence is already
// available, both families must return Ok. This is the opposite pole of the
// alert-priority tests and keeps the file self-falsifying.
// ---------------------------------------------------------------------------

#[test]
fn unalerted_available_returns_ok_for_busy_spin() {
    assert_unalerted_available_returns_ok(&BusySpinWaitStrategy::new(), &busy_spin());
}

#[test]
fn unalerted_available_returns_ok_for_yielding() {
    assert_unalerted_available_returns_ok(&YieldingWaitStrategy::new(), &simple_yielding());
}

#[test]
fn unalerted_available_returns_ok_for_sleeping() {
    assert_unalerted_available_returns_ok(&SleepingWaitStrategy::new(), &sleeping());
}

fn assert_unalerted_available_returns_ok(full: &dyn WaitStrategy, simple: &dyn WaitStrategy) {
    let cursor = Arc::new(Sequence::new(10));
    let alerted = AtomicBool::new(false);

    let full_result = full.wait_for_with_alert(5, &cursor, &[], &alerted);
    let simple_result = simple.wait_for_with_alert(5, &cursor, &[], &alerted);

    assert_eq!(full_result.unwrap(), 10);
    assert_eq!(simple_result.unwrap(), 10);
}

// ---------------------------------------------------------------------------
// Timeout-path terminal priority: alert must also win on the timeout API.
// Was already GREEN: the adapter's timeout loop checked alert first even
// before A.6, so this pins existing behaviour rather than fixing it.
// ---------------------------------------------------------------------------

#[test]
fn alert_wins_over_availability_and_timeout_for_yielding() {
    let cursor = Arc::new(Sequence::new(10));
    let alerted = AtomicBool::new(true);

    let full_result = YieldingWaitStrategy::new().wait_for_with_timeout_and_alert(
        5,
        &cursor,
        &[],
        Duration::from_millis(10),
        &alerted,
    );
    let simple_result = simple_yielding().wait_for_with_timeout_and_alert(
        5,
        &cursor,
        &[],
        Duration::from_millis(10),
        &alerted,
    );

    assert!(matches!(full_result, Err(DisruptorError::Alert)));
    assert!(
        matches!(simple_result, Err(DisruptorError::Alert)),
        "simple Yielding timeout path must match full Yielding alert priority"
    );
}

// ---------------------------------------------------------------------------
// Positive-timeout ordering sanity check across families.
// Both full and simple strategies must return an already-available sequence
// before a short deadline expires.
// ---------------------------------------------------------------------------

#[test]
fn positive_timeout_returns_available_sequence_for_yielding() {
    let cursor = Arc::new(Sequence::new(10));

    let full_result =
        YieldingWaitStrategy::new().wait_for_with_timeout(5, &cursor, &[], Duration::from_nanos(1));
    let simple_result =
        simple_yielding().wait_for_with_timeout(5, &cursor, &[], Duration::from_nanos(1));

    assert_eq!(full_result.unwrap(), 10);
    assert_eq!(simple_result.unwrap(), 10);
}

// ---------------------------------------------------------------------------
// Trait default implementation coverage.
// No built-in strategy uses the default `wait_for_with_timeout_and_alert`, so
// we provide a minimal implementation that relies on it. Was already GREEN:
// this closes a coverage gap rather than fixing a defect.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct DefaultTimeoutOnlyStrategy;

impl WaitStrategy for DefaultTimeoutOnlyStrategy {
    fn wait_for_with_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        alerted: &AtomicBool,
    ) -> badbatch::disruptor::Result<i64> {
        // Delegate to the default timeout implementation with an effectively
        // infinite timeout so we exercise the trait default path.
        self.wait_for_with_timeout_and_alert(
            sequence,
            cursor,
            dependent_sequences,
            Duration::from_mins(1),
            alerted,
        )
    }

    fn wait_for_with_shutdown_and_alert(
        &self,
        sequence: i64,
        cursor: &Sequence,
        dependent_sequences: &[Arc<Sequence>],
        _shutdown_flag: &AtomicBool,
        alerted: &AtomicBool,
    ) -> badbatch::disruptor::Result<i64> {
        self.wait_for_with_timeout_and_alert(
            sequence,
            cursor,
            dependent_sequences,
            Duration::from_mins(1),
            alerted,
        )
    }

    fn signal_all_when_blocking(&self) {}
}

#[test]
fn trait_default_timeout_implementation_prefers_availability() {
    let cursor = Arc::new(Sequence::new(7));
    let strategy = DefaultTimeoutOnlyStrategy;

    for attempt in 0..32 {
        let result = strategy.wait_for_with_timeout(7, &cursor, &[], Duration::from_nanos(1));
        assert_eq!(
            result.unwrap(),
            7,
            "trait default implementation returned Timeout before availability on attempt {attempt}"
        );
    }
}
