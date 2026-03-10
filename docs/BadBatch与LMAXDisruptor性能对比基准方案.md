# BadBatch 与 LMAX Disruptor 性能对比基准方案

## 1. 目标

本方案用于在同一台机器上，对 `badbatch` 与 `examples/disruptor` 中的原生 LMAX Disruptor 做一套**统一口径**的 head-to-head 性能对比。

本方案解决的问题不是“比较两边各自已有 benchmark 的数字”，而是：

- 为两边定义同一份 workload 规范
- 使用同一套场景拓扑、参数和指标
- 在相同环境下分别运行
- 以统一 JSON 输出结果
- 最终比较的是**实现本身**，不是 `Criterion`、`JMH` 或原有 perftest 的统计方式

## 2. 非目标

本方案第一阶段不做这些事情：

- 不直接比较现有 `Criterion` 与 `JMH` / `perftest` 输出
- 不做完整的延迟对比矩阵
- 不把 `macOS arm64` 的结果当作最终生产环境结论
- 不在第一版里引入复杂的自动核绑定与高级调参逻辑

第一阶段聚焦于**吞吐对比**；延迟对比作为第二阶段单独建设。

## 3. 设计原则

### 3.1 统一 workload，而不是统一框架

Rust 侧和 Java 侧都使用各自生态里最自然的可执行程序形式：

- Rust：独立二进制
- Java：独立 `main` 类

不复用现有 `Criterion` / `JMH` 统计框架作为最终结果来源。

### 3.2 同一场景，同一参数

每个对比场景必须对齐以下参数：

- 拓扑：producer / consumer / pipeline stage 数量
- buffer size
- wait strategy
- publication 模型：单事件或 batch
- batch size
- 总事件数
- 事件 payload
- consumer 处理逻辑
- warmup rounds
- measured rounds

### 3.3 结果必须可校验

每一轮都必须输出：

- `events_expected`
- `events_processed`
- `checksum_expected`
- `checksum_observed`
- `checksum_valid`

这样能同时防止两类问题：

- consumer 漏消费
- 编译器 / JIT 把处理逻辑优化掉

### 3.4 输出必须统一

Rust 和 Java 都输出统一 JSON，驱动脚本只做收集、汇总和展示，不再做语义推断。

## 4. 第一阶段对比场景

第一阶段固定为 4 个吞吐场景。

| 场景 ID | 拓扑 | Buffer Size | Events Total | Batch Size | Wait Strategy | 主指标 |
| --- | --- | ---: | ---: | ---: | --- | --- |
| `unicast` | `1P -> 1C` | 65536 | 100,000,000 | 1 | `yielding` | `ops/sec` |
| `unicast_batch` | `1P -> 1C` | 65536 | 100,000,000 | 10 | `yielding` | `ops/sec` |
| `mpsc_batch` | `3P -> 1C` | 65536 | 60,000,000 | 10 | `busy-spin` | `ops/sec` |
| `pipeline` | `1P -> Stage1 -> Stage2 -> Stage3` | 8192 | 100,000,000 | 1 | `yielding` | `ops/sec` |

说明：

- `mpsc_batch` 的 `60,000,000` 是总事件数，默认按 `20,000,000 x 3 producers` 平分。
- `mpsc_batch` 的 `events_total` 必须能被 `producer_count` 整除。
- 第一阶段只做吞吐，不在这 4 个场景里混入 latency percentile。
- 如果后续做 ceiling matrix，再额外增加一轮全 `busy-spin` 的可选矩阵。

## 5. 事件模型与处理逻辑

### 5.1 `unicast` / `unicast_batch` / `mpsc_batch`

事件结构统一为一个主值字段：

- `value: long` / `i64`

consumer 处理逻辑统一为：

- 读取 `value`
- 使用 `wrapping_add` / Java `long` 语义累加到 checksum
- 增加 processed counter

### 5.2 `pipeline`

事件结构统一为：

- `value`
- `stage1_value`
- `stage2_value`
- `stage3_value`

三个 stage 的逻辑固定为：

- Stage1: `stage1_value = value + 1`
- Stage2: `stage2_value = stage1_value + 3`
- Stage3: `stage3_value = stage2_value + 7`

最终 checksum 由 Stage3 输出值累加得到。

这组变换满足：

- 足够简单，易于两侧一致实现
- 有真实的跨 stage 数据依赖
- checksum 可精确推导与验证

## 6. 测量协议

### 6.1 轮次

每个场景执行：

- warmup rounds: `3`
- measured rounds: `7`

warmup rounds 不进入最终统计。

### 6.2 单轮定义

单轮测量的边界是：

- **起点（单生产者场景）**：consumer 已启动，producer 已准备完成，紧接着开始发布第一个事件
- **起点（多生产者场景）**：consumer 已启动，所有 producer 已准备完成，以释放统一 start signal 的瞬间作为起点
- **终点**：最后一个 consumer 已确认处理完本轮全部事件

不包含：

- 进程启动时间
- 编译时间
- JAR 打包时间
- binary 启动前的外部脚本准备时间

### 6.3 主指标

吞吐主指标采用：

- `median_ops_per_sec`

同时输出：

- `mean_ops_per_sec`
- `min_ops_per_sec`
- `max_ops_per_sec`
- `stddev_ops_per_sec`
- `cv`

其中：

- `cv = stddev / mean`

`median` 作为 headline，避免均值被极端 round 污染。

## 7. JSON 输出规范

每个场景每个实现输出一份 JSON。

示例：

```json
{
  "impl_name": "badbatch",
  "scenario": "unicast_batch",
  "event_padding": "none",
  "measurement_kind": "throughput",
  "buffer_size": 65536,
  "event_size_bytes": 32,
  "wait_strategy": "yielding",
  "producer_count": 1,
  "consumer_count": 1,
  "pipeline_stages": 1,
  "batch_size": 10,
  "events_total": 100000000,
  "warmup_rounds": 3,
  "measured_rounds": 7,
  "run_order": "rust-then-java",
  "env": {
    "os": "macos",
    "arch": "aarch64",
    "runtime": "rust"
  },
  "runs": [
    {
      "round_index": 1,
      "phase": "warmup",
      "elapsed_ns": 412345678,
      "ops_per_sec": 242512345.12,
      "events_expected": 100000000,
      "events_processed": 100000000,
      "checksum_expected": 4999999950000000,
      "checksum_observed": 4999999950000000,
      "checksum_valid": true
    }
  ],
  "summary": {
    "checksum_valid_all": true,
    "median_ops_per_sec": 241234567.89,
    "mean_ops_per_sec": 240987654.32,
    "min_ops_per_sec": 238765432.10,
    "max_ops_per_sec": 243456789.01,
    "stddev_ops_per_sec": 1234567.89,
    "cv": 0.0051
  }
}
```

要求：

- `runs` 中必须包含 warmup 和 measured 两类 round
- 统计 summary 只能基于 `phase == measured`
- `checksum_valid_all == true` 才能认为该场景结果有效

## 8. 运行时参数

### 8.1 Rust harness CLI

Rust harness 支持以下参数：

- `--scenario <unicast|unicast_batch|mpsc_batch|pipeline>`
- `--wait-strategy <yielding|busy-spin>`
- `--event-padding <none|64>`
- `--producer-path <builder|direct>`
- `--buffer-size <N>`
- `--events-total <N>`
- `--batch-size <N>`
- `--warmup-rounds <N>`
- `--measured-rounds <N>`
- `--run-order <rust-then-java|java-then-rust|standalone>`
- `--output <PATH>`

### 8.2 Java harness CLI

Java harness 保持与 Rust 相同的参数集合与字段语义。

说明：

- `producer-path` 当前只在 Rust harness 中存在，用于拆分“API 路径成本”和“引擎本体成本”。
- `event-padding` 当前只在 Rust harness 中存在，用于验证 `unicast` / `unicast_batch` 场景下 ring slot 布局对跨核 cache line 争用的影响。
- `builder` 表示走当前 BadBatch 默认的 `build_single_producer(...).build()` 路径。
- `direct` 表示在 Rust 侧绕开 `DisruptorHandle` / `SimpleProducer` / builder 组装路径，直接使用 `SingleProducerSequencer + RingBuffer + ProcessingSequenceBarrier`。
- `direct` 当前仅支持 `unicast` 与 `unicast_batch`，不改变 Java 侧实现。
- `direct` 模式的目的不是替换第一阶段默认口径，而是用于定位 Rust 单生产者热路径里 API 层的额外成本。
- `event-padding=64` 当前仅支持 `unicast` 与 `unicast_batch`；其余场景固定为 `none`。

### 8.3 驱动脚本

统一驱动脚本负责：

- 编译 Rust harness
- 编译 Java harness
- 逐场景串行执行
- 保存 JSON 结果
- 生成汇总对比表

第一阶段脚本支持的重点参数：

- `--scenario <all|unicast|unicast_batch|mpsc_batch|pipeline>`
- `--mode <full|quick>`
- `--order <rust-first|java-first>`
- `--rust-producer-path <builder|direct>`
- `--rust-event-padding <none|64>`
- `--results-dir <PATH>`

说明：

- 第一阶段先实现 `rust-first` 与 `java-first` 两种顺序
- 不实现更复杂的 `alternate` 自动调度
- `quick` 仅用于链路验证，不用于正式结论
- `--rust-producer-path direct` 当前仅允许搭配 `--scenario unicast` 或 `--scenario unicast_batch`
- `--rust-event-padding 64` 只会传给 Rust 侧的 `unicast` / `unicast_batch`；`mpsc_batch` 与 `pipeline` 会自动回退为 `none`

## 9. 运行环境要求

### 9.1 当前阶段环境定位

当前优先支持：

- 同机 `macOS arm64`

该阶段结果用于：

- 判断两种实现的大致量级
- 判断相对表现方向
- 暴露明显异常

不用于：

- 形成最终生产性能结论

### 9.2 Java 运行参数

Java 默认运行参数：

```text
-Xms2g -Xmx2g -XX:+AlwaysPreTouch
```

理由：

- 固定堆大小，减少 heap 扩缩干扰
- `AlwaysPreTouch` 减少首次 page fault 干扰
- 让 GC 更少成为结果解释变量

### 9.3 Rust 编译方式

Rust 默认使用：

```text
cargo build --release
```

可选 ceiling 对比再考虑：

```text
RUSTFLAGS="-C target-cpu=native"
```

第一阶段默认不把 `target-cpu=native` 写死到协议中。

## 10. 实现落点

### 10.1 Rust

- 可执行程序：`src/bin/h2h_rust.rs`

### 10.2 Java

- 源码目录：`tools/head_to_head/java`
- 主类：`com.lmax.disruptor.headtohead.HeadToHead`
- 编译方式：由驱动脚本直接使用 `javac` 编译 `tools/head_to_head/java` 与 `examples/disruptor/src/main/java`

### 10.3 驱动脚本

- 脚本：`scripts/run_head_to_head.sh`
- 结果目录：`head_to_head_results/<timestamp>/`

## 11. 正确性约束

每个场景都必须满足：

1. `events_processed == events_expected`
2. `checksum_observed == checksum_expected`
3. `checksum_valid == true`
4. `checksum_valid_all == true`

只要任意一条不满足：

- 该 round 记为失败
- 该场景 summary 记为无效
- 驱动脚本最终报告必须显式标红

## 12. 结果解释规则

### 12.1 可以得出的结论

- 在同一台机器上，BadBatch 相对 LMAX 的吞吐量级
- 不同拓扑下，两者的相对优势和短板
- batch / pipeline / multi-producer 场景下的差异方向

### 12.2 不能直接得出的结论

- badbatch 在 Linux 生产机上的最终发布结论
- Java 与 Rust 在所有硬件上的绝对优劣
- 尾延迟表现

尾延迟必须依赖第二阶段单独的 latency harness。

## 13. 第二阶段预留：单向延迟对比

第二阶段单独建设：

- `1P -> 1C` 单向延迟场景
- 每个 event 带时间戳
- consumer 记录单向 `publish -> consume` latency
- 输出 `p50 / p95 / p99 / max`

第二阶段不复用第一阶段吞吐 harness 的统计逻辑，避免混口径。

## 14. 验收标准

第一阶段完成的验收标准如下：

1. Rust 和 Java 都能独立运行 4 个场景
2. 两边 CLI 参数与 JSON 字段一致
3. 驱动脚本能串行跑完两边并生成对比摘要
4. 每个场景都带 per-round correctness 校验
5. `quick` 模式能在短时间内完成链路验证
6. `full` 模式参数符合本方案默认规范

## 15. 第一阶段实施顺序

1. 先实现 4 个吞吐场景的 Rust harness
2. 再实现 Java harness
3. 再实现统一驱动脚本与结果汇总
4. 用 `quick` 模式验证链路和 JSON 结构
5. 最后再进入正式 `full` 运行

---

本方案用于替代“直接比较现有 benchmark 数字”的做法。  
后续所有 BadBatch 与 LMAX 的性能对比，都以本方案定义的 head-to-head harness 为准。
