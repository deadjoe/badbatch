# Full Java LMAX head-to-head with `target-cpu=native` (2026-07-19)

Captured at commit **`bd85582`** (includes LMAX-aligned spin-then-yield
`YieldingWaitStrategy`).

Raw artifacts (typically gitignored):  
`head_to_head_results/full_native_20260719_180958/`.

## Environment

| Item | Value |
|------|--------|
| Host | Darwin arm64 (Apple Silicon T6000 / M1 Max), macOS 25.5 |
| git | `bd85582` |
| rustc | 1.97.1 (Homebrew) |
| Java | OpenJDK 26.0.1 |
| Mode | `full`, order=`rust-first`, padding=`none`, scenario=`all` |
| Harness | `scripts/run_head_to_head.sh` → `h2h_rust` + LMAX BEP |
| **RUSTFLAGS** | **`-C target-cpu=native`** |

| Scenario | Wait | Buffer | Events | Batch |
|----------|------|-------:|-------:|------:|
| unicast | yielding | 65536 | 1e8 | 1 |
| unicast_batch | yielding | 65536 | 1e8 | 10 |
| mpsc_batch | busy-spin | 65536 | 6e7 | 10 |
| pipeline | yielding | 8192 | 1e8 | 1 |

Warmup 3 + measured 7 rounds; checksums OK on every round.

## Results (median Melem/s)

| scenario | rust | java | rust/java | rust CV | java CV | checksum |
|----------|-----:|-----:|----------:|--------:|--------:|:--------:|
| unicast | 140.87 | 132.02 | **1.07×** | 0.003 | 0.206 | OK |
| unicast_batch | 138.88 | 112.28 | **1.24×** | 0.007 | 0.119 | OK |
| mpsc_batch | 87.72 | 70.04 | **1.25×** | 0.038 | 0.102 | OK |
| pipeline ⚠️ | 14.76 | 26.80 | **0.55×** | **0.301** | **0.670** | OK |

## Comparison to prior full run (no native, pure-yield, `fa04ba6`)

| scenario | rust/java then | rust/java now | note |
|----------|---------------:|--------------:|------|
| unicast | 0.19× | **1.07×** | Native + quieter path; old Java CV was also high |
| unicast_batch | 0.88× | **1.24×** | Rust clearly ahead on this machine |
| mpsc_batch | 1.31× | **1.25×** | Still ahead; same ballpark |
| pipeline | 0.43× | **0.55×** | Still behind; **both CVs high** — not a tight ranking |

## Interpretation

**Do claim**

- Full H2H at `bd85582` with **`target-cpu=native`** is archived (env + CV + checksums).
- On M1 Max, **unicast / unicast_batch / mpsc_batch** medians beat Java LMAX BEP in this run.
- Unicast and unicast_batch are nearly equal on the Rust side (~139–141 Melem/s) — no large single-publish tax under native.

**Do not claim**

- A single global “Rust beats Java” ratio (pipeline still loses).
- Pipeline **0.55×** as a precise gap: rust CV 0.30 / java CV 0.67 → re-run with
  busy-spin and/or buffer A/B before optimizing to a target ratio.
- Portable absolute Melem/s.

## Reproduce

```bash
export RUSTFLAGS="-C target-cpu=native"
bash scripts/run_head_to_head.sh --mode full --order rust-first --event-padding none
```

## Pipeline follow-ups (same session, full event counts)

Artifacts under `head_to_head_results/full_native_20260719_180958/pipeline_followups/`.

### Rust-only matrix (100M events, 3+7 rounds)

| config | median Melem/s | CV |
|--------|---------------:|---:|
| yielding 8k (H2H primary) | 14.76 | 0.301 |
| yielding 8k re-run | 15.18 | 0.170 |
| **busy-spin 8k** | **17.60** | 0.246 |
| yielding 64k | 12.61 | 0.253 |
| busy-spin 64k | 13.08 | 0.290 |

### Java matrix (same)

| config | median Melem/s | CV |
|--------|---------------:|---:|
| yielding 8k re-run | 24.66 | 0.398 |
| busy-spin 8k | 22.68 | 0.353 |
| **yielding 64k** | **63.31** | 0.495 |

### What this implies

1. **Larger buffer helps Java pipeline a lot (~2.5×: 25 → 63 Melem/s)** — classic
   backpressure relief when stages are uneven.
2. **Larger buffer does not help BadBatch (8k ≈ 15, 64k ≈ 13)** — Rust multi-stage path
   is **not** primarily “ring too small”; cost sits in stage coordination / wait /
   consumer loop, not producer wrap waits.
3. Busy-spin helps Rust only modestly (15 → 18 at 8k) and does not close the Java gap.
4. **Next optimization target is pipeline stage/barrier implementation**, not buffer
   size and not unicast claim path.

## Pipeline LMAX alignment follow-up (same day, after `wait`/`gating` fix)

Code changes (terminal-stage gating + dependents-only wait): see CHANGELOG
“Pipeline wait/gating LMAX alignment”. Measured on M1 Max with native.

### Full pipeline H2H (100M, yielding, buffer 8192)

| run | rust | java | rust/java | rust CV | java CV |
|-----|-----:|-----:|----------:|--------:|--------:|
| pre-fix (this file primary) | 14.76 | 26.80 | **0.55×** | 0.301 | 0.670 |
| post-fix r1 | 33.88 | 26.70 | **1.27×** | 0.319 | 0.645 |
| post-fix r2 | 24.52 | 19.23 | **1.28×** | 0.296 | 0.147 |
| post-fix r3 | 26.45 | 24.86 | **1.06×** | **0.079** | 0.698 |
| post-fix both-orders | 28.11 | 25.90 | **1.09×** | 0.345 | 0.644 |

**Interpretation:** aligning wait/gating with LMAX moves pipeline from a stable
deficit (~0.55×) to **roughly parity / slight lead** on this machine. Absolute
medians and CVs remain noisy under yielding; do not quote a single ratio.
Unicast smoke after the change stayed ~141 Melem/s (no regression).

Artifacts: `head_to_head_results/pipeline_fix_20260719_190407/`.

## Follow-ups remaining

Completed in [`H2H_POST_ALIGN.md`](./H2H_POST_ALIGN.md): busy-spin pipeline matrix,
full all-scenario refresh at `3b6d5b1`, claim-lock/affinity decision notes.
