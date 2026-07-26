//! Positive-timeout ordering contract shared by every built-in wait strategy.

use badbatch::disruptor::simple_wait_strategy::{BusySpin, BusySpinWithHint, Sleeping, Yielding};
use badbatch::disruptor::{
    BlockingWaitStrategy, BusySpinWaitStrategy, DisruptorError, Sequence, SleepingWaitStrategy,
    WaitStrategy, YieldingWaitStrategy,
};
use std::time::Duration;

const EXPIRED_BY_FIRST_POLL: Duration = Duration::from_nanos(1);
const UNAVAILABLE_TIMEOUT: Duration = Duration::from_millis(1);

fn assert_positive_timeout_ordering<W: WaitStrategy>(name: &str, strategy: &W) {
    let cursor = Sequence::new(7);

    // This is deliberately positive rather than Duration::ZERO. By the time a
    // timeout-first implementation reaches its first comparison, the one
    // nanosecond budget has expired; an availability-first implementation must
    // still return the already-published sequence.
    for attempt in 0..32 {
        let result = strategy.wait_for_with_timeout(7, &cursor, &[], EXPIRED_BY_FIRST_POLL);
        assert_eq!(
            result.unwrap_or_else(|error| {
                panic!("{name} checked timeout before availability on attempt {attempt}: {error:?}")
            }),
            7,
            "{name} returned the wrong available sequence on attempt {attempt}"
        );
    }

    // Exercise the opposite terminal result through the same real timeout
    // path, so a strategy that simply ignores the deadline cannot pass.
    let result = strategy.wait_for_with_timeout(8, &cursor, &[], UNAVAILABLE_TIMEOUT);
    assert!(
        matches!(result, Err(DisruptorError::Timeout)),
        "{name} did not time out for an unavailable sequence: {result:?}"
    );
}

#[test]
fn positive_timeout_availability_precedes_expiry_for_all_wait_strategies() {
    assert_positive_timeout_ordering("BlockingWaitStrategy", &BlockingWaitStrategy::new());
    assert_positive_timeout_ordering("YieldingWaitStrategy", &YieldingWaitStrategy::new());
    assert_positive_timeout_ordering("BusySpinWaitStrategy", &BusySpinWaitStrategy::new());
    assert_positive_timeout_ordering(
        "SleepingWaitStrategy",
        &SleepingWaitStrategy::new_with_duration(Duration::from_micros(1)),
    );

    assert_positive_timeout_ordering("simple::BusySpin", &BusySpin);
    assert_positive_timeout_ordering("simple::BusySpinWithHint", &BusySpinWithHint);
    assert_positive_timeout_ordering("simple::Yielding", &Yielding::new(1));
    assert_positive_timeout_ordering("simple::Sleeping", &Sleeping::new(1_000));
}
