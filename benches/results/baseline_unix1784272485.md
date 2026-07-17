# BadBatch post-modernization throughput baseline

- **UTC stamp**: `unix1784272485`
- **Host**: `Darwin Bearmac16.local 25.5.0 Darwin Kernel Version 25.5.0: Tue Jun  9 22:18:58 PDT 2026; root:xnu-12377.121.10~1/RELEASE_ARM64_T6000 arm64`
- **rustc**: `rustc 1.97.0 (2d8144b78 2026-07-07) (Homebrew)`
- **git**: `71d3bab`
- **Config**: events≈2000000, buffer=1024, wait=`BusySpinWaitStrategy`
- **Method**: wall-clock publish-all + wait for consumer count; `cargo run --release`
- **Generator**: `src/bin/baseline_metrics.rs`

| Scenario | Events | Time (s) | Throughput (Melem/s) | OK |
|----------|-------:|---------:|---------------------:|:--:|
| SPSC BusySpin pad=none | 2000000 | 0.012 | **170.37** | yes |
| SPSC BusySpin pad=128 | 2000000 | 0.070 | **28.49** | yes |
| SPSC BatchBusySpin batch=64 | 2000000 | 0.006 | **323.98** | yes |
| SPSC BatchBusySpin batch=256 | 2000000 | 0.006 | **335.87** | yes |
| MPSC BusySpin producers=2 | 2000000 | 0.153 | **13.08** | yes |
| MPSC BusySpin producers=4 | 2000000 | 0.326 | **6.13** | yes |
| WorkerPool BusySpin workers=2 | 2000000 | 0.232 | **8.61** | yes |
| WorkerPool BusySpin workers=4 | 2000000 | 0.347 | **5.77** | yes |
| Pipeline stages=2 BusySpin | 2000000 | 0.015 | **132.13** | yes |
| Pipeline stages=3 BusySpin | 1000000 | 0.014 | **73.00** | yes |

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
