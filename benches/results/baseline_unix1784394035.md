# BadBatch post-modernization throughput baseline

- **UTC stamp**: `unix1784394035`
- **Host**: `Darwin Bearmac16.local 25.5.0 Darwin Kernel Version 25.5.0: Tue Jun  9 22:18:58 PDT 2026; root:xnu-12377.121.10~1/RELEASE_ARM64_T6000 arm64`
- **rustc**: `rustc 1.97.1 (8bab26f4f 2026-07-14) (Homebrew)`
- **git**: `363be13`
- **Config**: events≈2000000, buffer=1024, wait=`BusySpinWaitStrategy`
- **Method**: wall-clock publish-all + wait for consumer count; `cargo run --release`
- **Generator**: `src/bin/baseline_metrics.rs`

| Scenario | Events | Time (s) | Throughput (Melem/s) | OK |
|----------|-------:|---------:|---------------------:|:--:|
| SPSC BusySpin pad=none | 2000000 | 0.020 | **101.15** | yes |
| SPSC BusySpin pad=128 | 2000000 | 0.123 | **16.25** | yes |
| SPSC BatchBusySpin batch=64 | 2000000 | 0.009 | **211.57** | yes |
| SPSC BatchBusySpin batch=256 | 2000000 | 0.007 | **304.94** | yes |
| MPSC BusySpin producers=2 | 2000000 | 0.261 | **7.65** | yes |
| MPSC BusySpin producers=4 | 2000000 | 0.328 | **6.10** | yes |
| WorkerPool BusySpin workers=2 | 2000000 | 0.173 | **11.53** | yes |
| WorkerPool BusySpin workers=4 | 2000000 | 0.430 | **4.65** | yes |
| Pipeline stages=2 BusySpin | 2000000 | 0.016 | **128.89** | yes |
| Pipeline stages=3 BusySpin | 1000000 | 0.021 | **46.70** | yes |

## Interpretation notes

- `pad=none` vs `pad=128`: false-sharing vs working-set tradeoff (esp. Apple Silicon).
- `BatchBusySpin`: range publish amortizes coordination (closer to LMAX batch paths).
- `MPSC`: multi-producer CAS claim + bitmap availability.
- `WorkerPool`: same-stage work-sharing (one handler per sequence), not fan-out.
- `Pipeline`: multi-stage dependency via `and_then`.
- Single-run wall-clock; re-run 3× and take median for serious comparisons.

## How to reproduce

```bash
RUSTFLAGS="-C target-cpu=native" cargo run --release --bin baseline_metrics -- --events 2_000_000 --buffer 1024
```
