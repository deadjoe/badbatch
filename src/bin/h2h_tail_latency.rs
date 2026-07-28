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
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
const LOGICAL_PAYLOAD_BYTES: usize = 32;
const ALLOCATION_PAYLOAD_BYTES: usize = 48;
const DEFAULT_INJECT_AT_MEASURED_PCT: u64 = 25;

struct CountingAllocator;

thread_local! {
    static ALLOCATION_TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATION_COUNT: Cell<u64> = const { Cell::new(0) };
    static ALLOCATION_BYTES: Cell<u64> = const { Cell::new(0) };
    static DEALLOCATION_COUNT: Cell<u64> = const { Cell::new(0) };
    static DEALLOCATION_BYTES: Cell<u64> = const { Cell::new(0) };
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: delegate the exact allocation request to the system allocator.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && ALLOCATION_TRACKING.get() {
            ALLOCATION_COUNT.set(ALLOCATION_COUNT.get().saturating_add(1));
            ALLOCATION_BYTES.set(
                ALLOCATION_BYTES
                    .get()
                    .saturating_add(u64::try_from(layout.size()).unwrap_or(u64::MAX)),
            );
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if ALLOCATION_TRACKING.get() {
            DEALLOCATION_COUNT.set(DEALLOCATION_COUNT.get().saturating_add(1));
            DEALLOCATION_BYTES.set(
                DEALLOCATION_BYTES
                    .get()
                    .saturating_add(u64::try_from(layout.size()).unwrap_or(u64::MAX)),
            );
        }
        // SAFETY: `pointer` and `layout` came from the delegated system allocator.
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AllocationCounters {
    allocations: u64,
    allocated_bytes: u64,
    deallocations: u64,
    deallocated_bytes: u64,
}

fn reset_allocation_counters() {
    ALLOCATION_COUNT.set(0);
    ALLOCATION_BYTES.set(0);
    DEALLOCATION_COUNT.set(0);
    DEALLOCATION_BYTES.set(0);
}

fn allocation_counters() -> AllocationCounters {
    AllocationCounters {
        allocations: ALLOCATION_COUNT.get(),
        allocated_bytes: ALLOCATION_BYTES.get(),
        deallocations: DEALLOCATION_COUNT.get(),
        deallocated_bytes: DEALLOCATION_BYTES.get(),
    }
}

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

#[repr(C)]
struct AllocationPayload {
    logical: [u64; 4],
    matching_padding: [u8; ALLOCATION_PAYLOAD_BYTES - LOGICAL_PAYLOAD_BYTES],
}

const _: () = assert!(std::mem::size_of::<AllocationPayload>() == ALLOCATION_PAYLOAD_BYTES);

impl AllocationPayload {
    fn new(sequence: u64) -> Self {
        Self {
            logical: [
                sequence,
                sequence.wrapping_add(1),
                sequence.wrapping_add(2),
                sequence.wrapping_add(3),
            ],
            matching_padding: [0; ALLOCATION_PAYLOAD_BYTES - LOGICAL_PAYLOAD_BYTES],
        }
    }

    fn checksum(&self) -> u64 {
        self.logical
            .iter()
            .fold(0_u64, |sum, value| sum.wrapping_add(*value))
    }
}

trait HandlerWorkload: Send + 'static {
    fn on_start(&mut self);
    fn apply(&mut self, sequence: u64, track: bool);
    fn stats(&self) -> WorkloadStats;
}

#[derive(Default)]
struct AllocationFreeWorkload;

impl HandlerWorkload for AllocationFreeWorkload {
    #[inline(always)]
    fn on_start(&mut self) {}

    #[inline(always)]
    fn apply(&mut self, _sequence: u64, _track: bool) {}

    fn stats(&self) -> WorkloadStats {
        WorkloadStats::default()
    }
}

struct AllocatingWorkload {
    retention: Vec<Option<Box<AllocationPayload>>>,
    next: usize,
}

impl AllocatingWorkload {
    fn new(retention_window: usize) -> Self {
        Self {
            retention: std::iter::repeat_with(|| None)
                .take(retention_window)
                .collect(),
            next: 0,
        }
    }
}

impl HandlerWorkload for AllocatingWorkload {
    fn on_start(&mut self) {
        reset_allocation_counters();
    }

    #[inline]
    fn apply(&mut self, sequence: u64, track: bool) {
        ALLOCATION_TRACKING.set(track);
        self.retention[self.next] = Some(Box::new(AllocationPayload::new(sequence)));
        ALLOCATION_TRACKING.set(false);
        self.next = (self.next + 1) % self.retention.len();
    }

    fn stats(&self) -> WorkloadStats {
        let retained_objects = self
            .retention
            .iter()
            .filter(|payload| payload.is_some())
            .count();
        let retained_checksum = self
            .retention
            .iter()
            .flatten()
            .fold(0_u64, |sum, payload| sum.wrapping_add(payload.checksum()));
        WorkloadStats {
            counters: allocation_counters(),
            retained_objects,
            retained_live_bytes: u64::try_from(retained_objects)
                .unwrap_or(u64::MAX)
                .saturating_mul(ALLOCATION_PAYLOAD_BYTES as u64),
            retained_checksum,
        }
    }
}

// --- CLI ----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandlerMode {
    AllocationFree,
    Allocating,
}

impl HandlerMode {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "allocation-free" => Ok(Self::AllocationFree),
            "allocating" => Ok(Self::Allocating),
            _ => Err(format!("unsupported handler-mode: {s}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::AllocationFree => "allocation-free",
            Self::Allocating => "allocating",
        }
    }
}

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
    handler_mode: HandlerMode,
    retention_window: Option<usize>,
    wait: WaitKind,
    pad: Pad,
    buffer_size: usize,
    events_total: u64,
    warmup_events: u64,
    rate: Option<u64>,
    load_levels: Vec<u64>,
    calibration_events: u64,
    calibration_duration: Duration,
    calibrate_only: bool,
    max_rate: Option<u64>,
    own_max: Option<u64>,
    cpu_list: Vec<usize>,
    affinity_failed: Arc<AtomicBool>,
    output: Option<PathBuf>,
    samples_output: Option<PathBuf>,
    timeout: Duration,
    inject_sleep: Option<Duration>,
    inject_at_measured_pct: u64,
}

fn parse_args() -> Result<Config, String> {
    let mut args = env::args().skip(1);
    let mut handler_mode = HandlerMode::AllocationFree;
    let mut retention_window = None;
    let mut wait = WaitKind::BusySpin;
    let mut pad = Pad::None;
    let mut buffer_size = None;
    let mut events_total = None;
    let mut warmup_events = None;
    let mut rate = None;
    let mut load_levels = None;
    let mut calibration_events = None;
    let mut calibration_duration_ms = None;
    let mut calibrate_only = false;
    let mut max_rate = None;
    let mut own_max = None;
    let mut cpu_list = Vec::new();
    let mut output = None;
    let mut samples_output = None;
    let mut inject_sleep = None;
    let mut inject_at_measured_pct = DEFAULT_INJECT_AT_MEASURED_PCT;
    let mut quick = false;
    // `quick` is consumed below when applying smaller defaults.

    while let Some(a) = args.next() {
        match a.as_str() {
            "--handler-mode" => {
                handler_mode =
                    HandlerMode::parse(&args.next().ok_or("missing --handler-mode value")?)?;
            }
            "--retention-window" => {
                retention_window = Some(
                    args.next()
                        .ok_or("missing --retention-window")?
                        .replace('_', "")
                        .parse()
                        .map_err(|e| format!("retention-window: {e}"))?,
                );
            }
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
            "--calibrate-only" => calibrate_only = true,
            "--max-rate" => {
                max_rate = Some(
                    args.next()
                        .ok_or("missing --max-rate")?
                        .replace('_', "")
                        .parse()
                        .map_err(|e| format!("max-rate: {e}"))?,
                );
            }
            "--own-max" => {
                own_max = Some(
                    args.next()
                        .ok_or("missing --own-max")?
                        .replace('_', "")
                        .parse()
                        .map_err(|e| format!("own-max: {e}"))?,
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
            "--inject-at-measured-pct" => {
                inject_at_measured_pct = args
                    .next()
                    .ok_or("missing --inject-at-measured-pct")?
                    .parse()
                    .map_err(|e| format!("inject-at-measured-pct: {e}"))?;
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
    if !calibrate_only && warmup_events >= events_total {
        return Err("warmup-events must be < events-total".into());
    }
    let recorded_samples = events_total.saturating_sub(warmup_events);
    if !calibrate_only && recorded_samples < MIN_RECORDED_SAMPLES {
        return Err(format!(
            "events-total - warmup-events must be at least {MIN_RECORDED_SAMPLES} \
             for p99.99, got {recorded_samples}"
        ));
    }
    if !calibrate_only {
        usize::try_from(recorded_samples)
            .map_err(|_| "recorded sample count must fit usize".to_string())?;
    }
    if calibration_duration_ms == 0 {
        return Err("calibration-duration-ms must be > 0".into());
    }
    if calibration_events == 0 {
        return Err("calibration-events must be > 0".into());
    }
    if calibration_events > i64::MAX as u64 {
        return Err("calibration-events must fit i64".into());
    }
    if !(1..=99).contains(&inject_at_measured_pct) {
        return Err("inject-at-measured-pct must be in 1..=99".into());
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
    if let Some(r) = own_max {
        if r == 0 {
            return Err("own-max must be > 0".into());
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

    match (handler_mode, retention_window) {
        (HandlerMode::AllocationFree, Some(_)) => {
            return Err("retention-window is valid only with handler-mode=allocating".into());
        }
        (HandlerMode::Allocating, None | Some(0)) => {
            return Err("handler-mode=allocating requires retention-window > 0".into());
        }
        (HandlerMode::Allocating, Some(window))
            if !calibrate_only && !measured_window_covers_retention(recorded_samples, window) =>
        {
            return Err("measured events must reach the full allocating retention-window".into());
        }
        _ => {}
    }

    let calibration_warmup_events = warmup_events.max(
        retention_window
            .and_then(|window| u64::try_from(window).ok())
            .unwrap_or(0),
    );
    if calibration_warmup_events
        .checked_add(calibration_events)
        .is_none_or(|events| events > i64::MAX as u64)
    {
        return Err("calibration warmup + events must fit i64".into());
    }

    let load_levels = load_levels.unwrap_or_else(|| DEFAULT_LOAD_LEVELS.to_vec());
    if !calibrate_only && rate.is_none() && load_levels.is_empty() {
        return Err("load-levels must contain at least one percentage".into());
    }
    if !calibrate_only && samples_output.is_none() {
        return Err("required: --samples-output".into());
    }
    if calibrate_only
        && (rate.is_some()
            || max_rate.is_some()
            || own_max.is_some()
            || inject_sleep.is_some()
            || samples_output.is_some())
    {
        return Err(
            "calibrate-only rejects measurement-only rate/max/own-max/injection/sample flags"
                .into(),
        );
    }

    Ok(Config {
        handler_mode,
        retention_window,
        wait,
        pad,
        buffer_size: buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE),
        events_total,
        warmup_events,
        rate,
        load_levels,
        calibration_events,
        calibration_duration: Duration::from_millis(calibration_duration_ms),
        calibrate_only,
        max_rate,
        own_max,
        cpu_list,
        affinity_failed: Arc::new(AtomicBool::new(false)),
        output,
        samples_output,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        inject_sleep,
        inject_at_measured_pct,
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

fn measured_window_covers_retention(recorded_samples: u64, retention_window: usize) -> bool {
    u64::try_from(retention_window).is_ok_and(|window| recorded_samples >= window)
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

    fn calibration_warmup_events(&self) -> u64 {
        self.warmup_events.max(
            self.retention_window
                .and_then(|window| u64::try_from(window).ok())
                .unwrap_or(0),
        )
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
  --handler-mode <allocation-free|allocating>
                                        (default allocation-free)
  --retention-window <N>                (required only for allocating mode)
  --wait-strategy <yielding|busy-spin>  (default busy-spin)
  --event-padding <none|128>            (slot padding; default none)
  --buffer-size <N>                     (power of two; default 65536)
  --events-total <N>                    (events per load level; default 1_000_000)
  --warmup-events <N>                   (discarded before recording; default 100_000)
  --rate <events/s>                     (single fixed rate; overrides load-levels)
  --load-levels <pct,pct,...>           (percent of max throughput; default 50,70,90)
  --max-rate <events/s>                 (skip calibration; use this as max throughput)
  --own-max <events/s>                  (this arm's independently calibrated max)
  --calibrate-only                      (emit this arm's max; no latency samples)
  --calibration-events <N>              (max events for calibration; default 100_000_000)
  --calibration-duration-ms <N>         (max duration for calibration; default 2000)
  --cpu-list <N,N,...>                  (pin producer/consumer to logical CPUs)
  --inject-sleep-ms <N>                 (inject one N-ms handler sleep for CO validation)
  --inject-at-measured-pct <1..99>      (default 25)
  --output <path.json>                  (latency statistics JSON)
  --samples-output <path.csv>           (required raw schedule/completion samples)
  --quick                               (smaller defaults)
"
    );
}

// --- handlers -----------------------------------------------------------------------

struct CalibrationHandler<H> {
    start: Arc<OnceLock<Instant>>,
    processed: Arc<AtomicU64>,
    ready: Arc<AtomicU64>,
    warmup_events: u64,
    workload: H,
    scratch: Vec<LatencySample>,
    cpu_affinity: Option<usize>,
    affinity_failed: Arc<AtomicBool>,
}

impl<H: HandlerWorkload> EventHandler<LatencyEvent> for CalibrationHandler<H> {
    fn on_event(
        &mut self,
        event: &mut LatencyEvent,
        sequence: i64,
        end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let sequence = u64::try_from(sequence).expect("calibration sequence must be non-negative");
        self.workload
            .apply(sequence, sequence >= self.warmup_events);
        let completion_ns = self
            .start
            .get()
            .expect("calibration epoch must be initialized")
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);
        let scratch_mask = u64::try_from(self.scratch.len() - 1).expect("buffer mask must fit u64");
        let index =
            usize::try_from(sequence & scratch_mask).expect("masked sequence must fit usize");
        self.scratch[index] = LatencySample {
            sequence,
            planned_ns: event.planned_ns,
            completion_ns,
            latency_ns: completion_ns.saturating_sub(event.planned_ns),
        };
        if end_of_batch {
            let completed = sequence.saturating_add(1);
            self.processed.store(completed, Ordering::Release);
        }
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        self.workload.on_start();
        pin_current_thread(self.cpu_affinity, &self.affinity_failed);
        self.ready.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn on_shutdown(&mut self) -> DisruptorResult<()> {
        std::hint::black_box(self.workload.stats().retained_checksum);
        std::hint::black_box(&self.scratch);
        Ok(())
    }
}

#[derive(Debug, Default)]
struct WorkloadStats {
    counters: AllocationCounters,
    retained_objects: usize,
    retained_live_bytes: u64,
    retained_checksum: u64,
}

#[derive(Debug)]
struct HandlerOutcome {
    samples: Vec<LatencySample>,
    workload: WorkloadStats,
    injection: Option<InjectionObservation>,
}

#[derive(Debug, Clone, Copy)]
struct InjectionObservation {
    sequence: u64,
    planned_ns: u64,
    started_ns: u64,
    completed_ns: u64,
}

struct LatencyHandler<H> {
    start: Arc<OnceLock<Instant>>,
    processed: Arc<AtomicU64>,
    ready: Arc<AtomicU64>,
    warmup_events: u64,
    final_sequence: i64,
    inject_sleep: Option<Duration>,
    inject_sequence: Option<u64>,
    injected: bool,
    injection_observation: Option<InjectionObservation>,
    workload: H,
    samples: Vec<LatencySample>,
    outcome_sink: Arc<Mutex<Option<HandlerOutcome>>>,
    cpu_affinity: Option<usize>,
    affinity_failed: Arc<AtomicBool>,
}

impl<H: HandlerWorkload> LatencyHandler<H> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        start: Arc<OnceLock<Instant>>,
        processed: Arc<AtomicU64>,
        ready: Arc<AtomicU64>,
        warmup_events: u64,
        events_total: u64,
        inject_sleep: Option<Duration>,
        inject_at_measured_pct: u64,
        workload: H,
        cpu_affinity: Option<usize>,
        affinity_failed: Arc<AtomicBool>,
        outcome_sink: Arc<Mutex<Option<HandlerOutcome>>>,
    ) -> Self {
        Self {
            start,
            processed,
            ready,
            warmup_events,
            final_sequence: i64::try_from(events_total).expect("events_total must fit i64") - 1,
            inject_sleep,
            inject_sequence: inject_sleep.map(|_| {
                warmup_events + (events_total - warmup_events) * inject_at_measured_pct / 100
            }),
            injected: false,
            injection_observation: None,
            workload,
            samples: Vec::with_capacity(
                usize::try_from(events_total - warmup_events)
                    .expect("validated sample count must fit usize"),
            ),
            outcome_sink,
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

impl<H: HandlerWorkload> EventHandler<LatencyEvent> for LatencyHandler<H> {
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
            let epoch = self
                .start
                .get()
                .expect("measurement epoch must be set before publishing");
            let started_ns = epoch.elapsed().as_nanos().try_into().unwrap_or(u64::MAX);
            thread::sleep(sleep);
            let completed_ns = epoch.elapsed().as_nanos().try_into().unwrap_or(u64::MAX);
            self.injection_observation = Some(InjectionObservation {
                sequence,
                planned_ns: event.planned_ns,
                started_ns,
                completed_ns,
            });
        }

        self.workload
            .apply(sequence, sequence >= self.warmup_events);

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
        self.workload.on_start();
        pin_current_thread(self.cpu_affinity, &self.affinity_failed);
        self.ready.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn on_shutdown(&mut self) -> DisruptorResult<()> {
        let samples = std::mem::take(&mut self.samples);
        let workload = self.workload.stats();
        *self
            .outcome_sink
            .lock()
            .expect("latency outcome sink mutex poisoned") = Some(HandlerOutcome {
            samples,
            workload,
            injection: self.injection_observation,
        });
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
    match cfg.handler_mode {
        HandlerMode::AllocationFree => {
            calibrate_max_rate_with_workload(cfg, wait, AllocationFreeWorkload)
        }
        HandlerMode::Allocating => calibrate_max_rate_with_workload(
            cfg,
            wait,
            AllocatingWorkload::new(
                cfg.retention_window
                    .expect("validated allocating retention window"),
            ),
        ),
    }
}

fn calibrate_max_rate_with_workload<W, H>(cfg: &Config, wait: W, workload: H) -> Result<f64, String>
where
    W: badbatch::disruptor::WaitStrategy + Clone + 'static,
    H: HandlerWorkload,
{
    let start = Arc::new(OnceLock::new());
    let processed = Arc::new(AtomicU64::new(0));
    let ready = Arc::new(AtomicU64::new(0));
    let calibration_warmup_events = cfg.calibration_warmup_events();

    let handler = CalibrationHandler {
        start: Arc::clone(&start),
        processed: Arc::clone(&processed),
        ready: Arc::clone(&ready),
        warmup_events: calibration_warmup_events,
        workload,
        scratch: vec![
            LatencySample {
                sequence: 0,
                planned_ns: 0,
                completion_ns: 0,
                latency_ns: 0,
            };
            cfg.buffer_size
        ],
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

    start
        .set(Instant::now())
        .map_err(|_| "calibration epoch was already initialized".to_string())?;
    for sequence in 0..calibration_warmup_events {
        handle
            .publish(|event| {
                event.sequence = i64::try_from(sequence).expect("calibration warmup must fit i64");
                event.planned_ns = 0;
            })
            .map_err(|error| {
                format!("calibration warmup publish failed at sequence {sequence}: {error}")
            })?;
    }
    if !wait_count(&processed, calibration_warmup_events, cfg.timeout) {
        handle.shutdown();
        return Err(format!(
            "timeout waiting for calibration warmup completion (got {})",
            processed.load(Ordering::Acquire)
        ));
    }

    let measure_start = Instant::now();
    let mut published = 0u64;
    while published < cfg.calibration_events && measure_start.elapsed() < cfg.calibration_duration {
        let sequence = calibration_warmup_events + published;
        handle
            .publish(|event| {
                event.sequence =
                    i64::try_from(sequence).expect("calibration sequence must fit i64");
                event.planned_ns = 0;
            })
            .map_err(|error| {
                format!("calibration publish failed at sequence {sequence}: {error}")
            })?;
        published += 1;
    }

    let expected_processed = calibration_warmup_events + published;
    if !wait_count(&processed, expected_processed, cfg.timeout) {
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
    own_max: f64,
    own_utilization: f64,
    actual_rate: f64,
    rate_valid: bool,
    workload_valid: bool,
    valid_run: bool,
    measurement_epoch_unix_ns: u64,
    clock_anchor_uncertainty_ns: u64,
    pause_check: Option<PauseCheck>,
    samples: Vec<LatencySample>,
    workload: WorkloadStats,
}

#[derive(Debug, Clone, Copy)]
struct ClockAnchor {
    monotonic: Instant,
    wall_unix_ns: u64,
    uncertainty_ns: u64,
}

impl ClockAnchor {
    fn capture() -> Result<Self, String> {
        let before = Instant::now();
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system clock predates Unix epoch: {error}"))?;
        let after = Instant::now();
        let span = after.duration_since(before);
        let midpoint = before + span / 2;
        let wall_unix_ns = u64::try_from(wall.as_nanos())
            .map_err(|_| "wall-clock timestamp does not fit u64 nanoseconds".to_string())?;
        let uncertainty_ns = u64::try_from(span.as_nanos().div_ceil(2)).unwrap_or(u64::MAX);
        Ok(Self {
            monotonic: midpoint,
            wall_unix_ns,
            uncertainty_ns,
        })
    }

    fn project_unix_ns(self, target: Instant) -> Result<u64, String> {
        let delta = target
            .checked_duration_since(self.monotonic)
            .ok_or("measurement epoch predates clock anchor")?;
        let delta_ns = u64::try_from(delta.as_nanos())
            .map_err(|_| "clock-anchor delta does not fit u64 nanoseconds".to_string())?;
        self.wall_unix_ns
            .checked_add(delta_ns)
            .ok_or_else(|| "measurement epoch Unix timestamp overflow".to_string())
    }
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
    injection_sequence: u64,
    injection_planned_ns: u64,
    pause_started_ns: u64,
    pause_completed_ns: u64,
    expected_affected_samples: u64,
    minimum_affected_samples: u64,
    maximum_affected_samples: u64,
    sample_count: usize,
    load_fraction: f64,
    minimum_drain_ns: u64,
    remaining_planned_ns_after_pause: u64,
    observed_p99_9_ns: f64,
    observed_max_ns: f64,
}

impl PauseCheck {
    fn backlog_in_range(self) -> bool {
        self.expected_affected_samples >= self.minimum_affected_samples
            && self.expected_affected_samples <= self.maximum_affected_samples
    }

    fn p99_9_visible(self) -> bool {
        self.backlog_in_range() && self.observed_p99_9_ns >= self.sleep_ns as f64 * 0.5
    }

    fn max_visible(self) -> bool {
        self.observed_max_ns >= self.sleep_ns as f64 * 0.8
    }

    fn drain_allowance_met(self) -> bool {
        self.remaining_planned_ns_after_pause >= self.minimum_drain_ns
    }

    fn double_drain_allowance_met(self) -> bool {
        self.remaining_planned_ns_after_pause >= self.minimum_drain_ns.saturating_mul(2)
    }

    fn is_valid(self) -> bool {
        self.backlog_in_range()
            && self.drain_allowance_met()
            && self.p99_9_visible()
            && self.max_visible()
    }
}

fn run_load(
    cfg: &Config,
    target_rate: u64,
    common_max: f64,
    own_max: f64,
) -> Result<LoadResult, String> {
    match cfg.wait {
        WaitKind::BusySpin => run_load_w(
            cfg,
            target_rate,
            common_max,
            own_max,
            BusySpinWaitStrategy::new(),
        ),
        WaitKind::Yielding => run_load_w(
            cfg,
            target_rate,
            common_max,
            own_max,
            YieldingWaitStrategy::new(),
        ),
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

fn run_load_w<W>(
    cfg: &Config,
    target_rate: u64,
    common_max: f64,
    own_max: f64,
    wait: W,
) -> Result<LoadResult, String>
where
    W: badbatch::disruptor::WaitStrategy + Clone + 'static,
{
    match cfg.handler_mode {
        HandlerMode::AllocationFree => run_load_with_workload(
            cfg,
            target_rate,
            common_max,
            own_max,
            wait,
            AllocationFreeWorkload,
        ),
        HandlerMode::Allocating => run_load_with_workload(
            cfg,
            target_rate,
            common_max,
            own_max,
            wait,
            AllocatingWorkload::new(
                cfg.retention_window
                    .expect("validated allocating retention window"),
            ),
        ),
    }
}

fn run_load_with_workload<W, H>(
    cfg: &Config,
    target_rate: u64,
    common_max: f64,
    own_max: f64,
    wait: W,
    workload: H,
) -> Result<LoadResult, String>
where
    W: badbatch::disruptor::WaitStrategy + Clone + 'static,
    H: HandlerWorkload,
{
    let start = Arc::new(OnceLock::new());
    let processed = Arc::new(AtomicU64::new(0));
    let ready = Arc::new(AtomicU64::new(0));
    let outcome_sink = Arc::new(Mutex::new(None));

    let handler = LatencyHandler::new(
        Arc::clone(&start),
        Arc::clone(&processed),
        Arc::clone(&ready),
        cfg.warmup_events,
        cfg.events_total,
        cfg.inject_sleep,
        cfg.inject_at_measured_pct,
        workload,
        cfg.cpu(1),
        Arc::clone(&cfg.affinity_failed),
        Arc::clone(&outcome_sink),
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

    let clock_anchor = ClockAnchor::capture()?;
    let measurement_epoch = Instant::now();
    let measurement_epoch_unix_ns = clock_anchor.project_unix_ns(measurement_epoch)?;
    start
        .set(measurement_epoch)
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
    let outcome = outcome_sink
        .lock()
        .map_err(|_| "outcome sink mutex poisoned".to_string())?
        .take()
        .ok_or("consumer did not publish latency outcome during shutdown")?;
    let samples = outcome.samples;
    let expected_samples = usize::try_from(cfg.events_total - cfg.warmup_events)
        .map_err(|_| "sample count does not fit usize")?;
    if samples.len() != expected_samples {
        return Err(format!(
            "latency sample count mismatch: expected {expected_samples}, got {}",
            samples.len()
        ));
    }
    let expected_allocations = u64::try_from(expected_samples).unwrap_or(u64::MAX);
    let workload_valid = match cfg.handler_mode {
        HandlerMode::AllocationFree => {
            outcome.workload.counters.allocations == 0
                && outcome.workload.counters.allocated_bytes == 0
                && outcome.workload.counters.deallocations == 0
                && outcome.workload.counters.deallocated_bytes == 0
                && outcome.workload.retained_objects == 0
                && outcome.workload.retained_live_bytes == 0
                && outcome.workload.retained_checksum == 0
        }
        HandlerMode::Allocating => {
            let retention_window = cfg
                .retention_window
                .expect("validated allocating retention window");
            let expected_retained = retention_window;
            let expected_deallocations = cfg.events_total.saturating_sub(
                cfg.warmup_events
                    .max(u64::try_from(retention_window).unwrap_or(u64::MAX)),
            );
            outcome.workload.counters.allocations == expected_allocations
                && outcome.workload.counters.allocated_bytes
                    == expected_allocations.saturating_mul(ALLOCATION_PAYLOAD_BYTES as u64)
                && outcome.workload.counters.deallocations == expected_deallocations
                && outcome.workload.counters.deallocated_bytes
                    == expected_deallocations.saturating_mul(ALLOCATION_PAYLOAD_BYTES as u64)
                && outcome.workload.retained_objects == expected_retained
                && outcome.workload.retained_live_bytes
                    == u64::try_from(expected_retained)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(ALLOCATION_PAYLOAD_BYTES as u64)
        }
    };

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
    let last_planned_ns = scheduled_ns(cfg.events_total - 1, target_rate)?;
    let pause_check = cfg
        .inject_sleep
        .map(|sleep| {
            let observation = outcome
                .injection
                .ok_or("configured pause injection was not observed")?;
            validate_injected_pause(
                sleep,
                target_rate,
                common_max,
                &stats,
                observation,
                last_planned_ns,
            )
        })
        .transpose()?;
    let valid_run = rate_valid && workload_valid && pause_check.is_none_or(PauseCheck::is_valid);

    Ok(LoadResult {
        load_pct: 0,
        target_rate,
        own_max,
        own_utilization: target_rate as f64 / own_max,
        actual_rate,
        rate_valid,
        workload_valid,
        valid_run,
        measurement_epoch_unix_ns,
        clock_anchor_uncertainty_ns: clock_anchor.uncertainty_ns,
        pause_check,
        samples,
        workload: outcome.workload,
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

fn validate_injected_pause(
    sleep: Duration,
    target_rate: u64,
    common_max: f64,
    stats: &LatencyStats,
    observation: InjectionObservation,
    last_planned_ns: u64,
) -> Result<PauseCheck, String> {
    let sleep_ns = sleep.as_nanos().try_into().unwrap_or(u64::MAX);
    let affected_numerator = u128::from(target_rate).saturating_mul(u128::from(sleep_ns));
    let expected_affected_samples = affected_numerator
        .saturating_add(999_999_999)
        .checked_div(1_000_000_000)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(u64::MAX)
        .max(1);
    let sample_count = u64::try_from(stats.count).unwrap_or(u64::MAX);
    let minimum_affected_samples = sample_count.saturating_add(999) / 1_000;
    let maximum_affected_samples = sample_count / 10;
    if !common_max.is_finite() || common_max <= 0.0 {
        return Err("common max must be finite and positive".into());
    }
    let common_max_u64 = common_max.floor() as u64;
    let minimum_drain_ns = if target_rate < common_max_u64 {
        let numerator = u128::from(sleep_ns).saturating_mul(u128::from(target_rate));
        let denominator = u128::from(common_max_u64 - target_rate);
        u64::try_from(numerator.div_ceil(denominator)).unwrap_or(u64::MAX)
    } else {
        u64::MAX
    };
    Ok(PauseCheck {
        sleep_ns,
        injection_sequence: observation.sequence,
        injection_planned_ns: observation.planned_ns,
        pause_started_ns: observation.started_ns,
        pause_completed_ns: observation.completed_ns,
        expected_affected_samples,
        minimum_affected_samples,
        maximum_affected_samples,
        sample_count: stats.count,
        load_fraction: target_rate as f64 / common_max,
        minimum_drain_ns,
        remaining_planned_ns_after_pause: last_planned_ns.saturating_sub(observation.completed_ns),
        observed_p99_9_ns: stats.p99_9,
        observed_max_ns: stats.max,
    })
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
    common_max: Option<f64>,
    own_max: f64,
    loads: &[LoadResult],
    provenance: BuildProvenance,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("{\n");
    writeln!(out, "  \"impl\": \"badbatch-builder-tail-latency\",").unwrap();
    out.push_str("  \"language\": \"rust\",\n");
    writeln!(
        out,
        "  \"run_mode\": \"{}\",",
        if cfg.calibrate_only {
            "calibration"
        } else {
            "measurement"
        }
    )
    .unwrap();
    out.push_str("  \"scenario\": \"unicast_tail_latency\",\n");
    out.push_str("  \"arrival_model\": \"open_loop_fixed_schedule\",\n");
    out.push_str("  \"latency_origin\": \"planned_send_time\",\n");
    out.push_str(
        "  \"raw_sample_columns\": [\"sequence\", \"planned_ns\", \
         \"completion_ns\", \"latency_ns\"],\n",
    );
    writeln!(out, "  \"wait_strategy\": \"{}\",", cfg.wait.as_str()).unwrap();
    writeln!(out, "  \"event_padding\": \"{}\",", cfg.pad.as_str()).unwrap();
    writeln!(
        out,
        "  \"handler_mode\": \"{}\",",
        cfg.handler_mode.as_str()
    )
    .unwrap();
    match cfg.retention_window {
        Some(window) => writeln!(out, "  \"retention_window\": {window},").unwrap(),
        None => out.push_str("  \"retention_window\": null,\n"),
    }
    writeln!(out, "  \"logical_payload_bytes\": {LOGICAL_PAYLOAD_BYTES},").unwrap();
    writeln!(
        out,
        "  \"allocation_payload_bytes\": {ALLOCATION_PAYLOAD_BYTES},"
    )
    .unwrap();
    out.push_str("  \"api_path\": \"builder\",\n");
    writeln!(out, "  \"buffer_size\": {},", cfg.buffer_size).unwrap();
    writeln!(out, "  \"events_total\": {},", cfg.events_total).unwrap();
    writeln!(out, "  \"warmup_events\": {},", cfg.warmup_events).unwrap();
    writeln!(
        out,
        "  \"calibration_warmup_events\": {},",
        cfg.calibration_warmup_events()
    )
    .unwrap();
    writeln!(
        out,
        "  \"calibration_events_limit\": {},",
        cfg.calibration_events
    )
    .unwrap();
    let legacy_max_rate = common_max.unwrap_or(own_max);
    writeln!(out, "  \"max_rate\": {legacy_max_rate:.6},").unwrap();
    writeln!(out, "  \"own_max\": {own_max:.6},").unwrap();
    if let Some(common_max) = common_max {
        writeln!(out, "  \"common_max\": {common_max:.6},").unwrap();
    } else {
        out.push_str("  \"common_max\": null,\n");
    }
    let threshold = VALID_RUN_THRESHOLD;
    writeln!(out, "  \"minimum_actual_target_ratio\": {threshold:.6},").unwrap();
    writeln!(
        out,
        "  \"inject_sleep_ms\": {},",
        cfg.inject_sleep.map_or(0, |d| d.as_millis() as u64)
    )
    .unwrap();
    writeln!(
        out,
        "  \"inject_at_measured_pct\": {},",
        cfg.inject_at_measured_pct
    )
    .unwrap();
    out.push_str("  \"provenance_source\": \"build_time\",\n");
    writeln!(out, "  \"provenance_valid\": {},", provenance.is_valid()).unwrap();
    writeln!(out, "  \"git_rev\": \"{}\",", provenance.git_rev).unwrap();
    writeln!(out, "  \"dirty\": {},", provenance.dirty_json()).unwrap();
    writeln!(out, "  \"harness_git_rev\": \"{}\",", provenance.git_rev).unwrap();
    writeln!(out, "  \"harness_git_dirty\": {},", provenance.dirty_json()).unwrap();
    writeln!(
        out,
        "  \"implementation_git_rev\": \"{}\",",
        provenance.git_rev
    )
    .unwrap();
    writeln!(
        out,
        "  \"implementation_git_dirty\": {},",
        provenance.dirty_json()
    )
    .unwrap();
    writeln!(
        out,
        "  \"artifact_valid\": {},",
        provenance.is_valid() && loads.iter().all(|load| load.valid_run)
    )
    .unwrap();
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
        writeln!(out, "      \"own_max\": {:.6},", load.own_max).unwrap();
        writeln!(
            out,
            "      \"own_utilization\": {:.9},",
            load.own_utilization
        )
        .unwrap();
        writeln!(out, "      \"actual_rate\": {:.6},", load.actual_rate).unwrap();
        writeln!(
            out,
            "      \"actual_target_ratio\": {:.6},",
            load.actual_rate / load.target_rate as f64
        )
        .unwrap();
        writeln!(out, "      \"rate_valid\": {},", load.rate_valid).unwrap();
        writeln!(out, "      \"workload_valid\": {},", load.workload_valid).unwrap();
        writeln!(out, "      \"valid_run\": {},", load.valid_run).unwrap();
        writeln!(
            out,
            "      \"measurement_epoch_unix_ns\": {},",
            load.measurement_epoch_unix_ns
        )
        .unwrap();
        writeln!(
            out,
            "      \"clock_anchor_uncertainty_ns\": {},",
            load.clock_anchor_uncertainty_ns
        )
        .unwrap();
        if let Some(check) = load.pause_check {
            out.push_str("      \"pause_validation\": {\n");
            writeln!(out, "        \"sleep_ns\": {},", check.sleep_ns).unwrap();
            writeln!(
                out,
                "        \"injection_sequence\": {},",
                check.injection_sequence
            )
            .unwrap();
            writeln!(
                out,
                "        \"injection_planned_ns\": {},",
                check.injection_planned_ns
            )
            .unwrap();
            writeln!(
                out,
                "        \"pause_started_ns\": {},",
                check.pause_started_ns
            )
            .unwrap();
            writeln!(
                out,
                "        \"pause_completed_ns\": {},",
                check.pause_completed_ns
            )
            .unwrap();
            writeln!(
                out,
                "        \"expected_affected_samples\": {},",
                check.expected_affected_samples
            )
            .unwrap();
            writeln!(out, "        \"sample_count\": {},", check.sample_count).unwrap();
            writeln!(
                out,
                "        \"minimum_affected_samples\": {},",
                check.minimum_affected_samples
            )
            .unwrap();
            writeln!(
                out,
                "        \"maximum_affected_samples\": {},",
                check.maximum_affected_samples
            )
            .unwrap();
            writeln!(
                out,
                "        \"backlog_in_range\": {},",
                check.backlog_in_range()
            )
            .unwrap();
            writeln!(
                out,
                "        \"load_fraction\": {:.9},",
                check.load_fraction
            )
            .unwrap();
            writeln!(
                out,
                "        \"minimum_drain_ns\": {},",
                check.minimum_drain_ns
            )
            .unwrap();
            writeln!(
                out,
                "        \"remaining_planned_ns_after_pause\": {},",
                check.remaining_planned_ns_after_pause
            )
            .unwrap();
            writeln!(
                out,
                "        \"drain_allowance_met\": {},",
                check.drain_allowance_met()
            )
            .unwrap();
            writeln!(
                out,
                "        \"double_drain_allowance_met\": {},",
                check.double_drain_allowance_met()
            )
            .unwrap();
            writeln!(out, "        \"p99.9_visible\": {},", check.p99_9_visible()).unwrap();
            writeln!(out, "        \"max_visible\": {},", check.max_visible()).unwrap();
            writeln!(out, "        \"valid\": {}", check.is_valid()).unwrap();
            out.push_str("      },\n");
        }
        let workload = &load.workload;
        let allocation_bytes_per_event = if workload.counters.allocations == 0 {
            0.0
        } else {
            workload.counters.allocated_bytes as f64 / workload.counters.allocations as f64
        };
        let retention_seconds = cfg
            .retention_window
            .map_or(0.0, |window| window as f64 / load.target_rate as f64);
        let logical_live_bytes = u64::try_from(workload.retained_objects)
            .unwrap_or(u64::MAX)
            .saturating_mul(LOGICAL_PAYLOAD_BYTES as u64);
        out.push_str("      \"workload\": {\n");
        writeln!(out, "        \"mode\": \"{}\",", cfg.handler_mode.as_str()).unwrap();
        match cfg.retention_window {
            Some(window) => writeln!(out, "        \"retention_window\": {window},").unwrap(),
            None => out.push_str("        \"retention_window\": null,\n"),
        }
        writeln!(
            out,
            "        \"retention_seconds\": {retention_seconds:.9},"
        )
        .unwrap();
        writeln!(
            out,
            "        \"logical_payload_bytes\": {LOGICAL_PAYLOAD_BYTES},"
        )
        .unwrap();
        writeln!(
            out,
            "        \"allocation_payload_bytes\": {ALLOCATION_PAYLOAD_BYTES},"
        )
        .unwrap();
        writeln!(
            out,
            "        \"runtime_or_matching_overhead_bytes\": {},",
            ALLOCATION_PAYLOAD_BYTES - LOGICAL_PAYLOAD_BYTES
        )
        .unwrap();
        writeln!(
            out,
            "        \"allocations\": {},",
            workload.counters.allocations
        )
        .unwrap();
        writeln!(
            out,
            "        \"allocated_bytes\": {},",
            workload.counters.allocated_bytes
        )
        .unwrap();
        writeln!(
            out,
            "        \"allocation_bytes_per_event\": {allocation_bytes_per_event:.6},"
        )
        .unwrap();
        writeln!(
            out,
            "        \"deallocations\": {},",
            workload.counters.deallocations
        )
        .unwrap();
        writeln!(
            out,
            "        \"deallocated_bytes\": {},",
            workload.counters.deallocated_bytes
        )
        .unwrap();
        writeln!(
            out,
            "        \"retained_objects\": {},",
            workload.retained_objects
        )
        .unwrap();
        writeln!(
            out,
            "        \"estimated_logical_live_bytes\": {logical_live_bytes},"
        )
        .unwrap();
        writeln!(
            out,
            "        \"observed_allocated_live_bytes\": {},",
            workload.retained_live_bytes
        )
        .unwrap();
        writeln!(
            out,
            "        \"retained_checksum\": {},",
            workload.retained_checksum
        )
        .unwrap();
        writeln!(out, "        \"valid\": {}", load.workload_valid).unwrap();
        out.push_str("      },\n");
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

    if cfg.calibrate_only {
        let own_max = calibrate_max_rate(&cfg).unwrap_or_else(|e| {
            eprintln!("calibration error: {e}");
            std::process::exit(1);
        });
        let json = write_result(&cfg, None, own_max, &[], provenance);
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
        if !provenance_valid {
            eprintln!("invalid calibration: build provenance failed");
            std::process::exit(3);
        }
        return;
    }

    let common_max = cfg
        .max_rate
        .or(cfg.rate)
        .map_or_else(|| calibrate_max_rate(&cfg), |rate| Ok(rate as f64))
        .unwrap_or_else(|e| {
            eprintln!("calibration error: {e}");
            std::process::exit(1);
        });
    let own_max = cfg.own_max.map_or(common_max, |rate| rate as f64);

    let targets: Vec<(u64, u64)> = if let Some(rate) = cfg.rate {
        vec![(0, rate)]
    } else {
        cfg.load_levels
            .iter()
            .map(|&pct| (pct, (common_max * pct as f64 / 100.0).round() as u64))
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
        match run_load(&cfg, target_rate, common_max, own_max) {
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
        let path = samples_path_for_load(
            cfg.samples_output
                .as_deref()
                .expect("validated measurement samples output"),
            load.load_pct,
        );
        if let Err(error) = ensure_parent(&path).and_then(|()| write_samples(&path, &load.samples))
        {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }

    let json = write_result(&cfg, Some(common_max), own_max, &loads, provenance);

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
            "invalid run: build provenance, actual/target rate, workload, or pause validation failed"
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
    fn injected_pause_is_taken_once_at_the_configured_recorded_fraction() {
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
            25,
            AllocationFreeWorkload,
            None,
            Arc::new(AtomicBool::new(false)),
            sink,
        );

        assert_eq!(handler.inject_sequence, Some(35_000));
        assert_eq!(handler.take_injection(34_999), None);
        assert_eq!(
            handler.take_injection(35_000),
            Some(Duration::from_millis(50))
        );
        assert_eq!(handler.take_injection(35_000), None);
        assert_eq!(handler.take_injection(35_001), None);
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
        let observation = InjectionObservation {
            sequence: 25_000,
            planned_ns: 250_000_000,
            started_ns: 250_000_000,
            completed_ns: 300_000_000,
        };
        let check = validate_injected_pause(
            Duration::from_millis(50),
            100_000,
            200_000.0,
            &visible,
            observation,
            1_000_000_000,
        )
        .unwrap();
        assert!(check.backlog_in_range());
        assert!(check.drain_allowance_met());
        assert!(check.double_drain_allowance_met());
        assert!(check.p99_9_visible());
        assert!(check.max_visible());
        assert!(check.is_valid());

        let hidden = LatencyStats {
            p99_9: 2_000.0,
            max: 3_000.0,
            ..visible
        };
        assert!(!validate_injected_pause(
            Duration::from_millis(50),
            100_000,
            200_000.0,
            &hidden,
            observation,
            1_000_000_000,
        )
        .unwrap()
        .is_valid());
    }

    #[test]
    fn injected_pause_gate_rejects_late_or_out_of_range_backlogs() {
        let visible = LatencyStats {
            p50: 1_000.0,
            p99: 2_000.0,
            p99_9: 50_000_000.0,
            p99_99: 50_000_000.0,
            max: 50_000_000.0,
            mean: 1_000.0,
            min: 500.0,
            count: 100_000,
        };
        let late = InjectionObservation {
            sequence: 99_000,
            planned_ns: 990_000_000,
            started_ns: 990_000_000,
            completed_ns: 1_040_000_000,
        };
        let late_check = validate_injected_pause(
            Duration::from_millis(50),
            100_000,
            200_000.0,
            &visible,
            late,
            1_080_000_000,
        )
        .unwrap();
        assert!(late_check.backlog_in_range());
        assert!(!late_check.drain_allowance_met());
        assert!(!late_check.is_valid());

        let oversized = validate_injected_pause(
            Duration::from_millis(200),
            100_000,
            200_000.0,
            &visible,
            InjectionObservation {
                completed_ns: 300_000_000,
                ..late
            },
            1_100_000_000,
        )
        .unwrap();
        assert!(!oversized.backlog_in_range());
        assert!(!oversized.is_valid());
    }

    #[test]
    fn allocating_workload_counts_only_measured_allocations_and_overwrites() {
        let mut workload = AllocatingWorkload::new(2);
        workload.on_start();
        workload.apply(0, false);
        workload.apply(1, true);
        workload.apply(2, true);

        let stats = workload.stats();
        assert_eq!(
            stats.counters,
            AllocationCounters {
                allocations: 2,
                allocated_bytes: (ALLOCATION_PAYLOAD_BYTES * 2) as u64,
                deallocations: 1,
                deallocated_bytes: ALLOCATION_PAYLOAD_BYTES as u64,
            }
        );
        assert_eq!(stats.retained_objects, 2);
        assert_eq!(
            stats.retained_live_bytes,
            (ALLOCATION_PAYLOAD_BYTES * 2) as u64
        );
        assert_ne!(stats.retained_checksum, 0);
    }

    #[test]
    fn allocation_free_workload_does_not_touch_counting_state() {
        let mut workload = AllocationFreeWorkload;
        workload.on_start();
        workload.apply(1, true);
        assert_eq!(workload.stats().counters, AllocationCounters::default());
    }

    #[test]
    fn allocating_measurement_must_fill_the_retention_window() {
        assert!(measured_window_covers_retention(262_144, 262_144));
        assert!(!measured_window_covers_retention(100_000, 262_144));
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
