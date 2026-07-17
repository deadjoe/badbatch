# BadBatch post-modernization throughput baseline (summary)

See **`BASELINE.md`** for full environment, three-run table, and interpretation.

## Median throughput (Apple Silicon, 3 runs)

| Scenario | Median Melem/s |
|----------|---------------:|
| SPSC BusySpin pad=none | **160.5** |
| SPSC BusySpin pad=128 | **28.1** |
| SPSC BatchBusySpin batch=64 | **396.3** |
| SPSC BatchBusySpin batch=256 | **454.3** |
| MPSC producers=2 | **13.1** |
| MPSC producers=4 | **7.0** |
| WorkerPool workers=2 | **11.0** |
| WorkerPool workers=4 | **6.6** |
| Pipeline stages=2 | **127.1** |
| Pipeline stages=3 | **73.0** |

```bash
RUSTFLAGS="-C target-cpu=native" cargo run --release --bin baseline_metrics -- --events 2_000_000 --buffer 1024
```
