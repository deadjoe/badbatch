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
    build_multi_producer, build_single_producer, BusySpinWaitStrategy, Producer, SlotPadding,
    YieldingWaitStrategy,
};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    impl_label: String,
    output: Option<PathBuf>,
    timeout: Duration,
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
    let mut impl_label = "badbatch-builder".into();
    let mut output = None;
    let mut quick = false;

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
            "--impl-label" => {
                impl_label = args.next().ok_or("missing --impl-label")?;
            }
            "--output" => {
                output = Some(PathBuf::from(args.next().ok_or("missing --output")?));
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
        impl_label,
        output,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
    })
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
        let p = Arc::clone(&processed);
        let c = Arc::clone(&checksum);

        let mut handle =
            build_single_producer(cfg.buffer_size, ComparisonEvent::default, wait.clone())
                .with_slot_padding(cfg.pad.slot_padding())
                .handle_events_with(move |e: &mut ComparisonEvent, _s, _end| {
                    c.fetch_add(e.value as u64, Ordering::Relaxed);
                    p.fetch_add(1, Ordering::Release);
                })
                .build();

        thread::sleep(Duration::from_millis(5));

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
        handle.shutdown();

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
        let p = Arc::clone(&processed);
        let c = Arc::clone(&checksum);

        let mut handle =
            build_multi_producer(cfg.buffer_size, ComparisonEvent::default, wait.clone())
                .handle_events_with(move |e: &mut ComparisonEvent, _s, _end| {
                    c.fetch_add(e.value as u64, Ordering::Relaxed);
                    p.fetch_add(1, Ordering::Release);
                })
                .build();

        thread::sleep(Duration::from_millis(5));

        let start_flag = Arc::new(AtomicBool::new(false));
        let mut joins = Vec::new();
        for id in 0..MPSC_PRODUCERS {
            let mut prod = handle.create_producer();
            let start_flag = Arc::clone(&start_flag);
            let batch = cfg.batch_size;
            let range_start = per * id as u64;
            let range_end = range_start + per;
            joins.push(thread::spawn(move || {
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
        let p = Arc::clone(&processed);
        let c = Arc::clone(&checksum);

        let mut handle =
            build_single_producer(cfg.buffer_size, ComparisonEvent::default, wait.clone())
                .with_slot_padding(cfg.pad.slot_padding())
                .handle_events_with(|e: &mut ComparisonEvent, _s, _end| {
                    e.stage1_value = e.value.wrapping_add(1);
                })
                .and_then()
                .handle_events_with(|e: &mut ComparisonEvent, _s, _end| {
                    e.stage2_value = e.stage1_value.wrapping_add(3);
                })
                .and_then()
                .handle_events_with(move |e: &mut ComparisonEvent, _s, _end| {
                    e.stage3_value = e.stage2_value.wrapping_add(7);
                    c.fetch_add(e.stage3_value as u64, Ordering::Relaxed);
                    p.fetch_add(1, Ordering::Release);
                })
                .build();

        thread::sleep(Duration::from_millis(10));

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
        handle.shutdown();

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

fn write_result(cfg: &Config, rounds: &[Round], summary: &Summary) -> String {
    use std::fmt::Write as _;

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
    writeln!(out, "  \"run_order\": \"{}\",", cfg.run_order).unwrap();
    writeln!(out, "  \"git_rev\": \"{}\",", git_rev()).unwrap();
    out.push_str("  \"rounds\": [\n");
    for (idx, r) in rounds.iter().enumerate() {
        out.push_str("    {\n");
        writeln!(out, "      \"index\": {},", r.index).unwrap();
        writeln!(out, "      \"phase\": \"{}\",", r.phase).unwrap();
        writeln!(out, "      \"elapsed_ns\": {},", r.elapsed_ns).unwrap();
        writeln!(out, "      \"events\": {},", r.events).unwrap();
        writeln!(out, "      \"ops_per_sec\": {:.6},", r.ops_per_sec).unwrap();
        writeln!(out, "      \"checksum_valid\": {}", r.checksum_ok).unwrap();
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
