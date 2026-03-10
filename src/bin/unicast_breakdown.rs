#![allow(
    missing_docs,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

use badbatch::disruptor::{
    BusySpinWaitStrategy, DefaultEventFactory, DisruptorError, ProcessingSequenceBarrier,
    RingBuffer, Sequence, SequenceBarrier, Sequencer, SequencerEnum, SingleProducerSequencer,
    WaitStrategy, YieldingWaitStrategy,
};
use std::env;
use std::fmt::Write as _;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Default, Clone, Copy)]
struct MicroEvent {
    value: i64,
}

#[derive(Debug, Default, Clone, Copy)]
struct ComparisonPayloadEvent {
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

#[derive(Debug, Default, Clone, Copy)]
#[repr(C, align(128))]
struct ComparisonPadded128Event {
    value: i64,
    stage1_value: i64,
    stage2_value: i64,
    stage3_value: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventLayout {
    Minimal,
    Comparison,
    ComparisonPadded64,
    ComparisonPadded128,
}

impl EventLayout {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "minimal" => Ok(Self::Minimal),
            "comparison" => Ok(Self::Comparison),
            "comparison-padded64" => Ok(Self::ComparisonPadded64),
            "comparison-padded128" => Ok(Self::ComparisonPadded128),
            _ => Err(format!("unsupported event layout: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Comparison => "comparison",
            Self::ComparisonPadded64 => "comparison-padded64",
            Self::ComparisonPadded128 => "comparison-padded128",
        }
    }

    fn event_size_bytes(self) -> usize {
        match self {
            Self::Minimal => size_of::<MicroEvent>(),
            Self::Comparison => size_of::<ComparisonPayloadEvent>(),
            Self::ComparisonPadded64 => size_of::<ComparisonPadded64Event>(),
            Self::ComparisonPadded128 => size_of::<ComparisonPadded128Event>(),
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
enum Mode {
    All,
    Baseline,
    ClaimOnly,
    ClaimWrite,
    ClaimWritePublish,
    EndToEnd,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "all" => Ok(Self::All),
            "baseline" => Ok(Self::Baseline),
            "claim-only" => Ok(Self::ClaimOnly),
            "claim-write" => Ok(Self::ClaimWrite),
            "claim-write-publish" => Ok(Self::ClaimWritePublish),
            "end-to-end" => Ok(Self::EndToEnd),
            _ => Err(format!("unsupported mode: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Baseline => "baseline",
            Self::ClaimOnly => "claim-only",
            Self::ClaimWrite => "claim-write",
            Self::ClaimWritePublish => "claim-write-publish",
            Self::EndToEnd => "end-to-end",
        }
    }

    fn phases(self) -> &'static [PhaseKind] {
        match self {
            Self::All => &[
                PhaseKind::Baseline,
                PhaseKind::ClaimOnly,
                PhaseKind::ClaimWrite,
                PhaseKind::ClaimWritePublish,
                PhaseKind::EndToEnd,
            ],
            Self::Baseline => &[PhaseKind::Baseline],
            Self::ClaimOnly => &[PhaseKind::ClaimOnly],
            Self::ClaimWrite => &[PhaseKind::ClaimWrite],
            Self::ClaimWritePublish => &[PhaseKind::ClaimWritePublish],
            Self::EndToEnd => &[PhaseKind::EndToEnd],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhaseKind {
    Baseline,
    ClaimOnly,
    ClaimWrite,
    ClaimWritePublish,
    EndToEnd,
}

impl PhaseKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::ClaimOnly => "claim_only",
            Self::ClaimWrite => "claim_write",
            Self::ClaimWritePublish => "claim_write_publish",
            Self::EndToEnd => "end_to_end",
        }
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
struct CliConfig {
    mode: Mode,
    wait_strategy: WaitStrategyKind,
    event_layout: EventLayout,
    events_total: u64,
    buffer_size: Option<usize>,
    allow_wrap: bool,
    warmup_rounds: usize,
    measured_rounds: usize,
    timeout_secs: u64,
    output: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct ResolvedConfig {
    mode: Mode,
    wait_strategy: WaitStrategyKind,
    event_layout: EventLayout,
    events_total: u64,
    buffer_size: usize,
    allow_wrap: bool,
    warmup_rounds: usize,
    measured_rounds: usize,
    timeout_secs: u64,
    output: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct RoundRecord {
    round_index: usize,
    phase: RoundPhase,
    elapsed_ns: u128,
    ops_per_sec: f64,
    events_expected: Option<u64>,
    events_observed: Option<u64>,
    checksum_expected: Option<i64>,
    checksum_observed: Option<i64>,
    last_sequence_expected: Option<i64>,
    last_sequence_observed: Option<i64>,
    valid: bool,
}

#[derive(Debug, Clone)]
struct PhaseSummary {
    valid_all: bool,
    median_ops_per_sec: f64,
    mean_ops_per_sec: f64,
    min_ops_per_sec: f64,
    max_ops_per_sec: f64,
    stddev_ops_per_sec: f64,
    cv: f64,
}

#[derive(Debug, Clone)]
struct PhaseResult {
    phase: PhaseKind,
    runs: Vec<RoundRecord>,
    summary: PhaseSummary,
}

#[derive(Debug, Clone)]
struct BreakdownResult {
    mode: Mode,
    wait_strategy: WaitStrategyKind,
    event_layout: EventLayout,
    events_total: u64,
    buffer_size: usize,
    allow_wrap: bool,
    event_size_bytes: usize,
    estimated_ring_bytes: u64,
    phases: Vec<PhaseResult>,
    publish_vs_end_to_end_ratio: Option<f64>,
    publish_vs_end_to_end_delta_ns_per_event: Option<f64>,
}

impl CliConfig {
    fn from_args() -> Result<Self, String> {
        let mut args = env::args().skip(1);
        let mut mode = Mode::All;
        let mut wait_strategy = WaitStrategyKind::Yielding;
        let mut event_layout = EventLayout::Minimal;
        let mut events_total = 20_000_000_u64;
        let mut buffer_size = None;
        let mut allow_wrap = false;
        let mut warmup_rounds = 3usize;
        let mut measured_rounds = 5usize;
        let mut timeout_secs = DEFAULT_TIMEOUT_SECS;
        let mut output = None;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--mode" => {
                    mode = Mode::parse(&args.next().ok_or("missing value for --mode")?)?;
                }
                "--wait-strategy" => {
                    wait_strategy = WaitStrategyKind::parse(
                        &args.next().ok_or("missing value for --wait-strategy")?,
                    )?;
                }
                "--event-layout" => {
                    event_layout = EventLayout::parse(
                        &args.next().ok_or("missing value for --event-layout")?,
                    )?;
                }
                "--events-total" => {
                    events_total =
                        parse_u64(&args.next().ok_or("missing value for --events-total")?)?;
                }
                "--buffer-size" => {
                    buffer_size = Some(parse_usize(
                        &args.next().ok_or("missing value for --buffer-size")?,
                    )?);
                }
                "--allow-wrap" => {
                    allow_wrap = true;
                }
                "--warmup-rounds" => {
                    warmup_rounds =
                        parse_usize(&args.next().ok_or("missing value for --warmup-rounds")?)?;
                }
                "--measured-rounds" => {
                    measured_rounds =
                        parse_usize(&args.next().ok_or("missing value for --measured-rounds")?)?;
                }
                "--timeout-secs" => {
                    timeout_secs =
                        parse_u64(&args.next().ok_or("missing value for --timeout-secs")?)?;
                }
                "--output" => {
                    output = Some(PathBuf::from(
                        args.next().ok_or("missing value for --output")?,
                    ));
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                _ => return Err(format!("unsupported argument: {arg}")),
            }
        }

        Ok(Self {
            mode,
            wait_strategy,
            event_layout,
            events_total,
            buffer_size,
            allow_wrap,
            warmup_rounds,
            measured_rounds,
            timeout_secs,
            output,
        })
    }
}

impl TryFrom<CliConfig> for ResolvedConfig {
    type Error = String;

    fn try_from(value: CliConfig) -> Result<Self, Self::Error> {
        if value.events_total == 0 {
            return Err(String::from("events_total must be > 0"));
        }
        if value.event_layout != EventLayout::Minimal && value.mode != Mode::EndToEnd {
            return Err(String::from(
                "non-minimal event layouts are only supported with --mode end-to-end",
            ));
        }
        if value.measured_rounds == 0 {
            return Err(String::from("measured_rounds must be > 0"));
        }
        let default_buffer = value
            .events_total
            .checked_next_power_of_two()
            .ok_or("events_total too large to round up to power of two")?;
        let buffer_size = value.buffer_size.unwrap_or(default_buffer as usize);
        if buffer_size == 0 || !buffer_size.is_power_of_two() {
            return Err(format!(
                "buffer size must be a non-zero power of two, got {buffer_size}"
            ));
        }
        if (buffer_size as u64) < value.events_total
            && !(value.allow_wrap && value.mode == Mode::EndToEnd)
        {
            return Err(format!(
                "buffer size must be >= events_total for no-wrap diagnostics unless --allow-wrap is set with --mode end-to-end; got buffer_size={buffer_size}, events_total={}",
                value.events_total,
            ));
        }

        Ok(Self {
            mode: value.mode,
            wait_strategy: value.wait_strategy,
            event_layout: value.event_layout,
            events_total: value.events_total,
            buffer_size,
            allow_wrap: value.allow_wrap,
            warmup_rounds: value.warmup_rounds,
            measured_rounds: value.measured_rounds,
            timeout_secs: value.timeout_secs,
            output: value.output,
        })
    }
}

struct DirectConsumerRuntime {
    ring_buffer: Arc<RingBuffer<MicroEvent>>,
    sequencer: Arc<SingleProducerSequencer>,
    barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready: Arc<AtomicBool>,
    consumer_handle: thread::JoinHandle<Result<(), String>>,
}

struct DirectComparisonRuntime {
    ring_buffer: Arc<RingBuffer<ComparisonPayloadEvent>>,
    sequencer: Arc<SingleProducerSequencer>,
    barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready: Arc<AtomicBool>,
    consumer_handle: thread::JoinHandle<Result<(), String>>,
}

struct DirectPaddedComparisonRuntime {
    ring_buffer: Arc<RingBuffer<ComparisonPadded64Event>>,
    sequencer: Arc<SingleProducerSequencer>,
    barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready: Arc<AtomicBool>,
    consumer_handle: thread::JoinHandle<Result<(), String>>,
}

struct DirectPadded128ComparisonRuntime {
    ring_buffer: Arc<RingBuffer<ComparisonPadded128Event>>,
    sequencer: Arc<SingleProducerSequencer>,
    barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready: Arc<AtomicBool>,
    consumer_handle: thread::JoinHandle<Result<(), String>>,
}

impl DirectConsumerRuntime {
    fn new<W>(buffer_size: usize, wait_strategy: W) -> Result<Self, String>
    where
        W: WaitStrategy + Send + Sync + Clone + 'static,
    {
        let ring_buffer = Arc::new(
            RingBuffer::new(buffer_size, DefaultEventFactory::<MicroEvent>::new())
                .map_err(|error| format!("failed to create ring buffer: {error:?}"))?,
        );
        let wait_strategy_arc: Arc<dyn WaitStrategy> = Arc::new(wait_strategy);
        let sequencer = Arc::new(SingleProducerSequencer::new(
            buffer_size,
            Arc::clone(&wait_strategy_arc),
        ));
        let consumer_sequence = Arc::new(Sequence::new_with_initial_value());
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_sequence));
        let barrier: Arc<dyn SequenceBarrier> = Arc::new(ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            wait_strategy_arc,
            Vec::new(),
            SequencerEnum::Single(Arc::clone(&sequencer)),
        ));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));
        let ready = Arc::new(AtomicBool::new(false));

        let consumer_handle = spawn_direct_consumer(
            Arc::clone(&ring_buffer),
            Arc::clone(&barrier),
            Arc::clone(&consumer_sequence),
            Arc::clone(&shutdown_flag),
            Arc::clone(&processed),
            Arc::clone(&checksum),
            Arc::clone(&ready),
        );

        Ok(Self {
            ring_buffer,
            sequencer,
            barrier,
            consumer_sequence,
            shutdown_flag,
            processed,
            checksum,
            ready,
            consumer_handle,
        })
    }

    fn shutdown(self) -> Result<(), String> {
        self.shutdown_flag.store(true, Ordering::Release);
        self.barrier.alert();
        self.consumer_handle
            .join()
            .map_err(|_| String::from("direct consumer thread panicked"))??;
        self.sequencer
            .remove_gating_sequence(Arc::clone(&self.consumer_sequence));
        Ok(())
    }
}

impl DirectComparisonRuntime {
    fn new<W>(buffer_size: usize, wait_strategy: W) -> Result<Self, String>
    where
        W: WaitStrategy + Send + Sync + Clone + 'static,
    {
        let ring_buffer = Arc::new(
            RingBuffer::new(
                buffer_size,
                DefaultEventFactory::<ComparisonPayloadEvent>::new(),
            )
            .map_err(|error| format!("failed to create comparison ring buffer: {error:?}"))?,
        );
        let wait_strategy_arc: Arc<dyn WaitStrategy> = Arc::new(wait_strategy);
        let sequencer = Arc::new(SingleProducerSequencer::new(
            buffer_size,
            Arc::clone(&wait_strategy_arc),
        ));
        let consumer_sequence = Arc::new(Sequence::new_with_initial_value());
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_sequence));
        let barrier: Arc<dyn SequenceBarrier> = Arc::new(ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            wait_strategy_arc,
            Vec::new(),
            SequencerEnum::Single(Arc::clone(&sequencer)),
        ));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));
        let ready = Arc::new(AtomicBool::new(false));

        let consumer_handle = spawn_direct_comparison_consumer(
            Arc::clone(&ring_buffer),
            Arc::clone(&barrier),
            Arc::clone(&consumer_sequence),
            Arc::clone(&shutdown_flag),
            Arc::clone(&processed),
            Arc::clone(&checksum),
            Arc::clone(&ready),
        );

        Ok(Self {
            ring_buffer,
            sequencer,
            barrier,
            consumer_sequence,
            shutdown_flag,
            processed,
            checksum,
            ready,
            consumer_handle,
        })
    }

    fn shutdown(self) -> Result<(), String> {
        self.shutdown_flag.store(true, Ordering::Release);
        self.barrier.alert();
        self.consumer_handle
            .join()
            .map_err(|_| String::from("comparison consumer thread panicked"))??;
        self.sequencer
            .remove_gating_sequence(Arc::clone(&self.consumer_sequence));
        Ok(())
    }
}

impl DirectPaddedComparisonRuntime {
    fn new<W>(buffer_size: usize, wait_strategy: W) -> Result<Self, String>
    where
        W: WaitStrategy + Send + Sync + Clone + 'static,
    {
        let ring_buffer = Arc::new(
            RingBuffer::new(
                buffer_size,
                DefaultEventFactory::<ComparisonPadded64Event>::new(),
            )
            .map_err(|error| {
                format!("failed to create padded comparison ring buffer: {error:?}")
            })?,
        );
        let wait_strategy_arc: Arc<dyn WaitStrategy> = Arc::new(wait_strategy);
        let sequencer = Arc::new(SingleProducerSequencer::new(
            buffer_size,
            Arc::clone(&wait_strategy_arc),
        ));
        let consumer_sequence = Arc::new(Sequence::new_with_initial_value());
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_sequence));
        let barrier: Arc<dyn SequenceBarrier> = Arc::new(ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            wait_strategy_arc,
            Vec::new(),
            SequencerEnum::Single(Arc::clone(&sequencer)),
        ));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));
        let ready = Arc::new(AtomicBool::new(false));

        let consumer_handle = spawn_direct_padded_comparison_consumer(
            Arc::clone(&ring_buffer),
            Arc::clone(&barrier),
            Arc::clone(&consumer_sequence),
            Arc::clone(&shutdown_flag),
            Arc::clone(&processed),
            Arc::clone(&checksum),
            Arc::clone(&ready),
        );

        Ok(Self {
            ring_buffer,
            sequencer,
            barrier,
            consumer_sequence,
            shutdown_flag,
            processed,
            checksum,
            ready,
            consumer_handle,
        })
    }

    fn shutdown(self) -> Result<(), String> {
        self.shutdown_flag.store(true, Ordering::Release);
        self.barrier.alert();
        self.consumer_handle
            .join()
            .map_err(|_| String::from("padded comparison consumer thread panicked"))??;
        self.sequencer
            .remove_gating_sequence(Arc::clone(&self.consumer_sequence));
        Ok(())
    }
}

impl DirectPadded128ComparisonRuntime {
    fn new<W>(buffer_size: usize, wait_strategy: W) -> Result<Self, String>
    where
        W: WaitStrategy + Send + Sync + Clone + 'static,
    {
        let ring_buffer = Arc::new(
            RingBuffer::new(
                buffer_size,
                DefaultEventFactory::<ComparisonPadded128Event>::new(),
            )
            .map_err(|error| {
                format!("failed to create padded-128 comparison ring buffer: {error:?}")
            })?,
        );
        let wait_strategy_arc: Arc<dyn WaitStrategy> = Arc::new(wait_strategy);
        let sequencer = Arc::new(SingleProducerSequencer::new(
            buffer_size,
            Arc::clone(&wait_strategy_arc),
        ));
        let consumer_sequence = Arc::new(Sequence::new_with_initial_value());
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_sequence));
        let barrier: Arc<dyn SequenceBarrier> = Arc::new(ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            wait_strategy_arc,
            Vec::new(),
            SequencerEnum::Single(Arc::clone(&sequencer)),
        ));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));
        let ready = Arc::new(AtomicBool::new(false));

        let consumer_handle = spawn_direct_padded128_comparison_consumer(
            Arc::clone(&ring_buffer),
            Arc::clone(&barrier),
            Arc::clone(&consumer_sequence),
            Arc::clone(&shutdown_flag),
            Arc::clone(&processed),
            Arc::clone(&checksum),
            Arc::clone(&ready),
        );

        Ok(Self {
            ring_buffer,
            sequencer,
            barrier,
            consumer_sequence,
            shutdown_flag,
            processed,
            checksum,
            ready,
            consumer_handle,
        })
    }

    fn shutdown(self) -> Result<(), String> {
        self.shutdown_flag.store(true, Ordering::Release);
        self.barrier.alert();
        self.consumer_handle
            .join()
            .map_err(|_| String::from("padded-128 comparison consumer thread panicked"))??;
        self.sequencer
            .remove_gating_sequence(Arc::clone(&self.consumer_sequence));
        Ok(())
    }
}

fn spawn_direct_consumer(
    ring_buffer: Arc<RingBuffer<MicroEvent>>,
    barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<(), String>> {
    thread::Builder::new()
        .name(String::from("unicast-breakdown-consumer"))
        .spawn(move || {
            ready.store(true, Ordering::Release);
            let mut next_sequence = 0_i64;

            loop {
                match barrier.wait_for_with_shutdown(next_sequence, &shutdown_flag) {
                    Ok(available_sequence) => {
                        if available_sequence < next_sequence {
                            continue;
                        }
                        let mut local_checksum = 0_i64;
                        let mut local_processed = 0_u64;
                        while next_sequence <= available_sequence {
                            let event = ring_buffer.get(next_sequence);
                            local_checksum = local_checksum.wrapping_add(event.value);
                            local_processed += 1;
                            next_sequence += 1;
                        }
                        checksum.fetch_add(local_checksum, Ordering::Relaxed);
                        processed.fetch_add(local_processed, Ordering::Release);
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
                            "consumer wait failed at sequence {next_sequence}: {error:?}"
                        ));
                    }
                }
            }

            Ok(())
        })
        .map_err(|error| format!("failed to spawn direct consumer: {error}"))
        .expect("direct consumer thread spawn must succeed")
}

fn spawn_direct_comparison_consumer(
    ring_buffer: Arc<RingBuffer<ComparisonPayloadEvent>>,
    barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<(), String>> {
    thread::Builder::new()
        .name(String::from("unicast-breakdown-comparison-consumer"))
        .spawn(move || {
            ready.store(true, Ordering::Release);
            let mut next_sequence = 0_i64;

            loop {
                match barrier.wait_for_with_shutdown(next_sequence, &shutdown_flag) {
                    Ok(available_sequence) => {
                        if available_sequence < next_sequence {
                            continue;
                        }
                        let mut local_checksum = 0_i64;
                        let mut local_processed = 0_u64;
                        while next_sequence <= available_sequence {
                            let event = ring_buffer.get(next_sequence);
                            local_checksum = local_checksum.wrapping_add(event.value);
                            local_processed += 1;
                            next_sequence += 1;
                        }
                        checksum.fetch_add(local_checksum, Ordering::Relaxed);
                        processed.fetch_add(local_processed, Ordering::Release);
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
                            "comparison consumer wait failed at sequence {next_sequence}: {error:?}"
                        ));
                    }
                }
            }

            Ok(())
        })
        .map_err(|error| format!("failed to spawn comparison consumer: {error}"))
        .expect("comparison consumer thread spawn must succeed")
}

fn spawn_direct_padded_comparison_consumer(
    ring_buffer: Arc<RingBuffer<ComparisonPadded64Event>>,
    barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<(), String>> {
    thread::Builder::new()
        .name(String::from("unicast-breakdown-padded-comparison-consumer"))
        .spawn(move || {
            ready.store(true, Ordering::Release);
            let mut next_sequence = 0_i64;

            loop {
                match barrier.wait_for_with_shutdown(next_sequence, &shutdown_flag) {
                    Ok(available_sequence) => {
                        if available_sequence < next_sequence {
                            continue;
                        }
                        let mut local_checksum = 0_i64;
                        let mut local_processed = 0_u64;
                        while next_sequence <= available_sequence {
                            let event = ring_buffer.get(next_sequence);
                            local_checksum = local_checksum.wrapping_add(event.value);
                            local_processed += 1;
                            next_sequence += 1;
                        }
                        checksum.fetch_add(local_checksum, Ordering::Relaxed);
                        processed.fetch_add(local_processed, Ordering::Release);
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
                            "padded comparison consumer wait failed at sequence {next_sequence}: {error:?}"
                        ));
                    }
                }
            }

            Ok(())
        })
        .map_err(|error| format!("failed to spawn padded comparison consumer: {error}"))
        .expect("padded comparison consumer thread spawn must succeed")
}

fn spawn_direct_padded128_comparison_consumer(
    ring_buffer: Arc<RingBuffer<ComparisonPadded128Event>>,
    barrier: Arc<dyn SequenceBarrier>,
    consumer_sequence: Arc<Sequence>,
    shutdown_flag: Arc<AtomicBool>,
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<(), String>> {
    thread::Builder::new()
        .name(String::from(
            "unicast-breakdown-padded128-comparison-consumer",
        ))
        .spawn(move || {
            ready.store(true, Ordering::Release);
            let mut next_sequence = 0_i64;

            loop {
                match barrier.wait_for_with_shutdown(next_sequence, &shutdown_flag) {
                    Ok(available_sequence) => {
                        if available_sequence < next_sequence {
                            continue;
                        }
                        let mut local_checksum = 0_i64;
                        let mut local_processed = 0_u64;
                        while next_sequence <= available_sequence {
                            let event = ring_buffer.get(next_sequence);
                            local_checksum = local_checksum.wrapping_add(event.value);
                            local_processed += 1;
                            next_sequence += 1;
                        }
                        checksum.fetch_add(local_checksum, Ordering::Relaxed);
                        processed.fetch_add(local_processed, Ordering::Release);
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
                            "padded-128 comparison consumer wait failed at sequence {next_sequence}: {error:?}"
                        ));
                    }
                }
            }

            Ok(())
        })
        .map_err(|error| format!("failed to spawn padded-128 comparison consumer: {error}"))
        .expect("padded-128 comparison consumer thread spawn must succeed")
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run_main() -> Result<(), String> {
    let config = ResolvedConfig::try_from(CliConfig::from_args()?)?;
    let result = run_breakdown(&config)?;
    let json = render_json(&result);

    if let Some(path) = &config.output {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create output directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        std::fs::write(path, &json)
            .map_err(|error| format!("failed to write output file {}: {error}", path.display()))?;
    }

    println!("{json}");
    Ok(())
}

fn run_breakdown(config: &ResolvedConfig) -> Result<BreakdownResult, String> {
    let mut phases = Vec::new();
    for phase in config.mode.phases() {
        phases.push(run_phase(config, *phase)?);
    }

    let publish_summary = phases
        .iter()
        .find(|phase| phase.phase == PhaseKind::ClaimWritePublish)
        .map(|phase| phase.summary.median_ops_per_sec);
    let end_to_end_summary = phases
        .iter()
        .find(|phase| phase.phase == PhaseKind::EndToEnd)
        .map(|phase| phase.summary.median_ops_per_sec);

    let (publish_vs_end_to_end_ratio, publish_vs_end_to_end_delta_ns_per_event) =
        match (publish_summary, end_to_end_summary) {
            (Some(publish), Some(end_to_end)) if end_to_end > 0.0 && publish > 0.0 => {
                let ratio = publish / end_to_end;
                let delta = (1_000_000_000.0 / end_to_end) - (1_000_000_000.0 / publish);
                (Some(ratio), Some(delta))
            }
            _ => (None, None),
        };

    Ok(BreakdownResult {
        mode: config.mode,
        wait_strategy: config.wait_strategy,
        event_layout: config.event_layout,
        events_total: config.events_total,
        buffer_size: config.buffer_size,
        allow_wrap: config.allow_wrap,
        event_size_bytes: config.event_layout.event_size_bytes(),
        estimated_ring_bytes: config.buffer_size as u64
            * config.event_layout.event_size_bytes() as u64,
        phases,
        publish_vs_end_to_end_ratio,
        publish_vs_end_to_end_delta_ns_per_event,
    })
}

fn run_phase(config: &ResolvedConfig, phase: PhaseKind) -> Result<PhaseResult, String> {
    let total_rounds = config.warmup_rounds + config.measured_rounds;
    let mut runs = Vec::with_capacity(total_rounds);
    for round in 0..total_rounds {
        let round_phase = if round < config.warmup_rounds {
            RoundPhase::Warmup
        } else {
            RoundPhase::Measured
        };
        let round_record = match config.wait_strategy {
            WaitStrategyKind::BusySpin => run_phase_with(
                config,
                phase,
                BusySpinWaitStrategy::new(),
                round + 1,
                round_phase,
            )?,
            WaitStrategyKind::Yielding => run_phase_with(
                config,
                phase,
                YieldingWaitStrategy::new(),
                round + 1,
                round_phase,
            )?,
        };
        runs.push(round_record);
    }

    Ok(PhaseResult {
        phase,
        summary: summarize(&runs),
        runs,
    })
}

fn run_phase_with<W>(
    config: &ResolvedConfig,
    phase: PhaseKind,
    wait_strategy: W,
    round_index: usize,
    round_phase: RoundPhase,
) -> Result<RoundRecord, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    match phase {
        PhaseKind::Baseline => Ok(run_baseline(config, round_index, round_phase)),
        PhaseKind::ClaimOnly => run_claim_only(config, wait_strategy, round_index, round_phase),
        PhaseKind::ClaimWrite => run_claim_write(config, wait_strategy, round_index, round_phase),
        PhaseKind::ClaimWritePublish => {
            run_claim_write_publish(config, wait_strategy, round_index, round_phase)
        }
        PhaseKind::EndToEnd => run_end_to_end(config, wait_strategy, round_index, round_phase),
    }
}

fn run_baseline(
    config: &ResolvedConfig,
    round_index: usize,
    round_phase: RoundPhase,
) -> RoundRecord {
    let expected = arithmetic_checksum(config.events_total);
    let start = Instant::now();
    let mut checksum = 0_i64;
    for value in 0..config.events_total {
        checksum = checksum.wrapping_add(std::hint::black_box(value as i64));
    }
    let elapsed = start.elapsed().as_nanos();

    RoundRecord {
        round_index,
        phase: round_phase,
        elapsed_ns: elapsed,
        ops_per_sec: ops_per_sec(config.events_total, elapsed),
        events_expected: Some(config.events_total),
        events_observed: Some(config.events_total),
        checksum_expected: Some(expected),
        checksum_observed: Some(checksum),
        last_sequence_expected: None,
        last_sequence_observed: None,
        valid: checksum == expected,
    }
}

fn run_claim_only<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    round_index: usize,
    round_phase: RoundPhase,
) -> Result<RoundRecord, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let sequencer = SingleProducerSequencer::new(
        config.buffer_size,
        Arc::new(wait_strategy) as Arc<dyn WaitStrategy>,
    );

    let start = Instant::now();
    let mut last_sequence = -1_i64;
    for _ in 0..config.events_total {
        last_sequence = sequencer
            .next()
            .map_err(|error| format!("claim-only next failed: {error:?}"))?;
        std::hint::black_box(last_sequence);
    }
    let elapsed = start.elapsed().as_nanos();
    let expected = config.events_total as i64 - 1;

    Ok(RoundRecord {
        round_index,
        phase: round_phase,
        elapsed_ns: elapsed,
        ops_per_sec: ops_per_sec(config.events_total, elapsed),
        events_expected: Some(config.events_total),
        events_observed: Some(config.events_total),
        checksum_expected: None,
        checksum_observed: None,
        last_sequence_expected: Some(expected),
        last_sequence_observed: Some(last_sequence),
        valid: last_sequence == expected,
    })
}

fn run_claim_write<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    round_index: usize,
    round_phase: RoundPhase,
) -> Result<RoundRecord, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let ring_buffer = Arc::new(
        RingBuffer::new(config.buffer_size, DefaultEventFactory::<MicroEvent>::new())
            .map_err(|error| format!("claim-write ring buffer failed: {error:?}"))?,
    );
    let sequencer = SingleProducerSequencer::new(
        config.buffer_size,
        Arc::new(wait_strategy) as Arc<dyn WaitStrategy>,
    );

    let start = Instant::now();
    for value in 0..config.events_total {
        let sequence = sequencer
            .next()
            .map_err(|error| format!("claim-write next failed: {error:?}"))?;
        let event = unsafe { &mut *ring_buffer.get_mut_unchecked(sequence) };
        event.value = value as i64;
    }
    let elapsed = start.elapsed().as_nanos();

    let checksum = checksum_ring_buffer(&ring_buffer, config.events_total);
    let expected = arithmetic_checksum(config.events_total);

    Ok(RoundRecord {
        round_index,
        phase: round_phase,
        elapsed_ns: elapsed,
        ops_per_sec: ops_per_sec(config.events_total, elapsed),
        events_expected: Some(config.events_total),
        events_observed: Some(config.events_total),
        checksum_expected: Some(expected),
        checksum_observed: Some(checksum),
        last_sequence_expected: Some(config.events_total as i64 - 1),
        last_sequence_observed: Some(sequencer.get_cursor().get()),
        valid: checksum == expected,
    })
}

fn run_claim_write_publish<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    round_index: usize,
    round_phase: RoundPhase,
) -> Result<RoundRecord, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let ring_buffer = Arc::new(
        RingBuffer::new(config.buffer_size, DefaultEventFactory::<MicroEvent>::new())
            .map_err(|error| format!("claim-write-publish ring buffer failed: {error:?}"))?,
    );
    let sequencer = SingleProducerSequencer::new(
        config.buffer_size,
        Arc::new(wait_strategy) as Arc<dyn WaitStrategy>,
    );

    let start = Instant::now();
    for value in 0..config.events_total {
        let sequence = sequencer
            .next()
            .map_err(|error| format!("claim-write-publish next failed: {error:?}"))?;
        let event = unsafe { &mut *ring_buffer.get_mut_unchecked(sequence) };
        event.value = value as i64;
        sequencer.publish(sequence);
    }
    let elapsed = start.elapsed().as_nanos();

    let checksum = checksum_ring_buffer(&ring_buffer, config.events_total);
    let expected = arithmetic_checksum(config.events_total);
    let cursor = sequencer.get_cursor().get();

    Ok(RoundRecord {
        round_index,
        phase: round_phase,
        elapsed_ns: elapsed,
        ops_per_sec: ops_per_sec(config.events_total, elapsed),
        events_expected: Some(config.events_total),
        events_observed: Some(config.events_total),
        checksum_expected: Some(expected),
        checksum_observed: Some(checksum),
        last_sequence_expected: Some(config.events_total as i64 - 1),
        last_sequence_observed: Some(cursor),
        valid: checksum == expected && cursor == config.events_total as i64 - 1,
    })
}

fn run_end_to_end<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    round_index: usize,
    round_phase: RoundPhase,
) -> Result<RoundRecord, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    match config.event_layout {
        EventLayout::Minimal => {
            run_end_to_end_minimal(config, wait_strategy, round_index, round_phase)
        }
        EventLayout::Comparison => {
            run_end_to_end_comparison(config, wait_strategy, round_index, round_phase)
        }
        EventLayout::ComparisonPadded64 => {
            run_end_to_end_comparison_padded64(config, wait_strategy, round_index, round_phase)
        }
        EventLayout::ComparisonPadded128 => {
            run_end_to_end_comparison_padded128(config, wait_strategy, round_index, round_phase)
        }
    }
}

fn run_end_to_end_minimal<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    round_index: usize,
    round_phase: RoundPhase,
) -> Result<RoundRecord, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let runtime = DirectConsumerRuntime::new(config.buffer_size, wait_strategy)?;
    let run_result = (|| -> Result<RoundRecord, String> {
        wait_for_ready(&runtime.ready, config.timeout_secs)?;

        let start = Instant::now();
        for value in 0..config.events_total {
            let sequence = runtime
                .sequencer
                .next()
                .map_err(|error| format!("end-to-end next failed: {error:?}"))?;
            let event = unsafe { &mut *runtime.ring_buffer.get_mut_unchecked(sequence) };
            event.value = value as i64;
            runtime.sequencer.publish(sequence);
        }
        wait_for_processed(&runtime.processed, config.events_total, config.timeout_secs)?;
        let elapsed = start.elapsed().as_nanos();

        let expected = arithmetic_checksum(config.events_total);
        let processed = runtime.processed.load(Ordering::Acquire);
        let checksum = runtime.checksum.load(Ordering::Acquire);

        Ok(RoundRecord {
            round_index,
            phase: round_phase,
            elapsed_ns: elapsed,
            ops_per_sec: ops_per_sec(config.events_total, elapsed),
            events_expected: Some(config.events_total),
            events_observed: Some(processed),
            checksum_expected: Some(expected),
            checksum_observed: Some(checksum),
            last_sequence_expected: Some(config.events_total as i64 - 1),
            last_sequence_observed: Some(runtime.sequencer.get_cursor().get()),
            valid: processed == config.events_total && checksum == expected,
        })
    })();

    let shutdown_result = runtime.shutdown();
    let round_record = run_result?;
    shutdown_result?;
    Ok(round_record)
}

fn run_end_to_end_comparison<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    round_index: usize,
    round_phase: RoundPhase,
) -> Result<RoundRecord, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let runtime = DirectComparisonRuntime::new(config.buffer_size, wait_strategy)?;
    let run_result = (|| -> Result<RoundRecord, String> {
        wait_for_ready(&runtime.ready, config.timeout_secs)?;

        let start = Instant::now();
        for value in 0..config.events_total {
            let sequence = runtime
                .sequencer
                .next()
                .map_err(|error| format!("comparison end-to-end next failed: {error:?}"))?;
            let event = unsafe { &mut *runtime.ring_buffer.get_mut_unchecked(sequence) };
            populate_comparison_event(event, value as i64);
            runtime.sequencer.publish(sequence);
        }
        wait_for_processed(&runtime.processed, config.events_total, config.timeout_secs)?;
        let elapsed = start.elapsed().as_nanos();

        let expected = arithmetic_checksum(config.events_total);
        let processed = runtime.processed.load(Ordering::Acquire);
        let checksum = runtime.checksum.load(Ordering::Acquire);

        Ok(RoundRecord {
            round_index,
            phase: round_phase,
            elapsed_ns: elapsed,
            ops_per_sec: ops_per_sec(config.events_total, elapsed),
            events_expected: Some(config.events_total),
            events_observed: Some(processed),
            checksum_expected: Some(expected),
            checksum_observed: Some(checksum),
            last_sequence_expected: Some(config.events_total as i64 - 1),
            last_sequence_observed: Some(runtime.sequencer.get_cursor().get()),
            valid: processed == config.events_total && checksum == expected,
        })
    })();

    let shutdown_result = runtime.shutdown();
    let round_record = run_result?;
    shutdown_result?;
    Ok(round_record)
}

fn run_end_to_end_comparison_padded64<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    round_index: usize,
    round_phase: RoundPhase,
) -> Result<RoundRecord, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let runtime = DirectPaddedComparisonRuntime::new(config.buffer_size, wait_strategy)?;
    let run_result = (|| -> Result<RoundRecord, String> {
        wait_for_ready(&runtime.ready, config.timeout_secs)?;

        let start = Instant::now();
        for value in 0..config.events_total {
            let sequence = runtime
                .sequencer
                .next()
                .map_err(|error| format!("padded comparison end-to-end next failed: {error:?}"))?;
            let event = unsafe { &mut *runtime.ring_buffer.get_mut_unchecked(sequence) };
            populate_padded_comparison_event(event, value as i64);
            runtime.sequencer.publish(sequence);
        }
        wait_for_processed(&runtime.processed, config.events_total, config.timeout_secs)?;
        let elapsed = start.elapsed().as_nanos();

        let expected = arithmetic_checksum(config.events_total);
        let processed = runtime.processed.load(Ordering::Acquire);
        let checksum = runtime.checksum.load(Ordering::Acquire);

        Ok(RoundRecord {
            round_index,
            phase: round_phase,
            elapsed_ns: elapsed,
            ops_per_sec: ops_per_sec(config.events_total, elapsed),
            events_expected: Some(config.events_total),
            events_observed: Some(processed),
            checksum_expected: Some(expected),
            checksum_observed: Some(checksum),
            last_sequence_expected: Some(config.events_total as i64 - 1),
            last_sequence_observed: Some(runtime.sequencer.get_cursor().get()),
            valid: processed == config.events_total && checksum == expected,
        })
    })();

    let shutdown_result = runtime.shutdown();
    let round_record = run_result?;
    shutdown_result?;
    Ok(round_record)
}

fn run_end_to_end_comparison_padded128<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    round_index: usize,
    round_phase: RoundPhase,
) -> Result<RoundRecord, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    let runtime = DirectPadded128ComparisonRuntime::new(config.buffer_size, wait_strategy)?;
    let run_result = (|| -> Result<RoundRecord, String> {
        wait_for_ready(&runtime.ready, config.timeout_secs)?;

        let start = Instant::now();
        for value in 0..config.events_total {
            let sequence = runtime.sequencer.next().map_err(|error| {
                format!("padded-128 comparison end-to-end next failed: {error:?}")
            })?;
            let event = unsafe { &mut *runtime.ring_buffer.get_mut_unchecked(sequence) };
            populate_padded128_comparison_event(event, value as i64);
            runtime.sequencer.publish(sequence);
        }
        wait_for_processed(&runtime.processed, config.events_total, config.timeout_secs)?;
        let elapsed = start.elapsed().as_nanos();

        let expected = arithmetic_checksum(config.events_total);
        let processed = runtime.processed.load(Ordering::Acquire);
        let checksum = runtime.checksum.load(Ordering::Acquire);

        Ok(RoundRecord {
            round_index,
            phase: round_phase,
            elapsed_ns: elapsed,
            ops_per_sec: ops_per_sec(config.events_total, elapsed),
            events_expected: Some(config.events_total),
            events_observed: Some(processed),
            checksum_expected: Some(expected),
            checksum_observed: Some(checksum),
            last_sequence_expected: Some(config.events_total as i64 - 1),
            last_sequence_observed: Some(runtime.sequencer.get_cursor().get()),
            valid: processed == config.events_total && checksum == expected,
        })
    })();

    let shutdown_result = runtime.shutdown();
    let round_record = run_result?;
    shutdown_result?;
    Ok(round_record)
}

fn wait_for_ready(ready: &AtomicBool, timeout_secs: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while !ready.load(Ordering::Acquire) {
        if Instant::now() >= deadline {
            return Err(String::from("consumer did not become ready before timeout"));
        }
        thread::yield_now();
    }
    Ok(())
}

fn wait_for_processed(
    processed: &AtomicU64,
    expected: u64,
    timeout_secs: u64,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while processed.load(Ordering::Acquire) != expected {
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for processed count: expected {expected}, observed {}",
                processed.load(Ordering::Acquire)
            ));
        }
        thread::yield_now();
    }
    Ok(())
}

fn checksum_ring_buffer(ring_buffer: &RingBuffer<MicroEvent>, events_total: u64) -> i64 {
    let mut checksum = 0_i64;
    for sequence in 0..events_total {
        checksum = checksum.wrapping_add(ring_buffer.get(sequence as i64).value);
    }
    checksum
}

fn arithmetic_checksum(events_total: u64) -> i64 {
    ((events_total - 1) as i64).wrapping_mul(events_total as i64) / 2
}

fn populate_comparison_event(event: &mut ComparisonPayloadEvent, value: i64) {
    event.value = value;
    event.stage1_value = 0;
    event.stage2_value = 0;
    event.stage3_value = 0;
}

fn populate_padded_comparison_event(event: &mut ComparisonPadded64Event, value: i64) {
    event.value = value;
    event.stage1_value = 0;
    event.stage2_value = 0;
    event.stage3_value = 0;
}

fn populate_padded128_comparison_event(event: &mut ComparisonPadded128Event, value: i64) {
    event.value = value;
    event.stage1_value = 0;
    event.stage2_value = 0;
    event.stage3_value = 0;
}

fn summarize(runs: &[RoundRecord]) -> PhaseSummary {
    let mut measured = runs
        .iter()
        .filter(|run| run.phase == RoundPhase::Measured)
        .map(|run| run.ops_per_sec)
        .collect::<Vec<_>>();
    measured.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));

    let len = measured.len();
    let median = if len == 0 {
        0.0
    } else if len % 2 == 1 {
        measured[len / 2]
    } else {
        (measured[len / 2 - 1] + measured[len / 2]) / 2.0
    };
    let mean = if len == 0 {
        0.0
    } else {
        measured.iter().sum::<f64>() / len as f64
    };
    let variance = if len <= 1 {
        0.0
    } else {
        measured
            .iter()
            .map(|value| {
                let delta = value - mean;
                delta * delta
            })
            .sum::<f64>()
            / len as f64
    };
    let stddev = variance.sqrt();
    let cv = if mean == 0.0 { 0.0 } else { stddev / mean };

    PhaseSummary {
        valid_all: runs.iter().all(|run| run.valid),
        median_ops_per_sec: median,
        mean_ops_per_sec: mean,
        min_ops_per_sec: measured.first().copied().unwrap_or(0.0),
        max_ops_per_sec: measured.last().copied().unwrap_or(0.0),
        stddev_ops_per_sec: stddev,
        cv,
    }
}

fn ops_per_sec(events_total: u64, elapsed_ns: u128) -> f64 {
    if elapsed_ns == 0 {
        return 0.0;
    }
    events_total as f64 / (elapsed_ns as f64 / 1_000_000_000.0)
}

#[allow(clippy::too_many_lines)]
fn render_json(result: &BreakdownResult) -> String {
    let mut output = String::new();
    output.push_str("{\n");
    push_kv(&mut output, 2, "mode", result.mode.as_str(), true);
    push_kv(
        &mut output,
        2,
        "wait_strategy",
        result.wait_strategy.as_str(),
        true,
    );
    push_kv(
        &mut output,
        2,
        "event_layout",
        result.event_layout.as_str(),
        true,
    );
    push_kv_number(&mut output, 2, "events_total", result.events_total, true);
    push_kv_number(&mut output, 2, "buffer_size", result.buffer_size, true);
    push_kv_bool(&mut output, 2, "allow_wrap", result.allow_wrap, true);
    push_kv_number(
        &mut output,
        2,
        "event_size_bytes",
        result.event_size_bytes,
        true,
    );
    push_kv_number(
        &mut output,
        2,
        "estimated_ring_bytes",
        result.estimated_ring_bytes,
        true,
    );
    if let Some(ratio) = result.publish_vs_end_to_end_ratio {
        push_kv_float(&mut output, 2, "publish_vs_end_to_end_ratio", ratio, true);
    }
    if let Some(delta) = result.publish_vs_end_to_end_delta_ns_per_event {
        push_kv_float(
            &mut output,
            2,
            "publish_vs_end_to_end_delta_ns_per_event",
            delta,
            true,
        );
    }
    output.push_str("  \"phases\": [\n");
    for (phase_index, phase) in result.phases.iter().enumerate() {
        output.push_str("    {\n");
        push_kv(&mut output, 6, "phase", phase.phase.as_str(), true);
        output.push_str("      \"runs\": [\n");
        for (run_index, run) in phase.runs.iter().enumerate() {
            output.push_str("        {\n");
            push_kv_number(&mut output, 10, "round_index", run.round_index, true);
            push_kv(&mut output, 10, "phase", run.phase.as_str(), true);
            push_kv_number(&mut output, 10, "elapsed_ns", run.elapsed_ns, true);
            push_kv_float(&mut output, 10, "ops_per_sec", run.ops_per_sec, true);
            push_opt_u64(
                &mut output,
                10,
                "events_expected",
                run.events_expected,
                true,
            );
            push_opt_u64(
                &mut output,
                10,
                "events_observed",
                run.events_observed,
                true,
            );
            push_opt_i64(
                &mut output,
                10,
                "checksum_expected",
                run.checksum_expected,
                true,
            );
            push_opt_i64(
                &mut output,
                10,
                "checksum_observed",
                run.checksum_observed,
                true,
            );
            push_opt_i64(
                &mut output,
                10,
                "last_sequence_expected",
                run.last_sequence_expected,
                true,
            );
            push_opt_i64(
                &mut output,
                10,
                "last_sequence_observed",
                run.last_sequence_observed,
                true,
            );
            push_kv_bool(&mut output, 10, "valid", run.valid, false);
            output.push_str("        }");
            if run_index + 1 != phase.runs.len() {
                output.push(',');
            }
            output.push('\n');
        }
        output.push_str("      ],\n");
        output.push_str("      \"summary\": {\n");
        push_kv_bool(&mut output, 8, "valid_all", phase.summary.valid_all, true);
        push_kv_float(
            &mut output,
            8,
            "median_ops_per_sec",
            phase.summary.median_ops_per_sec,
            true,
        );
        push_kv_float(
            &mut output,
            8,
            "mean_ops_per_sec",
            phase.summary.mean_ops_per_sec,
            true,
        );
        push_kv_float(
            &mut output,
            8,
            "min_ops_per_sec",
            phase.summary.min_ops_per_sec,
            true,
        );
        push_kv_float(
            &mut output,
            8,
            "max_ops_per_sec",
            phase.summary.max_ops_per_sec,
            true,
        );
        push_kv_float(
            &mut output,
            8,
            "stddev_ops_per_sec",
            phase.summary.stddev_ops_per_sec,
            true,
        );
        push_kv_float(&mut output, 8, "cv", phase.summary.cv, false);
        output.push_str("      }\n");
        output.push_str("    }");
        if phase_index + 1 != result.phases.len() {
            output.push(',');
        }
        output.push('\n');
    }
    output.push_str("  ]\n");
    output.push_str("}\n");
    output
}

fn push_kv(output: &mut String, indent: usize, key: &str, value: &str, trailing_comma: bool) {
    let _ = write!(
        output,
        "{:indent$}\"{key}\": \"{value}\"",
        "",
        indent = indent
    );
    if trailing_comma {
        output.push(',');
    }
    output.push('\n');
}

fn push_kv_number<T: std::fmt::Display>(
    output: &mut String,
    indent: usize,
    key: &str,
    value: T,
    trailing_comma: bool,
) {
    let _ = write!(output, "{:indent$}\"{key}\": {value}", "", indent = indent);
    if trailing_comma {
        output.push(',');
    }
    output.push('\n');
}

fn push_kv_float(output: &mut String, indent: usize, key: &str, value: f64, trailing_comma: bool) {
    let _ = write!(
        output,
        "{:indent$}\"{key}\": {:.6}",
        "",
        value,
        indent = indent
    );
    if trailing_comma {
        output.push(',');
    }
    output.push('\n');
}

fn push_kv_bool(output: &mut String, indent: usize, key: &str, value: bool, trailing_comma: bool) {
    let _ = write!(
        output,
        "{:indent$}\"{key}\": {}",
        "",
        value,
        indent = indent
    );
    if trailing_comma {
        output.push(',');
    }
    output.push('\n');
}

fn push_opt_u64(
    output: &mut String,
    indent: usize,
    key: &str,
    value: Option<u64>,
    trailing_comma: bool,
) {
    match value {
        Some(value) => push_kv_number(output, indent, key, value, trailing_comma),
        None => push_null(output, indent, key, trailing_comma),
    }
}

fn push_opt_i64(
    output: &mut String,
    indent: usize,
    key: &str,
    value: Option<i64>,
    trailing_comma: bool,
) {
    match value {
        Some(value) => push_kv_number(output, indent, key, value, trailing_comma),
        None => push_null(output, indent, key, trailing_comma),
    }
}

fn push_null(output: &mut String, indent: usize, key: &str, trailing_comma: bool) {
    let _ = write!(output, "{:indent$}\"{key}\": null", "", indent = indent);
    if trailing_comma {
        output.push(',');
    }
    output.push('\n');
}

fn parse_u64(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|error| format!("failed to parse integer '{value}': {error}"))
}

fn parse_usize(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|error| format!("failed to parse integer '{value}': {error}"))
}

fn print_help() {
    println!("Usage: cargo run --release --bin unicast_breakdown -- [options]");
    println!();
    println!("Options:");
    println!("  --mode <all|baseline|claim-only|claim-write|claim-write-publish|end-to-end>");
    println!("  --wait-strategy <yielding|busy-spin>");
    println!(
        "  --event-layout <minimal|comparison|comparison-padded64|comparison-padded128>  non-minimal layouts only valid with --mode end-to-end"
    );
    println!("  --events-total <u64>");
    println!("  --buffer-size <usize>          default: next power of two >= events_total");
    println!("  --allow-wrap                   only valid with --mode end-to-end");
    println!("  --warmup-rounds <usize>        default: 3");
    println!("  --measured-rounds <usize>      default: 5");
    println!("  --timeout-secs <u64>           default: 300");
    println!("  --output <path>");
}
