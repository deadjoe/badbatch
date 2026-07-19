# Post-alignment H2H refresh (2026-07-19)

> **Historical pre-fork evidence.** This capture predates the fork-level paired
> protocol in `scripts/run_head_to_head.sh`. Its within-process CVs and exact
> Rust/Java ratios are retained for audit history, not as a current ranking.

Commit **`3b6d5b1`** (LMAX wait availability + terminal-stage gating), Apple M1 Max,
`RUSTFLAGS=-C target-cpu=native`.

Raw: `head_to_head_results/post_align_20260719_191520/`.

## 1. Full all-scenario H2H (default wait strategies)

| scenario | rust Melem/s | java Melem/s | rust/java | rust CV | java CV | checksum |
|----------|-------------:|-------------:|----------:|--------:|--------:|:--------:|
| unicast | 141.60 | 79.41 | **1.78×** | 0.001 | 0.304 | OK |
| unicast_batch | 127.05 | 138.93 | **0.91×** | 0.016 | 0.104 | OK |
| mpsc_batch | 94.10 | 83.80 | **1.12×** | 0.036 | 0.105 | OK |
| pipeline (yielding 8k) | 30.11 | 24.91 | **1.21×** | **0.080** | 0.677 | OK |

### vs earlier full native (`H2H_FULL_NATIVE.md`, pre wait/gating fix)

| scenario | then rust/java | now | note |
|----------|---------------:|----:|------|
| unicast | 1.07× | 1.78× | Java softer this session; rust CV still tiny |
| unicast_batch | 1.24× | 0.91× | within run noise / JVM; rust still ~127 Melem/s |
| mpsc_batch | 1.25× | 1.12× | still ahead |
| pipeline | 0.55× | **1.21×** | alignment fix holds on full refresh |

**Do claim:** on this machine/native, BadBatch is competitive across the board after
pipeline alignment; pipeline rust CV is usable (0.08) under yielding this run.

**Do not claim:** a single global ratio, or portable absolute Melem/s.

## 2. Busy-spin full pipeline (100M events)

| config | rust | java | rust/java | rust CV | java CV |
|--------|-----:|-----:|----------:|--------:|--------:|
| busy-spin **8k** | **30.05** | 16.31 | **1.84×** | **0.042** | 0.431 |
| busy-spin **64k** | 32.18 | **54.30** | 0.59× | 0.351 | 0.553 |

### Busy-spin interpretation

1. **At the H2H default buffer (8192), busy-spin confirms Rust is not behind** —
   low-CV rust 30 Melem/s vs noisy Java 16.
2. **Java still scales with 64k under busy-spin (~16 → 54); Rust barely moves
   (~30 → 32).** Gap reappears when Java gets large slack; root is still multi-stage
   efficiency under high concurrency / large working sets, not “Rust is always slow.”
3. Prefer **busy-spin 8k** when you need a stable pipeline ranking on macOS;
   prefer **yielding** for fairer CPU sharing.

## 3. Claim-lock + affinity deep dive (item 3)

### Claim lock (`SingleProducerSequencer::claim_lock`)

| phase (`unicast_breakdown`, busy-spin, 2M) | Melem/s | ~ns/evt |
|--------------------------------------------|--------:|--------:|
| baseline arithmetic | 3028 | 0.33 |
| claim_only | 162 | **6.2** |
| claim_write | 169 | 5.9 |
| claim_write_publish | 155 | 6.5 |
| end_to_end (w/ consumer) | 34* | — |

\*e2e number here is layout/buffer constrained vs H2H unicast (~141 Melem/s at 64k).

**Cost structure:** each SP claim does `AtomicBool::swap(Acquire)` + `store(Release)`
on drop. That is real ARM barrier work (~part of the ~6 ns claim path).

**Decision: keep the lock.** Reasons:

1. Product path already **beats or matches Java unicast** (~141 Melem/s full H2H).
2. Lock is a deliberate **fail-closed residual** for `unsafe { new }` + shared `Arc`
   dual drivers (`ConcurrentClaimDriver`) — audit requirement, not accidental.
3. Removing it for a few ns would trade a measured win for a soundness regression
   risk on a path that is **not** the current performance gap (pipeline multi-stage).

**When to revisit:** only if a future unicast/batch H2H falls clearly behind Java
*and* profiles show claim_lock as the leaf hotspot (not gating wait).

### CPU affinity

- Builder / `ThreadBuilder` / `ElegantConsumer::with_affinity` already support pin.
- H2H harness intentionally does **not** pin (fair default laptop run).
- On M1 Max, pinning can help *or* hurt depending on core type (P vs E) and
  thermal; treat as a **deployment knob**, not a library default.

**Decision: no H2H affinity change for now.** Optional experiment later:
pin producer + 3 pipeline stages to P-cores only, re-run busy-spin 8k/64k.

## Reproduce

```bash
export RUSTFLAGS="-C target-cpu=native"

# Full all scenarios
bash scripts/run_head_to_head.sh --mode full --scenario all --order rust-first

# Busy-spin pipeline (manual; harness defaults yielding for pipeline)
cargo build --release --features bench-tools --bin h2h_rust
./target/release/h2h_rust --scenario pipeline --wait-strategy busy-spin \
  --buffer-size 8192 --events-total 100000000 --warmup-rounds 3 --measured-rounds 7 \
  --output /tmp/rust_pipe_busy.json
# Java: tools/head_to_head HeadToHead with the same flags
```

## Bottom line

| Question | Answer |
|----------|--------|
| Busy-spin confirms pipeline? | **Yes at 8k** (rust ~30, CV 0.04, ahead of Java) |
| Full refresh after align? | **Yes** — unicast/mpsc ahead; batch ~0.91×; pipeline ~1.21× |
| Claim lock / affinity now? | **No code change** — document only; not the active gap |
