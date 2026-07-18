# Post-audit Java LMAX head-to-head (2026-07-19)

Captured at commit `fa04ba6` after the P0–P2 audit rounds and the P3 hot-path
poison fix. Full raw artifacts (gitignored):  
`head_to_head_results/20260719_010308_51891/`.

This file is the **checked-in** summary so the run survives without the local
results directory.

## Environment

| Item | Value |
|------|--------|
| Host | Darwin arm64 (Apple Silicon T6000), macOS 25.5 |
| git | `fa04ba6` |
| rustc | 1.97.1 (Homebrew) |
| Java | OpenJDK 26.0.1 |
| Mode | `full`, order=`rust-first`, padding=`none`, scenario=`all` |
| Harness | `scripts/run_head_to_head.sh` → `h2h_rust` + LMAX BEP |
| RUSTFLAGS | *(empty in this run — not `-C target-cpu=native`)* |

Scenario configs (from harness):

| Scenario | Wait | Buffer | Events | Batch |
|----------|------|-------:|-------:|------:|
| unicast | yielding | 65536 | 1e8 | 1 |
| unicast_batch | yielding | 65536 | 1e8 | 10 |
| mpsc_batch | busy-spin | 65536 | 6e7 | 10 |
| pipeline | yielding | 8192 | 1e8 | 1 |

Warmup 3 + measured 7 rounds; checksums validated on every round.

## Results (median Melem/s)

| scenario | rust | java | rust/java | rust CV | java CV | checksum |
|----------|-----:|-----:|----------:|--------:|--------:|:--------:|
| unicast ⚠️ high CV | 13.14 | 69.52 | **0.19×** | 0.036 | **0.303** | OK |
| unicast_batch | 83.26 | 94.35 | **0.88×** | 0.067 | 0.132 | OK |
| mpsc_batch | 77.93 | 59.34 | **1.31×** | 0.027 | 0.108 | OK |
| pipeline | 7.42 | 17.25 | **0.43×** | 0.062 | 0.240 | OK |

## Comparison to prior full run (`full_20260717_174148`)

| scenario | rust/java then | rust/java now | note |
|----------|---------------:|--------------:|------|
| unicast | 0.30× | 0.19× | Java side high CV both times; not a stable ranking |
| unicast_batch | 1.03× | 0.88× | Still same ballpark; no “1.09× beat Java” claim |
| mpsc_batch | 1.14× | 1.31× | Rust multi-producer batch path remains competitive |
| pipeline | 0.45× | 0.43× | Stable deficit vs LMAX multi-stage |

## Poison / claim-path A/B (`unicast_breakdown` end-to-end)

Same machine, interleaved, pre-audit `da1097e` worktree vs `fa04ba6` (with
`fa04ba6` hot-path fix). 2M events, 1 warmup + 3 measured rounds.

| Wait strategy | old run1 | old run2 | new run1 | new run2 |
|---------------|---------:|---------:|---------:|---------:|
| busy-spin | 62.7 | 78.7 | **86.9** | **93.9** |
| yielding | 216.3 | 106.9 | **303.5** | **226.6** |

**Conclusion:** the per-claim poison flag (Relaxed load after `fa04ba6`) does
**not** regress end-to-end unicast; new code is at parity or slightly ahead
within machine noise. The earlier ~5× single-event drop was the pre-fix
Acquire/inline regression, not an inherent cost of poisoning.

## Interpretation (what we will / will not claim)

**Do claim**

- Full H2H at `fa04ba6` is archived with env, CV, and checksums.
- Batch unicast is competitive with Java (~0.88×); MPSC batch can beat Java
  on this machine (~1.3×).
- Poison checks on the claim path are not a measurable tax after the P3 fix.

**Do not claim**

- “Rust beats Java overall” or a single global ratio (unicast and pipeline lose).
- The old unsubstantiated **1.09×** headline (audit correctly rejected it).
- unicast ranking: Java CV > 0.25 makes that scenario’s median unreliable.

## Reproduce

```bash
# Full H2H (20–40 min on this machine)
bash scripts/run_head_to_head.sh --mode full --order rust-first --event-padding none

# Poison / claim A/B (needs a da1097e worktree + current tree)
RUSTFLAGS="-C target-cpu=native" cargo build --release --features bench-tools --bin unicast_breakdown
./target/release/unicast_breakdown --mode end-to-end --wait-strategy busy-spin \
  --events-total 2000000 --warmup-rounds 1 --measured-rounds 3 --output /tmp/e2e.json
```
