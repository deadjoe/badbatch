//! Head-to-head Rust harness for comparing `BadBatch` against native LMAX Disruptor.
//!
//! Pairs with `tools/head_to_head` (Java) and `scripts/run_head_to_head.sh`.
//! Measurement contract (must match Java):
//! - same scenarios, event payload, batching, warmup/measured rounds
//! - ops/s = `events_total` / `wall_seconds` (publish start → consumer completion)
//! - median over **measured** rounds only; checksum validates correctness
//!
//! Rust path under test: public **Builder** API (product path), not experimental
//! internal loops. See `tools/head_to_head/README.md`.

#![allow(
    missing_docs,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use badbatch::disruptor::{
    build_multi_producer, build_single_producer, BusySpinWaitStrategy, EventHandler, Producer,
    Result as DisruptorResult, SlotPadding, YieldingWaitStrategy,
};
#[cfg(feature = "bench-round-diagnostics")]
use badbatch::disruptor::{configure_bench_round_diagnostics, BenchProducerBackpressureSnapshot};
use core_affinity::CoreId;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(feature = "bench-round-diagnostics")]
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const MPSC_PRODUCERS: usize = 3;
const DEFAULT_TIMEOUT_SECS: u64 = 300;

// --- event model (must match Java ComparisonEvent) ---------------------------------

#[derive(Debug, Default, Clone, Copy)]
struct ComparisonEvent {
    value: i64,
    stage1_value: i64,
    stage2_value: i64,
    stage3_value: i64,
}

fn arithmetic_checksum(events_total: u64) -> i64 {
    // sum_{i=0}^{n-1} i = n*(n-1)/2
    let n = i128::from(events_total);
    ((n * (n - 1)) / 2) as i64
}

fn pipeline_checksum(events_total: u64) -> i64 {
    // stage1: v+1, stage2: s1+3, stage3: s2+7 → final = v+11
    let mut sum = 0_i64;
    for v in 0..events_total {
        sum = sum.wrapping_add((v as i64).wrapping_add(11));
    }
    sum
}

// --- CLI ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Unicast,
    UnicastBatch,
    MpscBatch,
    Pipeline,
}

impl Scenario {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "unicast" => Ok(Self::Unicast),
            "unicast_batch" => Ok(Self::UnicastBatch),
            "mpsc_batch" => Ok(Self::MpscBatch),
            "pipeline" => Ok(Self::Pipeline),
            _ => Err(format!("unsupported scenario: {s}")),
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

    fn default_wait(self) -> WaitKind {
        match self {
            Self::MpscBatch => WaitKind::BusySpin,
            _ => WaitKind::Yielding,
        }
    }

    fn default_buffer(self) -> usize {
        match self {
            Self::Pipeline => 8_192,
            _ => 65_536,
        }
    }

    fn default_events(self, quick: bool) -> u64 {
        if quick {
            match self {
                Self::MpscBatch => 3_000_000,
                _ => 1_000_000,
            }
        } else {
            match self {
                Self::MpscBatch => 60_000_000,
                _ => 100_000_000,
            }
        }
    }

    fn default_batch(self) -> usize {
        match self {
            Self::Unicast | Self::Pipeline => 1,
            Self::UnicastBatch | Self::MpscBatch => 10,
        }
    }
}

#[derive(Clone, Copy, Debug)]
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
    scenario: Scenario,
    wait: WaitKind,
    pad: Pad,
    buffer_size: usize,
    events_total: u64,
    batch_size: usize,
    warmup_rounds: usize,
    measured_rounds: usize,
    run_order: String,
    pair_id: String,
    fork_index: usize,
    harness_rev: Option<String>,
    implementation_rev: Option<String>,
    harness_dirty: Option<bool>,
    implementation_dirty: Option<bool>,
    impl_label: String,
    output: Option<PathBuf>,
    timeout: Duration,
    cpu_list: Vec<usize>,
    affinity_failed: Arc<AtomicBool>,
    #[cfg(feature = "bench-round-diagnostics")]
    round_diagnostics: bool,
}

fn parse_args() -> Result<Config, String> {
    let mut args = env::args().skip(1);
    let mut scenario = None;
    let mut wait = None;
    let mut pad = Pad::None;
    let mut buffer_size = None;
    let mut events_total = None;
    let mut batch_size = None;
    let mut warmup_rounds = 3usize;
    let mut measured_rounds = 7usize;
    let mut run_order = "standalone".into();
    let mut pair_id = "standalone".into();
    let mut fork_index = 0usize;
    let mut harness_rev = None;
    let mut implementation_rev = None;
    let mut harness_dirty = None;
    let mut implementation_dirty = None;
    let mut impl_label = "badbatch-builder".into();
    let mut output = None;
    let mut quick = false;
    let mut cpu_list = Vec::new();
    #[cfg(feature = "bench-round-diagnostics")]
    let mut round_diagnostics = false;

    while let Some(a) = args.next() {
        match a.as_str() {
            "--scenario" => {
                scenario = Some(Scenario::parse(
                    &args.next().ok_or("missing --scenario value")?,
                )?);
            }
            "--wait-strategy" => {
                wait = Some(WaitKind::parse(
                    &args.next().ok_or("missing --wait-strategy value")?,
                )?);
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
            "--batch-size" => {
                batch_size = Some(
                    args.next()
                        .ok_or("missing --batch-size")?
                        .parse()
                        .map_err(|e| format!("batch-size: {e}"))?,
                );
            }
            "--warmup-rounds" => {
                warmup_rounds = args
                    .next()
                    .ok_or("missing --warmup-rounds")?
                    .parse()
                    .map_err(|e| format!("warmup-rounds: {e}"))?;
            }
            "--measured-rounds" => {
                measured_rounds = args
                    .next()
                    .ok_or("missing --measured-rounds")?
                    .parse()
                    .map_err(|e| format!("measured-rounds: {e}"))?;
            }
            "--run-order" => {
                run_order = args.next().ok_or("missing --run-order")?;
            }
            "--pair-id" => {
                pair_id = args.next().ok_or("missing --pair-id")?;
            }
            "--fork-index" => {
                fork_index = args
                    .next()
                    .ok_or("missing --fork-index")?
                    .parse()
                    .map_err(|e| format!("fork-index: {e}"))?;
            }
            "--harness-rev" => {
                harness_rev = Some(args.next().ok_or("missing --harness-rev")?);
            }
            "--implementation-rev" => {
                implementation_rev = Some(args.next().ok_or("missing --implementation-rev")?);
            }
            "--harness-dirty" => {
                harness_dirty = Some(
                    args.next()
                        .ok_or("missing --harness-dirty")?
                        .parse()
                        .map_err(|e| format!("harness-dirty: {e}"))?,
                );
            }
            "--implementation-dirty" => {
                implementation_dirty = Some(
                    args.next()
                        .ok_or("missing --implementation-dirty")?
                        .parse()
                        .map_err(|e| format!("implementation-dirty: {e}"))?,
                );
            }
            "--impl-label" => {
                impl_label = args.next().ok_or("missing --impl-label")?;
            }
            "--output" => {
                output = Some(PathBuf::from(args.next().ok_or("missing --output")?));
            }
            "--cpu-list" => {
                cpu_list = parse_cpu_list(&args.next().ok_or("missing --cpu-list")?)?;
            }
            #[cfg(feature = "bench-round-diagnostics")]
            "--round-diagnostics" => round_diagnostics = true,
            #[cfg(not(feature = "bench-round-diagnostics"))]
            "--round-diagnostics" => {
                return Err(
                    "--round-diagnostics requires --features bench-round-diagnostics".into(),
                );
            }
            "--quick" => quick = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let scenario = scenario.ok_or("required: --scenario")?;
    let wait = wait.unwrap_or_else(|| scenario.default_wait());
    let buffer_size = buffer_size.unwrap_or_else(|| scenario.default_buffer());
    let events_total = events_total.unwrap_or_else(|| scenario.default_events(quick));
    let batch_size = batch_size.unwrap_or_else(|| scenario.default_batch());

    if !buffer_size.is_power_of_two() || buffer_size == 0 {
        return Err(format!(
            "buffer-size must be power of two, got {buffer_size}"
        ));
    }
    if measured_rounds == 0 {
        return Err("measured-rounds must be > 0".into());
    }
    if batch_size == 0 || events_total == 0 {
        return Err("batch-size and events-total must be > 0".into());
    }
    if scenario == Scenario::MpscBatch && !events_total.is_multiple_of(MPSC_PRODUCERS as u64) {
        return Err(format!(
            "mpsc_batch events-total must be divisible by {MPSC_PRODUCERS}"
        ));
    }
    if pad != Pad::None && scenario == Scenario::MpscBatch {
        return Err("event-padding not supported for mpsc_batch in this harness".into());
    }
    let required_cpus = match scenario {
        Scenario::Unicast | Scenario::UnicastBatch => 2,
        Scenario::MpscBatch | Scenario::Pipeline => 4,
    };
    if !cpu_list.is_empty() && cpu_list.len() < required_cpus {
        return Err(format!(
            "scenario {} requires at least {required_cpus} CPUs, got {}",
            scenario.as_str(),
            cpu_list.len()
        ));
    }
    if quick {
        warmup_rounds = warmup_rounds.min(1);
        measured_rounds = measured_rounds.min(2);
    }

    Ok(Config {
        scenario,
        wait,
        pad,
        buffer_size,
        events_total,
        batch_size,
        warmup_rounds,
        measured_rounds,
        run_order,
        pair_id,
        fork_index,
        harness_rev,
        implementation_rev,
        harness_dirty,
        implementation_dirty,
        impl_label,
        output,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        cpu_list,
        affinity_failed: Arc::new(AtomicBool::new(false)),
        #[cfg(feature = "bench-round-diagnostics")]
        round_diagnostics,
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

impl Config {
    fn cpu(&self, index: usize) -> Option<usize> {
        self.cpu_list.get(index).copied()
    }

    fn affinity_verified_all(&self) -> bool {
        !self.cpu_list.is_empty() && !self.affinity_failed.load(Ordering::Acquire)
    }

    fn affinity_roles(&self) -> Vec<(&'static str, usize)> {
        if self.cpu_list.is_empty() {
            return Vec::new();
        }
        match self.scenario {
            Scenario::Unicast | Scenario::UnicastBatch => vec![
                ("publisher", self.cpu_list[0]),
                ("consumer", self.cpu_list[1]),
            ],
            Scenario::MpscBatch => vec![
                ("coordinator", self.cpu_list[0]),
                ("producer_0", self.cpu_list[0]),
                ("producer_1", self.cpu_list[1]),
                ("producer_2", self.cpu_list[2]),
                ("consumer", self.cpu_list[3]),
            ],
            Scenario::Pipeline => vec![
                ("publisher", self.cpu_list[0]),
                ("stage_1", self.cpu_list[1]),
                ("stage_2", self.cpu_list[2]),
                ("stage_3", self.cpu_list[3]),
            ],
        }
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
Usage: h2h_rust --scenario <unicast|unicast_batch|mpsc_batch|pipeline> [options]

Options:
  --wait-strategy <yielding|busy-spin>
  --event-padding <none|128>     (slot padding; default none)
  --buffer-size <N>              (power of two)
  --events-total <N>
  --batch-size <N>
  --warmup-rounds <N>            (default 3)
  --measured-rounds <N>          (default 7)
  --run-order <label>            (metadata only)
  --pair-id <label>              (paired-fork metadata)
  --fork-index <N>               (paired-fork metadata)
  --harness-rev <rev>            (orchestrator provenance)
  --implementation-rev <rev>     (orchestrator provenance)
  --harness-dirty <true|false>    (orchestrator provenance)
  --implementation-dirty <bool>  (orchestrator provenance)
  --cpu-list <N,N,...>          (pin measured worker roles to logical CPUs)
  --round-diagnostics           (per-round probe; requires bench-round-diagnostics)
  --impl-label <label>           (default badbatch-builder)
  --output <path.json>
  --quick                        (smaller defaults / fewer rounds)
"
    );
}

// --- stats -------------------------------------------------------------------------

#[derive(Clone)]
struct Round {
    index: usize,
    phase: &'static str,
    elapsed_ns: u128,
    events: u64,
    ops_per_sec: f64,
    checksum_ok: bool,
    #[cfg(feature = "bench-round-diagnostics")]
    diagnostics: Option<RoundDiagnostics>,
}

#[cfg(feature = "bench-round-diagnostics")]
const LOG2_HISTOGRAM_BINS: usize = 64;

#[cfg(feature = "bench-round-diagnostics")]
#[derive(Clone, Debug)]
struct HistogramSnapshot {
    count: u64,
    sum: u64,
    min: u64,
    max: u64,
    log2_bins: [u64; LOG2_HISTOGRAM_BINS],
}

#[cfg(feature = "bench-round-diagnostics")]
#[derive(Debug)]
struct Log2Histogram {
    count: u64,
    sum: u64,
    min: u64,
    max: u64,
    bins: [u64; LOG2_HISTOGRAM_BINS],
}

#[cfg(feature = "bench-round-diagnostics")]
impl Default for Log2Histogram {
    fn default() -> Self {
        Self {
            count: 0,
            sum: 0,
            min: u64::MAX,
            max: 0,
            bins: [0; LOG2_HISTOGRAM_BINS],
        }
    }
}

#[cfg(feature = "bench-round-diagnostics")]
impl Log2Histogram {
    #[inline]
    fn observe(&mut self, value: u64) {
        debug_assert!(value > 0);
        let bin = (u64::BITS - 1 - value.leading_zeros()) as usize;
        self.count = self.count.saturating_add(1);
        self.sum = self.sum.saturating_add(value);
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        self.bins[bin] = self.bins[bin].saturating_add(1);
    }

    fn snapshot(&self) -> HistogramSnapshot {
        HistogramSnapshot {
            count: self.count,
            sum: self.sum,
            min: if self.count == 0 { 0 } else { self.min },
            max: self.max,
            log2_bins: self.bins,
        }
    }
}

#[cfg(feature = "bench-round-diagnostics")]
#[derive(Clone, Debug)]
struct ConsumerBatchDiagnostics {
    role: &'static str,
    batch_size: HistogramSnapshot,
    queue_depth: HistogramSnapshot,
}

#[cfg(feature = "bench-round-diagnostics")]
#[derive(Clone, Debug)]
struct RoundDiagnostics {
    consumers: Vec<ConsumerBatchDiagnostics>,
    producer_backpressure: Option<BenchProducerBackpressureSnapshot>,
}

#[cfg(feature = "bench-round-diagnostics")]
#[derive(Debug)]
struct BatchProbe {
    role: &'static str,
    batch_size: Log2Histogram,
    queue_depth: Log2Histogram,
    sender: Option<Sender<ConsumerBatchDiagnostics>>,
}

#[cfg(feature = "bench-round-diagnostics")]
impl BatchProbe {
    #[inline]
    fn observe(&mut self, batch_size: i64, queue_depth: i64) {
        let batch_size = u64::try_from(batch_size).expect("batch size must be positive");
        let queue_depth = u64::try_from(queue_depth).expect("queue depth must be positive");
        self.batch_size.observe(batch_size);
        self.queue_depth.observe(queue_depth);
    }

    fn finish(&mut self) {
        let Some(sender) = self.sender.take() else {
            return;
        };
        let _ = sender.send(ConsumerBatchDiagnostics {
            role: self.role,
            batch_size: self.batch_size.snapshot(),
            queue_depth: self.queue_depth.snapshot(),
        });
    }
}

#[cfg(feature = "bench-round-diagnostics")]
fn batch_probe_pair(
    enabled: bool,
    role: &'static str,
) -> (
    Option<BatchProbe>,
    Option<Receiver<ConsumerBatchDiagnostics>>,
) {
    if !enabled {
        return (None, None);
    }
    let (sender, receiver) = mpsc::channel();
    (
        Some(BatchProbe {
            role,
            batch_size: Log2Histogram::default(),
            queue_depth: Log2Histogram::default(),
            sender: Some(sender),
        }),
        Some(receiver),
    )
}

#[cfg(feature = "bench-round-diagnostics")]
fn collect_round_diagnostics(
    cfg: &Config,
    receivers: Vec<Option<Receiver<ConsumerBatchDiagnostics>>>,
    producer_backpressure: Option<BenchProducerBackpressureSnapshot>,
) -> Result<Option<RoundDiagnostics>, String> {
    if !cfg.round_diagnostics {
        return Ok(None);
    }
    let mut consumers = Vec::with_capacity(receivers.len());
    for receiver in receivers {
        let receiver = receiver.ok_or("missing enabled batch diagnostics receiver")?;
        let snapshot = receiver
            .recv_timeout(Duration::from_secs(1))
            .map_err(|error| format!("missing batch diagnostics at shutdown: {error}"))?;
        if snapshot.batch_size.sum != cfg.events_total {
            return Err(format!(
                "{} batch diagnostics saw {} events, expected {}",
                snapshot.role, snapshot.batch_size.sum, cfg.events_total
            ));
        }
        consumers.push(snapshot);
    }
    Ok(Some(RoundDiagnostics {
        consumers,
        producer_backpressure,
    }))
}

struct Summary {
    checksum_valid_all: bool,
    median_ops_per_sec: f64,
    mean_ops_per_sec: f64,
    min_ops_per_sec: f64,
    max_ops_per_sec: f64,
    stddev_ops_per_sec: f64,
    cv: f64,
}

fn summarize(rounds: &[Round], warmup: usize) -> Summary {
    let measured: Vec<f64> = rounds.iter().skip(warmup).map(|r| r.ops_per_sec).collect();
    let checksum_valid_all = rounds.iter().all(|r| r.checksum_ok);
    if measured.is_empty() {
        return Summary {
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
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = if sorted.len() % 2 == 1 {
        sorted[sorted.len() / 2]
    } else {
        f64::midpoint(sorted[sorted.len() / 2 - 1], sorted[sorted.len() / 2])
    };
    let mean = measured.iter().sum::<f64>() / measured.len() as f64;
    let min = sorted[0];
    let max = *sorted.last().unwrap();
    let var = measured
        .iter()
        .map(|x| {
            let d = x - mean;
            d * d
        })
        .sum::<f64>()
        / measured.len() as f64;
    let stddev = var.sqrt();
    let cv = if mean > 0.0 { stddev / mean } else { 0.0 };
    Summary {
        checksum_valid_all,
        median_ops_per_sec: median,
        mean_ops_per_sec: mean,
        min_ops_per_sec: min,
        max_ops_per_sec: max,
        stddev_ops_per_sec: stddev,
        cv,
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

#[derive(Clone, Copy)]
enum TerminalMode {
    Value,
    Pipeline,
}

struct TerminalHandler {
    processed: Arc<AtomicU64>,
    checksum: Arc<AtomicU64>,
    ready: Arc<AtomicU64>,
    events_total: u64,
    final_sequence: i64,
    local_checksum: u64,
    mode: TerminalMode,
    cpu_affinity: Option<usize>,
    affinity_failed: Arc<AtomicBool>,
    #[cfg(feature = "bench-round-diagnostics")]
    batch_probe: Option<BatchProbe>,
}

impl TerminalHandler {
    fn new(
        processed: Arc<AtomicU64>,
        checksum: Arc<AtomicU64>,
        ready: Arc<AtomicU64>,
        events_total: u64,
        mode: TerminalMode,
        cpu_affinity: Option<usize>,
        affinity_failed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            processed,
            checksum,
            ready,
            events_total,
            final_sequence: i64::try_from(events_total).expect("events_total must fit i64") - 1,
            local_checksum: 0,
            mode,
            cpu_affinity,
            affinity_failed,
            #[cfg(feature = "bench-round-diagnostics")]
            batch_probe: None,
        }
    }

    #[cfg(feature = "bench-round-diagnostics")]
    fn with_batch_probe(mut self, batch_probe: Option<BatchProbe>) -> Self {
        self.batch_probe = batch_probe;
        self
    }

    fn complete_if_final(&self, sequence: i64) {
        if sequence == self.final_sequence {
            // Exactly one completion publication per round on both harness sides.
            self.checksum.store(self.local_checksum, Ordering::SeqCst);
            self.processed.store(self.events_total, Ordering::SeqCst);
        }
    }
}

impl EventHandler<ComparisonEvent> for TerminalHandler {
    fn on_event(
        &mut self,
        event: &mut ComparisonEvent,
        sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let value = match self.mode {
            TerminalMode::Value => event.value,
            TerminalMode::Pipeline => {
                event.stage3_value = event.stage2_value.wrapping_add(7);
                event.stage3_value
            }
        };
        self.local_checksum = self.local_checksum.wrapping_add(value as u64);
        self.complete_if_final(sequence);
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        pin_current_thread(self.cpu_affinity, &self.affinity_failed);
        self.ready.fetch_add(1, Ordering::Release);
        Ok(())
    }

    #[cfg(feature = "bench-round-diagnostics")]
    fn on_batch_start(&mut self, batch_size: i64, queue_depth: i64) -> DisruptorResult<()> {
        if let Some(probe) = &mut self.batch_probe {
            probe.observe(batch_size, queue_depth);
        }
        Ok(())
    }

    #[cfg(feature = "bench-round-diagnostics")]
    fn on_shutdown(&mut self) -> DisruptorResult<()> {
        if let Some(probe) = &mut self.batch_probe {
            probe.finish();
        }
        Ok(())
    }
}

struct PipelineStageHandler {
    stage: u8,
    ready: Arc<AtomicU64>,
    cpu_affinity: Option<usize>,
    affinity_failed: Arc<AtomicBool>,
    #[cfg(feature = "bench-round-diagnostics")]
    batch_probe: Option<BatchProbe>,
}

impl EventHandler<ComparisonEvent> for PipelineStageHandler {
    fn on_event(
        &mut self,
        event: &mut ComparisonEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        match self.stage {
            1 => event.stage1_value = event.value.wrapping_add(1),
            2 => event.stage2_value = event.stage1_value.wrapping_add(3),
            _ => unreachable!("pipeline stage must be 1 or 2"),
        }
        Ok(())
    }

    fn on_start(&mut self) -> DisruptorResult<()> {
        pin_current_thread(self.cpu_affinity, &self.affinity_failed);
        self.ready.fetch_add(1, Ordering::Release);
        Ok(())
    }

    #[cfg(feature = "bench-round-diagnostics")]
    fn on_batch_start(&mut self, batch_size: i64, queue_depth: i64) -> DisruptorResult<()> {
        if let Some(probe) = &mut self.batch_probe {
            probe.observe(batch_size, queue_depth);
        }
        Ok(())
    }

    #[cfg(feature = "bench-round-diagnostics")]
    fn on_shutdown(&mut self) -> DisruptorResult<()> {
        if let Some(probe) = &mut self.batch_probe {
            probe.finish();
        }
        Ok(())
    }
}

// --- scenarios ---------------------------------------------------------------------

fn run_unicast(cfg: &Config, batch: bool) -> Result<Vec<Round>, String> {
    match cfg.wait {
        WaitKind::BusySpin => run_unicast_w(cfg, batch, &BusySpinWaitStrategy),
        WaitKind::Yielding => run_unicast_w(cfg, batch, &YieldingWaitStrategy),
    }
}

fn run_unicast_w<W>(cfg: &Config, batch: bool, wait: &W) -> Result<Vec<Round>, String>
where
    W: badbatch::disruptor::WaitStrategy + Clone + 'static,
{
    let expected = arithmetic_checksum(cfg.events_total);
    let total_rounds = cfg.warmup_rounds + cfg.measured_rounds;
    let mut rounds = Vec::with_capacity(total_rounds);

    for i in 0..total_rounds {
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicU64::new(0));
        let ready = Arc::new(AtomicU64::new(0));
        #[cfg(feature = "bench-round-diagnostics")]
        let (consumer_probe, consumer_receiver) =
            batch_probe_pair(cfg.round_diagnostics, "consumer");
        let handler = TerminalHandler::new(
            Arc::clone(&processed),
            Arc::clone(&checksum),
            Arc::clone(&ready),
            cfg.events_total,
            TerminalMode::Value,
            cfg.cpu(1),
            Arc::clone(&cfg.affinity_failed),
        );
        #[cfg(feature = "bench-round-diagnostics")]
        let handler = handler.with_batch_probe(consumer_probe);

        let mut handle =
            build_single_producer(cfg.buffer_size, ComparisonEvent::default, wait.clone())
                .with_slot_padding(cfg.pad.slot_padding())
                .handle_events_with_handler(handler)
                .build();

        if !wait_count(&ready, 1, cfg.timeout) {
            handle.shutdown();
            return Err("timeout waiting for unicast consumer readiness".into());
        }
        if cfg.affinity_failed.load(Ordering::Acquire) {
            handle.shutdown();
            return Err("failed to pin unicast worker threads".into());
        }

        let start = Instant::now();
        if batch {
            let mut published = 0u64;
            while published < cfg.events_total {
                let chunk = (cfg.batch_size as u64).min(cfg.events_total - published) as usize;
                let base = published;
                let _ = handle.batch_publish(chunk, |iter| {
                    for (j, e) in iter.enumerate() {
                        e.value = (base + j as u64) as i64;
                        e.stage1_value = 0;
                        e.stage2_value = 0;
                        e.stage3_value = 0;
                    }
                });
                published += chunk as u64;
            }
        } else {
            for v in 0..cfg.events_total {
                let _ = handle.publish(|e| {
                    e.value = v as i64;
                    e.stage1_value = 0;
                    e.stage2_value = 0;
                    e.stage3_value = 0;
                });
            }
        }

        if !wait_count(&processed, cfg.events_total, cfg.timeout) {
            handle.shutdown();
            return Err(format!(
                "timeout waiting for unicast completion (got {})",
                processed.load(Ordering::Acquire)
            ));
        }
        let elapsed = start.elapsed();
        #[cfg(feature = "bench-round-diagnostics")]
        let producer_backpressure = handle.bench_producer_backpressure_snapshot();
        handle.shutdown();
        #[cfg(feature = "bench-round-diagnostics")]
        let diagnostics =
            collect_round_diagnostics(cfg, vec![consumer_receiver], producer_backpressure)?;

        let got = checksum.load(Ordering::Acquire) as i64;
        let ops = cfg.events_total as f64 / elapsed.as_secs_f64().max(1e-12);
        rounds.push(Round {
            index: i + 1,
            phase: if i < cfg.warmup_rounds {
                "warmup"
            } else {
                "measured"
            },
            elapsed_ns: elapsed.as_nanos(),
            events: cfg.events_total,
            ops_per_sec: ops,
            checksum_ok: got == expected,
            #[cfg(feature = "bench-round-diagnostics")]
            diagnostics,
        });
    }
    Ok(rounds)
}

fn run_mpsc_batch(cfg: &Config) -> Result<Vec<Round>, String> {
    match cfg.wait {
        WaitKind::BusySpin => run_mpsc_batch_w(cfg, &BusySpinWaitStrategy),
        WaitKind::Yielding => run_mpsc_batch_w(cfg, &YieldingWaitStrategy),
    }
}

fn run_mpsc_batch_w<W>(cfg: &Config, wait: &W) -> Result<Vec<Round>, String>
where
    W: badbatch::disruptor::WaitStrategy + Clone + 'static,
{
    let expected = arithmetic_checksum(cfg.events_total);
    let per = cfg.events_total / MPSC_PRODUCERS as u64;
    let total_rounds = cfg.warmup_rounds + cfg.measured_rounds;
    let mut rounds = Vec::with_capacity(total_rounds);

    for i in 0..total_rounds {
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicU64::new(0));
        let ready = Arc::new(AtomicU64::new(0));
        #[cfg(feature = "bench-round-diagnostics")]
        let (consumer_probe, consumer_receiver) =
            batch_probe_pair(cfg.round_diagnostics, "consumer");
        let handler = TerminalHandler::new(
            Arc::clone(&processed),
            Arc::clone(&checksum),
            Arc::clone(&ready),
            cfg.events_total,
            TerminalMode::Value,
            cfg.cpu(3),
            Arc::clone(&cfg.affinity_failed),
        );
        #[cfg(feature = "bench-round-diagnostics")]
        let handler = handler.with_batch_probe(consumer_probe);

        let mut handle =
            build_multi_producer(cfg.buffer_size, ComparisonEvent::default, wait.clone())
                .handle_events_with_handler(handler)
                .build();

        if !wait_count(&ready, 1, cfg.timeout) {
            handle.shutdown();
            return Err("timeout waiting for mpsc consumer readiness".into());
        }
        if cfg.affinity_failed.load(Ordering::Acquire) {
            handle.shutdown();
            return Err("failed to pin mpsc consumer thread".into());
        }

        let start_flag = Arc::new(AtomicBool::new(false));
        let producers_ready = Arc::new(AtomicU64::new(0));
        let mut joins = Vec::new();
        for id in 0..MPSC_PRODUCERS {
            let mut prod = handle.create_producer();
            let start_flag = Arc::clone(&start_flag);
            let producers_ready = Arc::clone(&producers_ready);
            let batch = cfg.batch_size;
            let range_start = per * id as u64;
            let range_end = range_start + per;
            let producer_cpu = cfg.cpu(id);
            let affinity_failed = Arc::clone(&cfg.affinity_failed);
            joins.push(thread::spawn(move || {
                pin_current_thread(producer_cpu, &affinity_failed);
                producers_ready.fetch_add(1, Ordering::Release);
                while !start_flag.load(Ordering::Acquire) {
                    std::hint::spin_loop();
                }
                let mut published = range_start;
                while published < range_end {
                    let chunk = batch.min((range_end - published) as usize);
                    let base = published;
                    let _ = prod.batch_publish(chunk, |iter| {
                        for (j, e) in iter.enumerate() {
                            e.value = (base + j as u64) as i64;
                            e.stage1_value = 0;
                            e.stage2_value = 0;
                            e.stage3_value = 0;
                        }
                    });
                    published += chunk as u64;
                }
            }));
        }

        if !wait_count(&producers_ready, MPSC_PRODUCERS as u64, cfg.timeout) {
            handle.shutdown();
            return Err("timeout waiting for mpsc producer readiness".into());
        }
        if cfg.affinity_failed.load(Ordering::Acquire) {
            handle.shutdown();
            return Err("failed to pin mpsc producer threads".into());
        }

        let start = Instant::now();
        start_flag.store(true, Ordering::Release);
        for j in joins {
            j.join().map_err(|_| "producer panic".to_string())?;
        }
        if !wait_count(&processed, cfg.events_total, cfg.timeout) {
            handle.shutdown();
            return Err("timeout mpsc completion".into());
        }
        let elapsed = start.elapsed();
        handle.shutdown();
        #[cfg(feature = "bench-round-diagnostics")]
        let diagnostics = collect_round_diagnostics(cfg, vec![consumer_receiver], None)?;

        let got = checksum.load(Ordering::Acquire) as i64;
        let ops = cfg.events_total as f64 / elapsed.as_secs_f64().max(1e-12);
        rounds.push(Round {
            index: i + 1,
            phase: if i < cfg.warmup_rounds {
                "warmup"
            } else {
                "measured"
            },
            elapsed_ns: elapsed.as_nanos(),
            events: cfg.events_total,
            ops_per_sec: ops,
            checksum_ok: got == expected,
            #[cfg(feature = "bench-round-diagnostics")]
            diagnostics,
        });
    }
    Ok(rounds)
}

fn run_pipeline(cfg: &Config) -> Result<Vec<Round>, String> {
    match cfg.wait {
        WaitKind::BusySpin => run_pipeline_w(cfg, &BusySpinWaitStrategy),
        WaitKind::Yielding => run_pipeline_w(cfg, &YieldingWaitStrategy),
    }
}

fn run_pipeline_w<W>(cfg: &Config, wait: &W) -> Result<Vec<Round>, String>
where
    W: badbatch::disruptor::WaitStrategy + Clone + 'static,
{
    let expected = pipeline_checksum(cfg.events_total);
    let total_rounds = cfg.warmup_rounds + cfg.measured_rounds;
    let mut rounds = Vec::with_capacity(total_rounds);

    for i in 0..total_rounds {
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicU64::new(0));
        let ready = Arc::new(AtomicU64::new(0));
        #[cfg(feature = "bench-round-diagnostics")]
        let (stage1_probe, stage1_receiver) = batch_probe_pair(cfg.round_diagnostics, "stage_1");
        #[cfg(feature = "bench-round-diagnostics")]
        let (stage2_probe, stage2_receiver) = batch_probe_pair(cfg.round_diagnostics, "stage_2");
        #[cfg(feature = "bench-round-diagnostics")]
        let (stage3_probe, stage3_receiver) = batch_probe_pair(cfg.round_diagnostics, "stage_3");
        let stage1 = PipelineStageHandler {
            stage: 1,
            ready: Arc::clone(&ready),
            cpu_affinity: cfg.cpu(1),
            affinity_failed: Arc::clone(&cfg.affinity_failed),
            #[cfg(feature = "bench-round-diagnostics")]
            batch_probe: stage1_probe,
        };
        let stage2 = PipelineStageHandler {
            stage: 2,
            ready: Arc::clone(&ready),
            cpu_affinity: cfg.cpu(2),
            affinity_failed: Arc::clone(&cfg.affinity_failed),
            #[cfg(feature = "bench-round-diagnostics")]
            batch_probe: stage2_probe,
        };
        let stage3 = TerminalHandler::new(
            Arc::clone(&processed),
            Arc::clone(&checksum),
            Arc::clone(&ready),
            cfg.events_total,
            TerminalMode::Pipeline,
            cfg.cpu(3),
            Arc::clone(&cfg.affinity_failed),
        );
        #[cfg(feature = "bench-round-diagnostics")]
        let stage3 = stage3.with_batch_probe(stage3_probe);

        let mut handle =
            build_single_producer(cfg.buffer_size, ComparisonEvent::default, wait.clone())
                .with_slot_padding(cfg.pad.slot_padding())
                .handle_events_with_handler(stage1)
                .and_then()
                .handle_events_with_handler(stage2)
                .and_then()
                .handle_events_with_handler(stage3)
                .build();

        if !wait_count(&ready, 3, cfg.timeout) {
            handle.shutdown();
            return Err("timeout waiting for pipeline stage readiness".into());
        }
        if cfg.affinity_failed.load(Ordering::Acquire) {
            handle.shutdown();
            return Err("failed to pin pipeline worker threads".into());
        }

        let start = Instant::now();
        for v in 0..cfg.events_total {
            let _ = handle.publish(|e| {
                e.value = v as i64;
                e.stage1_value = 0;
                e.stage2_value = 0;
                e.stage3_value = 0;
            });
        }
        if !wait_count(&processed, cfg.events_total, cfg.timeout) {
            handle.shutdown();
            return Err("timeout pipeline completion".into());
        }
        let elapsed = start.elapsed();
        #[cfg(feature = "bench-round-diagnostics")]
        let producer_backpressure = handle.bench_producer_backpressure_snapshot();
        handle.shutdown();
        #[cfg(feature = "bench-round-diagnostics")]
        let diagnostics = collect_round_diagnostics(
            cfg,
            vec![stage1_receiver, stage2_receiver, stage3_receiver],
            producer_backpressure,
        )?;

        let got = checksum.load(Ordering::Acquire) as i64;
        let ops = cfg.events_total as f64 / elapsed.as_secs_f64().max(1e-12);
        rounds.push(Round {
            index: i + 1,
            phase: if i < cfg.warmup_rounds {
                "warmup"
            } else {
                "measured"
            },
            elapsed_ns: elapsed.as_nanos(),
            events: cfg.events_total,
            ops_per_sec: ops,
            checksum_ok: got == expected,
            #[cfg(feature = "bench-round-diagnostics")]
            diagnostics,
        });
    }
    Ok(rounds)
}

// --- JSON --------------------------------------------------------------------------

fn git_rev() -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".into())
}

fn git_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
}

#[cfg(feature = "bench-round-diagnostics")]
fn write_histogram_json(out: &mut String, indent: &str, histogram: &HistogramSnapshot) {
    use std::fmt::Write as _;

    writeln!(out, "{indent}{{").unwrap();
    writeln!(out, "{indent}  \"count\": {},", histogram.count).unwrap();
    writeln!(out, "{indent}  \"sum\": {},", histogram.sum).unwrap();
    writeln!(out, "{indent}  \"min\": {},", histogram.min).unwrap();
    writeln!(out, "{indent}  \"max\": {},", histogram.max).unwrap();
    writeln!(
        out,
        "{indent}  \"mean\": {:.6},",
        if histogram.count == 0 {
            0.0
        } else {
            histogram.sum as f64 / histogram.count as f64
        }
    )
    .unwrap();
    write!(out, "{indent}  \"log2_bins\": [").unwrap();
    for (index, count) in histogram.log2_bins.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        write!(out, "{count}").unwrap();
    }
    out.push_str("]\n");
    write!(out, "{indent}}}").unwrap();
}

#[cfg(feature = "bench-round-diagnostics")]
fn write_round_diagnostics_json(out: &mut String, diagnostics: &RoundDiagnostics) {
    use std::fmt::Write as _;

    out.push_str("      \"diagnostics\": {\n");
    out.push_str("        \"batch_processing\": [\n");
    for (index, consumer) in diagnostics.consumers.iter().enumerate() {
        out.push_str("          {\n");
        writeln!(out, "            \"role\": \"{}\",", consumer.role).unwrap();
        out.push_str("            \"batch_size\": ");
        write_histogram_json(out, "            ", &consumer.batch_size);
        out.push_str(",\n");
        out.push_str("            \"queue_depth\": ");
        write_histogram_json(out, "            ", &consumer.queue_depth);
        out.push('\n');
        out.push_str(if index + 1 == diagnostics.consumers.len() {
            "          }\n"
        } else {
            "          },\n"
        });
    }
    out.push_str("        ],\n");
    out.push_str("        \"producer_backpressure\": {\n");
    if let Some(producer) = diagnostics.producer_backpressure {
        out.push_str("          \"supported\": true,\n");
        out.push_str("          \"iteration_action\": \"spin_loop\",\n");
        writeln!(out, "          \"entries\": {},", producer.entries).unwrap();
        writeln!(
            out,
            "          \"wait_loop_iterations\": {},",
            producer.wait_loop_iterations
        )
        .unwrap();
        writeln!(
            out,
            "          \"max_wait_loop_iterations\": {}",
            producer.max_wait_loop_iterations
        )
        .unwrap();
    } else {
        out.push_str("          \"supported\": false,\n");
        out.push_str("          \"iteration_action\": \"unsupported_multi_producer\",\n");
        out.push_str("          \"entries\": 0,\n");
        out.push_str("          \"wait_loop_iterations\": 0,\n");
        out.push_str("          \"max_wait_loop_iterations\": 0\n");
    }
    out.push_str("        }\n");
    out.push_str("      }\n");
}

fn write_result(cfg: &Config, rounds: &[Round], summary: &Summary) -> String {
    use std::fmt::Write as _;

    let local_rev = git_rev();
    let harness_rev = cfg.harness_rev.as_deref().unwrap_or(&local_rev);
    let implementation_rev = cfg.implementation_rev.as_deref().unwrap_or(&local_rev);
    let mut out = String::new();
    out.push_str("{\n");
    writeln!(out, "  \"impl\": \"{}\",", cfg.impl_label).unwrap();
    out.push_str("  \"language\": \"rust\",\n");
    writeln!(out, "  \"scenario\": \"{}\",", cfg.scenario.as_str()).unwrap();
    writeln!(out, "  \"wait_strategy\": \"{}\",", cfg.wait.as_str()).unwrap();
    writeln!(out, "  \"event_padding\": \"{}\",", cfg.pad.as_str()).unwrap();
    out.push_str("  \"api_path\": \"builder\",\n");
    writeln!(out, "  \"buffer_size\": {},", cfg.buffer_size).unwrap();
    writeln!(out, "  \"events_total\": {},", cfg.events_total).unwrap();
    writeln!(out, "  \"batch_size\": {},", cfg.batch_size).unwrap();
    writeln!(out, "  \"warmup_rounds\": {},", cfg.warmup_rounds).unwrap();
    writeln!(out, "  \"measured_rounds\": {},", cfg.measured_rounds).unwrap();
    #[cfg(feature = "bench-round-diagnostics")]
    writeln!(out, "  \"round_diagnostics\": {},", cfg.round_diagnostics).unwrap();
    writeln!(out, "  \"run_order\": \"{}\",", cfg.run_order).unwrap();
    writeln!(out, "  \"pair_id\": \"{}\",", cfg.pair_id).unwrap();
    writeln!(out, "  \"fork_index\": {},", cfg.fork_index).unwrap();
    writeln!(out, "  \"harness_git_rev\": \"{harness_rev}\",").unwrap();
    writeln!(out, "  \"implementation_rev\": \"{implementation_rev}\",").unwrap();
    writeln!(
        out,
        "  \"harness_dirty\": {},",
        cfg.harness_dirty.unwrap_or_else(git_dirty)
    )
    .unwrap();
    writeln!(
        out,
        "  \"implementation_dirty\": {},",
        cfg.implementation_dirty.unwrap_or_else(git_dirty)
    )
    .unwrap();
    writeln!(out, "  \"git_rev\": \"{local_rev}\",").unwrap();
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
    out.push_str("    \"role_cpu_map\": {");
    let roles = cfg.affinity_roles();
    if roles.is_empty() {
        out.push_str("}\n");
    } else {
        out.push('\n');
        for (index, (role, cpu)) in roles.iter().enumerate() {
            writeln!(
                out,
                "      \"{role}\": {cpu}{}",
                if index + 1 == roles.len() { "" } else { "," }
            )
            .unwrap();
        }
        out.push_str("    }\n");
    }
    out.push_str("  },\n");
    out.push_str("  \"rounds\": [\n");
    for (idx, r) in rounds.iter().enumerate() {
        out.push_str("    {\n");
        writeln!(out, "      \"index\": {},", r.index).unwrap();
        writeln!(out, "      \"phase\": \"{}\",", r.phase).unwrap();
        writeln!(out, "      \"elapsed_ns\": {},", r.elapsed_ns).unwrap();
        writeln!(out, "      \"events\": {},", r.events).unwrap();
        writeln!(out, "      \"ops_per_sec\": {:.6},", r.ops_per_sec).unwrap();
        #[cfg(not(feature = "bench-round-diagnostics"))]
        writeln!(out, "      \"checksum_valid\": {}", r.checksum_ok).unwrap();
        #[cfg(feature = "bench-round-diagnostics")]
        if let Some(diagnostics) = &r.diagnostics {
            writeln!(out, "      \"checksum_valid\": {},", r.checksum_ok).unwrap();
            write_round_diagnostics_json(&mut out, diagnostics);
        } else {
            writeln!(out, "      \"checksum_valid\": {}", r.checksum_ok).unwrap();
        }
        if idx + 1 == rounds.len() {
            out.push_str("    }\n");
        } else {
            out.push_str("    },\n");
        }
    }
    out.push_str("  ],\n");
    out.push_str("  \"summary\": {\n");
    writeln!(
        out,
        "    \"checksum_valid_all\": {},",
        summary.checksum_valid_all
    )
    .unwrap();
    writeln!(
        out,
        "    \"median_ops_per_sec\": {:.6},",
        summary.median_ops_per_sec
    )
    .unwrap();
    writeln!(
        out,
        "    \"mean_ops_per_sec\": {:.6},",
        summary.mean_ops_per_sec
    )
    .unwrap();
    writeln!(
        out,
        "    \"min_ops_per_sec\": {:.6},",
        summary.min_ops_per_sec
    )
    .unwrap();
    writeln!(
        out,
        "    \"max_ops_per_sec\": {:.6},",
        summary.max_ops_per_sec
    )
    .unwrap();
    writeln!(
        out,
        "    \"stddev_ops_per_sec\": {:.6},",
        summary.stddev_ops_per_sec
    )
    .unwrap();
    writeln!(out, "    \"cv\": {:.6}", summary.cv).unwrap();
    out.push_str("  }\n");
    out.push_str("}\n");
    out
}

fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            print_help();
            std::process::exit(2);
        }
    };

    #[cfg(feature = "bench-round-diagnostics")]
    configure_bench_round_diagnostics(cfg.round_diagnostics);

    pin_current_thread(cfg.cpu(0), &cfg.affinity_failed);
    if !cfg.cpu_list.is_empty() && cfg.affinity_failed.load(Ordering::Acquire) {
        eprintln!("error: failed to pin publisher/coordinator thread");
        std::process::exit(1);
    }

    let rounds = match cfg.scenario {
        Scenario::Unicast => run_unicast(&cfg, false),
        Scenario::UnicastBatch => run_unicast(&cfg, true),
        Scenario::MpscBatch => run_mpsc_batch(&cfg),
        Scenario::Pipeline => run_pipeline(&cfg),
    };

    let rounds = match rounds {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    let summary = summarize(&rounds, cfg.warmup_rounds);
    let json = write_result(&cfg, &rounds, &summary);

    if let Some(path) = &cfg.output {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = fs::write(path, &json) {
            eprintln!("failed to write {}: {e}", path.display());
            std::process::exit(1);
        }
    }
    print!("{json}");

    if !summary.checksum_valid_all {
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_handler_publishes_completion_only_at_final_sequence() {
        let processed = Arc::new(AtomicU64::new(0));
        let checksum = Arc::new(AtomicU64::new(0));
        let ready = Arc::new(AtomicU64::new(0));
        let mut handler = TerminalHandler::new(
            Arc::clone(&processed),
            Arc::clone(&checksum),
            Arc::clone(&ready),
            3,
            TerminalMode::Value,
            None,
            Arc::new(AtomicBool::new(false)),
        );

        handler.on_start().unwrap();
        assert_eq!(ready.load(Ordering::Acquire), 1);
        for (sequence, value) in [10_i64, 20, 30].into_iter().enumerate() {
            let mut event = ComparisonEvent {
                value,
                ..ComparisonEvent::default()
            };
            handler
                .on_event(&mut event, sequence as i64, sequence == 2)
                .unwrap();
            if sequence < 2 {
                assert_eq!(processed.load(Ordering::Acquire), 0);
                assert_eq!(checksum.load(Ordering::Acquire), 0);
            }
        }

        assert_eq!(checksum.load(Ordering::Acquire), 60);
        assert_eq!(processed.load(Ordering::Acquire), 3);
    }

    #[cfg(feature = "bench-round-diagnostics")]
    #[test]
    fn log2_histogram_uses_floor_log2_bins() {
        let mut histogram = Log2Histogram::default();
        for value in [1_u64, 2, 3, 4, 7, 8] {
            histogram.observe(value);
        }
        let snapshot = histogram.snapshot();
        assert_eq!(snapshot.count, 6);
        assert_eq!(snapshot.sum, 25);
        assert_eq!(snapshot.min, 1);
        assert_eq!(snapshot.max, 8);
        assert_eq!(snapshot.log2_bins[0..4], [1, 2, 2, 1]);
        assert_eq!(snapshot.log2_bins.iter().sum::<u64>(), 6);
    }
}
