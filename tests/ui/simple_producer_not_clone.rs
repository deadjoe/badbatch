use badbatch::disruptor::{
    open_single_producer_poller, BusySpinWaitStrategy, DefaultEventFactory,
};

fn main() {
    let (producer, _poller, _shutdown) = open_single_producer_poller(
        8,
        DefaultEventFactory::<i64>::new(),
        BusySpinWaitStrategy,
    )
    .unwrap();
    let _second = producer.clone();
}
