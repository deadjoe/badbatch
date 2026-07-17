//! Shared fixtures for builder unit tests.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TestEvent {
    pub value: i64,
    pub data: String,
}

impl Default for TestEvent {
    fn default() -> Self {
        Self {
            value: -1,
            data: String::new(),
        }
    }
}

pub(super) fn test_event_factory() -> TestEvent {
    TestEvent::default()
}

pub(super) fn wait_until<F>(timeout: Duration, mut condition: F, description: &str)
where
    F: FnMut() -> bool,
{
    let start = std::time::Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < timeout,
            "timed out waiting for {description} after {timeout:?}"
        );
        std::thread::yield_now();
    }
}
