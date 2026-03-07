# BadBatch Benchmark Usage

本文件面向“运行 benchmark 的测试者”。重点说明：

- 怎么跑
- 可以调哪些参数
- 日志在哪里
- 输出字段是什么意思
- 哪些结果可以直接看，哪些结果容易误读

如果你要了解各个 suite 的更完整设计口径和与 LMAX Disruptor 的可比性细节，请再看 [`benches/README.md`](../benches/README.md)。

## 1. 最常用命令

### 快速检查

```bash
./scripts/run_benchmarks.sh quick
./scripts/run_benchmarks.sh minimal
./scripts/run_benchmarks.sh compile
```

用途：

- `quick`
  - 跑轻量 sanity suite，适合开发期和 CI
- `minimal`
  - 跑更短的 quick 版本，适合调试 benchmark harness
- `compile`
  - 只检查 benchmark 是否能编译，不执行

### 单套件运行

```bash
./scripts/run_benchmarks.sh spsc
./scripts/run_benchmarks.sh mpsc
./scripts/run_benchmarks.sh pipeline
./scripts/run_benchmarks.sh latency
./scripts/run_benchmarks.sh throughput
./scripts/run_benchmarks.sh scaling
```

用途：

- `spsc`
  - 单生产者单消费者
- `mpsc`
  - 真正的多生产者单消费者
- `pipeline`
  - 多 stage 依赖流水线
- `latency`
  - 一跳单向延迟对比
- `throughput`
  - steady-state 吞吐上限和 channel 对照
- `scaling`
  - buffer size / workload / payload 行为

### 全量运行

```bash
./scripts/run_benchmarks.sh all
```

`all` 当前按这个顺序运行：

1. `quick`
2. `spsc`
3. `mpsc`
4. `pipeline`
5. `latency`
6. `throughput`
7. `scaling`

### 其他命令

```bash
./scripts/run_benchmarks.sh regression
./scripts/run_benchmarks.sh report
./scripts/run_benchmarks.sh optimize
```

注意：

- `regression` 当前只是重跑一次 `comprehensive_benchmarks`，不是自动历史 diff
- `report` 当前只会重跑 `comprehensive_benchmarks` 来生成 / 刷新 Criterion HTML 报告
- `optimize` 主要对 Linux 有用，macOS 只会给出有限提示

## 2. 可调参数

runner 支持这些环境变量：

```bash
TIMEOUT_SECONDS=1200 ./scripts/run_benchmarks.sh throughput
SPSC_TIMEOUT=1200 ./scripts/run_benchmarks.sh spsc
SCALING_TIMEOUT=1200 ./scripts/run_benchmarks.sh scaling
LOG_DIR=ci_benchmark_logs ./scripts/run_benchmarks.sh all
```

参数含义：

- `TIMEOUT_SECONDS`
  - 默认单 suite timeout，默认值 `600`
- `SPSC_TIMEOUT`
  - `single_producer_single_consumer` 的 timeout，默认值 `900`
- `SCALING_TIMEOUT`
  - `buffer_size_scaling` 的 timeout，默认值 `900`
- `LOG_DIR`
  - 日志目录，默认值 `benchmark_logs`

`minimal` 模式固定使用：

```bash
--sample-size 10 --warm-up-time 1 --measurement-time 1
```

其他 suite 的 sample size、warm-up、measurement time 由各自 benchmark 代码定义，测试者通常不需要手工覆盖，除非你明确是在调试 Criterion 本身。

## 3. 日志和产物

默认输出位置：

- suite 原始日志：`benchmark_logs/<suite>.log`
- compile-only 日志：`benchmark_logs/<suite>_compile.log`
- Criterion HTML 报告：`target/criterion/`

常见查看方式：

```bash
ls benchmark_logs/
cat benchmark_logs/single_producer_single_consumer.log
grep '^WARNING:' benchmark_logs/*.log
```

## 4. 运行是否成功，先看什么

一次 benchmark run 是否“可用”，建议按这个顺序检查：

1. suite 是否成功结束
2. 是否存在 `WARNING:`
3. summary 是否显示了合理的 `Peak Case`
4. 结果是否符合对应 suite 的测量口径

最重要的判断规则：

- 只要某个 suite 的 log 里出现 `WARNING:`，这一套结果就不应直接当成干净结论
- `all` 模式下一个 suite 失败，不会阻止其他 suite 继续跑，所以必须看最终失败列表

## 5. 输出字段解释

### 5.1 单 suite 输出

每个 suite 跑完后，runner 会打印一段 `Detailed Results`，包含：

- `Performance Metrics (first 10 cases)`
  - 只展示前 10 个 case
  - 不是按性能排序
- `Statistical Analysis`
  - 来自 Criterion 的 outlier 汇总
- `Performance Assessment`
  - formatter 根据主结果生成的简短说明

对于 `latency_comparison`，输出稍有不同：

- 先打印 `Latency Statistics`
- 再打印 `Criterion Batch Metrics (for reference)`

### 5.2 Summary 字段

summary 中常见字段含义如下：

- `Peak Case`
  - 当前 suite 的主结果
- `Peak Throughput`
  - 主结果的吞吐
- `Time`
  - 主结果的 Criterion 中位估计时间
- `Samples`
  - 主结果对应的 sample 数
- `Iterations`
  - 主结果对应的 Criterion iteration 数
- `Highest Reported Throughput`
  - 全局 headline，只是辅助提示，不是项目最终性能总分

### 5.3 Peak Case 的选择规则

formatter 选择 `Peak Case` 的规则是：

- 默认跳过 `baseline`
- 如果同时存在 `pause:0ms` 和非 `pause:0ms` case，优先在 `pause:0ms` 里选
- 在候选集合中按 throughput 最大值选主结果
- `latency` 例外：按 `mean latency` 最低来选

### 5.4 WARNING 的意义

`WARNING:` 表示 benchmark 自己报告的异常信号，例如：

- timeout
- shutdown 异常
- suite 内部的安全降级提示

结论规则很简单：

- `WARNING: 0`，这套结果才有资格进入性能分析
- `WARNING: > 0`，先看日志，不要直接拿数值做结论

## 6. 指标解释

### 6.1 Throughput 单位

当前吞吐单位包括：

- `Kelem/s`
- `Melem/s`
- `Gelem/s`

都表示“每秒处理的元素 / 事件数”。

### 6.2 Time 字段

`Time` 通常来自 Criterion 的中位估计值，表示该 benchmark case 在当前 measurement 模式下的中位时间。

注意：

- 它是 case 级时间，不一定等于“单事件端到端延迟”
- 尤其在 `latency` suite 中，不应把 batch time 误读成单事件 latency

### 6.3 Latency 指标

`latency_comparison` 真正应该看的指标是：

- `mean`
- `median`
- `p95`
- `p99`
- `max`

其中：

- `mean`
  - 适合看总体水平
- `p95` / `p99`
  - 适合看尾延迟
- `max`
  - 只适合作为极端值参考，不适合作为主 KPI

## 7. 哪些 suite 回答什么问题

### `quick`

适合：

- smoke / sanity
- CI
- 看“这次提交有没有把 benchmark 体系或基础性能明显打坏”

不适合：

- 当作正式性能主 KPI

### `spsc`

适合：

- 看 SPSC 主路径
- 比较 wait strategy
- 比较单事件发布和 batch 发布

### `mpsc`

适合：

- 看真正的多生产者并发吞吐

### `pipeline`

适合：

- 看多 stage 依赖流水线的 end-to-end 表现

### `latency`

适合：

- 看一跳单向延迟
- 对照 `std::sync::mpsc` 和 `crossbeam`

注意：

- 这里看的是单向 `publish -> on_event`
- 不是 ping-pong round-trip latency

### `throughput`

适合：

- 看 steady-state 吞吐上限
- 看 sequencer + consumer 主数据面是否健康

注意：

- `Batch_MS_*` 不是“真实并发 MPSC”
- 它只是 multi-producer sequencer 在单线程驱动下的吞吐

### `scaling`

适合：

- 看 buffer size、payload、处理成本变化后的行为

注意：

- `MemoryUsage` 更像 payload / allocation 行为探针
- 不是严格意义上的内存 profiler

## 8. 最容易误读的点

### 8.1 `Quick` 很高，不代表项目主 KPI 更高

`quick` 是轻量 sanity suite，不应盖过正式 suite。

### 8.2 `Latency/Disruptor/BusySpin time: ... thrpt: ...` 不是单事件 latency

那是整批事件的 batch wall time。真正的一跳延迟看 `Latency Statistics`。

### 8.3 `Batch_MS_*` 不是“真 MPSC”

真实多 producer 并发 benchmark 要看 `mpsc` suite。

### 8.4 `Highest Reported Throughput` 只是 headline

它可以帮助快速扫结果，但不能替代 suite-by-suite 分析。

## 9. 推荐使用方式

### 日常开发

```bash
./scripts/run_benchmarks.sh minimal
./scripts/run_benchmarks.sh quick
```

### 改了热路径后

```bash
./scripts/run_benchmarks.sh spsc
./scripts/run_benchmarks.sh throughput
```

### 改了并发协调逻辑后

```bash
./scripts/run_benchmarks.sh mpsc
./scripts/run_benchmarks.sh pipeline
./scripts/run_benchmarks.sh latency
```

### 准备做正式回归

```bash
./scripts/run_benchmarks.sh all
./scripts/run_benchmarks.sh report
```

## 10. 使用者视角的有效性标准

当你把一轮 benchmark 结果拿去做 review 时，最低要求是：

- 所有目标 suite 成功完成
- 对应 log 中没有 `WARNING:`
- 看的是正确 suite 的正确指标
- 没有把 `Latency Statistics` 和 batch throughput 混为一谈
- 没有把 `Batch_MS_*` 当成真 MPSC

如果以上条件成立，这套 benchmark 结果可以作为 `badbatch` 当前开发过程中的有效性能反馈依据。
