# BadBatch Benchmark Guide

本文件是 `badbatch` benchmark 体系的权威说明。目标不是只告诉你“怎么跑”，而是把当前 `benches/`、`scripts/run_benchmarks.sh`、`scripts/result_formatter.sh` 的真实行为、测量口径、结果边界和常见误读一次写清楚。

如果本文件与其他旧说明存在冲突，以本文件和当前代码实现为准。

## 1. Benchmark 体系的目标

`badbatch` 的 benchmark 体系主要解决四类问题：

- 验证当前 revision 在当前机器上是否存在明显性能退化
- 观察不同拓扑下的 steady-state 行为
- 为热路径、wait strategy、pipeline、buffer 选择提供工程反馈
- 为与 `examples/disruptor` 中原生 LMAX Disruptor 做有限可比的对照提供基础

它适合：

- 同机、同环境、同 revision family 的相对比较
- 开发期性能回归
- 设计方向判断

它不等于：

- 发布级跨机器绝对性能证明
- 自动完成 CPU pinning / core isolation 的严谨实验平台
- 与所有外部实现完全一一口径对齐的统一标准

## 2. 组成

benchmark 体系由三部分组成：

1. `benches/*.rs`
   真正的 Criterion benchmark 和自定义延迟统计逻辑。
2. `scripts/run_benchmarks.sh`
   负责按 suite 运行、设置 timeout、写日志、输出摘要。
3. `scripts/result_formatter.sh`
   负责解析 Criterion 日志、自定义 latency 输出，并生成 summary。

相关目录：

- benchmark 代码：`benches/`
- benchmark 日志：`benchmark_logs/`
- Criterion HTML 报告：`target/criterion/`
- 原生 LMAX 参考实现：`examples/disruptor/`

## 3. 如何运行

### 3.1 推荐命令

```bash
# 快速 sanity / CI 级检查
./scripts/run_benchmarks.sh quick

# 最小化短跑，适合调试 benchmark harness
./scripts/run_benchmarks.sh minimal

# 单套件
./scripts/run_benchmarks.sh spsc
./scripts/run_benchmarks.sh mpsc
./scripts/run_benchmarks.sh pipeline
./scripts/run_benchmarks.sh latency
./scripts/run_benchmarks.sh throughput
./scripts/run_benchmarks.sh scaling

# 全量
./scripts/run_benchmarks.sh all
```

### 3.2 直接调用 Criterion

```bash
cargo bench --bench comprehensive_benchmarks
cargo bench --bench single_producer_single_consumer
cargo bench --bench multi_producer_single_consumer
cargo bench --bench pipeline_processing
cargo bench --bench latency_comparison
cargo bench --bench throughput_comparison
cargo bench --bench buffer_size_scaling
```

### 3.3 其他常用命令

```bash
# 只检查能否编译
./scripts/run_benchmarks.sh compile

# 轻量回归
./scripts/run_benchmarks.sh regression

# 生成 / 刷新 HTML 报告
./scripts/run_benchmarks.sh report
```

注意：

- `minimal` 本质上是 `comprehensive_benchmarks` 的短时间配置版
- `regression` 当前不是“和历史基线自动做 diff”，而是重新跑一次 `comprehensive_benchmarks`
- `report` 当前只会重跑 `comprehensive_benchmarks` 来生成或刷新 HTML 报告，不会自动把所有 suite 全量再跑一遍

## 4. Runner 的真实行为

`scripts/run_benchmarks.sh` 当前行为如下：

- 每个 suite 单独执行一次 `cargo bench --bench <suite>`
- suite 的完整 stdout/stderr 会被写入 `benchmark_logs/<suite>.log`
- `all` 模式下，一个 suite 失败不会阻止后续 suite 继续跑
- `all` 当前执行顺序是：
  - `quick`
  - `spsc`
  - `mpsc`
  - `pipeline`
  - `latency`
  - `throughput`
  - `scaling`
- 每个 suite 结束后，会立即调用 formatter 打印一份简要解读
- 全部结束后，再打印一份统一 summary

timeout 行为：

- 默认 `TIMEOUT_SECONDS=600`
- `single_producer_single_consumer` 使用 `SPSC_TIMEOUT=900`
- `buffer_size_scaling` 使用 `SCALING_TIMEOUT=900`
- 如果系统有 GNU `timeout` 或 macOS `gtimeout`，runner 会启用外层 timeout
- 如果系统没有 timeout 命令，runner 仍可工作，但失去外层超时保护

`minimal` 的实际命令等价于：

```bash
cargo bench --bench comprehensive_benchmarks -- --sample-size 10 --warm-up-time 1 --measurement-time 1
```

## 5. Formatter 的真实行为

`scripts/result_formatter.sh` 做四件事：

- 从 Criterion 的 `time: [...]` 和 `thrpt: [...]` 中提取中位估计值
- 从 `latency_comparison.rs` 自定义打印的延迟统计中提取 `mean / median / p95 / p99 / max`
- 为每个 suite 选择一个主结果用于 summary
- 统计日志中的 `WARNING:` 行数

### 5.1 Peak Case 的选择规则

对普通吞吐型 suite：

- 默认忽略 `baseline`
- 如果同一 suite 同时存在 `pause:0ms` 和非 `pause:0ms` case，优先只在 `pause:0ms` 中选择
- 在候选集合中按 throughput 最大值选择 `Peak Case`
- 如果 suite 没有 throughput，则退回第一个非 baseline 结果

对 `latency_comparison`：

- 不用 Criterion throughput 选主结果
- 直接选择自定义 latency stats 中 `mean` 最低的实现

### 5.2 Samples / Iterations

当前 formatter 会从被选中的 `Peak Case` 对应的 `Collecting ... iterations` 行提取：

- `Samples`
- `Iterations`

它不再错误地绑定到第一条 baseline。

### 5.3 Highest Reported Throughput

summary 顶部的 `Highest Reported Throughput`：

- 只在吞吐型 suite 内比较
- 当前会跳过 `comprehensive_benchmarks` 和 `latency_comparison`
- 它只是 convenience headline，不是“项目最终性能总分”

### 5.4 WARNING 的含义

`WARNING:` 表示 benchmark 自己报告了异常提示，例如：

- timeout
- shutdown 异常
- suite 内部的安全降级提示

只要某个 suite 的 log 中出现 `WARNING:`，就不应把该次结果当成干净的性能结论，即使 `cargo bench` 本身没有退出失败。

## 6. Benchmark 设计原则

当前 benchmark 代码遵循这些原则：

- 尽量把 Disruptor、handler、工作线程等放在 timed section 外
- Criterion 热路径优先使用 `iter_custom`
- 异步完成判定统一使用“单调递增计数 + 本轮 target”
- 避免“每轮清零计数再等固定值”这类会污染 iteration 边界的写法
- `BusySpin` / `Yielding` 的等待器分别使用 spin / yield
- `Blocking` / `Sleeping` 相关 benchmark 使用 cooperative waiter，避免 harness 自己引入 `sleep(1ms)` 级假慢
- 需要时 suite 内部自己带 timeout，避免永久挂起
- `quick` 更偏 smoke / sanity，不作为最终主 KPI

“单调递增计数 + 当前 target” 是这套 benchmark 正确性的关键基础。它确保每个 iteration 等到的是“这一轮自己发布的工作”，而不是上一次 iteration 的残留完成事件。

## 7. 各 Suite 的测量对象和口径

### 7.1 `comprehensive_benchmarks.rs`

用途：

- 快速 sanity 检查
- CI / 开发期轻量回归

内容：

- `Safe_SPSC`
  - 单生产者单消费者
  - `BusySpinWaitStrategy`
  - burst `100` / `1000`
- `Safe_Throughput`
  - buffer `256` / `1024`
  - 每轮 `100` 个事件
- `Safe_Latency`
  - 单事件 publish -> consume 完成时间
- `Channel_Baseline`
  - `std::sync::mpsc`
  - 每个 iteration 重新创建 channel 和线程

Criterion 配置：

- 全局：`measurement_time=5s`
- 全局：`warm_up_time=2s`
- 全局：`sample_size=15`
- 个别 group 还会自己缩短 measurement time

解读方式：

- 这是 smoke/perf sanity，不是正式 KPI
- `Channel_Baseline` 带线程创建成本，不能和 steady-state Disruptor 结果直接横比
- 适合回答“这次提交有没有把基本路径打坏”

### 7.2 `single_producer_single_consumer.rs`

用途：

- SPSC 主路径吞吐
- wait strategy 差异
- 单事件 API 路径和 batch publication 路径差异

当前矩阵：

- buffer：`1024`
- burst：`100` / `1000`
- pause：当前只测 `0ms`
- case：
  - `BusySpin`
  - `Yielding`
  - `Blocking`
  - `Sleeping`
  - `BatchBusySpin`
  - `BatchYielding`
  - `baseline`

Criterion 配置：

- `measurement_time=10s`
- `warm_up_time=3s`
- `sample_size=20`

关键口径：

- `BusySpin` / `Yielding` / `Blocking` / `Sleeping`
  - 用 `publish_event`
  - 更接近常见 API 使用路径
- `BatchBusySpin` / `BatchYielding`
  - 用 `batch_publish`
  - 更接近 LMAX throughput tests 和 `disruptor-rs` 的高吞吐 publication 模型

解读方式：

- 看 SPSC 主路径极限，优先关注 `BatchBusySpin` / `BatchYielding`
- 看更接近业务常见单事件发布路径，优先关注非 batch case
- `Blocking` / `Sleeping` 现在不再被 benchmark harness 的 1ms 轮询污染，但仍然会体现它们本身的策略代价

### 7.3 `multi_producer_single_consumer.rs`

用途：

- 真正的 MPSC 吞吐
- 观察多生产者争用下单事件发布和批量发布的差异

当前矩阵：

- `PRODUCER_COUNT = 3`
- buffer：`1024`
- burst：`10` / `100` / `500`
- pause：`0ms` / `1ms`
- 对于 `burst=500`，当前只跑 `pause=0ms`

Criterion 配置：

- `measurement_time=15s`
- `warm_up_time=5s`
- `sample_size=10`

实现特征：

- producer 是持久线程，不在每个 iteration 中重复 spawn
- main thread 用 generation counter 触发每一轮 burst
- `MPSC_SingleEvent_BusySpin`
  - 每个 producer 用 `try_publish_event`
- `MPSC_Batch_BusySpin`
  - 每个 producer 一次 `batch_publish` 一个 burst

解读方式：

- 这是仓库里真正的 MPSC benchmark
- 它比 `throughput_comparison.rs` 里的 `Batch_MS_*` 更能代表真实多 producer 并发争用
- `batch` 远高于 `single-event` 是预期现象，不是异常
- formatter 会优先选择 `pause:0ms` case 作为 suite headline

### 7.4 `pipeline_processing.rs`

用途：

- 测试有依赖关系的多 stage pipeline

当前矩阵：

- buffer：`2048`
- burst：`50` / `200` / `1000`
- stage 拓扑：
  - `TwoStage`
  - `ThreeStage`
  - `FourStage`
- wait strategy：
  - `BusySpin`
  - `Yielding`

Criterion 配置：

- `measurement_time=20s`
- `warm_up_time=5s`
- `sample_size` 未显式设置，使用 Criterion 默认值

实现特征：

- stage 内逻辑是固定算术变换，不包含外部 I/O
- 用最后一个 stage 的计数作为 end-to-end 完成条件

解读方式：

- 它回答的是“依赖流水线在当前拓扑和 wait strategy 下大致能跑到什么级别”
- stage 数增加导致吞吐下降是正常现象
- 在 macOS 和异构核环境下，`BusySpin` 不一定每次都赢 `Yielding`

### 7.5 `latency_comparison.rs`

用途：

- 比较一跳单向延迟：`publish -> on_event`
- 对照 `std::sync::mpsc::sync_channel` 和 `crossbeam::channel::bounded`

当前矩阵：

- buffer：`1024`
- 每个 Criterion iteration 发送 `500` 个事件
- 参与对比：
  - `Disruptor/BusySpin`
  - `StdMpsc/sync_channel`
  - `Crossbeam/bounded`

Criterion 配置：

- `measurement_time=10s`
- `warm_up_time=3s`
- `sample_size=20`

关键口径：

- Criterion 自己的 `time / thrpt` 统计的是“500 个事件整批完成”的 wall time
- 真正应该看的 latency headline 是 suite 自己打印的：
  - `mean`
  - `median`
  - `p95`
  - `p99`
  - `max`

解读方式：

- 判断一跳延迟时，优先看 `Latency Statistics`
- 不要把 `Criterion Batch Metrics` 当成单事件 latency
- 这组测试是单向 latency，不是 round-trip ping-pong latency

### 7.6 `throughput_comparison.rs`

用途：

- 看 steady-state 吞吐上限
- 比较不同 buffer size、wait strategy、publication 模型，以及和 channel 的对照

当前矩阵：

- 每个 Criterion iteration 固定发布 `10_000` 个事件
- buffer：`256` / `1024` / `4096`
- 单生产者：
  - `Batch_SP_BusySpin`
  - `Batch_SP_Yielding`
  - `Batch_SP_Blocking`，当前只测到 `1024` 及以下
- multi-producer sequencer：
  - `Batch_MS_BusySpin`
  - `Batch_MS_Yielding`
  - 当前只在 `1024` buffer 下测试
- `try_publish_buf1024`
- `std::sync::mpsc`
- `crossbeam`

Criterion 配置：

- `measurement_time=10s`
- `warm_up_time=3s`
- `sample_size` 未显式设置，使用 Criterion 默认值

关键口径：

- Disruptor 侧使用分块 `batch_publish`
- chunk size 为 `min(buffer_size, 256)`
- 它更偏 steady-state 吞吐，而不是模拟单事件 API 路径

非常重要的边界：

- `Batch_MS_*` 中的 `MS` 表示 multi-producer sequencer
- 但 publish 动作仍由单线程驱动
- 所以它不是“真实并发 MPSC”
- 真正的 MPSC 请看 `multi_producer_single_consumer.rs`

解读方式：

- 这组 suite 更适合回答“sequencer + consumer 主数据面有多强”
- 不适合把 `Batch_MS_*` 直接当成真实多 producer 并发吞吐结论

### 7.7 `buffer_size_scaling.rs`

用途：

- 看不同 buffer size、不同处理成本、不同 payload 模式下的行为

当前矩阵不是全覆盖矩阵，而是压缩后的代表性集合：

- `FastProcessing`
  - buffer：`64` / `128` / `256`
  - workload：`1000`
  - 每事件几乎无额外处理
- `MediumProcessing`
  - `buffer=256`
  - `workload=1000`
  - 每事件约 `1us` 人工 busy work
- `SlowProcessing`
  - `buffer=512`
  - `workload=1000`
  - 每事件约 `10us` 人工 busy work
- `MemoryUsage`
  - buffer：`64` / `256` / `1024`
  - 更像 payload / allocation 行为探针，不是严格内存 profiler
- `BufferUtil`
  - `buffer=256`
  - `burst=100`
  - 用 `try_publish` + 人工 backpressure 观察 buffer 利用模式

Criterion 配置：

- `measurement_time=10s`
- `warm_up_time=3s`
- `sample_size` 未显式设置，使用 Criterion 默认值

解读方式：

- 这组 suite 用来帮助判断 buffer 选择是否合理，不是单纯追求最大吞吐
- `MemoryUsage` 不是精确内存占用测量工具
- `BufferUtil` 更偏 backpressure 行为观察

## 8. 如何正确解读输出

### 8.1 单 suite 输出

每个 suite 跑完后，runner 会打印：

- `Performance Metrics (first 10 cases)`
  - 只展示前 10 个 case
  - 不是按性能排序
- `Statistical Analysis`
  - 从 Criterion 日志里提取 outlier 汇总
- `Performance Assessment`
  - 基于该 suite 的主结果生成的一段简短判断

对 `latency_comparison`：

- 会先打印 `Latency Statistics`
- 再把 Criterion 的 `time / thrpt` 作为 `for reference` 的 batch metric 展示

### 8.2 Summary 输出

summary 中每个 suite 会显示：

- `Peak Case`
- `Peak Throughput`
- `Time`
- `Samples`
- `Iterations`

其中：

- `Peak Case` 是当前 formatter 规则挑出的 suite 主结果
- `Samples` / `Iterations` 现在对应的是这个主结果本身，不是第一条 baseline
- `Latency` 会改为显示 `Lowest Mean Latency`

### 8.3 哪些值能当主 KPI

推荐优先级：

- 一跳延迟：看 `Latency` 的 `mean / p95 / p99`
- SPSC：看 `single_producer_single_consumer`
- 真 MPSC：看 `multi_producer_single_consumer`
- pipeline：看 `pipeline_processing`
- steady-state 吞吐上限：看 `throughput_comparison`

不建议直接当最终结论的 headline：

- `Quick Test Suite`
- `Highest Reported Throughput`
- `throughput_comparison` 中的 `Batch_MS_*` 作为“真实 MPSC”

## 9. 常见误读

### 9.1 `Quick` 很高，不代表主 KPI 一定更高

`comprehensive_benchmarks` 是 smoke/sanity 套件，矩阵轻、路径简单，不应盖过正式 suite。

### 9.2 `Latency/Disruptor/BusySpin time: ... thrpt: ...` 不是单事件延迟

那是 500 个事件整批完成的 batch wall time。真正的一跳延迟看 `Latency Statistics`。

### 9.3 `Throughput/Disruptor/Batch_MS_*` 不是“真 MPSC”

它测的是 multi-producer sequencer 在单线程驱动下的 steady-state 吞吐。真实多 producer 并发 benchmark 在 `multi_producer_single_consumer.rs`。

### 9.4 `Blocking` / `Sleeping` 不再被 harness 自己的 1ms 轮询污染

它们依然可能显著慢于 `BusySpin` / `Yielding`，但差距现在主要反映 wait strategy 自身，而不是 benchmark harness 的测量错误。

### 9.5 `WARNING:` 不可忽视

只要 log 中有 `WARNING:`，就不应把这次结果当成干净结论，即使 suite 没有退出失败。

## 10. 环境因素

当前 benchmark 尚未做 CPU pinning 或隔离核控制，所以结果天然会受这些因素影响：

- CPU 频率调度
- 后台负载
- 笔记本是否接电
- macOS 调度行为
- 异构核分配
- Rust 版本和 target 特性

工程上应该默认接受：

- `5%` 到 `15%` 的吞吐波动
- 延迟项更大的自然波动

如果要做发布级性能结论，建议转到目标：

- `x86-64 Linux`
- `ARM Linux server`

并配合：

- 固定 CPU 亲和性
- 控制后台负载
- 固定 Rust 版本
- 固定系统电源和 governor 策略

## 11. 推荐工作流

### 11.1 日常开发

```bash
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
./scripts/run_benchmarks.sh minimal
```

### 11.2 改了并发逻辑或热路径之后

```bash
./scripts/run_benchmarks.sh spsc
./scripts/run_benchmarks.sh mpsc
./scripts/run_benchmarks.sh throughput
```

### 11.3 改了 barrier / pipeline / wait strategy 之后

```bash
./scripts/run_benchmarks.sh pipeline
./scripts/run_benchmarks.sh latency
```

### 11.4 做较正式检查

```bash
./scripts/run_benchmarks.sh all
./scripts/run_benchmarks.sh report
```

检查顺序建议：

1. 所有 suite 是否成功
2. 所有 log 是否 `0 WARNING`
3. 先看 `Latency`、`SPSC`、`MPSC`、`Pipeline`
4. 再看 `Throughput` 和 `Buffer Scaling`
5. 最后再看 `Highest Reported Throughput`

## 12. 与原生 LMAX Disruptor 的可比性

### 12.1 基本原则

`badbatch` 和原生 LMAX Disruptor 不是同语言、同运行时、同 benchmark harness，因此不能把任意两行数字直接横比。

如果要比较，至少应先对齐：

- 相同拓扑
- 相同 producer / consumer 数
- 相同 publication 模型
- 相同口径：单事件还是 batch，单向 latency 还是 round-trip latency

### 12.2 最接近的可比项

优先使用 `examples/disruptor` 的 `perftest`，不要先拿 JMH 结果直接横比。

最接近的映射：

- `badbatch single_producer_single_consumer / BusySpin or Yielding`
  - 对应 LMAX `OneToOneSequencedThroughputTest`
- `badbatch single_producer_single_consumer / BatchBusySpin or BatchYielding`
  - 对应 LMAX `OneToOneSequencedBatchThroughputTest`
- `badbatch multi_producer_single_consumer / MPSC_Batch_BusySpin`
  - 对应 LMAX `ThreeToOneSequencedBatchThroughputTest`
- `badbatch pipeline_processing / ThreeStage_*`
  - 对应 LMAX `OneToThreePipelineSequencedThroughputTest`

不应直接横比的：

- `badbatch latency_comparison`
  - 单向 `publish -> on_event`
- LMAX `PingPongSequencedLatencyTest`
  - round-trip ping-pong latency

### 12.3 如何运行原生 LMAX 参考项

```bash
cd examples/disruptor
./gradlew perfJar

java -cp build/libs/disruptor-perf-*.jar com.lmax.disruptor.sequenced.OneToOneSequencedThroughputTest
java -cp build/libs/disruptor-perf-*.jar com.lmax.disruptor.sequenced.OneToOneSequencedBatchThroughputTest
java -cp build/libs/disruptor-perf-*.jar com.lmax.disruptor.sequenced.ThreeToOneSequencedBatchThroughputTest
java -cp build/libs/disruptor-perf-*.jar com.lmax.disruptor.sequenced.OneToThreePipelineSequencedThroughputTest
```

如果你还想看原生项目自己的微基准：

```bash
cd examples/disruptor
./gradlew jmh
```

或者：

```bash
cd examples/disruptor
./gradlew jmhJar
java -jar build/libs/disruptor-*-jmh.jar SingleProducerSingleConsumer
java -jar build/libs/disruptor-*-jmh.jar MultiProducerSingleConsumer
```

JMH 结果更适合看原生项目内部微基准，不建议直接和当前 `badbatch` 的端到端 suite 一一横比。

## 13. Review Checklist

把一次 benchmark 结果拿来做正式 review 时，至少检查：

- 所有 suite 是否成功
- 所有相关 log 是否没有 `WARNING:`
- 是否看对了 suite 的主 KPI
- 是否把 `Latency Statistics` 和 Criterion batch metrics 区分开
- 是否把 `Batch_MS_*` 和真实 MPSC 区分开
- 是否在同一机器、同一系统状态、同一 Rust 版本上做对比
- 是否把 `Quick` 只当 sanity，不当最终性能结论

如果以上条件成立，这套 benchmark 结果现在可以作为 `badbatch` 开发过程中的真实性能反馈手段使用。
