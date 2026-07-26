use badbatch::disruptor::{BusySpinWaitStrategy, SimpleProducer};

fn assert_sync<T: Sync>() {}

fn main() {
    assert_sync::<SimpleProducer<i64, BusySpinWaitStrategy>>();
}
