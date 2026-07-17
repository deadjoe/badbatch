//! Deterministic throughput baseline runner for post-modernization metrics.
//!
//! ```bash
//! RUSTFLAGS="-C target-cpu=native" cargo run --release --bin baseline_metrics -- --events 2_000_000 --buffer 1024
//! ```
//!
//! Writes markdown to `benches/results/baseline_latest.md` (tracked) and
//! `target/baseline/metrics_<stamp>.md`.

#![allow(missing_docs, clippy::print_stdout, clippy::print_stderr)]

use badbatch::disruptor::{
    build_multi_producer, build_single_producer, BusySpinWaitStrategy, Producer,
};
use std::env;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Default, Clone, Copy)]
struct Ev {
    value: i64,
}

struct Row {
    scenario: String,
    events: u64,
    elapsed_s: f64,
    melem_s: f64,
    ok: bool,
}

fn parse_args() -> (u64, usize) {
    let mut events = 2_000_000_u64;
    let mut buffer = 1024_usize;
    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--events" => {
                i += 1;
                events = args
                    .get(i)
                    .and_then(|s| s.replace('_', "").parse().ok())
                    .unwrap_or(events);
            }
            "--buffer" => {
                i += 1;
                buffer = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(buffer);
            }
            _ => {}
        }
        i += 1;
    }
    (events, buffer)
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

fn melem(events: u64, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64().max(1e-12);
    (events as f64 / secs) / 1_000_000.0
}

fn spsc(events: u64, buffer: usize, pad: bool) -> Row {
    let name = if pad {
        "SPSC BusySpin pad=128"
    } else {
        "SPSC BusySpin pad=none"
    };
    let counter = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&counter);
    let mut d = build_single_producer(buffer, Ev::default, BusySpinWaitStrategy)
        .with_cache_line_padding(pad)
        .handle_events_with(move |e: &mut Ev, _, _| {
            std::hint::black_box(e.value);
            c.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    let start = Instant::now();
    for i in 0..events {
        d.publish(|e| e.value = i as i64);
    }
    let ok = wait_count(&counter, events, Duration::from_secs(30));
    let elapsed = start.elapsed();
    d.shutdown();

    Row {
        scenario: name.to_string(),
        events,
        elapsed_s: elapsed.as_secs_f64(),
        melem_s: melem(events, elapsed),
        ok,
    }
}

fn spsc_batch(events: u64, buffer: usize, batch: usize) -> Row {
    let counter = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&counter);
    let mut d = build_single_producer(buffer, Ev::default, BusySpinWaitStrategy)
        .handle_events_with(move |e: &mut Ev, _, _| {
            std::hint::black_box(e.value);
            c.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    let start = Instant::now();
    let mut sent = 0_u64;
    while sent < events {
        let n = ((events - sent) as usize).min(batch);
        d.batch_publish(n, |iter| {
            for (i, e) in iter.enumerate() {
                e.value = (sent + i as u64) as i64;
            }
        });
        sent += n as u64;
    }
    let ok = wait_count(&counter, events, Duration::from_secs(30));
    let elapsed = start.elapsed();
    d.shutdown();

    Row {
        scenario: format!("SPSC BatchBusySpin batch={batch}"),
        events,
        elapsed_s: elapsed.as_secs_f64(),
        melem_s: melem(events, elapsed),
        ok,
    }
}

fn mpsc(events: u64, buffer: usize, producers: usize) -> Row {
    let counter = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&counter);
    let mut d = build_multi_producer(buffer.max(64), Ev::default, BusySpinWaitStrategy)
        .handle_events_with(move |e: &mut Ev, _, _| {
            std::hint::black_box(e.value);
            c.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    let per = events / producers as u64;
    let start = Instant::now();
    let mut handles = Vec::new();
    for p in 0..producers {
        let mut prod = d.create_producer();
        handles.push(thread::spawn(move || {
            let base = p as u64 * per;
            for i in 0..per {
                prod.publish(|e| e.value = (base + i) as i64);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let total = per * producers as u64;
    let ok = wait_count(&counter, total, Duration::from_secs(60));
    let elapsed = start.elapsed();
    d.shutdown();

    Row {
        scenario: format!("MPSC BusySpin producers={producers}"),
        events: total,
        elapsed_s: elapsed.as_secs_f64(),
        melem_s: melem(total, elapsed),
        ok,
    }
}

fn worker_pool(events: u64, buffer: usize, workers: usize) -> Row {
    let counter = Arc::new(AtomicU64::new(0));
    let c0 = Arc::clone(&counter);
    let mut builder = build_single_producer(buffer, Ev::default, BusySpinWaitStrategy)
        .handle_events_with(move |e: &mut Ev, _, _| {
            std::hint::black_box(e.value);
            c0.fetch_add(1, Ordering::Relaxed);
        });
    for _ in 1..workers {
        let c = Arc::clone(&counter);
        builder = builder.handle_events_with(move |e: &mut Ev, _, _| {
            std::hint::black_box(e.value);
            c.fetch_add(1, Ordering::Relaxed);
        });
    }
    let mut d = builder.build();

    let start = Instant::now();
    for i in 0..events {
        d.publish(|e| e.value = i as i64);
    }
    let ok = wait_count(&counter, events, Duration::from_secs(60));
    let elapsed = start.elapsed();
    d.shutdown();

    Row {
        scenario: format!("WorkerPool BusySpin workers={workers}"),
        events,
        elapsed_s: elapsed.as_secs_f64(),
        melem_s: melem(events, elapsed),
        ok,
    }
}

fn pipeline2(events: u64, buffer: usize) -> Row {
    let counter = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&counter);
    let mut d = build_single_producer(buffer, Ev::default, BusySpinWaitStrategy)
        .handle_events_with(|e: &mut Ev, _, _| {
            e.value = e.value.wrapping_add(1);
        })
        .and_then()
        .handle_events_with(move |e: &mut Ev, _, _| {
            std::hint::black_box(e.value);
            c.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    let start = Instant::now();
    for i in 0..events {
        d.publish(|e| e.value = i as i64);
    }
    let ok = wait_count(&counter, events, Duration::from_secs(60));
    let elapsed = start.elapsed();
    d.shutdown();

    Row {
        scenario: "Pipeline stages=2 BusySpin".into(),
        events,
        elapsed_s: elapsed.as_secs_f64(),
        melem_s: melem(events, elapsed),
        ok,
    }
}

fn pipeline3(events: u64, buffer: usize) -> Row {
    let counter = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&counter);
    let mut d = build_single_producer(buffer, Ev::default, BusySpinWaitStrategy)
        .handle_events_with(|e: &mut Ev, _, _| {
            e.value = e.value.wrapping_add(1);
        })
        .and_then()
        .handle_events_with(|e: &mut Ev, _, _| {
            e.value = e.value.wrapping_add(1);
        })
        .and_then()
        .handle_events_with(move |e: &mut Ev, _, _| {
            std::hint::black_box(e.value);
            c.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    let start = Instant::now();
    for i in 0..events {
        d.publish(|e| e.value = i as i64);
    }
    let ok = wait_count(&counter, events, Duration::from_secs(60));
    let elapsed = start.elapsed();
    d.shutdown();

    Row {
        scenario: "Pipeline stages=3 BusySpin".into(),
        events,
        elapsed_s: elapsed.as_secs_f64(),
        melem_s: melem(events, elapsed),
        ok,
    }
}

fn main() {
    let (events, buffer) = parse_args();
    let stamp = chrono_like_stamp();
    let host = uname_s();
    let rustc = rustc_v();
    let git = git_head();

    eprintln!("baseline_metrics events={events} buffer={buffer}");
    eprintln!("host={host}");
    eprintln!("rustc={rustc}");
    eprintln!("git={git}");

    // Warmup
    let _ = spsc(50_000, buffer, false);

    let rows = vec![
        spsc(events, buffer, false),
        spsc(events, buffer, true),
        spsc_batch(events, buffer, 64),
        spsc_batch(events, buffer, 256),
        mpsc(events, buffer, 2),
        mpsc(events, buffer, 4),
        worker_pool(events, buffer, 2),
        worker_pool(events, buffer, 4),
        pipeline2(events, buffer),
        pipeline3(events / 2, buffer),
    ];

    let md = format_markdown(&stamp, &host, &rustc, &git, events, buffer, &rows);
    print!("{md}");

    let out_dir = "target/baseline";
    let _ = fs::create_dir_all(out_dir);
    let path = format!("{out_dir}/metrics_{stamp}.md");
    fs::write(&path, &md).expect("write metrics");
    let _ = fs::create_dir_all("benches/results");
    let committed = format!("benches/results/baseline_{stamp}.md");
    fs::write(&committed, &md).expect("write committed baseline");
    fs::write("benches/results/baseline_latest.md", &md).expect("write latest");
    eprintln!("wrote {path}");
    eprintln!("wrote {committed}");
}

fn format_markdown(
    stamp: &str,
    host: &str,
    rustc: &str,
    git: &str,
    events: u64,
    buffer: usize,
    rows: &[Row],
) -> String {
    let mut s = String::new();
    s.push_str("# BadBatch post-modernization throughput baseline\n\n");
    s.push_str(&format!("- **UTC stamp**: `{stamp}`\n"));
    s.push_str(&format!("- **Host**: `{host}`\n"));
    s.push_str(&format!("- **rustc**: `{rustc}`\n"));
    s.push_str(&format!("- **git**: `{git}`\n"));
    s.push_str(&format!(
        "- **Config**: events≈{events}, buffer={buffer}, wait=`BusySpinWaitStrategy`\n"
    ));
    s.push_str(
        "- **Method**: wall-clock publish-all + wait for consumer count; `cargo run --release`\n",
    );
    s.push_str("- **Generator**: `src/bin/baseline_metrics.rs`\n\n");
    s.push_str("| Scenario | Events | Time (s) | Throughput (Melem/s) | OK |\n");
    s.push_str("|----------|-------:|---------:|---------------------:|:--:|\n");
    for r in rows {
        s.push_str(&format!(
            "| {} | {} | {:.3} | **{:.2}** | {} |\n",
            r.scenario,
            r.events,
            r.elapsed_s,
            r.melem_s,
            if r.ok { "yes" } else { "NO" }
        ));
    }
    s.push_str("\n## Interpretation notes\n\n");
    s.push_str(
        "- `pad=none` vs `pad=128`: false-sharing vs working-set tradeoff (esp. Apple Silicon).\n",
    );
    s.push_str(
        "- `BatchBusySpin`: range publish amortizes coordination (closer to LMAX batch paths).\n",
    );
    s.push_str("- `MPSC`: multi-producer CAS claim + bitmap availability.\n");
    s.push_str(
        "- `WorkerPool`: same-stage work-sharing (one handler per sequence), not fan-out.\n",
    );
    s.push_str("- `Pipeline`: multi-stage dependency via `and_then`.\n");
    s.push_str("- Single-run wall-clock; re-run 3× and take median for serious comparisons.\n");
    s.push_str("\n## How to reproduce\n\n");
    s.push_str("```bash\n");
    s.push_str("RUSTFLAGS=\"-C target-cpu=native\" cargo run --release --bin baseline_metrics -- --events 2_000_000 --buffer 1024\n");
    s.push_str("```\n");
    s
}

fn chrono_like_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix{secs}")
}

fn uname_s() -> String {
    std::process::Command::new("uname")
        .arg("-a")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn rustc_v() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn git_head() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}
