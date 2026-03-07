#![allow(
    missing_docs,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use badbatch::disruptor::{
    build_multi_producer, build_single_producer, BusySpinWaitStrategy, EventHandler, Producer,
    Result as DisruptorResult, WaitStrategy, YieldingWaitStrategy,
};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
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

#[derive(Debug, Clone)]
struct CliConfig {
    scenario: Scenario,
    wait_strategy: Option<WaitStrategyKind>,
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

        Ok(Self {
            scenario,
            wait_strategy,
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
    measurement_kind: String,
    buffer_size: usize,
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

struct SummingHandler {
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready_count: Arc<AtomicU64>,
}

impl EventHandler<ComparisonEvent> for SummingHandler {
    fn on_event(
        &mut self,
        event: &mut ComparisonEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let value = event.value;
        add_wrapping_i64(&self.checksum, value);
        self.processed.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

struct PipelineStage1Handler {
    ready_count: Arc<AtomicU64>,
}

impl EventHandler<ComparisonEvent> for PipelineStage1Handler {
    fn on_event(
        &mut self,
        event: &mut ComparisonEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        event.stage1_value = event.value.wrapping_add(1);
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

struct PipelineStage2Handler {
    ready_count: Arc<AtomicU64>,
}

impl EventHandler<ComparisonEvent> for PipelineStage2Handler {
    fn on_event(
        &mut self,
        event: &mut ComparisonEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        event.stage2_value = event.stage1_value.wrapping_add(3);
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

struct PipelineStage3Handler {
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready_count: Arc<AtomicU64>,
}

impl EventHandler<ComparisonEvent> for PipelineStage3Handler {
    fn on_event(
        &mut self,
        event: &mut ComparisonEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        event.stage3_value = event.stage2_value.wrapping_add(7);
        let value = event.stage3_value;
        add_wrapping_i64(&self.checksum, value);
        self.processed.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
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
    let expected_checksum = arithmetic_checksum(config.events_total);
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));
        let handler = SummingHandler {
            processed: Arc::clone(&processed),
            checksum: Arc::clone(&checksum),
            ready_count: Arc::clone(&ready_count),
        };

        let mut disruptor = build_single_producer(
            config.buffer_size,
            ComparisonEvent::default,
            wait_strategy.clone(),
        )
        .handle_events_with_handler(handler)
        .build();

        wait_for_ready(&ready_count, 1, "unicast consumer")?;

        let start = Instant::now();
        for value in 0..config.events_total {
            let value = value as i64;
            disruptor.publish(move |event| {
                event.value = value;
                event.stage1_value = 0;
                event.stage2_value = 0;
                event.stage3_value = 0;
            });
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

    Ok(build_result(config, wait_strategy_kind, runs))
}

fn run_unicast_batch_with<W>(
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
        let handler = SummingHandler {
            processed: Arc::clone(&processed),
            checksum: Arc::clone(&checksum),
            ready_count: Arc::clone(&ready_count),
        };

        let mut disruptor = build_single_producer(
            config.buffer_size,
            ComparisonEvent::default,
            wait_strategy.clone(),
        )
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
                    event.value = value;
                    event.stage1_value = 0;
                    event.stage2_value = 0;
                    event.stage3_value = 0;
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

    Ok(build_result(config, wait_strategy_kind, runs))
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
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));

        let mut disruptor = build_multi_producer(
            config.buffer_size,
            ComparisonEvent::default,
            wait_strategy.clone(),
        )
        .handle_events_with({
            let processed = Arc::clone(&processed);
            let checksum = Arc::clone(&checksum);
            move |event: &mut ComparisonEvent, _sequence, _end_of_batch| {
                add_wrapping_i64(&checksum, event.value);
                processed.fetch_add(1, Ordering::Release);
            }
        })
        .build();

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

    Ok(build_result(config, wait_strategy_kind, runs))
}

fn run_pipeline_with<W>(
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

        let stage1 = PipelineStage1Handler {
            ready_count: Arc::clone(&ready_count),
        };
        let stage2 = PipelineStage2Handler {
            ready_count: Arc::clone(&ready_count),
        };
        let stage3 = PipelineStage3Handler {
            processed: Arc::clone(&processed),
            checksum: Arc::clone(&checksum),
            ready_count: Arc::clone(&ready_count),
        };

        let mut disruptor = build_single_producer(
            config.buffer_size,
            ComparisonEvent::default,
            wait_strategy.clone(),
        )
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
            disruptor.publish(move |event| {
                event.value = value;
                event.stage1_value = 0;
                event.stage2_value = 0;
                event.stage3_value = 0;
            });
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

    Ok(build_result(config, wait_strategy_kind, runs))
}

fn build_result(
    config: &ResolvedConfig,
    wait_strategy: WaitStrategyKind,
    runs: Vec<RoundRecord>,
) -> HarnessResult {
    HarnessResult {
        impl_name: String::from("badbatch"),
        scenario: config.scenario,
        measurement_kind: String::from("throughput"),
        buffer_size: config.buffer_size,
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
    let mut current = target.load(Ordering::Relaxed);
    loop {
        let next = current.wrapping_add(value);
        match target.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
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
        "measurement_kind",
        &result.measurement_kind,
        true,
    );
    write_json_number(&mut json, 2, "buffer_size", result.buffer_size as u64, true);
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
    println!("  --buffer-size <N>");
    println!("  --events-total <N>");
    println!("  --batch-size <N>");
    println!("  --warmup-rounds <N>");
    println!("  --measured-rounds <N>");
    println!("  --run-order <rust-then-java|java-then-rust|standalone>");
    println!("  --output <PATH>");
}
