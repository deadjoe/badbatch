//! Standalone tail-latency harness for `BadBatch`.
//!
//! Open-loop measurement: the producer follows a deterministic offered-rate
//! schedule and never waits for consumer completion between publications.
//! Bounded-ring backpressure can still delay a publish, but it does not move the
//! planned timestamp. Latency is measured from the *planned* send time to the
//! *completion* of the handler, which prevents coordinated omission from hiding
//! that delay.
//!
//! The harness first calibrates the maximum sustainable throughput for the
//! chosen wait strategy / padding / buffer size, then measures tail latency at
//! fixed fractions of that throughput (offered-load levels). Reporting latency
//! without the corresponding offered load is meaningless; this harness makes
//! the load explicit.
//!
//! Outputs p50 / p99 / p99.9 / p99.99 / max per load level and can dump raw
//! samples.
//!
//! Counterfactual validation: `--inject-sleep-ms N` makes the handler sleep
//! once for N milliseconds; a correct harness will show a ~N-ms spike in
//! p99.9 and max.

#![allow(
    missing_docs,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use badbatch::disruptor::{
    build_single_producer, BusySpinWaitStrategy, EventHandler, Result as DisruptorResult,
    SlotPadding, YieldingWaitStrategy,
};
use core_affinity::CoreId;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_EVENTS_TOTAL: u64 = 1_000_000;
const DEFAULT_WARMUP_EVENTS: u64 = 100_000;
const DEFAULT_BUFFER_SIZE: usize = 65_536;
const DEFAULT_CALIBRATION_EVENTS: u64 = 100_000_000;
const DEFAULT_CALIBRATION_DURATION_MS: u64 = 2_000;
const DEFAULT_LOAD_LEVELS: &[u64] = &[50, 70, 90];
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const VALID_RUN_THRESHOLD: f64 = 0.95;
const MIN_RECORDED_SAMPLES: u64 = 100_000;
const QUICK_EVENTS_TOTAL: u64 = 110_000;
const QUICK_WARMUP_EVENTS: u64 = 10_000;
const QUICK_CALIBRATION_EVENTS: u64 = 1_000_000;

// --- event model --------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy)]
struct LatencyEvent {
    sequence: i64,
    planned_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LatencySample {
    sequence: u64,
    planned_ns: u64,
    completion_ns: u64,
    latency_ns: u64,
}

// --- CLI ----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitKind {
    BusySpin,
    Yielding,
}

impl WaitKind {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "busy-spin" => Ok(Self::BusySpin),
            "yielding" => Ok(Self::Yielding),
            _ => Err(format!("unsupported wait-strategy: {s}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::BusySpin => "busy-spin",
            Self::Yielding => "yielding",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pad {
    None,
    Align128,
}

impl Pad {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "none" => Ok(Self::None),
            "128" => Ok(Self::Align128),
            "64" => Err("padding 64 removed; use none or 128".into()),
            _ => Err(format!("unsupported event-padding: {s}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Align128 => "128",
        }
    }

    fn slot_padding(self) -> SlotPadding {
        match self {
            Self::None => SlotPadding::None,
            Self::Align128 => SlotPadding::CacheLine128,
        }
    }
}

struct Config {
    wait: WaitKind,
    pad: Pad,
    buffer_size: usize,
    events_total: u64,
    warmup_events: u64,
    rate: Option<u64>,
    load_levels: Vec<u64>,
    calibration_events: u64,
    calibration_duration: Duration,
    max_rate: Option<u64>,
    cpu_list: Vec<usize>,
    affinity_failed: Arc<AtomicBool>,
    output: Option<PathBuf>,
    samples_output: PathBuf,
    timeout: Duration,
    inject_sleep: Option<Duration>,
}

fn parse_args() -> Result<Config, String> {
    let mut args = env::args().skip(1);
    let mut wait = WaitKind::BusySpin;
    let mut pad = Pad::None;
    let mut buffer_size = None;
    let mut events_total = None;
    let mut warmup_events = None;
    let mut rate = None;
    let mut load_levels = None;
    let mut calibration_events = None;
    let mut calibration_duration_ms = None;
    let mut max_rate = None;
    let mut cpu_list = Vec::new();
    let mut output = None;
    let mut samples_output = None;
    let mut inject_sleep = None;
    let mut quick = false;
    // `quick` is consumed below when applying smaller defaults.

    while let Some(a) = args.next() {
        match a.as_str() {
            "--wait-strategy" => {
                wait = WaitKind::parse(&args.next().ok_or("missing --wait-strategy value")?)?;
            }
            "--event-padding" => {
                pad = Pad::parse(&args.next().ok_or("missing --event-padding value")?)?;
            }
            "--buffer-size" => {
                buffer_size = Some(
                    args.next()
                        .ok_or("missing --buffer-size")?
                        .parse()
                        .map_err(|e| format!("buffer-size: {e}"))?,
                );
            }
            "--events-total" => {
                events_total = Some(
                    args.next()
                        .ok_or("missing --events-total")?
                        .replace('_', "")
                        .parse()
                        .map_err(|e| format!("events-total: {e}"))?,
                );
            }
            "--warmup-events" => {
                warmup_events = Some(
                    args.next()
                        .ok_or("missing --warmup-events")?
                        .replace('_', "")
                        .parse()
                        .map_err(|e| format!("warmup-events: {e}"))?,
                );
            }
            "--rate" => {
                rate = Some(
                    args.next()
                        .ok_or("missing --rate")?
                        .replace('_', "")
                        .parse()
                        .map_err(|e| format!("rate: {e}"))?,
                );
            }
            "--load-levels" => {
                let raw = args.next().ok_or("missing --load-levels value")?;
                load_levels = Some(parse_percent_list(&raw)?);
            }
            "--calibration-events" => {
                calibration_events = Some(
                    args.next()
                        .ok_or("missing --calibration-events")?
                        .replace('_', "")
                        .parse()
                        .map_err(|e| format!("calibration-events: {e}"))?,
                );
            }
            "--calibration-duration-ms" => {
                calibration_duration_ms = Some(
                    args.next()
                        .ok_or("missing --calibration-duration-ms")?
                        .parse()
                        .map_err(|e| format!("calibration-duration-ms: {e}"))?,
                );
            }
            "--max-rate" => {
                max_rate = Some(
                    args.next()
                        .ok_or("missing --max-rate")?
                        .replace('_', "")
                        .parse()
                        .map_err(|e| format!("max-rate: {e}"))?,
                );
            }
            "--cpu-list" => {
                cpu_list = parse_cpu_list(&args.next().ok_or("missing --cpu-list")?)?;
            }
            "--output" => {
                output = Some(PathBuf::from(args.next().ok_or("missing --output")?));
            }
            "--samples-output" => {
                samples_output = Some(PathBuf::from(
                    args.next().ok_or("missing --samples-output")?,
                ));
            }
            "--inject-sleep-ms" => {
                let ms: u64 = args
                    .next()
                    .ok_or("missing --inject-sleep-ms")?
                    .parse()
                    .map_err(|e| format!("inject-sleep-ms: {e}"))?;
                if ms == 0 {
                    return Err("inject-sleep-ms must be > 0".into());
                }
                inject_sleep = Some(Duration::from_millis(ms));
            }
            "--quick" => quick = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let mut events_total = events_total.unwrap_or(DEFAULT_EVENTS_TOTAL);
    let mut warmup_events = warmup_events.unwrap_or(DEFAULT_WARMUP_EVENTS);
    let mut calibration_events = calibration_events.unwrap_or(DEFAULT_CALIBRATION_EVENTS);
    let calibration_duration_ms =
        calibration_duration_ms.unwrap_or(DEFAULT_CALIBRATION_DURATION_MS);

    if quick {
        events_total = events_total.min(QUICK_EVENTS_TOTAL);
        warmup_events = warmup_events.min(QUICK_WARMUP_EVENTS);
        calibration_events = calibration_events.min(QUICK_CALIBRATION_EVENTS);
    }

    if events_total == 0 {
        return Err("events-total must be > 0".into());
    }
    if events_total > i64::MAX as u64 {
        return Err("events-total must fit i64".into());
    }
    if !buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE).is_power_of_two()
        || buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE) == 0
    {
        return Err(format!(
            "buffer-size must be power of two, got {}",
            buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE)
        ));
    }
    if warmup_events >= events_total {
        return Err("warmup-events must be < events-total".into());
    }
    let recorded_samples = events_total - warmup_events;
    if recorded_samples < MIN_RECORDED_SAMPLES {
        return Err(format!(
            "events-total - warmup-events must be at least {MIN_RECORDED_SAMPLES} \
             for p99.99, got {recorded_samples}"
        ));
    }
    usize::try_from(recorded_samples)
        .map_err(|_| "recorded sample count must fit usize".to_string())?;
    if calibration_duration_ms == 0 {
        return Err("calibration-duration-ms must be > 0".into());
    }
    if calibration_events == 0 {
        return Err("calibration-events must be > 0".into());
    }
    if calibration_events > i64::MAX as u64 {
        return Err("calibration-events must fit i64".into());
    }
    if let Some(r) = rate {
        if r == 0 {
            return Err("rate must be > 0".into());
        }
    }
    if let Some(r) = max_rate {
        if r == 0 {
            return Err("max-rate must be > 0".into());
        }
    }
    if let Some(levels) = &load_levels {
        for &lvl in levels {
            if lvl == 0 || lvl > 100 {
                return Err(format!("load level must be in 1..=100, got {lvl}"));
            }
        }
    }
    if !cpu_list.is_empty() && cpu_list.len() < 2 {
        return Err(
            "tail-latency harness needs at least 2 CPUs for producer/consumer isolation".into(),
        );
    }

    let load_levels = load_levels.unwrap_or_else(|| DEFAULT_LOAD_LEVELS.to_vec());
    if rate.is_none() && load_levels.is_empty() {
        return Err("load-levels must contain at least one percentage".into());
    }
    let samples_output = samples_output.ok_or("required: --samples-output")?;

    Ok(Config {
        wait,
        pad,
        buffer_size: buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE),
        events_total,
        warmup_events,
        rate,
        load_levels,
        calibration_events,
        calibration_duration: Duration::from_millis(calibration_duration_ms),
        max_rate,
        cpu_list,
        affinity_failed: Arc::new(AtomicBool::new(false)),
        output,
        samples_output,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        inject_sleep,
    })
}

fn parse_cpu_list(value: &str) -> Result<Vec<usize>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let cpus = value
        .split(',')
        .map(|part| {
            part.parse::<usize>()
                .map_err(|error| format!("invalid CPU {part:?}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut unique = cpus.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != cpus.len() {
        return Err("cpu-list must contain unique CPU IDs".into());
    }
    Ok(cpus)
}

fn parse_percent_list(value: &str) -> Result<Vec<u64>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|part| {
            part.parse::<u64>()
                .map_err(|error| format!("invalid load level {part:?}: {error}"))
        })
        .collect()
}

impl Config {
    fn cpu(&self, index: usize) -> Option<usize> {
        self.cpu_list.get(index).copied()
    }

    fn affinity_verified_all(&self) -> bool {
        !self.cpu_list.is_empty() && !self.affinity_failed.load(Ordering::Acquire)
    }
}

fn pin_current_thread(cpu: Option<usize>, failed: &AtomicBool) {
    let Some(cpu) = cpu else {
        return;
    };
    let set = core_affinity::set_for_current(CoreId { id: cpu });
    #[cfg(target_os = "linux")]
    let verified =
        set && core_affinity::get_core_ids().is_some_and(|ids| ids.len() == 1 && ids[0].id == cpu);
    #[cfg(not(target_os = "linux"))]
    let verified = set;
    if !verified {
        failed.store(true, Ordering::Release);
    }
}

fn print_help() {
    println!(
        "\
Usage: h2h_tail_latency [options]

Options:
  --wait-strategy <yielding|busy-spin>  (default busy-spin)
  --event-padding <none|128>            (slot padding; default none)
  --buffer-size <N>                     (power of two; default 65536)
  --events-total <N>                    (events per load level; default 1_000_000)
  --warmup-events <N>                   (discarded before recording; default 100_000)
  --rate <events/s>                     (single fixed rate; overrides load-levels)
  --load-levels <pct,pct,...>           (percent of max throughput; default 50,70,90)
  --max-rate <events/s>                 (skip calibration; use this as max throughput)
  --calibration-events <N>              (max events for calibration; default 100_000_000)
  --calibration-duration-ms <N>         (max duration for calibration; default 2000)
  --cpu-list <N,N,...>                  (pin producer/consumer to logical CPUs)
  --inject-sleep-ms <N>                 (inject one N-ms handler sleep for CO validation)
  --output <path.json>                  (latency statistics JSON)
  --samples-output <path.csv>           (required raw schedule/completion samples)
  --quick                               (smaller defaults)
"
    );
}

// --- handlers -----------------------------------------------------------------------

struct CalibrationHandler {
    processed: Arc<AtomicU64>,
    ready: Arc<AtomicU64>,
    cpu_affinity: Option<usize>,
    affinity_failed: Arc<AtomicBool>,
}

impl EventHandler<LatencyEvent> for CalibrationHandler {
    fn on_event(
        &mut self,
        _event: &mut LatencyEvent,
        sequence: i64,
        end_of_batch: bool,
    ) -> DisruptorResult<()> {
        if end_of_batch {
            let completed = u64::try_from(sequence)
                .expect("calibration sequence must be non-negative")
                .saturating_add(1);
            self.processed.store(completed, Ordering::Release);
        }
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        pin_current_thread(self.cpu_affinity, &self.affinity_failed);
        self.ready.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

struct LatencyHandler {
    start: Arc<OnceLock<Instant>>,
    processed: Arc<AtomicU64>,
    ready: Arc<AtomicU64>,
    warmup_events: u64,
    final_sequence: i64,
    inject_sleep: Option<Duration>,
    inject_sequence: Option<u64>,
    injected: bool,
    samples: Vec<LatencySample>,
    samples_sink: Arc<Mutex<Option<Vec<LatencySample>>>>,
    cpu_affinity: Option<usize>,
    affinity_failed: Arc<AtomicBool>,
}

impl LatencyHandler {
    #[allow(clippy::too_many_arguments)]
    fn new(
        start: Arc<OnceLock<Instant>>,
        processed: Arc<AtomicU64>,
        ready: Arc<AtomicU64>,
        warmup_events: u64,
        events_total: u64,
        inject_sleep: Option<Duration>,
        cpu_affinity: Option<usize>,
        affinity_failed: Arc<AtomicBool>,
        samples_sink: Arc<Mutex<Option<Vec<LatencySample>>>>,
    ) -> Self {
        Self {
            start,
            processed,
            ready,
            warmup_events,
            final_sequence: i64::try_from(events_total).expect("events_total must fit i64") - 1,
            inject_sleep,
            inject_sequence: inject_sleep
                .map(|_| warmup_events + (events_total - warmup_events) / 2),
            injected: false,
            samples: Vec::with_capacity(
                usize::try_from(events_total - warmup_events)
                    .expect("validated sample count must fit usize"),
            ),
            samples_sink,
            cpu_affinity,
            affinity_failed,
        }
    }

    fn take_injection(&mut self, sequence: u64) -> Option<Duration> {
        if !self.injected && self.inject_sequence == Some(sequence) {
            self.injected = true;
            self.inject_sleep
        } else {
            None
        }
    }
}

impl EventHandler<LatencyEvent> for LatencyHandler {
    fn on_event(
        &mut self,
        event: &mut LatencyEvent,
        sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let sequence = u64::try_from(sequence).expect("latency sequence must be non-negative");
        debug_assert_eq!(event.sequence, sequence as i64);

        // Inject a single deterministic pause to validate that the harness
        // does not hide latency via coordinated omission.
        if let Some(sleep) = self.take_injection(sequence) {
            thread::sleep(sleep);
        }

        let completion_ns = self
            .start
            .get()
            .expect("measurement epoch must be set before publishing")
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);
        let latency_ns = completion_ns.saturating_sub(event.planned_ns);

        if sequence >= self.warmup_events {
            self.samples.push(LatencySample {
                sequence,
                planned_ns: event.planned_ns,
                completion_ns,
                latency_ns,
            });
        }

        if sequence as i64 == self.final_sequence {
            self.processed.store(sequence + 1, Ordering::Release);
        }
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        pin_current_thread(self.cpu_affinity, &self.affinity_failed);
        self.ready.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn on_shutdown(&mut self) -> DisruptorResult<()> {
        let samples = std::mem::take(&mut self.samples);
        *self
            .samples_sink
            .lock()
            .expect("latency samples sink mutex poisoned") = Some(samples);
        Ok(())
    }
}

fn wait_count(counter: &AtomicU64, target: u64, timeout: Duration) -> bool {
    let start = Instant::now();
    while counter.load(Ordering::Acquire) < target {
        if start.elapsed() > timeout {
            return false;
        }
        std::hint::spin_loop();
    }
    true
}

// --- calibration --------------------------------------------------------------------

fn calibrate_max_rate(cfg: &Config) -> Result<f64, String> {
    match cfg.wait {
        WaitKind::BusySpin => calibrate_max_rate_w(cfg, BusySpinWaitStrategy::new()),
        WaitKind::Yielding => calibrate_max_rate_w(cfg, YieldingWaitStrategy::new()),
    }
}

fn calibrate_max_rate_w<W>(cfg: &Config, wait: W) -> Result<f64, String>
where
    W: badbatch::disruptor::WaitStrategy + Clone + 'static,
{
    let processed = Arc::new(AtomicU64::new(0));
    let ready = Arc::new(AtomicU64::new(0));

    let handler = CalibrationHandler {
        processed: Arc::clone(&processed),
        ready: Arc::clone(&ready),
        cpu_affinity: cfg.cpu(1),
        affinity_failed: Arc::clone(&cfg.affinity_failed),
    };

    let mut handle = build_single_producer(cfg.buffer_size, LatencyEvent::default, wait)
        .with_slot_padding(cfg.pad.slot_padding())
        .handle_events_with_handler(handler)
        .build();

    if !wait_count(&ready, 1, cfg.timeout) {
        handle.shutdown();
        return Err("timeout waiting for calibration consumer readiness".into());
    }
    if cfg.affinity_failed.load(Ordering::Acquire) {
        handle.shutdown();
        return Err("failed to pin calibration consumer thread".into());
    }

    pin_current_thread(cfg.cpu(0), &cfg.affinity_failed);
    if cfg.affinity_failed.load(Ordering::Acquire) {
        handle.shutdown();
        return Err("failed to pin calibration producer thread".into());
    }

    let measure_start = Instant::now();
    let mut published = 0u64;
    while published < cfg.calibration_events && measure_start.elapsed() < cfg.calibration_duration {
        handle
            .publish(|event| {
                event.sequence = i64::try_from(published).expect("calibration-events must fit i64");
                event.planned_ns = 0;
            })
            .map_err(|error| {
                format!("calibration publish failed at sequence {published}: {error}")
            })?;
        published += 1;
    }

    if !wait_count(&processed, published, cfg.timeout) {
        handle.shutdown();
        return Err(format!(
            "timeout waiting for calibration completion (got {})",
            processed.load(Ordering::Acquire)
        ));
    }

    let elapsed = measure_start.elapsed();
    handle.shutdown();

    if elapsed.as_secs_f64() == 0.0 {
        return Err("calibration duration was zero".into());
    }
    Ok(published as f64 / elapsed.as_secs_f64())
}

// --- measurement --------------------------------------------------------------------

struct LoadResult {
    load_pct: u64,
    target_rate: u64,
    actual_rate: f64,
    rate_valid: bool,
    valid_run: bool,
    pause_check: Option<PauseCheck>,
    samples: Vec<LatencySample>,
}

#[derive(Debug, Clone, Copy)]
struct LatencyStats {
    p50: f64,
    p99: f64,
    p99_9: f64,
    p99_99: f64,
    max: f64,
    mean: f64,
    min: f64,
    count: usize,
}

#[derive(Debug, Clone, Copy)]
struct PauseCheck {
    sleep_ns: u64,
    expected_affected_samples: u64,
    sample_count: usize,
    observed_p99_9_ns: f64,
    observed_max_ns: f64,
}

impl PauseCheck {
    fn enough_samples_for_p99_9(self) -> bool {
        let p99_9_tail_count = (self.sample_count as u64).saturating_add(999) / 1_000;
        self.expected_affected_samples >= p99_9_tail_count
    }

    fn p99_9_visible(self) -> bool {
        self.enough_samples_for_p99_9() && self.observed_p99_9_ns >= self.sleep_ns as f64 * 0.5
    }

    fn max_visible(self) -> bool {
        self.observed_max_ns >= self.sleep_ns as f64 * 0.8
    }

    fn is_valid(self) -> bool {
        self.p99_9_visible() && self.max_visible()
    }
}

fn run_load(cfg: &Config, target_rate: u64) -> Result<LoadResult, String> {
    match cfg.wait {
        WaitKind::BusySpin => run_load_w(cfg, target_rate, BusySpinWaitStrategy::new()),
        WaitKind::Yielding => run_load_w(cfg, target_rate, YieldingWaitStrategy::new()),
    }
}

fn scheduled_ns(sequence: u64, rate: u64) -> Result<u64, String> {
    let nanos = u128::from(sequence)
        .checked_mul(1_000_000_000)
        .ok_or("planned send timestamp overflow")?
        / u128::from(rate);
    u64::try_from(nanos).map_err(|_| "planned send timestamp does not fit u64".into())
}

fn rate_is_valid(actual_rate: f64, target_rate: u64) -> bool {
    actual_rate >= target_rate as f64 * VALID_RUN_THRESHOLD
}

fn run_load_w<W>(cfg: &Config, target_rate: u64, wait: W) -> Result<LoadResult, String>
where
    W: badbatch::disruptor::WaitStrategy + Clone + 'static,
{
    let start = Arc::new(OnceLock::new());
    let processed = Arc::new(AtomicU64::new(0));
    let ready = Arc::new(AtomicU64::new(0));
    let samples_sink = Arc::new(Mutex::new(None));

    let handler = LatencyHandler::new(
        Arc::clone(&start),
        Arc::clone(&processed),
        Arc::clone(&ready),
        cfg.warmup_events,
        cfg.events_total,
        cfg.inject_sleep,
        cfg.cpu(1),
        Arc::clone(&cfg.affinity_failed),
        Arc::clone(&samples_sink),
    );

    let mut handle = build_single_producer(cfg.buffer_size, LatencyEvent::default, wait)
        .with_slot_padding(cfg.pad.slot_padding())
        .handle_events_with_handler(handler)
        .build();

    if !wait_count(&ready, 1, cfg.timeout) {
        handle.shutdown();
        return Err("timeout waiting for consumer readiness".into());
    }
    if cfg.affinity_failed.load(Ordering::Acquire) {
        handle.shutdown();
        return Err("failed to pin consumer thread".into());
    }

    pin_current_thread(cfg.cpu(0), &cfg.affinity_failed);
    if cfg.affinity_failed.load(Ordering::Acquire) {
        handle.shutdown();
        return Err("failed to pin producer thread".into());
    }

    start
        .set(Instant::now())
        .map_err(|_| "measurement epoch was already initialized".to_string())?;
    let mut first_send_ns = None;
    let mut last_send_ns = 0_u64;

    for i in 0..cfg.events_total {
        let planned_ns = scheduled_ns(i, target_rate)?;
        while start
            .get()
            .expect("measurement epoch initialized")
            .elapsed()
            .as_nanos()
            < u128::from(planned_ns)
        {
            std::hint::spin_loop();
        }

        handle
            .publish(|event| {
                event.sequence = i64::try_from(i).expect("events_total must fit i64");
                event.planned_ns = planned_ns;
            })
            .map_err(|error| format!("publish failed at sequence {i}: {error}"))?;

        let actual_ns = start
            .get()
            .expect("measurement epoch initialized")
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);
        if first_send_ns.is_none() {
            first_send_ns = Some(actual_ns);
        }
        last_send_ns = actual_ns;
    }

    if !wait_count(&processed, cfg.events_total, cfg.timeout) {
        handle.shutdown();
        return Err(format!(
            "timeout waiting for completion (got {})",
            processed.load(Ordering::Acquire)
        ));
    }

    handle.shutdown();
    let samples = samples_sink
        .lock()
        .map_err(|_| "samples sink mutex poisoned".to_string())?
        .take()
        .ok_or("consumer did not publish latency samples during shutdown")?;
    let expected_samples = usize::try_from(cfg.events_total - cfg.warmup_events)
        .map_err(|_| "sample count does not fit usize")?;
    if samples.len() != expected_samples {
        return Err(format!(
            "latency sample count mismatch: expected {expected_samples}, got {}",
            samples.len()
        ));
    }

    let first_send_ns = first_send_ns.unwrap_or(0);
    let elapsed_s = (last_send_ns.saturating_sub(first_send_ns)) as f64 / 1e9;
    let sent_count = cfg.events_total.saturating_sub(1).max(1);
    let actual_rate = if elapsed_s > 0.0 {
        sent_count as f64 / elapsed_s
    } else {
        0.0
    };
    let rate_valid = rate_is_valid(actual_rate, target_rate);
    let stats = compute_stats(&samples);
    let pause_check = cfg
        .inject_sleep
        .map(|sleep| validate_injected_pause(sleep, target_rate, &stats));
    let valid_run = rate_valid && pause_check.is_none_or(PauseCheck::is_valid);

    Ok(LoadResult {
        load_pct: 0,
        target_rate,
        actual_rate,
        rate_valid,
        valid_run,
        pause_check,
        samples,
    })
}

// --- stats --------------------------------------------------------------------------

fn percentile_basis_points(sorted: &[u64], basis_points: u64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    assert!((1..=10_000).contains(&basis_points));
    // Nearest-rank without floating-point boundary drift:
    // rank = ceil(N * basis_points / 10_000), index = rank - 1.
    let numerator = (sorted.len() as u128) * u128::from(basis_points);
    let rank = numerator.div_ceil(10_000);
    let index = usize::try_from(rank.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(sorted.len() - 1);
    sorted[index] as f64
}

fn compute_stats(samples: &[LatencySample]) -> LatencyStats {
    let mut latencies = samples
        .iter()
        .map(|sample| sample.latency_ns)
        .collect::<Vec<_>>();
    latencies.sort_unstable();
    let samples = latencies.as_slice();
    let count = samples.len();
    if count == 0 {
        return LatencyStats {
            p50: 0.0,
            p99: 0.0,
            p99_9: 0.0,
            p99_99: 0.0,
            max: 0.0,
            mean: 0.0,
            min: 0.0,
            count: 0,
        };
    }
    let sum = samples
        .iter()
        .map(|&sample| u128::from(sample))
        .sum::<u128>();
    LatencyStats {
        p50: percentile_basis_points(samples, 5_000),
        p99: percentile_basis_points(samples, 9_900),
        p99_9: percentile_basis_points(samples, 9_990),
        p99_99: percentile_basis_points(samples, 9_999),
        max: samples[count - 1] as f64,
        mean: sum as f64 / count as f64,
        min: samples[0] as f64,
        count,
    }
}

fn validate_injected_pause(sleep: Duration, target_rate: u64, stats: &LatencyStats) -> PauseCheck {
    let sleep_ns = sleep.as_nanos().try_into().unwrap_or(u64::MAX);
    let affected_numerator = u128::from(target_rate).saturating_mul(u128::from(sleep_ns));
    let expected_affected_samples = affected_numerator
        .saturating_add(999_999_999)
        .checked_div(1_000_000_000)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(u64::MAX)
        .max(1);
    PauseCheck {
        sleep_ns,
        expected_affected_samples,
        sample_count: stats.count,
        observed_p99_9_ns: stats.p99_9,
        observed_max_ns: stats.max,
    }
}

// --- JSON ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct BuildProvenance {
    git_rev: &'static str,
    dirty: Option<bool>,
}

impl BuildProvenance {
    fn current() -> Self {
        Self {
            git_rev: option_env!("BADBATCH_BUILD_GIT_REV").unwrap_or("unknown"),
            dirty: match option_env!("BADBATCH_BUILD_GIT_DIRTY") {
                Some("false") => Some(false),
                Some("true") => Some(true),
                _ => None,
            },
        }
    }

    fn is_valid(self) -> bool {
        self.git_rev.len() == 40
            && self.git_rev.bytes().all(|byte| byte.is_ascii_hexdigit())
            && self.dirty == Some(false)
    }

    fn dirty_json(self) -> &'static str {
        match self.dirty {
            Some(false) => "false",
            Some(true) => "true",
            None => "null",
        }
    }
}

fn write_result(
    cfg: &Config,
    max_rate: f64,
    loads: &[LoadResult],
    provenance: BuildProvenance,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("{\n");
    writeln!(out, "  \"impl\": \"badbatch-builder-tail-latency\",").unwrap();
    out.push_str("  \"language\": \"rust\",\n");
    out.push_str("  \"scenario\": \"unicast_tail_latency\",\n");
    out.push_str("  \"arrival_model\": \"open_loop_fixed_schedule\",\n");
    out.push_str("  \"latency_origin\": \"planned_send_time\",\n");
    out.push_str(
        "  \"raw_sample_columns\": [\"sequence\", \"planned_ns\", \
         \"completion_ns\", \"latency_ns\"],\n",
    );
    writeln!(out, "  \"wait_strategy\": \"{}\",", cfg.wait.as_str()).unwrap();
    writeln!(out, "  \"event_padding\": \"{}\",", cfg.pad.as_str()).unwrap();
    out.push_str("  \"api_path\": \"builder\",\n");
    writeln!(out, "  \"buffer_size\": {},", cfg.buffer_size).unwrap();
    writeln!(out, "  \"events_total\": {},", cfg.events_total).unwrap();
    writeln!(out, "  \"warmup_events\": {},", cfg.warmup_events).unwrap();
    writeln!(out, "  \"max_rate\": {max_rate:.6},").unwrap();
    let threshold = VALID_RUN_THRESHOLD;
    writeln!(out, "  \"minimum_actual_target_ratio\": {threshold:.6},").unwrap();
    writeln!(
        out,
        "  \"inject_sleep_ms\": {},",
        cfg.inject_sleep.map_or(0, |d| d.as_millis() as u64)
    )
    .unwrap();
    out.push_str("  \"provenance_source\": \"build_time\",\n");
    writeln!(out, "  \"provenance_valid\": {},", provenance.is_valid()).unwrap();
    writeln!(out, "  \"git_rev\": \"{}\",", provenance.git_rev).unwrap();
    writeln!(out, "  \"dirty\": {},", provenance.dirty_json()).unwrap();
    out.push_str("  \"cpu_affinity\": {\n");
    out.push_str("    \"requested_cpu_list\": [");
    for (index, cpu) in cfg.cpu_list.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        write!(out, "{cpu}").unwrap();
    }
    out.push_str("],\n");
    writeln!(
        out,
        "    \"mode\": \"{}\",",
        if cfg.cpu_list.is_empty() {
            "none"
        } else {
            "per-thread"
        }
    )
    .unwrap();
    writeln!(
        out,
        "    \"verified_all\": {},",
        cfg.affinity_verified_all()
    )
    .unwrap();
    out.push_str("    \"role_cpu_map\": {\n");
    if cfg.cpu_list.len() >= 2 {
        writeln!(out, "      \"producer\": {},", cfg.cpu_list[0]).unwrap();
        writeln!(out, "      \"consumer\": {}", cfg.cpu_list[1]).unwrap();
    }
    out.push_str("    }\n");
    out.push_str("  },\n");
    out.push_str("  \"loads\": [\n");
    for (idx, load) in loads.iter().enumerate() {
        let stats = compute_stats(&load.samples);
        out.push_str("    {\n");
        writeln!(out, "      \"load_pct\": {},", load.load_pct).unwrap();
        writeln!(out, "      \"target_rate\": {},", load.target_rate).unwrap();
        writeln!(out, "      \"actual_rate\": {:.6},", load.actual_rate).unwrap();
        writeln!(
            out,
            "      \"actual_target_ratio\": {:.6},",
            load.actual_rate / load.target_rate as f64
        )
        .unwrap();
        writeln!(out, "      \"rate_valid\": {},", load.rate_valid).unwrap();
        writeln!(out, "      \"valid_run\": {},", load.valid_run).unwrap();
        if let Some(check) = load.pause_check {
            out.push_str("      \"pause_validation\": {\n");
            writeln!(out, "        \"sleep_ns\": {},", check.sleep_ns).unwrap();
            writeln!(
                out,
                "        \"expected_affected_samples\": {},",
                check.expected_affected_samples
            )
            .unwrap();
            writeln!(
                out,
                "        \"enough_samples_for_p99.9\": {},",
                check.enough_samples_for_p99_9()
            )
            .unwrap();
            writeln!(out, "        \"p99.9_visible\": {},", check.p99_9_visible()).unwrap();
            writeln!(out, "        \"max_visible\": {},", check.max_visible()).unwrap();
            writeln!(out, "        \"valid\": {}", check.is_valid()).unwrap();
            out.push_str("      },\n");
        }
        out.push_str("      \"latency_ns\": {\n");
        writeln!(out, "        \"count\": {},", stats.count).unwrap();
        writeln!(out, "        \"mean\": {:.6},", stats.mean).unwrap();
        writeln!(out, "        \"min\": {:.6},", stats.min).unwrap();
        writeln!(out, "        \"p50\": {:.6},", stats.p50).unwrap();
        writeln!(out, "        \"p99\": {:.6},", stats.p99).unwrap();
        writeln!(out, "        \"p99.9\": {:.6},", stats.p99_9).unwrap();
        writeln!(out, "        \"p99.99\": {:.6},", stats.p99_99).unwrap();
        writeln!(out, "        \"max\": {:.6}", stats.max).unwrap();
        out.push_str("      }\n");
        out.push_str(if idx + 1 == loads.len() {
            "    }\n"
        } else {
            "    },\n"
        });
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))
}

fn write_samples(path: &Path, samples: &[LatencySample]) -> Result<(), String> {
    use std::io::{BufWriter, Write as _};

    let file = fs::File::create(path)
        .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
    let mut out = BufWriter::new(file);
    writeln!(out, "sequence,planned_ns,completion_ns,latency_ns")
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    for sample in samples {
        writeln!(
            out,
            "{},{},{},{}",
            sample.sequence, sample.planned_ns, sample.completion_ns, sample.latency_ns
        )
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    out.flush()
        .map_err(|error| format!("failed to flush {}: {error}", path.display()))
}

fn samples_path_for_load(base: &Path, load_pct: u64) -> PathBuf {
    if load_pct == 0 {
        return base.to_path_buf();
    }
    let ext = base.extension().and_then(|e| e.to_str()).unwrap_or("csv");
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("samples");
    base.with_file_name(format!("{stem}-{load_pct}.{ext}"))
}

// --- main ---------------------------------------------------------------------------

fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            print_help();
            std::process::exit(2);
        }
    };

    let provenance = BuildProvenance::current();
    let provenance_valid = provenance.is_valid();
    if !provenance_valid {
        eprintln!(
            "warning: invalid build provenance (git_rev={}, dirty={}); \
             results will be hard-invalidated",
            provenance.git_rev,
            provenance.dirty_json()
        );
    }

    let max_rate = cfg
        .max_rate
        .or(cfg.rate)
        .map_or_else(|| calibrate_max_rate(&cfg), |rate| Ok(rate as f64))
        .unwrap_or_else(|e| {
            eprintln!("calibration error: {e}");
            std::process::exit(1);
        });

    let targets: Vec<(u64, u64)> = if let Some(rate) = cfg.rate {
        vec![(0, rate)]
    } else {
        cfg.load_levels
            .iter()
            .map(|&pct| (pct, (max_rate * pct as f64 / 100.0).round() as u64))
            .filter(|(_, rate)| *rate > 0)
            .collect()
    };

    if targets.is_empty() {
        eprintln!("error: no positive target rates computed");
        std::process::exit(2);
    }

    let mut loads = Vec::with_capacity(targets.len());
    let mut any_invalid = !provenance_valid;
    for (pct, target_rate) in targets {
        match run_load(&cfg, target_rate) {
            Ok(mut load) => {
                load.load_pct = pct;
                load.valid_run &= provenance_valid;
                if !load.valid_run {
                    any_invalid = true;
                }
                loads.push(load);
            }
            Err(e) => {
                eprintln!("load {pct}% error: {e}");
                std::process::exit(1);
            }
        }
    }

    for load in &loads {
        let path = samples_path_for_load(&cfg.samples_output, load.load_pct);
        if let Err(error) = ensure_parent(&path).and_then(|()| write_samples(&path, &load.samples))
        {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }

    let json = write_result(&cfg, max_rate, &loads, provenance);

    if let Some(path) = &cfg.output {
        if let Err(error) = ensure_parent(path).and_then(|()| {
            fs::write(path, &json)
                .map_err(|error| format!("failed to write {}: {error}", path.display()))
        }) {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
    print!("{json}");

    if any_invalid {
        eprintln!(
            "invalid run: build provenance, actual/target rate, or injected-pause validation failed"
        );
        std::process::exit(3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_f64_eq(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < f64::EPSILON,
            "expected {expected}, got {actual}"
        );
    }

    fn sample(sequence: u64, latency_ns: u64) -> LatencySample {
        LatencySample {
            sequence,
            planned_ns: sequence,
            completion_ns: sequence + latency_ns,
            latency_ns,
        }
    }

    #[test]
    fn fixed_schedule_uses_integer_nanosecond_offsets_without_drift() {
        assert_eq!(scheduled_ns(0, 3).unwrap(), 0);
        assert_eq!(scheduled_ns(1, 3).unwrap(), 333_333_333);
        assert_eq!(scheduled_ns(2, 3).unwrap(), 666_666_666);
        assert_eq!(scheduled_ns(3, 3).unwrap(), 1_000_000_000);
        assert_eq!(scheduled_ns(1_000_000, 1_000_000).unwrap(), 1_000_000_000);
    }

    #[test]
    fn percentile_uses_nearest_rank_for_tail_boundaries() {
        let values = (1..=10_000).collect::<Vec<u64>>();
        assert_f64_eq(percentile_basis_points(&values, 5_000), 5_000.0);
        assert_f64_eq(percentile_basis_points(&values, 9_900), 9_900.0);
        assert_f64_eq(percentile_basis_points(&values, 9_990), 9_990.0);
        assert_f64_eq(percentile_basis_points(&values, 9_999), 9_999.0);
    }

    #[test]
    fn injected_pause_is_taken_once_at_the_middle_of_recorded_events() {
        let start = Arc::new(OnceLock::new());
        let processed = Arc::new(AtomicU64::new(0));
        let ready = Arc::new(AtomicU64::new(0));
        let sink = Arc::new(Mutex::new(None));
        let mut handler = LatencyHandler::new(
            start,
            processed,
            ready,
            10_000,
            110_000,
            Some(Duration::from_millis(50)),
            None,
            Arc::new(AtomicBool::new(false)),
            sink,
        );

        assert_eq!(handler.inject_sequence, Some(60_000));
        assert_eq!(handler.take_injection(59_999), None);
        assert_eq!(
            handler.take_injection(60_000),
            Some(Duration::from_millis(50))
        );
        assert_eq!(handler.take_injection(60_000), None);
        assert_eq!(handler.take_injection(60_001), None);
    }

    #[test]
    fn injected_pause_gate_requires_both_p999_and_max_visibility() {
        let visible = LatencyStats {
            p50: 1_000.0,
            p99: 2_000.0,
            p99_9: 30_000_000.0,
            p99_99: 49_000_000.0,
            max: 50_000_000.0,
            mean: 1_000.0,
            min: 500.0,
            count: 100_000,
        };
        let check = validate_injected_pause(Duration::from_millis(50), 100_000, &visible);
        assert!(check.enough_samples_for_p99_9());
        assert!(check.p99_9_visible());
        assert!(check.max_visible());
        assert!(check.is_valid());

        let hidden = LatencyStats {
            p99_9: 2_000.0,
            max: 3_000.0,
            ..visible
        };
        assert!(!validate_injected_pause(Duration::from_millis(50), 100_000, &hidden).is_valid());
    }

    #[test]
    fn actual_rate_gate_is_inclusive_at_95_percent() {
        assert!(rate_is_valid(95_000.0, 100_000));
        assert!(!rate_is_valid(94_999.0, 100_000));
    }

    #[test]
    fn build_provenance_requires_a_clean_full_git_revision() {
        let clean = BuildProvenance {
            git_rev: "0123456789abcdef0123456789abcdef01234567",
            dirty: Some(false),
        };
        assert!(clean.is_valid());
        assert!(!BuildProvenance {
            dirty: Some(true),
            ..clean
        }
        .is_valid());
        assert!(!BuildProvenance {
            git_rev: "unknown",
            ..clean
        }
        .is_valid());
        assert!(!BuildProvenance {
            dirty: None,
            ..clean
        }
        .is_valid());
    }

    #[test]
    fn statistics_use_latency_field_not_absolute_timestamps() {
        let samples = vec![sample(0, 10), sample(1, 20), sample(2, 30), sample(3, 40)];
        let stats = compute_stats(&samples);
        assert_eq!(stats.count, 4);
        assert_f64_eq(stats.min, 10.0);
        assert_f64_eq(stats.p50, 20.0);
        assert_f64_eq(stats.p99_99, 40.0);
        assert_f64_eq(stats.max, 40.0);
        assert_f64_eq(stats.mean, 25.0);
    }
}
