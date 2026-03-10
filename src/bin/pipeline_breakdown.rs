#![allow(
    missing_docs,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use badbatch::disruptor::{
    build_single_producer, BusySpinWaitStrategy, EventHandler, Result as DisruptorResult,
    WaitStrategy, YieldingWaitStrategy,
};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::marker::PhantomData;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT_SECS: u64 = 300;

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

trait PipelineEvent: Default + Send + Sync + 'static {
    fn populate_input(&mut self, value: i64);
    fn apply_stage1(&mut self) -> i64;
    fn apply_stage2(&mut self) -> i64;
    fn apply_stage3(&mut self) -> i64;
}

impl PipelineEvent for ComparisonEvent {
    fn populate_input(&mut self, value: i64) {
        self.value = value;
        self.stage1_value = 0;
        self.stage2_value = 0;
        self.stage3_value = 0;
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

impl PipelineEvent for ComparisonPadded64Event {
    fn populate_input(&mut self, value: i64) {
        self.value = value;
        self.stage1_value = 0;
        self.stage2_value = 0;
        self.stage3_value = 0;
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
enum Mode {
    All,
    Single,
    Two,
    Three,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "all" => Ok(Self::All),
            "single-stage" => Ok(Self::Single),
            "two-stage" => Ok(Self::Two),
            "three-stage" => Ok(Self::Three),
            _ => Err(format!("unsupported mode: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Single => "single-stage",
            Self::Two => "two-stage",
            Self::Three => "three-stage",
        }
    }

    fn phases(self) -> &'static [PhaseKind] {
        match self {
            Self::All => &[PhaseKind::Single, PhaseKind::Two, PhaseKind::Three],
            Self::Single => &[PhaseKind::Single],
            Self::Two => &[PhaseKind::Two],
            Self::Three => &[PhaseKind::Three],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhaseKind {
    Single,
    Two,
    Three,
}

impl PhaseKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single_stage",
            Self::Two => "two_stage",
            Self::Three => "three_stage",
        }
    }

    fn stage_count(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Two => 2,
            Self::Three => 3,
        }
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

    fn event_size_bytes(self) -> usize {
        match self {
            Self::None => size_of::<ComparisonEvent>(),
            Self::Pad64 => size_of::<ComparisonPadded64Event>(),
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
    event_padding: EventPadding,
    events_total: u64,
    buffer_size: usize,
    warmup_rounds: usize,
    measured_rounds: usize,
    timeout_secs: u64,
    output: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct ResolvedConfig {
    mode: Mode,
    wait_strategy: WaitStrategyKind,
    event_padding: EventPadding,
    events_total: u64,
    buffer_size: usize,
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
    events_expected: u64,
    events_processed: u64,
    checksum_expected: i64,
    checksum_observed: i64,
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
    stage_count: usize,
    runs: Vec<RoundRecord>,
    summary: PhaseSummary,
}

#[derive(Debug, Clone)]
struct BreakdownResult {
    mode: Mode,
    wait_strategy: WaitStrategyKind,
    event_padding: EventPadding,
    event_size_bytes: usize,
    events_total: u64,
    buffer_size: usize,
    warmup_rounds: usize,
    measured_rounds: usize,
    phases: Vec<PhaseResult>,
    single_to_two_delta_ns_per_event: Option<f64>,
    two_to_three_delta_ns_per_event: Option<f64>,
}

impl CliConfig {
    fn from_args() -> Result<Self, String> {
        let mut args = env::args().skip(1);
        let mut mode = Mode::All;
        let mut wait_strategy = WaitStrategyKind::Yielding;
        let mut event_padding = EventPadding::None;
        let mut events_total = 20_000_000_u64;
        let mut buffer_size = 8_192_usize;
        let mut warmup_rounds = 3_usize;
        let mut measured_rounds = 5_usize;
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
                "--event-padding" => {
                    event_padding = EventPadding::parse(
                        &args.next().ok_or("missing value for --event-padding")?,
                    )?;
                }
                "--events-total" => {
                    events_total =
                        parse_u64(&args.next().ok_or("missing value for --events-total")?)?;
                }
                "--buffer-size" => {
                    buffer_size =
                        parse_usize(&args.next().ok_or("missing value for --buffer-size")?)?;
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
            event_padding,
            events_total,
            buffer_size,
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
        if value.buffer_size == 0 || !value.buffer_size.is_power_of_two() {
            return Err(format!(
                "buffer size must be a non-zero power of two, got {}",
                value.buffer_size
            ));
        }
        if value.measured_rounds == 0 {
            return Err(String::from("measured_rounds must be > 0"));
        }

        Ok(Self {
            mode: value.mode,
            wait_strategy: value.wait_strategy,
            event_padding: value.event_padding,
            events_total: value.events_total,
            buffer_size: value.buffer_size,
            warmup_rounds: value.warmup_rounds,
            measured_rounds: value.measured_rounds,
            timeout_secs: value.timeout_secs,
            output: value.output,
        })
    }
}

struct Stage1PassHandler<E> {
    ready_count: Arc<AtomicU64>,
    marker: PhantomData<fn() -> E>,
}

impl<E> EventHandler<E> for Stage1PassHandler<E>
where
    E: PipelineEvent,
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

struct Stage2PassHandler<E> {
    ready_count: Arc<AtomicU64>,
    marker: PhantomData<fn() -> E>,
}

impl<E> EventHandler<E> for Stage2PassHandler<E>
where
    E: PipelineEvent,
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

struct Stage123FinalHandler<E> {
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready_count: Arc<AtomicU64>,
    pending_processed: u64,
    pending_checksum: i64,
    marker: PhantomData<fn() -> E>,
}

impl<E> Stage123FinalHandler<E> {
    fn flush(&mut self) {
        if self.pending_processed == 0 {
            return;
        }
        self.processed
            .fetch_add(self.pending_processed, Ordering::Release);
        self.checksum
            .fetch_add(self.pending_checksum, Ordering::Relaxed);
        self.pending_processed = 0;
        self.pending_checksum = 0;
    }
}

impl<E> EventHandler<E> for Stage123FinalHandler<E>
where
    E: PipelineEvent,
{
    fn on_event(
        &mut self,
        event: &mut E,
        _sequence: i64,
        end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let _ = event.apply_stage1();
        let _ = event.apply_stage2();
        self.pending_checksum = self.pending_checksum.wrapping_add(event.apply_stage3());
        self.pending_processed += 1;
        if end_of_batch {
            self.flush();
        }
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn on_shutdown(&mut self) -> DisruptorResult<()> {
        self.flush();
        Ok(())
    }
}

struct Stage23FinalHandler<E> {
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready_count: Arc<AtomicU64>,
    pending_processed: u64,
    pending_checksum: i64,
    marker: PhantomData<fn() -> E>,
}

impl<E> Stage23FinalHandler<E> {
    fn flush(&mut self) {
        if self.pending_processed == 0 {
            return;
        }
        self.processed
            .fetch_add(self.pending_processed, Ordering::Release);
        self.checksum
            .fetch_add(self.pending_checksum, Ordering::Relaxed);
        self.pending_processed = 0;
        self.pending_checksum = 0;
    }
}

impl<E> EventHandler<E> for Stage23FinalHandler<E>
where
    E: PipelineEvent,
{
    fn on_event(
        &mut self,
        event: &mut E,
        _sequence: i64,
        end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let _ = event.apply_stage2();
        self.pending_checksum = self.pending_checksum.wrapping_add(event.apply_stage3());
        self.pending_processed += 1;
        if end_of_batch {
            self.flush();
        }
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn on_shutdown(&mut self) -> DisruptorResult<()> {
        self.flush();
        Ok(())
    }
}

struct Stage3FinalHandler<E> {
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicI64>,
    ready_count: Arc<AtomicU64>,
    pending_processed: u64,
    pending_checksum: i64,
    marker: PhantomData<fn() -> E>,
}

impl<E> Stage3FinalHandler<E> {
    fn flush(&mut self) {
        if self.pending_processed == 0 {
            return;
        }
        self.processed
            .fetch_add(self.pending_processed, Ordering::Release);
        self.checksum
            .fetch_add(self.pending_checksum, Ordering::Relaxed);
        self.pending_processed = 0;
        self.pending_checksum = 0;
    }
}

impl<E> EventHandler<E> for Stage3FinalHandler<E>
where
    E: PipelineEvent,
{
    fn on_event(
        &mut self,
        event: &mut E,
        _sequence: i64,
        end_of_batch: bool,
    ) -> DisruptorResult<()> {
        self.pending_checksum = self.pending_checksum.wrapping_add(event.apply_stage3());
        self.pending_processed += 1;
        if end_of_batch {
            self.flush();
        }
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.ready_count.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn on_shutdown(&mut self) -> DisruptorResult<()> {
        self.flush();
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
    let result = match config.wait_strategy {
        WaitStrategyKind::Yielding => run_with_wait_strategy(
            &config,
            YieldingWaitStrategy::new(),
            WaitStrategyKind::Yielding,
        ),
        WaitStrategyKind::BusySpin => run_with_wait_strategy(
            &config,
            BusySpinWaitStrategy::new(),
            WaitStrategyKind::BusySpin,
        ),
    }?;
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

    if result.phases.iter().any(|phase| !phase.summary.valid_all) {
        return Err(String::from(
            "one or more pipeline breakdown phases failed correctness validation",
        ));
    }

    Ok(())
}

fn run_with_wait_strategy<W>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<BreakdownResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
{
    match config.event_padding {
        EventPadding::None => run_with_wait_strategy_typed::<W, ComparisonEvent>(
            config,
            wait_strategy,
            wait_strategy_kind,
        ),
        EventPadding::Pad64 => run_with_wait_strategy_typed::<W, ComparisonPadded64Event>(
            config,
            wait_strategy,
            wait_strategy_kind,
        ),
    }
}

fn run_with_wait_strategy_typed<W, E>(
    config: &ResolvedConfig,
    wait_strategy: W,
    wait_strategy_kind: WaitStrategyKind,
) -> Result<BreakdownResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
    E: PipelineEvent,
{
    let mut phases = Vec::with_capacity(config.mode.phases().len());
    for &phase in config.mode.phases() {
        phases.push(run_phase::<W, E>(config, wait_strategy.clone(), phase)?);
    }

    Ok(BreakdownResult {
        mode: config.mode,
        wait_strategy: wait_strategy_kind,
        event_padding: config.event_padding,
        event_size_bytes: config.event_padding.event_size_bytes(),
        events_total: config.events_total,
        buffer_size: config.buffer_size,
        warmup_rounds: config.warmup_rounds,
        measured_rounds: config.measured_rounds,
        single_to_two_delta_ns_per_event: phase_delta_ns(
            &phases,
            PhaseKind::Single,
            PhaseKind::Two,
        ),
        two_to_three_delta_ns_per_event: phase_delta_ns(&phases, PhaseKind::Two, PhaseKind::Three),
        phases,
    })
}

fn run_phase<W, E>(
    config: &ResolvedConfig,
    wait_strategy: W,
    phase: PhaseKind,
) -> Result<PhaseResult, String>
where
    W: WaitStrategy + Send + Sync + Clone + 'static,
    E: PipelineEvent,
{
    let expected_checksum = pipeline_checksum(config.events_total, phase);
    let ready_target = phase.stage_count() as u64;
    let mut runs = Vec::with_capacity(config.warmup_rounds + config.measured_rounds);

    for round in 0..(config.warmup_rounds + config.measured_rounds) {
        let ready_count = Arc::new(AtomicU64::new(0));
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicI64::new(0));

        let mut disruptor = match phase {
            PhaseKind::Single => {
                let handler = Stage123FinalHandler::<E> {
                    processed: Arc::clone(&processed),
                    checksum: Arc::clone(&checksum),
                    ready_count: Arc::clone(&ready_count),
                    pending_processed: 0,
                    pending_checksum: 0,
                    marker: PhantomData,
                };
                build_single_producer(config.buffer_size, E::default, wait_strategy.clone())
                    .handle_events_with_handler(handler)
                    .build()
            }
            PhaseKind::Two => {
                let stage1 = Stage1PassHandler::<E> {
                    ready_count: Arc::clone(&ready_count),
                    marker: PhantomData,
                };
                let stage2 = Stage23FinalHandler::<E> {
                    processed: Arc::clone(&processed),
                    checksum: Arc::clone(&checksum),
                    ready_count: Arc::clone(&ready_count),
                    pending_processed: 0,
                    pending_checksum: 0,
                    marker: PhantomData,
                };
                build_single_producer(config.buffer_size, E::default, wait_strategy.clone())
                    .handle_events_with_handler(stage1)
                    .and_then()
                    .handle_events_with_handler(stage2)
                    .build()
            }
            PhaseKind::Three => {
                let stage1 = Stage1PassHandler::<E> {
                    ready_count: Arc::clone(&ready_count),
                    marker: PhantomData,
                };
                let stage2 = Stage2PassHandler::<E> {
                    ready_count: Arc::clone(&ready_count),
                    marker: PhantomData,
                };
                let stage3 = Stage3FinalHandler::<E> {
                    processed: Arc::clone(&processed),
                    checksum: Arc::clone(&checksum),
                    ready_count: Arc::clone(&ready_count),
                    pending_processed: 0,
                    pending_checksum: 0,
                    marker: PhantomData,
                };
                build_single_producer(config.buffer_size, E::default, wait_strategy.clone())
                    .handle_events_with_handler(stage1)
                    .and_then()
                    .handle_events_with_handler(stage2)
                    .and_then()
                    .handle_events_with_handler(stage3)
                    .build()
            }
        };

        wait_for_counter(
            &ready_count,
            ready_target,
            "pipeline stages ready",
            config.timeout_secs,
        )?;

        let start = Instant::now();
        for value in 0..config.events_total {
            let value = value as i64;
            disruptor.publish(move |event| event.populate_input(value));
        }
        wait_for_counter(
            &processed,
            config.events_total,
            "pipeline completion",
            config.timeout_secs,
        )?;
        let elapsed = start.elapsed().as_nanos();

        runs.push(RoundRecord {
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
            valid: processed.load(Ordering::Acquire) == config.events_total
                && checksum.load(Ordering::Acquire) == expected_checksum,
        });

        disruptor.shutdown();
    }

    Ok(PhaseResult {
        phase,
        stage_count: phase.stage_count(),
        summary: summarize_runs(&runs),
        runs,
    })
}

fn pipeline_checksum(count: u64, phase: PhaseKind) -> i64 {
    let base = arithmetic_checksum(count);
    let per_event_offset = match phase {
        PhaseKind::Single | PhaseKind::Two | PhaseKind::Three => 11_i64,
    };
    base.wrapping_add((count as i64).wrapping_mul(per_event_offset))
}

fn arithmetic_checksum(count: u64) -> i64 {
    let count = u128::from(count);
    let sum = count.saturating_mul(count.saturating_sub(1)) / 2;
    (sum as u64) as i64
}

fn summarize_runs(runs: &[RoundRecord]) -> PhaseSummary {
    let measured: Vec<f64> = runs
        .iter()
        .filter(|run| run.phase == RoundPhase::Measured)
        .map(|run| run.ops_per_sec)
        .collect();
    let valid_all = runs.iter().all(|run| run.valid);

    if measured.is_empty() {
        return PhaseSummary {
            valid_all,
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

    PhaseSummary {
        valid_all,
        median_ops_per_sec: median,
        mean_ops_per_sec: mean,
        min_ops_per_sec: min,
        max_ops_per_sec: max,
        stddev_ops_per_sec: stddev,
        cv,
    }
}

fn phase_delta_ns(phases: &[PhaseResult], lower: PhaseKind, upper: PhaseKind) -> Option<f64> {
    let lower_summary = phases.iter().find(|phase| phase.phase == lower)?;
    let upper_summary = phases.iter().find(|phase| phase.phase == upper)?;
    if lower_summary.summary.median_ops_per_sec == 0.0
        || upper_summary.summary.median_ops_per_sec == 0.0
    {
        return None;
    }
    Some(
        (1_000_000_000.0 / upper_summary.summary.median_ops_per_sec)
            - (1_000_000_000.0 / lower_summary.summary.median_ops_per_sec),
    )
}

fn wait_for_counter(
    counter: &AtomicU64,
    expected: u64,
    label: &str,
    timeout_secs: u64,
) -> Result<(), String> {
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
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

fn ops_per_sec(events_total: u64, elapsed_ns: u128) -> f64 {
    if elapsed_ns == 0 {
        return 0.0;
    }
    (events_total as f64) / (elapsed_ns as f64 / 1_000_000_000.0)
}

fn render_json(result: &BreakdownResult) -> String {
    let mut json = String::new();
    json.push_str("{\n");
    write_json_field(&mut json, 2, "tool", "pipeline_breakdown", true);
    write_json_field(&mut json, 2, "mode", result.mode.as_str(), true);
    write_json_field(
        &mut json,
        2,
        "wait_strategy",
        result.wait_strategy.as_str(),
        true,
    );
    write_json_field(
        &mut json,
        2,
        "event_padding",
        result.event_padding.as_str(),
        true,
    );
    write_json_number(
        &mut json,
        2,
        "event_size_bytes",
        result.event_size_bytes as u64,
        true,
    );
    write_json_number(&mut json, 2, "events_total", result.events_total, true);
    write_json_number(&mut json, 2, "buffer_size", result.buffer_size as u64, true);
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
    write_optional_float(
        &mut json,
        2,
        "single_to_two_delta_ns_per_event",
        result.single_to_two_delta_ns_per_event,
        true,
    );
    write_optional_float(
        &mut json,
        2,
        "two_to_three_delta_ns_per_event",
        result.two_to_three_delta_ns_per_event,
        true,
    );

    json.push_str("  \"phases\": [\n");
    for (phase_index, phase) in result.phases.iter().enumerate() {
        json.push_str("    {\n");
        write_json_field(&mut json, 6, "phase", phase.phase.as_str(), true);
        write_json_number(&mut json, 6, "stage_count", phase.stage_count as u64, true);

        json.push_str("      \"runs\": [\n");
        for (index, run) in phase.runs.iter().enumerate() {
            json.push_str("        {\n");
            write_json_number(&mut json, 10, "round_index", run.round_index as u64, true);
            write_json_field(&mut json, 10, "phase", run.phase.as_str(), true);
            write_json_number_u128(&mut json, 10, "elapsed_ns", run.elapsed_ns, true);
            write_json_float(&mut json, 10, "ops_per_sec", run.ops_per_sec, true);
            write_json_number(&mut json, 10, "events_expected", run.events_expected, true);
            write_json_number(
                &mut json,
                10,
                "events_processed",
                run.events_processed,
                true,
            );
            write_json_i64(
                &mut json,
                10,
                "checksum_expected",
                run.checksum_expected,
                true,
            );
            write_json_i64(
                &mut json,
                10,
                "checksum_observed",
                run.checksum_observed,
                true,
            );
            write_json_bool(&mut json, 10, "valid", run.valid, false);
            if index + 1 == phase.runs.len() {
                json.push_str("        }\n");
            } else {
                json.push_str("        },\n");
            }
        }
        json.push_str("      ],\n");

        json.push_str("      \"summary\": {\n");
        write_json_bool(&mut json, 8, "valid_all", phase.summary.valid_all, true);
        write_json_float(
            &mut json,
            8,
            "median_ops_per_sec",
            phase.summary.median_ops_per_sec,
            true,
        );
        write_json_float(
            &mut json,
            8,
            "mean_ops_per_sec",
            phase.summary.mean_ops_per_sec,
            true,
        );
        write_json_float(
            &mut json,
            8,
            "min_ops_per_sec",
            phase.summary.min_ops_per_sec,
            true,
        );
        write_json_float(
            &mut json,
            8,
            "max_ops_per_sec",
            phase.summary.max_ops_per_sec,
            true,
        );
        write_json_float(
            &mut json,
            8,
            "stddev_ops_per_sec",
            phase.summary.stddev_ops_per_sec,
            true,
        );
        write_json_float(&mut json, 8, "cv", phase.summary.cv, false);
        json.push_str("      }\n");
        if phase_index + 1 == result.phases.len() {
            json.push_str("    }\n");
        } else {
            json.push_str("    },\n");
        }
    }
    json.push_str("  ]\n");
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

fn write_optional_float(
    output: &mut String,
    indent: usize,
    key: &str,
    value: Option<f64>,
    trailing: bool,
) {
    let comma = if trailing { "," } else { "" };
    match value {
        Some(value) => {
            let _ = writeln!(
                output,
                "{space}\"{key}\": {:.6}{comma}",
                value,
                space = " ".repeat(indent)
            );
        }
        None => {
            let _ = writeln!(
                output,
                "{space}\"{key}\": null{comma}",
                space = " ".repeat(indent)
            );
        }
    }
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

fn parse_usize(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|error| format!("invalid usize value {value}: {error}"))
}

fn parse_u64(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|error| format!("invalid u64 value {value}: {error}"))
}

fn print_help() {
    println!("Usage: cargo run --release --bin pipeline_breakdown -- [options]");
    println!();
    println!("Options:");
    println!("  --mode <all|single-stage|two-stage|three-stage>");
    println!("  --wait-strategy <yielding|busy-spin>");
    println!("  --event-padding <none|64>");
    println!("  --events-total <N>");
    println!("  --buffer-size <N>");
    println!("  --warmup-rounds <N>");
    println!("  --measured-rounds <N>");
    println!("  --timeout-secs <N>");
    println!("  --output <PATH>");
}
