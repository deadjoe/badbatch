use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};

#[derive(Default)]
struct MyEvent {
    value: i64,
}

fn main() {
    let handle = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
        .handle_events_with(|_e: &mut MyEvent, _s, _b| {})
        .build();
    let _extra = handle.create_producer();
}
