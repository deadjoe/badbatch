# Post-modernization throughput baseline (macOS)

## Post-audit capture (2026-07-19, commit `fa04ba6`)

Re-captured after the 2026-07-18 audit rounds (P0 soundness, P1 failure
semantics, P2 surface) plus the P3 hot-path fix (`#[inline]` on claim methods,
Relaxed poison-flag loads, forget-based panic guards).

| Scenario | Run1 | Run2 | Run3 | **Median** |
|----------|-----:|-----:|-----:|-----------:|
| SPSC BusySpin pad=none | 65.2 | 101.2 | 180.9 | **101.2** |
| SPSC BusySpin pad=128 | 21.2 | 16.3 | 22.3 | **21.2** |
| SPSC BatchBusySpin batch=64 | 417.2 | 211.6 | 304.6 | **304.6** |
| SPSC BatchBusySpin batch=256 | 377.0 | 304.9 | 334.8 | **334.8** |
| MPSC BusySpin producers=2 | 7.1 | 7.7 | 8.6 | **7.7** |
| MPSC BusySpin producers=4 | 5.8 | 6.1 | 5.6 | **5.8** |
| WorkerPool workers=2 | 7.8 | 11.5 | 9.9 | **9.9** |
| WorkerPool workers=4 | 4.9 | 4.7 | 4.6 | **4.7** |
| Pipeline stages=2 | 118.3 | 128.9 | 81.2 | **118.3** |
| Pipeline stages=3 | 40.1 | 46.7 | 61.5 | **46.7** |

**Read this capture with its variance in mind.** The machine was noisier than
at the original capture (SPSC spread 65–181). The controlled comparison is the
**interleaved A/B on identical machine state** against pre-audit `da1097e`:
SPSC 136.5 (old) vs 160.9 (new), pipeline 110 vs 116, MPSC-2 8.2 vs 8.4
(medians, Melem/s) — the audited code is at **parity within run noise**; the
deltas against the table below are machine drift, not code cost.

Bisection note: the P1 poisoning commit initially cost ~5× on single-event
paths (inline-threshold regression on `next_n` + per-event Acquire load
pairing STLR/LDAR into a StoreLoad barrier on ARM). Fixed in `fa04ba6`; see
that commit message before adding anything to the claim hot path.

## Original capture (commits `5c1d84a` / `71d3bab`)

Recorded on the development machine after commits `5c1d84a` / `71d3bab` (consumer engine + monomorphization + WorkerPool).

## Environment

| Item | Value |
|------|--------|
| Host | Darwin arm64 (Apple Silicon T6000), macOS 25.5 |
| rustc | 1.97.0 (Homebrew) |
| Build | `cargo run --release --bin baseline_metrics` with `RUSTFLAGS=-C target-cpu=native` |
| Method | Publish all events then spin-wait for consumer counter; wall-clock |
| Config | `buffer=1024`, `BusySpinWaitStrategy`, ~2M events (pipeline-3 uses 1M) |
| Git | `71d3bab` at capture time |

## Results (3 runs → **median** Melem/s)

| Scenario | Run1 | Run2 | Run3 | **Median** |
|----------|-----:|-----:|-----:|-----------:|
| SPSC BusySpin pad=none | 85.2 | 160.5 | 170.4 | **160.5** |
| SPSC BusySpin pad=128 | 17.7 | 28.1 | 28.5 | **28.1** |
| SPSC BatchBusySpin batch=64 | 396.3 | 466.8 | 324.0 | **396.3** |
| SPSC BatchBusySpin batch=256 | 454.3 | 465.6 | 335.9 | **454.3** |
| MPSC BusySpin producers=2 | 11.8 | 15.2 | 13.1 | **13.1** |
| MPSC BusySpin producers=4 | 7.0 | 7.2 | 6.1 | **7.0** |
| WorkerPool workers=2 | 12.8 | 11.0 | 8.6 | **11.0** |
| WorkerPool workers=4 | 6.6 | 6.9 | 5.8 | **6.6** |
| Pipeline stages=2 | 127.1 | 96.0 | 132.1 | **127.1** |
| Pipeline stages=3 | 44.8 | 95.1 | 73.0 | **73.0** |

Raw run logs: `benches/results/baseline_unix*.md` (last three captures).

## Interpretation (actionable)

1. **Default slot padding should stay `None` on Apple Silicon**  
   For this tiny `Ev { i64 }` event, **pad=128 is ~5–6× slower** than inline (28 vs 160 Melem/s). Matches earlier design notes: larger working set dominates vs false-sharing relief on this platform.

2. **Batch publish is the high-throughput path**  
   Batch 64/256 sits ~**350–450 Melem/s**, well above single-event SPSC. This is the path to advertise for bulk ingest.

3. **docs/DESIGN.md 20–100M SPSC range is still a coarse planning band**  
   Median single-event SPSC ~**160 Melem/s** on this method sits above the old band (methodology differs from criterion / Java H2H; still a healthy modernized signal).

4. **MPSC / WorkerPool are contention-bound, not “broken”**  
   2 producers ~13 Melem/s, 4 producers ~7 Melem/s; WorkerPool 2 ~11, 4 ~6.6. More CAS fighters on one work cursor / multi-producer claim **reduces** throughput for a trivial handler — expected. Do **not** “optimize” by adding locks; improve only with affinity + real work amortization + batch claim later if needed.

5. **Pipeline remains strong**  
   2-stage ~**127 Melem/s** median — dependency barriers are not catastrophic for light handlers.

## What we will *not* do from this data alone

- Force CacheLine128 by default (hurts here).
- Add WorkerPool workers by default for empty handlers.
- Chase MPSC to SPSC numbers without a different claim design experiment.

## What to do next (data-driven)

| Priority | Action |
|----------|--------|
| Docs | Document “batch first, pad only after measure, WorkerPool for real multi-core work not empty handlers” |
| Optional opt | Batch path already hot; keep consumer_engine sequential path lean (already unified) |
| Optional exp | WorkerPool batch-claim (claim N sequences per CAS) if multi-worker empty-handler throughput becomes a product goal |
| Linux | Re-run same binary when Linux env exists; compare pad=none vs pad=128 (x86 often flips the padding story) |

## Reproduce

```bash
RUSTFLAGS="-C target-cpu=native" cargo run --release --bin baseline_metrics -- --events 2_000_000 --buffer 1024
# run 3×; update this table’s medians
```

Also: `./scripts/run_baseline.sh` (wraps the same binary after upgrade).
