#![allow(
    missing_docs,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use badbatch::disruptor::{
    build_multi_producer, build_single_producer, BusySpinWaitStrategy, DefaultEventFactory,
    DisruptorError, EventHandler, ProcessingSequenceBarrier, Producer, Result as DisruptorResult,
    RingBuffer, Sequence, SequenceBarrier, Sequencer, SequencerEnum, SimpleProducer,
    SingleProducerSequencer, WaitStrategy, YieldingWaitStrategy,
};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT_SECS: u64 = 300;
const MPSC_PRODUCER_COUNT: usize = 3;

#[derive(Debug, Default, Clone, Copy)]
struct ComparisonEvent {
    value: i64,
    stage1_value: i64,
    stage2_value: i64,
    stage3_value: i64,
}

#[derive(Debug, Default, Clone, Copy)]
#[repr(C, align(64))]
struct ComparisonPadded64Event {
    value: i64,
    stage1_value: i64,
    stage2_value: i64,
    stage3_value: i64,
}

trait UnicastEvent: Default + Send + Sync + 'static {
    fn populate(&mut self, value: i64);
    fn value(&self) -> i64;
    fn apply_stage1(&mut self) -> i64;
    fn apply_stage2(&mut self) -> i64;
    fn apply_stage3(&mut self) -> i64;
}

impl UnicastEvent for ComparisonEvent {
    fn populate(&mut self, value: i64) {
        self.value = value;
        self.stage1_value = 0;
        self.stage2_value = 0;
        self.stage3_value = 0;
    }

    fn value(&self) -> i64 {
        self.value
    }

    fn apply_stage1(&mut self) -> i64 {
        self.stage1_value = self.value.wrapping_add(1);
        self.stage1_value
    }

    fn apply_stage2(&mut self) -> i64 {
        self.stage2_value = self.stage1_value.wrapping_add(3);
        self.stage2_value
    }

    fn apply_stage3(&mut self) -> i64 {
        self.stage3_value = self.stage2_value.wrapping_add(7);
        self.stage3_value
    }
}

impl UnicastEvent for ComparisonPadded64Event {
    fn populate(&mut self, value: i64) {
        self.value = value;
        self.stage1_value = 0;
        self.stage2_value = 0;
        self.stage3_value = 0;
    }

    fn value(&self) -> i64 {
        self.value
    }

    fn apply_stage1(&mut self) -> i64 {
        self.stage1_value = self.value.wrapping_add(1);
        self.stage1_value
    }

    fn apply_stage2(&mut self) -> i64 {
        self.stage2_value = self.stage1_value.wrapping_add(3);
        self.stage2_value
    }

    fn apply_stage3(&mut self) -> i64 {
        self.stage3_value = self.stage2_value.wrapping_add(7);
        self.stage3_value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventPadding {
    None,
    Pad64,
}

impl EventPadding {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "64" => Ok(Self::Pad64),
            _ => Err(format!("unsupported event padding: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pad64 => "64",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Unicast,
    UnicastBatch,
    MpscBatch,
    Pipeline,
}

impl Scenario {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "unicast" => Ok(Self::Unicast),
            "unicast_batch" => Ok(Self::UnicastBatch),
            "mpsc_batch" => Ok(Self::MpscBatch),
            "pipeline" => Ok(Self::Pipeline),
            _ => Err(format!("unsupported scenario: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Unicast => "unicast",
            Self::UnicastBatch => "unicast_batch",
            Self::MpscBatch => "mpsc_batch",
            Self::Pipeline => "pipeline",
        }
    }

    fn default_wait_strategy(self) -> WaitStrategyKind {
        match self {
            Self::MpscBatch => WaitStrategyKind::BusySpin,
            Self::Unicast | Self::UnicastBatch | Self::Pipeline => WaitStrategyKind::Yielding,
        }
    }

    fn default_buffer_size(self) -> usize {
        match self {
            Self::Pipeline => 8_192,
            Self::Unicast | Self::UnicastBatch | Self::MpscBatch => 65_536,
        }
    }

    fn default_events_total(self) -> u64 {
        match self {
            Self::MpscBatch => 60_000_000,
            Self::Unicast | Self::UnicastBatch | Self::Pipeline => 100_000_000,
        }
    }

    fn default_batch_size(self) -> usize {
        match self {
            Self::Unicast | Self::Pipeline => 1,
            Self::UnicastBatch | Self::MpscBatch => 10,
        }
    }

    fn producer_count(self) -> usize {
        match self {
            Self::MpscBatch => MPSC_PRODUCER_COUNT,
            Self::Unicast | Self::UnicastBatch | Self::Pipeline => 1,
        }
    }

    fn consumer_count(self) -> usize {
        match self {
            Self::Pipeline => 3,
            Self::Unicast | Self::UnicastBatch | Self::MpscBatch => 1,
        }
    }

    fn pipeline_stages(self) -> usize {
        match self {
            Self::Pipeline => 3,
            Self::Unicast | Self::UnicastBatch | Self::MpscBatch => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitStrategyKind {
    BusySpin,
    Yielding,
}

impl WaitStrategyKind {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "busy-spin" => Ok(Self::BusySpin),
            "yielding" => Ok(Self::Yielding),
            _ => Err(format!("unsupported wait strategy: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::BusySpin => "busy-spin",
            Self::Yielding => "yielding",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProducerPath {
    Builder,
    Direct,
}

impl ProducerPath {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "builder" => Ok(Self::Builder),
            "direct" => Ok(Self::Direct),
            _ => Err(format!("unsupported producer path: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Builder => "builder",
            Self::Direct => "direct",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsumerPath {
    Builder,
    Direct,
}

impl ConsumerPath {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "builder" => Ok(Self::Builder),
            "direct" => Ok(Self::Direct),
            _ => Err(format!("unsupported consumer path: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Builder => "builder",
            Self::Direct => "direct",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsumerMode {
    LibraryBuilder,
    BuilderDyn,
    BuilderStatic,
    BuilderStaticPerBatch,
    DirectPerEvent,
    DirectPerBatch,
}

impl ConsumerMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "library-builder" => Ok(Self::LibraryBuilder),
            "builder-dyn" => Ok(Self::BuilderDyn),
            "builder-static" => Ok(Self::BuilderStatic),
            "builder-static-per-batch" => Ok(Self::BuilderStaticPerBatch),
            "direct-per-event" => Ok(Self::DirectPerEvent),
            "direct-per-batch" => Ok(Self::DirectPerBatch),
            _ => Err(format!("unsupported consumer mode: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::LibraryBuilder => "library-builder",
            Self::BuilderDyn => "builder-dyn",
            Self::BuilderStatic => "builder-static",
            Self::BuilderStaticPerBatch => "builder-static-per-batch",
            Self::DirectPerEvent => "direct-per-event",
            Self::DirectPerBatch => "direct-per-batch",
        }
    }

    fn default_for(path: ConsumerPath) -> Self {
        match path {
            ConsumerPath::Builder => Self::LibraryBuilder,
            ConsumerPath::Direct => Self::DirectPerBatch,
        }
    }

    fn path(self) -> ConsumerPath {
        match self {
            Self::LibraryBuilder
            | Self::BuilderDyn
            | Self::BuilderStatic
            | Self::BuilderStaticPerBatch => ConsumerPath::Builder,
            Self::DirectPerEvent | Self::DirectPerBatch => ConsumerPath::Direct,
        }
    }
}

#[derive(Debug, Clone)]
struct CliConfig {
    scenario: Scenario,
    wait_strategy: Option<WaitStrategyKind>,
    event_padding: EventPadding,
    producer_path: ProducerPath,
    consumer_path: Option<ConsumerPath>,
    consumer_mode: Option<ConsumerMode>,
    buffer_size: Option<usize>,
    events_total: Option<u64>,
    batch_size: Option<usize>,
    warmup_rounds: usize,
    measured_rounds: usize,
    run_order: String,
    output: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct ResolvedConfig {
    scenario: Scenario,
    wait_strategy: WaitStrategyKind,
    event_padding: EventPadding,
    producer_path: ProducerPath,
    consumer_path: ConsumerPath,
    consumer_mode: ConsumerMode,
    buffer_size: usize,
    events_total: u64,
    batch_size: usize,
    warmup_rounds: usize,
    measured_rounds: usize,
    run_order: String,
    output: Option<PathBuf>,
}

impl CliConfig {
    fn from_args() -> Result<Self, String> {
        let mut args = env::args().skip(1);

        let mut scenario = None;
        let mut wait_strategy = None;
        let mut event_padding = EventPadding::None;
        let mut producer_path = ProducerPath::Builder;
        let mut consumer_path = None;
        let mut consumer_mode = None;
        let mut buffer_size = None;
        let mut events_total = None;
        let mut batch_size = None;
        let mut warmup_rounds = 3usize;
        let mut measured_rounds = 7usize;
        let mut run_order = String::from("standalone");
        let mut output = None;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--scenario" => {
                    let value = args.next().ok_or("missing value for --scenario")?;
                    scenario = Some(Scenario::parse(&value)?);
                }
                "--wait-strategy" => {
                    let value = args.next().ok_or("missing value for --wait-strategy")?;
                    wait_strategy = Some(WaitStrategyKind::parse(&value)?);
                }
                "--event-padding" => {
                    let value = args.next().ok_or("missing value for --event-padding")?;
                    event_padding = EventPadding::parse(&value)?;
                }
                "--producer-path" => {
                    let value = args.next().ok_or("missing value for --producer-path")?;
                    producer_path = ProducerPath::parse(&value)?;
                }
                "--consumer-path" => {
                    let value = args.next().ok_or("missing value for --consumer-path")?;
                    consumer_path = Some(ConsumerPath::parse(&value)?);
                }
                "--consumer-mode" => {
                    let value = args.next().ok_or("missing value for --consumer-mode")?;
                    consumer_mode = Some(ConsumerMode::parse(&value)?);
                }
                "--buffer-size" => {
                    let value = args.next().ok_or("missing value for --buffer-size")?;
                    buffer_size = Some(parse_usize(&value, "--buffer-size")?);
                }
                "--events-total" => {
                    let value = args.next().ok_or("missing value for --events-total")?;
                    events_total = Some(parse_u64(&value, "--events-total")?);
                }
                "--batch-size" => {
                    let value = args.next().ok_or("missing value for --batch-size")?;
                    batch_size = Some(parse_usize(&value, "--batch-size")?);
                }
                "--warmup-rounds" => {
                    let value = args.next().ok_or("missing value for --warmup-rounds")?;
                    warmup_rounds = parse_usize(&value, "--warmup-rounds")?;
                }
                "--measured-rounds" => {
                    let value = args.next().ok_or("missing value for --measured-rounds")?;
                    measured_rounds = parse_usize(&value, "--measured-rounds")?;
                }
                "--run-order" => {
                    run_order = args.next().ok_or("missing value for --run-order")?;
                }
                "--output" => {
                    let value = args.next().ok_or("missing value for --output")?;
                    output = Some(PathBuf::from(value));
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                _ => return Err(format!("unsupported argument: {arg}")),
            }
        }

        Ok(Self {
            scenario: scenario.ok_or("missing required argument --scenario")?,
            wait_strategy,
            event_padding,
            producer_path,
            consumer_path,
            consumer_mode,
            buffer_size,
            events_total,
            batch_size,
            warmup_rounds,
            measured_rounds,
            run_order,
            output,
        })
    }
}

impl TryFrom<CliConfig> for ResolvedConfig {
    type Error = String;

    fn try_from(value: CliConfig) -> Result<Self, Self::Error> {
        let scenario = value.scenario;
        let wait_strategy = value
            .wait_strategy
            .unwrap_or_else(|| scenario.default_wait_strategy());
        let mut consumer_path = value.consumer_path.unwrap_or(match value.producer_path {
            ProducerPath::Builder => ConsumerPath::Builder,
            ProducerPath::Direct => ConsumerPath::Direct,
        });
        let consumer_mode = value
            .consumer_mode
            .unwrap_or_else(|| ConsumerMode::default_for(consumer_path));
        consumer_path = consumer_mode.path();
        let buffer_size = value
            .buffer_size
            .unwrap_or_else(|| scenario.default_buffer_size());
        let events_total = value
            .events_total
            .unwrap_or_else(|| scenario.default_events_total());
        let batch_size = value
            .batch_size
            .unwrap_or_else(|| scenario.default_batch_size());

        if buffer_size == 0 || !buffer_size.is_power_of_two() {
            return Err(format!(
                "buffer size must be a non-zero power of two, got {buffer_size}"
            ));
        }
        if value.measured_rounds == 0 {
            return Err(String::from("measured_rounds must be > 0"));
        }
        if batch_size == 0 {
            return Err(String::from("batch_size must be > 0"));
        }
        if events_total == 0 {
            return Err(String::from("events_total must be > 0"));
        }
        if scenario == Scenario::MpscBatch && events_total % MPSC_PRODUCER_COUNT as u64 != 0 {
            return Err(format!(
                "mpsc_batch requires events_total divisible by {MPSC_PRODUCER_COUNT}, got {events_total}"
            ));
        }
        if value.event_padding != EventPadding::None
            && !matches!(
                scenario,
                Scenario::Unicast | Scenario::UnicastBatch | Scenario::Pipeline
            )
        {
            return Err(format!(
                "event-padding is only supported for unicast, unicast_batch, and pipeline, got {}",
                scenario.as_str()
            ));
        }
        if value.producer_path == ProducerPath::Direct
            && !matches!(scenario, Scenario::Unicast | Scenario::UnicastBatch)
        {
            return Err(format!(
                "producer_path=direct is only supported for unicast and unicast_batch, got {}",
                scenario.as_str()
            ));
        }
        if consumer_path == ConsumerPath::Direct
            && !matches!(scenario, Scenario::Unicast | Scenario::UnicastBatch)
        {
            return Err(format!(
                "consumer_path=direct is only supported for unicast and unicast_batch, got {}",
                scenario.as_str()
            ));
        }

        Ok(Self {
            scenario,
            wait_strategy,
            event_padding: value.event_padding,
            producer_path: value.producer_path,
            consumer_path,
            consumer_mode,
            buffer_size,
            events_total,
            batch_size,
            warmup_rounds: value.warmup_rounds,
            measured_rounds: value.measured_rounds,
            run_order: value.run_order,
            output: value.output,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoundPhase {
    Warmup,
    Measured,
}

impl RoundPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Warmup => "warmup",
            Self::Measured => "measured",
        }
    }
}

#[derive(Debug, Clone)]
struct RoundRecord {
    round_index: usize,
    phase: RoundPhase,
    elapsed_ns: u128,
    ops_per_sec: f64,
    events_expected: u64,
    events_processed: u64,
    checksum_expected: i64,
    checksum_observed: i64,
    checksum_valid: bool,
}

#[derive(Debug, Clone)]
struct SummaryStats {
    checksum_valid_all: bool,
    median_ops_per_sec: f64,
    mean_ops_per_sec: f64,
    min_ops_per_sec: f64,
    max_ops_per_sec: f64,
    stddev_ops_per_sec: f64,
    cv: f64,
}

#[derive(Debug, Clone)]
struct EnvInfo {
    os: String,
    arch: String,
    runtime: String,
    runtime_version: String,
}

#[derive(Debug, Clone)]
struct HarnessResult {
    impl_name: String,
    scenario: Scenario,
    event_padding: EventPadding,
    producer_path: ProducerPath,
    consumer_path: ConsumerPath,
    consumer_mode: ConsumerMode,
    measurement_kind: String,
    buffer_size: usize,
    event_size_bytes: usize,
    wait_strategy: WaitStrategyKind,
    producer_count: usize,
    consumer_count: usize,
    pipeline_stages: usize,
    batch_size: usize,
    events_total: u64,
    warmup_rounds: usize,
    measured_rounds: usize,
    run_order: String,
    env: EnvInfo,
    runs: Vec<RoundRecord>,
    summary: SummaryStats,
}

struct SummingHandler<E> {
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready_count: Arc<AtomicU64>,
    marker: PhantomData<fn() -> E>,
}

impl<E> EventHandler<E> for SummingHandler<E>
where
    E: UnicastEvent,
{
    fn on_event(
        &mut self,
        event: &mut E,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let value = event.value();
        add_wrapping_i64(&self.checksum, value);
        self.processed.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

struct PipelineStage1Handler<E> {
    ready_count: Arc<AtomicU64>,
    marker: PhantomData<fn() -> E>,
}

impl<E> EventHandler<E> for PipelineStage1Handler<E>
where
    E: UnicastEvent,
{
    fn on_event(
        &mut self,
        event: &mut E,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let _ = event.apply_stage1();
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

struct PipelineStage2Handler<E> {
    ready_count: Arc<AtomicU64>,
    marker: PhantomData<fn() -> E>,
}

impl<E> EventHandler<E> for PipelineStage2Handler<E>
where
    E: UnicastEvent,
{
    fn on_event(
        &mut self,
        event: &mut E,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let _ = event.apply_stage2();
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

struct PipelineStage3Handler<E> {
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready_count: Arc<AtomicU64>,
    marker: PhantomData<fn() -> E>,
}

impl<E> EventHandler<E> for PipelineStage3Handler<E>
where
    E: UnicastEvent,
{
    fn on_event(
        &mut self,
        event: &mut E,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let value = event.apply_stage3();
        add_wrapping_i64(&self.checksum, value);
        self.processed.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

struct LowLevelSingleProducerRuntime<E> {
    ring_buffer: Arc<RingBuffer<E>>,
    sequencer: Arc<SingleProducerSequencer>,
    sequence_barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    consumer_handle: thread::JoinHandle<Result<(), String>>,
}

impl<E> LowLevelSingleProducerRuntime<E>
where
    E: UnicastEvent,
{
    fn new<W>(
        buffer_size: usize,
        wait_strategy: W,
        consumer_mode: ConsumerMode,
        ready_count: Arc<AtomicU64>,
        processed: Arc<AtomicU64>,
        checksum: Arc<AtomicI64>,
    ) -> Result<Self, String>
    where
        W: WaitStrategy + Send + Sync + Clone + 'static,
    {
        let ring_buffer = Arc::new(
            RingBuffer::new(buffer_size, DefaultEventFactory::<E>::new())
                .map_err(|error| format!("failed to create direct ring buffer: {error:?}"))?,
        );
        let wait_strategy_arc: Arc<dyn WaitStrategy> = Arc::new(wait_strategy);
        let sequencer = Arc::new(SingleProducerSequencer::new(
            buffer_size,
            Arc::clone(&wait_strategy_arc),
        ));
        let consumer_sequence = Arc::new(Sequence::new_with_initial_value());
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_sequence));

        let sequence_barrier: Arc<dyn SequenceBarrier> = Arc::new(ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            wait_strategy_arc,
            Vec::new(),
            SequencerEnum::Single(Arc::clone(&sequencer)),
        ));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let consumer_handle = match consumer_mode {
            ConsumerMode::DirectPerBatch => spawn_direct_unicast_consumer(
                Arc::clone(&ring_buffer),
                Arc::clone(&sequence_barrier),
                Arc::clone(&consumer_sequence),
                Arc::clone(&shutdown_flag),
                ready_count,
                processed,
                checksum,
            ),
            ConsumerMode::DirectPerEvent => spawn_direct_per_event_unicast_consumer(
                Arc::clone(&ring_buffer),
                Arc::clone(&sequence_barrier),
                Arc::clone(&consumer_sequence),
                Arc::clone(&shutdown_flag),
                ready_count,
                processed,
                checksum,
            ),
            ConsumerMode::LibraryBuilder | ConsumerMode::BuilderDyn => {
                let handler: Box<dyn EventHandler<E> + Send + Sync> = Box::new(SummingHandler {
                    processed,
                    checksum,
                    ready_count,
                    marker: PhantomData,
                });
                spawn_builder_style_unicast_consumer(
                    Arc::clone(&ring_buffer),
                    Arc::clone(&sequence_barrier),
                    Arc::clone(&consumer_sequence),
                    Arc::clone(&shutdown_flag),
                    handler,
                )
            }
            ConsumerMode::BuilderStatic => spawn_static_handler_unicast_consumer(
                Arc::clone(&ring_buffer),
                Arc::clone(&sequence_barrier),
                Arc::clone(&consumer_sequence),
                Arc::clone(&shutdown_flag),
                SummingHandler {
                    processed,
                    checksum,
                    ready_count,
                    marker: PhantomData,
                },
            ),
            ConsumerMode::BuilderStaticPerBatch => spawn_static_handler_unicast_consumer_per_batch(
                Arc::clone(&ring_buffer),
                Arc::clone(&sequence_barrier),
                Arc::clone(&consumer_sequence),
                Arc::clone(&shutdown_flag),
                SummingHandler {
                    processed,
                    checksum,
                    ready_count,
                    marker: PhantomData,
                },
            ),
        };

        Ok(Self {
            ring_buffer,
            sequencer,
            sequence_barrier,
            consumer_sequence,
            shutdown_flag,
            consumer_handle,
        })
    }

    fn shutdown(self) -> Result<(), String> {
        self.shutdown_flag.store(true, Ordering::Release);
        self.sequence_barrier.alert();
        self.consumer_handle
            .join()
            .map_err(|_| String::from("direct unicast consumer thread panicked"))??;
        self.sequencer
            .remove_gating_sequence(Arc::clone(&self.consumer_sequence));
        Ok(())
    }
}

fn spawn_builder_style_unicast_consumer<E>(
    ring_buffer: Arc<RingBuffer<E>>,
    sequence_barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    mut event_handler: Box<dyn EventHandler<E> + Send + Sync>,
) -> thread::JoinHandle<Result<(), String>>
where
    E: UnicastEvent,
{
    thread::Builder::new()
        .name(String::from("h2h-builder-consumer"))
        .spawn(move || {
            let mut next_sequence = 0_i64;

            event_handler
                .on_start()
                .map_err(|error| format!("builder-style consumer on_start failed: {error:?}"))?;

            loop {
                if shutdown_flag.load(Ordering::Acquire) {
                    break;
                }

                match sequence_barrier.wait_for_with_shutdown(next_sequence, &shutdown_flag) {
                    Ok(available_sequence) => {
                        let batch_size = available_sequence - next_sequence + 1;
                        let queue_depth = available_sequence - consumer_sequence.get();
                        let _ = event_handler.on_batch_start(batch_size, queue_depth);

                        while next_sequence <= available_sequence {
                            let end_of_batch = next_sequence == available_sequence;
                            let event =
                                unsafe { &mut *ring_buffer.get_mut_unchecked(next_sequence) };
                            event_handler.on_event(event, next_sequence, end_of_batch).map_err(
                                |error| {
                                    format!(
                                        "builder-style consumer on_event failed at {next_sequence}: {error:?}"
                                    )
                                },
                            )?;
                            consumer_sequence.set(next_sequence);
                            next_sequence += 1;
                        }
                    }
                    Err(DisruptorError::Alert) => {
                        if shutdown_flag.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    Err(error) => {
                        if shutdown_flag.load(Ordering::Acquire) {
                            break;
                        }
                        return Err(format!(
                            "builder-style consumer failed at sequence {next_sequence}: {error:?}"
                        ));
                    }
                }
            }

            event_handler.on_shutdown().map_err(|error| {
                format!("builder-style consumer on_shutdown failed: {error:?}")
            })?;

            Ok(())
        })
        .map_err(|error| format!("failed to spawn builder-style consumer thread: {error}"))
        .expect("builder-style consumer thread spawn must succeed")
}

fn spawn_static_handler_unicast_consumer<E, H>(
    ring_buffer: Arc<RingBuffer<E>>,
    sequence_barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    mut event_handler: H,
) -> thread::JoinHandle<Result<(), String>>
where
    E: UnicastEvent,
    H: EventHandler<E> + Send + 'static,
{
    thread::Builder::new()
        .name(String::from("h2h-static-consumer"))
        .spawn(move || {
            let mut next_sequence = 0_i64;

            event_handler
                .on_start()
                .map_err(|error| format!("static consumer on_start failed: {error:?}"))?;

            loop {
                if shutdown_flag.load(Ordering::Acquire) {
                    break;
                }

                match sequence_barrier.wait_for_with_shutdown(next_sequence, &shutdown_flag) {
                    Ok(available_sequence) => {
                        let batch_size = available_sequence - next_sequence + 1;
                        let queue_depth = available_sequence - consumer_sequence.get();
                        let _ = event_handler.on_batch_start(batch_size, queue_depth);

                        while next_sequence <= available_sequence {
                            let end_of_batch = next_sequence == available_sequence;
                            let event =
                                unsafe { &mut *ring_buffer.get_mut_unchecked(next_sequence) };
                            event_handler.on_event(event, next_sequence, end_of_batch).map_err(
                                |error| {
                                    format!(
                                        "static consumer on_event failed at {next_sequence}: {error:?}"
                                    )
                                },
                            )?;
                            consumer_sequence.set(next_sequence);
                            next_sequence += 1;
                        }
                    }
                    Err(DisruptorError::Alert) => {
                        if shutdown_flag.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    Err(error) => {
                        if shutdown_flag.load(Ordering::Acquire) {
                            break;
                        }
                        return Err(format!(
                            "static consumer failed at sequence {next_sequence}: {error:?}"
                        ));
                    }
                }
            }

            event_handler
                .on_shutdown()
                .map_err(|error| format!("static consumer on_shutdown failed: {error:?}"))?;

            Ok(())
        })
        .map_err(|error| format!("failed to spawn static consumer thread: {error}"))
        .expect("static consumer thread spawn must succeed")
}

fn spawn_static_handler_unicast_consumer_per_batch<E, H>(
    ring_buffer: Arc<RingBuffer<E>>,
    sequence_barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    mut event_handler: H,
) -> thread::JoinHandle<Result<(), String>>
where
    E: UnicastEvent,
    H: EventHandler<E> + Send + 'static,
{
    thread::Builder::new()
        .name(String::from("h2h-static-batch-consumer"))
        .spawn(move || {
            let mut next_sequence = 0_i64;

            event_handler
                .on_start()
                .map_err(|error| format!("static batch consumer on_start failed: {error:?}"))?;

            loop {
                if shutdown_flag.load(Ordering::Acquire) {
                    break;
                }

                match sequence_barrier.wait_for_with_shutdown(next_sequence, &shutdown_flag) {
                    Ok(available_sequence) => {
                        let batch_size = available_sequence - next_sequence + 1;
                        let queue_depth = available_sequence - consumer_sequence.get();
                        let _ = event_handler.on_batch_start(batch_size, queue_depth);

                        while next_sequence <= available_sequence {
                            let end_of_batch = next_sequence == available_sequence;
                            let event =
                                unsafe { &mut *ring_buffer.get_mut_unchecked(next_sequence) };
                            event_handler.on_event(event, next_sequence, end_of_batch).map_err(
                                |error| {
                                    format!(
                                        "static batch consumer on_event failed at {next_sequence}: {error:?}"
                                    )
                                },
                            )?;
                            next_sequence += 1;
                        }

                        consumer_sequence.set(available_sequence);
                    }
                    Err(DisruptorError::Alert) => {
                        if shutdown_flag.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    Err(error) => {
                        if shutdown_flag.load(Ordering::Acquire) {
                            break;
                        }
                        return Err(format!(
                            "static batch consumer failed at sequence {next_sequence}: {error:?}"
                        ));
                    }
                }
            }

            event_handler.on_shutdown().map_err(|error| {
                format!("static batch consumer on_shutdown failed: {error:?}")
            })?;

            Ok(())
        })
        .map_err(|error| format!("failed to spawn static batch consumer thread: {error}"))
        .expect("static batch consumer thread spawn must succeed")
}

fn spawn_direct_unicast_consumer<E>(
    ring_buffer: Arc<RingBuffer<E>>,
    sequence_barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    ready_count: Arc<AtomicU64>,
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
) -> thread::JoinHandle<Result<(), String>>
where
    E: UnicastEvent,
{
    thread::spawn(move || {
        ready_count.fetch_add(1, Ordering::Release);
        let mut next_sequence = consumer_sequence.get() + 1;

        loop {
            match sequence_barrier.wait_for_with_shutdown(next_sequence, &shutdown_flag) {
                Ok(available_sequence) => {
                    if available_sequence < next_sequence {
                        continue;
                    }

                    while next_sequence <= available_sequence {
                        let event = unsafe { &mut *ring_buffer.get_mut_unchecked(next_sequence) };
                        add_wrapping_i64(&checksum, event.value());
                        processed.fetch_add(1, Ordering::Release);
                        next_sequence += 1;
                    }

                    consumer_sequence.set(available_sequence);
                }
                Err(DisruptorError::Alert) => {
                    if shutdown_flag.load(Ordering::Acquire) {
                        break;
                    }
                }
                Err(error) => {
                    return Err(format!(
                        "direct unicast consumer failed at sequence {next_sequence}: {error:?}"
                    ));
                }
            }
        }

        Ok(())
    })
}

fn spawn_direct_per_event_unicast_consumer<E>(
    ring_buffer: Arc<RingBuffer<E>>,
    sequence_barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    ready_count: Arc<AtomicU64>,
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
) -> thread::JoinHandle<Result<(), String>>
where
    E: UnicastEvent,
{
    thread::spawn(move || {
        ready_count.fetch_add(1, Ordering::Release);
        let mut next_sequence = consumer_sequence.get() + 1;

        loop {
            match sequence_barrier.wait_for_with_shutdown(next_sequence, &shutdown_flag) {
                Ok(available_sequence) => {
                    if available_sequence < next_sequence {
                        continue;
                    }

                    while next_sequence <= available_sequence {
                        let event = unsafe { &mut *ring_buffer.get_mut_unchecked(next_sequence) };
                        add_wrapping_i64(&checksum, event.value());
                        processed.fetch_add(1, Ordering::Release);
                        consumer_sequence.set(next_sequence);
                        next_sequence += 1;
                    }
                }
                Err(DisruptorError::Alert) => {
                    if shutdown_flag.load(Ordering::Acquire) {
                        break;
                    }
                }
                Err(error) => {
                    return Err(format!(
                        "direct per-event consumer failed at sequence {next_sequence}: {error:?}"
                    ));
                }
            }
        }

        Ok(())
    })
}

#[inline]
fn populate_event<E>(event: &mut E, value: i64)
where
    E: UnicastEvent,
{
    event.populate(value);
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run_main() -> Result<(), String> {
    let cli = CliConfig::from_args()?;
    let config = ResolvedConfig::try_from(cli)?;
    let result = run_scenario(&config)?;
    let json = render_json(&result);

    if let Some(path) = &config.output {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create output directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        fs::write(path, &json)
            .map_err(|error| format!("failed to write output file {}: {error}", path.display()))?;
    }

    println!("{json}");

    if !result.summary.checksum_valid_all {
        return Err(String::from(
            "one or more rounds failed correctness validation",
        ));
    }

    Ok(())
}

fn run_scenario(config: &ResolvedConfig) -> Result<HarnessResult, String> {
    match (config.scenario, config.wait_strategy) {
        (Scenario::Unicast, WaitStrategyKind::Yielding) => run_unicast_with(
            config,
            YieldingWaitStrategy::new(),
            WaitStrategyKind::Yielding,
        ),
        (Scenario::Unicast, WaitStrategyKind::BusySpin) => run_unicast_with(
            config,
            BusySpinWaitStrategy::new(),
            WaitStrategyKind::BusySpin,
        ),
        (Scenario::UnicastBatch, WaitStrategyKind::Yielding) => run_unicast_batch_with(
            config,
            YieldingWaitStrategy::new(),
            WaitStrategyKind::Yielding,
        ),
        (Scenario::UnicastBatch, WaitStrategyKind::BusySpin) => run_unicast_batch_with(
            config,
            BusySpinWaitStrategy::new(),
            WaitStrategyKind::BusySpin,
        ),
        (Scenario::MpscBatch, WaitStrategyKind::Yielding) => run_mpsc_batch_with(
            config,
            YieldingWaitStrategy::new(),
            WaitStrategyKind::Yielding,
        ),
        (Scenario::MpscBatch, WaitStrategyKind::BusySpin) => run_mpsc_batch_with(
            config,
            BusySpinWaitStrategy::new(),
            WaitStrategyKind::BusySpin,
        ),
        (Scenario::Pipeline, WaitStrategyKind::Yielding) => run_pipeline_with(
            config,
            YieldingWaitStrategy::new(),
            WaitStrategyKind::Yielding,
        ),
        (Scenario::Pipeline, WaitStrategyKind::BusySpin) => run_pipeline_with(
            config,
            BusySpinWaitStrategy::new(),
            WaitStrategyKind::BusySpin,
        ),
    }
}

fn run_unicast_with<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    if config.producer_path == ProducerPath::Builder
        && config.consumer_mode == ConsumerMode::LibraryBuilder
    {
        return run_unicast_builder_with(config, wait_strategy, wait_strategy_kind);
    }

    match config.event_padding {
        EventPadding::None => {
            run_unicast_with_typed::<W, ComparisonEvent>(config, wait_strategy, wait_strategy_kind)
        }
        EventPadding::Pad64 => run_unicast_with_typed::<W, ComparisonPadded64Event>(
            config,
            wait_strategy,
            wait_strategy_kind,
        ),
    }
}

fn run_unicast_batch_with<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    if config.producer_path == ProducerPath::Builder
        && config.consumer_mode == ConsumerMode::LibraryBuilder
    {
        return run_unicast_batch_builder_with(config, wait_strategy, wait_strategy_kind);
    }

    match config.event_padding {
        EventPadding::None => run_unicast_batch_with_typed::<W, ComparisonEvent>(
            config,
            wait_strategy,
            wait_strategy_kind,
        ),
        EventPadding::Pad64 => run_unicast_batch_with_typed::<W, ComparisonPadded64Event>(
            config,
            wait_strategy,
            wait_strategy_kind,
        ),
    }
}

fn run_unicast_builder_with<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let expected_checksum = arithmetic_checksum(config.events_total);
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));
        let handler = SummingHandler::<ComparisonEvent> {
            processed: Arc::clone(&processed),
            checksum: Arc::clone(&checksum),
            ready_count: Arc::clone(&ready_count),
            marker: PhantomData,
        };

        let mut builder = build_single_producer(
            config.buffer_size,
            ComparisonEvent::default,
            wait_strategy.clone(),
        );
        if config.event_padding == EventPadding::Pad64 {
            builder = builder.with_cache_line_padding(true);
        }
        let mut disruptor = builder.handle_events_with_handler(handler).build();

        wait_for_ready(&ready_count, 1, "unicast consumer")?;

        let start = Instant::now();
        for value in 0..config.events_total {
            let value = value as i64;
            disruptor.publish(move |event| populate_event(event, value));
        }
        wait_for_processed(&processed, config.events_total, "unicast completion")?;
        let elapsed = start.elapsed().as_nanos();

        let round_record = RoundRecord {
            round_index: round + 1,
            phase: if round < config.warmup_rounds {
                RoundPhase::Warmup
            } else {
                RoundPhase::Measured
            },
            elapsed_ns: elapsed,
            ops_per_sec: ops_per_sec(config.events_total, elapsed),
            events_expected: config.events_total,
            events_processed: processed.load(Ordering::Acquire),
            checksum_expected: expected_checksum,
            checksum_observed: checksum.load(Ordering::Acquire),
            checksum_valid: processed.load(Ordering::Acquire) == config.events_total
                && checksum.load(Ordering::Acquire) == expected_checksum,
        };

        disruptor.shutdown();
        runs.push(round_record);
    }

    Ok(build_result(
        config,
        wait_strategy_kind,
        runs,
        std::mem::size_of::<ComparisonEvent>(),
    ))
}

fn run_unicast_batch_builder_with<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let expected_checksum = arithmetic_checksum(config.events_total);
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));
        let handler = SummingHandler::<ComparisonEvent> {
            processed: Arc::clone(&processed),
            checksum: Arc::clone(&checksum),
            ready_count: Arc::clone(&ready_count),
            marker: PhantomData,
        };

        let mut builder = build_single_producer(
            config.buffer_size,
            ComparisonEvent::default,
            wait_strategy.clone(),
        );
        if config.event_padding == EventPadding::Pad64 {
            builder = builder.with_cache_line_padding(true);
        }
        let mut disruptor = builder.handle_events_with_handler(handler).build();

        wait_for_ready(&ready_count, 1, "unicast batch consumer")?;

        let start = Instant::now();
        let mut published = 0_u64;
        while published < config.events_total {
            let chunk_len = ((config.events_total - published) as usize).min(config.batch_size);
            let chunk_start = published;
            disruptor.batch_publish(chunk_len, |iter| {
                for (index, event) in iter.enumerate() {
                    let value = (chunk_start + index as u64) as i64;
                    populate_event(event, value);
                }
            });
            published += chunk_len as u64;
        }
        wait_for_processed(&processed, config.events_total, "unicast batch completion")?;
        let elapsed = start.elapsed().as_nanos();

        let round_record = RoundRecord {
            round_index: round + 1,
            phase: if round < config.warmup_rounds {
                RoundPhase::Warmup
            } else {
                RoundPhase::Measured
            },
            elapsed_ns: elapsed,
            ops_per_sec: ops_per_sec(config.events_total, elapsed),
            events_expected: config.events_total,
            events_processed: processed.load(Ordering::Acquire),
            checksum_expected: expected_checksum,
            checksum_observed: checksum.load(Ordering::Acquire),
            checksum_valid: processed.load(Ordering::Acquire) == config.events_total
                && checksum.load(Ordering::Acquire) == expected_checksum,
        };

        disruptor.shutdown();
        runs.push(round_record);
    }

    Ok(build_result(
        config,
        wait_strategy_kind,
        runs,
        std::mem::size_of::<ComparisonEvent>(),
    ))
}

fn run_unicast_with_typed<W, E>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
    E: UnicastEvent,
{
    if config.producer_path == ProducerPath::Direct
        && config.consumer_mode == ConsumerMode::DirectPerBatch
    {
        return run_unicast_direct_with::<W, E>(config, wait_strategy, wait_strategy_kind);
    }
    if config.producer_path != ProducerPath::Builder
        || config.consumer_mode != ConsumerMode::LibraryBuilder
    {
        return run_unicast_hybrid_with::<W, E>(config, wait_strategy, wait_strategy_kind);
    }

    let expected_checksum = arithmetic_checksum(config.events_total);
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));
        let handler = SummingHandler::<E> {
            processed: Arc::clone(&processed),
            checksum: Arc::clone(&checksum),
            ready_count: Arc::clone(&ready_count),
            marker: PhantomData,
        };

        let mut disruptor =
            build_single_producer(config.buffer_size, E::default, wait_strategy.clone())
                .handle_events_with_handler(handler)
                .build();

        wait_for_ready(&ready_count, 1, "unicast consumer")?;

        let start = Instant::now();
        for value in 0..config.events_total {
            let value = value as i64;
            disruptor.publish(move |event| populate_event(event, value));
        }
        wait_for_processed(&processed, config.events_total, "unicast completion")?;
        let elapsed = start.elapsed().as_nanos();

        let round_record = RoundRecord {
            round_index: round + 1,
            phase: if round < config.warmup_rounds {
                RoundPhase::Warmup
            } else {
                RoundPhase::Measured
            },
            elapsed_ns: elapsed,
            ops_per_sec: ops_per_sec(config.events_total, elapsed),
            events_expected: config.events_total,
            events_processed: processed.load(Ordering::Acquire),
            checksum_expected: expected_checksum,
            checksum_observed: checksum.load(Ordering::Acquire),
            checksum_valid: processed.load(Ordering::Acquire) == config.events_total
                && checksum.load(Ordering::Acquire) == expected_checksum,
        };

        disruptor.shutdown();
        runs.push(round_record);
    }

    Ok(build_result(
        config,
        wait_strategy_kind,
        runs,
        std::mem::size_of::<E>(),
    ))
}

fn run_unicast_batch_with_typed<W, E>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
    E: UnicastEvent,
{
    if config.producer_path == ProducerPath::Direct
        && config.consumer_mode == ConsumerMode::DirectPerBatch
    {
        return run_unicast_batch_direct_with::<W, E>(config, wait_strategy, wait_strategy_kind);
    }
    if config.producer_path != ProducerPath::Builder
        || config.consumer_mode != ConsumerMode::LibraryBuilder
    {
        return run_unicast_batch_hybrid_with::<W, E>(config, wait_strategy, wait_strategy_kind);
    }

    let expected_checksum = arithmetic_checksum(config.events_total);
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));
        let handler = SummingHandler::<E> {
            processed: Arc::clone(&processed),
            checksum: Arc::clone(&checksum),
            ready_count: Arc::clone(&ready_count),
            marker: PhantomData,
        };

        let mut disruptor =
            build_single_producer(config.buffer_size, E::default, wait_strategy.clone())
                .handle_events_with_handler(handler)
                .build();

        wait_for_ready(&ready_count, 1, "unicast batch consumer")?;

        let start = Instant::now();
        let mut published = 0_u64;
        while published < config.events_total {
            let chunk_len = ((config.events_total - published) as usize).min(config.batch_size);
            let chunk_start = published;
            disruptor.batch_publish(chunk_len, |iter| {
                for (index, event) in iter.enumerate() {
                    let value = (chunk_start + index as u64) as i64;
                    populate_event(event, value);
                }
            });
            published += chunk_len as u64;
        }
        wait_for_processed(&processed, config.events_total, "unicast batch completion")?;
        let elapsed = start.elapsed().as_nanos();

        let round_record = RoundRecord {
            round_index: round + 1,
            phase: if round < config.warmup_rounds {
                RoundPhase::Warmup
            } else {
                RoundPhase::Measured
            },
            elapsed_ns: elapsed,
            ops_per_sec: ops_per_sec(config.events_total, elapsed),
            events_expected: config.events_total,
            events_processed: processed.load(Ordering::Acquire),
            checksum_expected: expected_checksum,
            checksum_observed: checksum.load(Ordering::Acquire),
            checksum_valid: processed.load(Ordering::Acquire) == config.events_total
                && checksum.load(Ordering::Acquire) == expected_checksum,
        };

        disruptor.shutdown();
        runs.push(round_record);
    }

    Ok(build_result(
        config,
        wait_strategy_kind,
        runs,
        std::mem::size_of::<E>(),
    ))
}

fn run_unicast_direct_with<W, E>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
    E: UnicastEvent,
{
    let expected_checksum = arithmetic_checksum(config.events_total);
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));

        let runtime = LowLevelSingleProducerRuntime::<E>::new(
            config.buffer_size,
            wait_strategy.clone(),
            ConsumerMode::DirectPerBatch,
            Arc::clone(&ready_count),
            Arc::clone(&processed),
            Arc::clone(&checksum),
        )?;

        let run_result = (|| -> Result<RoundRecord, String> {
            wait_for_ready(&ready_count, 1, "direct unicast consumer")?;

            let start = Instant::now();
            for value in 0..config.events_total {
                let value = value as i64;
                let sequence = runtime
                    .sequencer
                    .next()
                    .map_err(|error| format!("failed to claim unicast sequence: {error:?}"))?;
                let event = unsafe { &mut *runtime.ring_buffer.get_mut_unchecked(sequence) };
                populate_event(event, value);
                runtime.sequencer.publish(sequence);
            }
            wait_for_processed(&processed, config.events_total, "direct unicast completion")?;
            let elapsed = start.elapsed().as_nanos();

            Ok(RoundRecord {
                round_index: round + 1,
                phase: if round < config.warmup_rounds {
                    RoundPhase::Warmup
                } else {
                    RoundPhase::Measured
                },
                elapsed_ns: elapsed,
                ops_per_sec: ops_per_sec(config.events_total, elapsed),
                events_expected: config.events_total,
                events_processed: processed.load(Ordering::Acquire),
                checksum_expected: expected_checksum,
                checksum_observed: checksum.load(Ordering::Acquire),
                checksum_valid: processed.load(Ordering::Acquire) == config.events_total
                    && checksum.load(Ordering::Acquire) == expected_checksum,
            })
        })();

        let shutdown_result = runtime.shutdown();
        let round_record = run_result?;
        shutdown_result?;
        runs.push(round_record);
    }

    Ok(build_result(
        config,
        wait_strategy_kind,
        runs,
        std::mem::size_of::<E>(),
    ))
}

fn run_unicast_batch_direct_with<W, E>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
    E: UnicastEvent,
{
    let expected_checksum = arithmetic_checksum(config.events_total);
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));

        let runtime = LowLevelSingleProducerRuntime::<E>::new(
            config.buffer_size,
            wait_strategy.clone(),
            ConsumerMode::DirectPerBatch,
            Arc::clone(&ready_count),
            Arc::clone(&processed),
            Arc::clone(&checksum),
        )?;

        let run_result = (|| -> Result<RoundRecord, String> {
            wait_for_ready(&ready_count, 1, "direct unicast batch consumer")?;

            let start = Instant::now();
            let mut published = 0_u64;
            while published < config.events_total {
                let chunk_len = ((config.events_total - published) as usize).min(config.batch_size);
                let chunk_len_i64 =
                    i64::try_from(chunk_len).expect("batch size must fit in i64 for direct path");
                let end_sequence = runtime.sequencer.next_n(chunk_len_i64).map_err(|error| {
                    format!("failed to claim direct unicast batch sequences: {error:?}")
                })?;
                let start_sequence = end_sequence - (chunk_len_i64 - 1);
                let chunk_start = published;
                let iter = unsafe {
                    runtime
                        .ring_buffer
                        .batch_iter_mut(start_sequence, end_sequence)
                };
                for (index, event) in iter.enumerate() {
                    let value = (chunk_start + index as u64) as i64;
                    populate_event(event, value);
                }
                runtime
                    .sequencer
                    .publish_range(start_sequence, end_sequence);
                published += chunk_len as u64;
            }
            wait_for_processed(
                &processed,
                config.events_total,
                "direct unicast batch completion",
            )?;
            let elapsed = start.elapsed().as_nanos();

            Ok(RoundRecord {
                round_index: round + 1,
                phase: if round < config.warmup_rounds {
                    RoundPhase::Warmup
                } else {
                    RoundPhase::Measured
                },
                elapsed_ns: elapsed,
                ops_per_sec: ops_per_sec(config.events_total, elapsed),
                events_expected: config.events_total,
                events_processed: processed.load(Ordering::Acquire),
                checksum_expected: expected_checksum,
                checksum_observed: checksum.load(Ordering::Acquire),
                checksum_valid: processed.load(Ordering::Acquire) == config.events_total
                    && checksum.load(Ordering::Acquire) == expected_checksum,
            })
        })();

        let shutdown_result = runtime.shutdown();
        let round_record = run_result?;
        shutdown_result?;
        runs.push(round_record);
    }

    Ok(build_result(
        config,
        wait_strategy_kind,
        runs,
        std::mem::size_of::<E>(),
    ))
}

fn run_unicast_hybrid_with<W, E>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
    E: UnicastEvent,
{
    let expected_checksum = arithmetic_checksum(config.events_total);
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));

        let runtime = LowLevelSingleProducerRuntime::<E>::new(
            config.buffer_size,
            wait_strategy.clone(),
            config.consumer_mode,
            Arc::clone(&ready_count),
            Arc::clone(&processed),
            Arc::clone(&checksum),
        )?;

        let run_result = (|| -> Result<RoundRecord, String> {
            wait_for_ready(&ready_count, 1, "hybrid unicast consumer")?;

            let start = Instant::now();
            match config.producer_path {
                ProducerPath::Builder => {
                    let mut producer = SimpleProducer::new(
                        Arc::clone(&runtime.ring_buffer),
                        SequencerEnum::Single(Arc::clone(&runtime.sequencer)),
                    );
                    for value in 0..config.events_total {
                        let value = value as i64;
                        producer.publish(move |event| populate_event(event, value));
                    }
                }
                ProducerPath::Direct => {
                    for value in 0..config.events_total {
                        let value = value as i64;
                        let sequence = runtime.sequencer.next().map_err(|error| {
                            format!("failed to claim unicast sequence: {error:?}")
                        })?;
                        let event =
                            unsafe { &mut *runtime.ring_buffer.get_mut_unchecked(sequence) };
                        populate_event(event, value);
                        runtime.sequencer.publish(sequence);
                    }
                }
            }
            wait_for_processed(&processed, config.events_total, "hybrid unicast completion")?;
            let elapsed = start.elapsed().as_nanos();

            Ok(RoundRecord {
                round_index: round + 1,
                phase: if round < config.warmup_rounds {
                    RoundPhase::Warmup
                } else {
                    RoundPhase::Measured
                },
                elapsed_ns: elapsed,
                ops_per_sec: ops_per_sec(config.events_total, elapsed),
                events_expected: config.events_total,
                events_processed: processed.load(Ordering::Acquire),
                checksum_expected: expected_checksum,
                checksum_observed: checksum.load(Ordering::Acquire),
                checksum_valid: processed.load(Ordering::Acquire) == config.events_total
                    && checksum.load(Ordering::Acquire) == expected_checksum,
            })
        })();

        let shutdown_result = runtime.shutdown();
        let round_record = run_result?;
        shutdown_result?;
        runs.push(round_record);
    }

    Ok(build_result(
        config,
        wait_strategy_kind,
        runs,
        std::mem::size_of::<E>(),
    ))
}

fn run_unicast_batch_hybrid_with<W, E>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
    E: UnicastEvent,
{
    let expected_checksum = arithmetic_checksum(config.events_total);
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));

        let runtime = LowLevelSingleProducerRuntime::<E>::new(
            config.buffer_size,
            wait_strategy.clone(),
            config.consumer_mode,
            Arc::clone(&ready_count),
            Arc::clone(&processed),
            Arc::clone(&checksum),
        )?;

        let run_result = (|| -> Result<RoundRecord, String> {
            wait_for_ready(&ready_count, 1, "hybrid unicast batch consumer")?;

            let start = Instant::now();
            match config.producer_path {
                ProducerPath::Builder => {
                    let mut producer = SimpleProducer::new(
                        Arc::clone(&runtime.ring_buffer),
                        SequencerEnum::Single(Arc::clone(&runtime.sequencer)),
                    );
                    let mut published = 0_u64;
                    while published < config.events_total {
                        let chunk_len =
                            ((config.events_total - published) as usize).min(config.batch_size);
                        let chunk_start = published;
                        producer.batch_publish(chunk_len, |iter| {
                            for (index, event) in iter.enumerate() {
                                let value = (chunk_start + index as u64) as i64;
                                populate_event(event, value);
                            }
                        });
                        published += chunk_len as u64;
                    }
                }
                ProducerPath::Direct => {
                    let mut published = 0_u64;
                    while published < config.events_total {
                        let chunk_len =
                            ((config.events_total - published) as usize).min(config.batch_size);
                        let chunk_len_i64 = i64::try_from(chunk_len)
                            .expect("batch size must fit in i64 for hybrid direct producer");
                        let end_sequence =
                            runtime.sequencer.next_n(chunk_len_i64).map_err(|error| {
                                format!("failed to claim hybrid unicast batch sequences: {error:?}")
                            })?;
                        let start_sequence = end_sequence - (chunk_len_i64 - 1);
                        let chunk_start = published;
                        let iter = unsafe {
                            runtime
                                .ring_buffer
                                .batch_iter_mut(start_sequence, end_sequence)
                        };
                        for (index, event) in iter.enumerate() {
                            let value = (chunk_start + index as u64) as i64;
                            populate_event(event, value);
                        }
                        runtime
                            .sequencer
                            .publish_range(start_sequence, end_sequence);
                        published += chunk_len as u64;
                    }
                }
            }
            wait_for_processed(
                &processed,
                config.events_total,
                "hybrid unicast batch completion",
            )?;
            let elapsed = start.elapsed().as_nanos();

            Ok(RoundRecord {
                round_index: round + 1,
                phase: if round < config.warmup_rounds {
                    RoundPhase::Warmup
                } else {
                    RoundPhase::Measured
                },
                elapsed_ns: elapsed,
                ops_per_sec: ops_per_sec(config.events_total, elapsed),
                events_expected: config.events_total,
                events_processed: processed.load(Ordering::Acquire),
                checksum_expected: expected_checksum,
                checksum_observed: checksum.load(Ordering::Acquire),
                checksum_valid: processed.load(Ordering::Acquire) == config.events_total
                    && checksum.load(Ordering::Acquire) == expected_checksum,
            })
        })();

        let shutdown_result = runtime.shutdown();
        let round_record = run_result?;
        shutdown_result?;
        runs.push(round_record);
    }

    Ok(build_result(
        config,
        wait_strategy_kind,
        runs,
        std::mem::size_of::<E>(),
    ))
}

fn run_mpsc_batch_with<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let expected_checksum = arithmetic_checksum(config.events_total);
    let events_per_producer = config.events_total / MPSC_PRODUCER_COUNT as u64;
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));
        let handler = SummingHandler {
            processed: Arc::clone(&processed),
            checksum: Arc::clone(&checksum),
            ready_count: Arc::clone(&ready_count),
            marker: PhantomData,
        };

        let mut disruptor = build_multi_producer(
            config.buffer_size,
            ComparisonEvent::default,
            wait_strategy.clone(),
        )
        .handle_events_with_handler(handler)
        .build();

        wait_for_ready(&ready_count, 1, "mpsc consumer")?;

        let start_flag = Arc::new(AtomicU64::new(0));
        let producers_ready = Arc::new(AtomicU64::new(0));
        let mut threads = Vec::with_capacity(MPSC_PRODUCER_COUNT);

        for producer_id in 0..MPSC_PRODUCER_COUNT {
            let mut producer = disruptor.create_producer();
            let start_flag = Arc::clone(&start_flag);
            let producers_ready = Arc::clone(&producers_ready);
            let batch_size = config.batch_size;

            threads.push(thread::spawn(move || {
                producers_ready.fetch_add(1, Ordering::Release);
                while start_flag.load(Ordering::Acquire) == 0 {
                    std::hint::spin_loop();
                }

                let producer_start = events_per_producer * producer_id as u64;
                let producer_end = producer_start + events_per_producer;
                let mut published = producer_start;
                while published < producer_end {
                    let chunk_len = ((producer_end - published) as usize).min(batch_size);
                    let chunk_start = published;
                    producer.batch_publish(
                        chunk_len,
                        |iter: badbatch::disruptor::ring_buffer::BatchIterMut<
                            '_,
                            ComparisonEvent,
                        >| {
                            for (index, event) in iter.enumerate() {
                                let value = (chunk_start + index as u64) as i64;
                                event.value = value;
                                event.stage1_value = 0;
                                event.stage2_value = 0;
                                event.stage3_value = 0;
                            }
                        },
                    );
                    published += chunk_len as u64;
                }
            }));
        }

        wait_for_ready(
            &producers_ready,
            MPSC_PRODUCER_COUNT as u64,
            "mpsc producers",
        )?;

        let start = Instant::now();
        start_flag.store(1, Ordering::Release);

        for handle in threads {
            handle
                .join()
                .map_err(|_| String::from("mpsc producer thread panicked"))?;
        }

        wait_for_processed(&processed, config.events_total, "mpsc completion")?;
        let elapsed = start.elapsed().as_nanos();

        let round_record = RoundRecord {
            round_index: round + 1,
            phase: if round < config.warmup_rounds {
                RoundPhase::Warmup
            } else {
                RoundPhase::Measured
            },
            elapsed_ns: elapsed,
            ops_per_sec: ops_per_sec(config.events_total, elapsed),
            events_expected: config.events_total,
            events_processed: processed.load(Ordering::Acquire),
            checksum_expected: expected_checksum,
            checksum_observed: checksum.load(Ordering::Acquire),
            checksum_valid: processed.load(Ordering::Acquire) == config.events_total
                && checksum.load(Ordering::Acquire) == expected_checksum,
        };

        disruptor.shutdown();
        runs.push(round_record);
    }

    Ok(build_result(
        config,
        wait_strategy_kind,
        runs,
        std::mem::size_of::<ComparisonEvent>(),
    ))
}

fn run_pipeline_with<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    run_pipeline_builder_with(config, wait_strategy, wait_strategy_kind)
}

fn run_pipeline_builder_with<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<HarnessResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let expected_checksum = pipeline_checksum(config.events_total);
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));

        let stage1 = PipelineStage1Handler::<ComparisonEvent> {
            ready_count: Arc::clone(&ready_count),
            marker: PhantomData,
        };
        let stage2 = PipelineStage2Handler::<ComparisonEvent> {
            ready_count: Arc::clone(&ready_count),
            marker: PhantomData,
        };
        let stage3 = PipelineStage3Handler::<ComparisonEvent> {
            processed: Arc::clone(&processed),
            checksum: Arc::clone(&checksum),
            ready_count: Arc::clone(&ready_count),
            marker: PhantomData,
        };

        let mut builder = build_single_producer(
            config.buffer_size,
            ComparisonEvent::default,
            wait_strategy.clone(),
        );
        if config.event_padding == EventPadding::Pad64 {
            builder = builder.with_cache_line_padding(true);
        }
        let mut disruptor = builder
            .handle_events_with_handler(stage1)
            .and_then()
            .handle_events_with_handler(stage2)
            .and_then()
            .handle_events_with_handler(stage3)
            .build();

        wait_for_ready(&ready_count, 3, "pipeline stages")?;

        let start = Instant::now();
        for value in 0..config.events_total {
            let value = value as i64;
            disruptor.publish(move |event| populate_event(event, value));
        }
        wait_for_processed(&processed, config.events_total, "pipeline completion")?;
        let elapsed = start.elapsed().as_nanos();

        let round_record = RoundRecord {
            round_index: round + 1,
            phase: if round < config.warmup_rounds {
                RoundPhase::Warmup
            } else {
                RoundPhase::Measured
            },
            elapsed_ns: elapsed,
            ops_per_sec: ops_per_sec(config.events_total, elapsed),
            events_expected: config.events_total,
            events_processed: processed.load(Ordering::Acquire),
            checksum_expected: expected_checksum,
            checksum_observed: checksum.load(Ordering::Acquire),
            checksum_valid: processed.load(Ordering::Acquire) == config.events_total
                && checksum.load(Ordering::Acquire) == expected_checksum,
        };

        disruptor.shutdown();
        runs.push(round_record);
    }

    Ok(build_result(
        config,
        wait_strategy_kind,
        runs,
        std::mem::size_of::<ComparisonEvent>(),
    ))
}

fn build_result(
    config: &ResolvedConfig,
    wait_strategy: WaitStrategyKind,
    runs: Vec<RoundRecord>,
    event_size_bytes: usize,
) -> HarnessResult {
    HarnessResult {
        impl_name: String::from("badbatch"),
        scenario: config.scenario,
        event_padding: config.event_padding,
        producer_path: config.producer_path,
        consumer_path: config.consumer_path,
        consumer_mode: config.consumer_mode,
        measurement_kind: String::from("throughput"),
        buffer_size: config.buffer_size,
        event_size_bytes,
        wait_strategy,
        producer_count: config.scenario.producer_count(),
        consumer_count: config.scenario.consumer_count(),
        pipeline_stages: config.scenario.pipeline_stages(),
        batch_size: config.batch_size,
        events_total: config.events_total,
        warmup_rounds: config.warmup_rounds,
        measured_rounds: config.measured_rounds,
        run_order: config.run_order.clone(),
        env: EnvInfo {
            os: env::consts::OS.to_string(),
            arch: env::consts::ARCH.to_string(),
            runtime: String::from("rust"),
            runtime_version: rust_version_string(),
        },
        summary: summarize_runs(&runs),
        runs,
    }
}

fn summarize_runs(runs: &[RoundRecord]) -> SummaryStats {
    let measured: Vec<f64> = runs
        .iter()
        .filter(|run| run.phase == RoundPhase::Measured)
        .map(|run| run.ops_per_sec)
        .collect();
    let checksum_valid_all = runs.iter().all(|run| run.checksum_valid);

    if measured.is_empty() {
        return SummaryStats {
            checksum_valid_all,
            median_ops_per_sec: 0.0,
            mean_ops_per_sec: 0.0,
            min_ops_per_sec: 0.0,
            max_ops_per_sec: 0.0,
            stddev_ops_per_sec: 0.0,
            cv: 0.0,
        };
    }

    let mut sorted = measured.clone();
    sorted.sort_by(f64::total_cmp);
    let median = if sorted.len() % 2 == 0 {
        let upper = sorted.len() / 2;
        (sorted[upper - 1] + sorted[upper]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    };

    let mean = measured.iter().sum::<f64>() / measured.len() as f64;
    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let variance = measured
        .iter()
        .map(|value| {
            let delta = value - mean;
            delta * delta
        })
        .sum::<f64>()
        / measured.len() as f64;
    let stddev = variance.sqrt();
    let cv = if mean == 0.0 { 0.0 } else { stddev / mean };

    SummaryStats {
        checksum_valid_all,
        median_ops_per_sec: median,
        mean_ops_per_sec: mean,
        min_ops_per_sec: min,
        max_ops_per_sec: max,
        stddev_ops_per_sec: stddev,
        cv,
    }
}

fn wait_for_ready(counter: &AtomicU64, expected: u64, label: &str) -> Result<(), String> {
    wait_for_atomic(counter, expected, label)
}

fn wait_for_processed(counter: &AtomicU64, expected: u64, label: &str) -> Result<(), String> {
    wait_for_atomic(counter, expected, label)
}

fn wait_for_atomic(counter: &AtomicU64, expected: u64, label: &str) -> Result<(), String> {
    let start = Instant::now();
    let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
    while counter.load(Ordering::Acquire) < expected {
        if start.elapsed() > timeout {
            return Err(format!(
                "timed out waiting for {label}: expected >= {expected}, got {}",
                counter.load(Ordering::Acquire)
            ));
        }
        std::hint::spin_loop();
    }
    Ok(())
}

fn arithmetic_checksum(count: u64) -> i64 {
    let count = u128::from(count);
    let sum = count.saturating_mul(count.saturating_sub(1)) / 2;
    (sum as u64) as i64
}

fn pipeline_checksum(count: u64) -> i64 {
    let count_u128 = u128::from(count);
    let sum = count_u128.saturating_mul(count_u128.saturating_sub(1)) / 2 + (11_u128 * count_u128);
    (sum as u64) as i64
}

fn ops_per_sec(events_total: u64, elapsed_ns: u128) -> f64 {
    if elapsed_ns == 0 {
        return 0.0;
    }
    (events_total as f64) / (elapsed_ns as f64 / 1_000_000_000.0)
}

fn add_wrapping_i64(target: &AtomicI64, value: i64) {
    target.fetch_add(value, Ordering::Relaxed);
}

fn rust_version_string() -> String {
    Command::new("rustc")
        .arg("-V")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| String::from("unknown"))
}

fn render_json(result: &HarnessResult) -> String {
    let mut json = String::new();
    json.push_str("{\n");
    write_json_field(&mut json, 2, "impl_name", &result.impl_name, true);
    write_json_field(&mut json, 2, "scenario", result.scenario.as_str(), true);
    write_json_field(
        &mut json,
        2,
        "event_padding",
        result.event_padding.as_str(),
        true,
    );
    write_json_field(
        &mut json,
        2,
        "producer_path",
        result.producer_path.as_str(),
        true,
    );
    write_json_field(
        &mut json,
        2,
        "consumer_path",
        result.consumer_path.as_str(),
        true,
    );
    write_json_field(
        &mut json,
        2,
        "consumer_mode",
        result.consumer_mode.as_str(),
        true,
    );
    write_json_field(
        &mut json,
        2,
        "measurement_kind",
        &result.measurement_kind,
        true,
    );
    write_json_number(&mut json, 2, "buffer_size", result.buffer_size as u64, true);
    write_json_number(
        &mut json,
        2,
        "event_size_bytes",
        result.event_size_bytes as u64,
        true,
    );
    write_json_field(
        &mut json,
        2,
        "wait_strategy",
        result.wait_strategy.as_str(),
        true,
    );
    write_json_number(
        &mut json,
        2,
        "producer_count",
        result.producer_count as u64,
        true,
    );
    write_json_number(
        &mut json,
        2,
        "consumer_count",
        result.consumer_count as u64,
        true,
    );
    write_json_number(
        &mut json,
        2,
        "pipeline_stages",
        result.pipeline_stages as u64,
        true,
    );
    write_json_number(&mut json, 2, "batch_size", result.batch_size as u64, true);
    write_json_number(&mut json, 2, "events_total", result.events_total, true);
    write_json_number(
        &mut json,
        2,
        "warmup_rounds",
        result.warmup_rounds as u64,
        true,
    );
    write_json_number(
        &mut json,
        2,
        "measured_rounds",
        result.measured_rounds as u64,
        true,
    );
    write_json_field(&mut json, 2, "run_order", &result.run_order, true);

    json.push_str("  \"env\": {\n");
    write_json_field(&mut json, 4, "os", &result.env.os, true);
    write_json_field(&mut json, 4, "arch", &result.env.arch, true);
    write_json_field(&mut json, 4, "runtime", &result.env.runtime, true);
    write_json_field(
        &mut json,
        4,
        "runtime_version",
        &result.env.runtime_version,
        false,
    );
    json.push_str("  },\n");

    json.push_str("  \"runs\": [\n");
    for (index, run) in result.runs.iter().enumerate() {
        json.push_str("    {\n");
        write_json_number(&mut json, 6, "round_index", run.round_index as u64, true);
        write_json_field(&mut json, 6, "phase", run.phase.as_str(), true);
        write_json_number_u128(&mut json, 6, "elapsed_ns", run.elapsed_ns, true);
        write_json_float(&mut json, 6, "ops_per_sec", run.ops_per_sec, true);
        write_json_number(&mut json, 6, "events_expected", run.events_expected, true);
        write_json_number(&mut json, 6, "events_processed", run.events_processed, true);
        write_json_i64(
            &mut json,
            6,
            "checksum_expected",
            run.checksum_expected,
            true,
        );
        write_json_i64(
            &mut json,
            6,
            "checksum_observed",
            run.checksum_observed,
            true,
        );
        write_json_bool(&mut json, 6, "checksum_valid", run.checksum_valid, false);
        if index + 1 == result.runs.len() {
            json.push_str("    }\n");
        } else {
            json.push_str("    },\n");
        }
    }
    json.push_str("  ],\n");

    json.push_str("  \"summary\": {\n");
    write_json_bool(
        &mut json,
        4,
        "checksum_valid_all",
        result.summary.checksum_valid_all,
        true,
    );
    write_json_float(
        &mut json,
        4,
        "median_ops_per_sec",
        result.summary.median_ops_per_sec,
        true,
    );
    write_json_float(
        &mut json,
        4,
        "mean_ops_per_sec",
        result.summary.mean_ops_per_sec,
        true,
    );
    write_json_float(
        &mut json,
        4,
        "min_ops_per_sec",
        result.summary.min_ops_per_sec,
        true,
    );
    write_json_float(
        &mut json,
        4,
        "max_ops_per_sec",
        result.summary.max_ops_per_sec,
        true,
    );
    write_json_float(
        &mut json,
        4,
        "stddev_ops_per_sec",
        result.summary.stddev_ops_per_sec,
        true,
    );
    write_json_float(&mut json, 4, "cv", result.summary.cv, false);
    json.push_str("  }\n");
    json.push_str("}\n");
    json
}

fn write_json_field(output: &mut String, indent: usize, key: &str, value: &str, trailing: bool) {
    let comma = if trailing { "," } else { "" };
    let _ = writeln!(
        output,
        "{space}\"{key}\": \"{value}\"{comma}",
        space = " ".repeat(indent)
    );
}

fn write_json_number(output: &mut String, indent: usize, key: &str, value: u64, trailing: bool) {
    let comma = if trailing { "," } else { "" };
    let _ = writeln!(
        output,
        "{space}\"{key}\": {value}{comma}",
        space = " ".repeat(indent)
    );
}

fn write_json_number_u128(
    output: &mut String,
    indent: usize,
    key: &str,
    value: u128,
    trailing: bool,
) {
    let comma = if trailing { "," } else { "" };
    let _ = writeln!(
        output,
        "{space}\"{key}\": {value}{comma}",
        space = " ".repeat(indent)
    );
}

fn write_json_i64(output: &mut String, indent: usize, key: &str, value: i64, trailing: bool) {
    let comma = if trailing { "," } else { "" };
    let _ = writeln!(
        output,
        "{space}\"{key}\": {value}{comma}",
        space = " ".repeat(indent)
    );
}

fn write_json_float(output: &mut String, indent: usize, key: &str, value: f64, trailing: bool) {
    let comma = if trailing { "," } else { "" };
    let _ = writeln!(
        output,
        "{space}\"{key}\": {:.6}{comma}",
        value,
        space = " ".repeat(indent)
    );
}

fn write_json_bool(output: &mut String, indent: usize, key: &str, value: bool, trailing: bool) {
    let comma = if trailing { "," } else { "" };
    let literal = if value { "true" } else { "false" };
    let _ = writeln!(
        output,
        "{space}\"{key}\": {literal}{comma}",
        space = " ".repeat(indent)
    );
}

fn parse_usize(value: &str, flag: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|error| format!("invalid value for {flag}: {value} ({error})"))
}

fn parse_u64(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|error| format!("invalid value for {flag}: {value} ({error})"))
}

fn print_help() {
    println!("Usage: cargo run --release --bin h2h_rust -- --scenario <SCENARIO> [options]");
    println!();
    println!("Options:");
    println!("  --scenario <unicast|unicast_batch|mpsc_batch|pipeline>");
    println!("  --wait-strategy <yielding|busy-spin>");
    println!("  --event-padding <none|64>");
    println!("  --producer-path <builder|direct>");
    println!("  --consumer-path <builder|direct>");
    println!("  --consumer-mode <library-builder|builder-dyn|builder-static|builder-static-per-batch|direct-per-event|direct-per-batch>");
    println!("  --buffer-size <N>");
    println!("  --events-total <N>");
    println!("  --batch-size <N>");
    println!("  --warmup-rounds <N>");
    println!("  --measured-rounds <N>");
    println!("  --run-order <rust-then-java|java-then-rust|standalone>");
    println!("  --output <PATH>");
}
