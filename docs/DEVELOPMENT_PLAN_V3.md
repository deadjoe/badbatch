# BadBatch 开发计划 v3（最终冻结稿）

- **基线**：`f396fa2`
- **状态**：**已冻结**。经 @Codex 多轮审查，全部修正采纳；@Codex / @Grok / @Kimi 一致同意。
- **决策**：@bearbone 已就全部六项执行分支拍板，均已并入正文（见文末「决策记录」）。
- **依据**：第三方两层审查报告 + 四人两轮只读复核 + 完整仓库的 Vultr Linux 证据 + `docs/private/LINUX_VPS_PERFORMANCE_HANDOFF_20260720.md` 的作者自述结论。

> **贯穿全篇的两条纪律**
> 1. **倍率不跨实验相乘**——不同实验、不同时间窗的比值中位数不得相乘用于预告收益。
> 2. **证据不跨场景兑换**——多场景验收时各场景独立达标，不允许"两个场景大赢补一个场景回退"。

> **行号约定**：正文中所有 `file.rs:N` 形式的行号均**相对基线 `f396fa2`**（已逐条复核，在该基线上准确）。批次落地后代码会移动，这些引用作为**历史锚点保留、不随 HEAD 更新**——追着 HEAD 改只会在下一批再失效一次。已知因 A.2 / A.6 落地而不再对应的引用，在 A.6 段落末尾列明。

---

## 批次 0｜证据保全

### 0.1 结果与手册的外部备份 —— ⚠️ owner 报告已完成，**尚未经我方验证**

@bearbone 报告已将 `head_to_head_results/`（工作树约 42 MB）与 `docs/private/LINUX_VPS_PERFORMANCE_HANDOFF_20260720.md` 手工复制至仓库外目录。

> **状态诚实声明**：以上为 **owner reported complete**。我方**没有**外部副本路径，也**没有**执行任何校验。在补做下述验证之前，本项不得按"已审计完成"对待。
>
> **补验口径（完成门禁 = 逐文件密码学哈希清单比对）**：
>
> 文件数与总字节数、文件名 manifest 都**只能作辅助 sanity check**——两个内容错误的文件可以保持相同的数量与总量，因此**不足以证明内容完整**。
>
> 必须执行：对源端全部 **3111** 个文件生成**排序后的 SHA-256 清单**，对外部副本生成同样的清单，**逐行比对一致**。`docs/private/LINUX_VPS_PERFORMANCE_HANDOFF_20260720.md` 单文件同样需哈希与字节数一致。
>
> 在取得外部副本路径并完成上述比对之前，本项状态保持 **owner reported / 未验证**，且**不得清理、移动或删除任何 ignored evidence**。

**决策：这两份材料不纳入公开仓库跟踪。** 理由：

1. 目录名 `head_to_head_results/linux_vultr_<ip>/` 含 VPS IP，纳入跟踪会把它写进公开仓库的文件树与全部历史，与 handoff §17"VPS identity、cost、IP、SSH label remain only in this ignored handoff"的既定决定相冲突（已核实 `environment.txt` 内部**无** IP / SSH / root@ 泄漏，问题仅在路径名）；
2. 内容含 1142 个 `.class` 编译产物、924 个 JSON、`perf.data` 二进制（1.2–2.1 MB/个）与 3.8–4.6 MB 的 c2c report。纳入跟踪会把**约 42 MB 原始 payload 永久写入历史**，显著增大对象库与完整 checkout 体积；实际 pack 大小取决于 zlib/delta 压缩效果，但**撤销仍需改写历史**。

### 0.2 游离实验提交的抢救 —— ✅ 已完成

问题：产生全部 Linux 证据的**源码**曾处于游离态。`git fsck --unreachable --no-reflogs` 列出三个提交，`git for-each-ref --contains` 均为 0，实验分支与工作树已不存在。

> **准确的风险表述**：它们当时无任何 ref 保护；默认 `git gc` 通常有 unreachable grace/expire 保护，但一旦达到过期阈值，或执行 `git gc --prune=now` 及等价 prune，即被永久删除。
>
> **丢失的究竟是什么**：现存的 JSON / CSV 仍可用于**重算统计量**；真正会永久失去的是**实验代码路径的重建、复现与审计能力**——无法再确认某个数字由哪段实现产生、无法在新主机上重跑同一实验臂、无法核查实现是否如报告所述。

处置：建立三个**附注 tag**（annotated，用途与禁令写入 tag 对象本身）并推送至 `origin`。

| Tag | 提交 | 内容 |
|---|---|---|
| `archive/claim-lock-bypass` | `5cef79a` | SP claim-lock bypass 实验臂。**UNSAFE — MUST NOT BE MERGED**：在并发 raw `Arc<SingleProducerSequencer>` claim driver 下不健全；保留仅为使 `docs/PERFORMANCE.md` 的测量可复现、可审计 |
| `archive/causal-matrix` | `3b8361e` | Linux causal matrix harness（lock × backoff、R/W1/W3/SB handler gradient） |
| `archive/pmu-harness` | `11508e7` | Linux PMU 与 `perf c2c` 采集 harness |

验证口径：本地 `for-each-ref --contains` 由 0 变为 1/2/1（`3b8361e` 为 2，因其同时被自身 tag 与后继 `11508e7` 的 tag 覆盖，与提交拓扑吻合）；`git fsck --unreachable` 对三者命中数为 0；`git ls-remote --tags` 显示三个 peeled ref 已在远端。

### 0.3 门禁边界

**这是 0.1 / 0.2 完成前的门禁。0.2 已完成；0.1 待补验（见上）。** 门禁一旦满足即解除，否则 F 批将永远无法启动。

**已解除**（因 0.2 完成、对象已被 tag 保护）：

- 常规 `git gc` / prune 不再构成同一风险——三个实验提交现由 `archive/*` tag 保护。

**仍然长期有效的保护**（与批次先后无关）：

- 在 0.1 的副本完整性**验证通过之前**，不得清理、移动或删除任何 ignored evidence（`head_to_head_results/`、`docs/private/`）；
- 任何时候**不得删除或改写** `archive/claim-lock-bypass` / `archive/causal-matrix` / `archive/pmu-harness` 三个 tag（本地与远端）；
- 对外发布性能声明时须标注对应 commit 与实验窗口。

**从不阻塞**：纯设计、代码编写与单元测试（批次 A–E 的实现工作）。

---

## 批次 A｜wait / poller 行为正确性

四项同源，作为**一个改动集**处理。根因：**零超时探测从未被当作一等场景设计**，被迫用 `wait_for_with_timeout(ZERO)` 冒充 try-read。

### A.1 修 `SleepingWaitStrategy` 超时路径顺序倒置

`src/disruptor/wait_strategy.rs:739-752` 的 `// Check timeout first` 在读 availability **之前**判 timeout；默认实现 `wait_strategy.rs:118-133` 的顺序为 alerted → available → timeout。

`EventPoller::poll` 传 `Duration::ZERO`（`event_poller.rs:100-117`），`elapsed() >= ZERO` 恒真 → 首轮即 `Timeout` → `event_poller.rs:116` 折叠为 `Polling::Idle`。**已发布事件永远读不到，且重复 poll 永远如此。**

> 此顺序修正**独立于 A.3 / A.4，必须单独完成**。

### A.2 修 `SimpleWaitStrategyAdapter` 陈旧 cursor 截断批量上界

`src/disruptor/simple_wait_strategy.rs:158-186`：首个循环把 `available = cursor.get()` 存入局部；等待 dependent sequences 期间**不刷新**；末尾返回 `min(available, dep_min)`。cursor 涨到 100 仍可能返回 1。

不丢事件，但把本可批量交付的前缀切成单条，直接伤害 pipeline batching。

### A.3 新增真正的 non-blocking availability 原语

- **必须保留连续前缀语义**：不得只取 `min(cursor, deps)`。multi-producer cursor 是 highest-claimed，必须调用等价于 `sequence_barrier.rs:209 resolve_highest_published` → `get_highest_published_sequence` 的 contiguous-prefix 扫描，**否则会把尚未 publish 的空洞暴露给 poller**。
- **必须保留 alert / shutdown 的前后检查。**
- **两个零超时调用点共用同一原语**：`event_poller.rs:110` 与 `event_processor.rs:387`。若决定 `try_run_once` 保留在 timeout 路径，须书面说明理由。
- wait strategy 此后只负责"等待"，不再承担 try-read 职责。

### A.4 零超时路径不再创建 `Instant`

A.3 使**两个 zero-timeout / try-read 调用不再创建 `Instant`**。

> **真正的 positive-timeout 路径保留计时基准**——`wait_strategy.rs:118` 的 `Instant::now()` 在正超时语义下是必需的，不予删除。

### A.5 新增测试

- **MPSC 乱序发布空洞不可见**（对应 A.3 的连续前缀要求）；
- dependent-sequence 批量上界（对应 A.2）；
- alert / shutdown 路径；
- **poller × 全部 wait strategy 一致性矩阵**：同一已发布事件，所有策略必须返回一致结果。

> 现状：八个实现两两之间的顺序一致性**没有任何测试守护**。

### A.6 统一两套 WaitStrategy 语义

`WaitStrategy`（LMAX 全接口）与 `SimpleWaitStrategy`（`simple_wait_strategy.rs:14`，自述 "inspired by disruptor-rs"）行为分叉，例如 `SimpleYielding::backoff` 与完整 `Yielding` 的 miss 处理不同。

> ⚠️ **正确性硬前置**：实施 A.6 前，必须先新增真正调用 timeout
> 路径、覆盖全部八个 wait strategy 实现的正 timeout 顺序矩阵，验证
> 已发布 sequence 的 availability 必须先于 timeout。A.3 之后 poller 直接走
> non-blocking availability 原语，现有 A.5 的 poller 矩阵不再调用 wait
> strategy，只能证明构造兼容，不能充当这项语义重构的安全网。
>
> ⚠️ **受文末并行规则约束**：统一动作若触及 `YieldingWaitStrategy` / `BusySpinWaitStrategy` / `wait_for_with_alert`，必须等基线落盘。

> **落地后订正**（A.6 已完成并推送，`19632e9..7b37c6b`）。本节开头点名的那处分叉是本计划对 A.6 的唯一具体指认，落地后发现它在两个方向上都需要修正：
>
> 1. **被点名的 `SimpleYielding::backoff` miss 分叉，不经 `WaitStrategy` 契约暴露。** backoff 只决定"怎么空转"，不影响返回值——两个 `Yielding` 在同样输入下的终局（`Ok` / `Err(Timeout)` / `Err(Alert)`）本来就一致。因此它**在原理上无法被任何行为测试或等价性测试钉住**，最终是靠 `Yielding::is_spin_phase` 纯谓词 + 锚定 `YieldingWaitStrategy::SPIN_TRIES` 常量固定的（自旋总数两边都恰好 100）。
> 2. **真正更硬、且可观测的那处分叉，本计划一字未提**：adapter 在数据已可用时**根本不检查 alert**——barrier 已 alert 时 full 返回 `Err(Alert)`、simple 返回 `Ok`。这是经契约可观察的行为差，也是 A.6 实际修掉的最重要一条。
>
> **已知未测窗口**：adapter 的第二次 alert 采样（读完可用性之后、判超时之前）只在 alert 恰好落入该窗口时可观测，单线程确定性测试钉不住，知情留空而非用时序依赖用例假装覆盖。
>
> **失效行号**：本节的 `simple_wait_strategy.rs:14` 与 A.2 节的 `simple_wait_strategy.rs:158-186` 在基线 `f396fa2` 上准确，A.2 / A.6 落地后已不再对应（trait 现位于 `:29`，所述旧 adapter 循环已被整段删除）。按前文「行号约定」保留不改。同节的 `wait_strategy.rs:118` / `:739` 仍然有效——该文件全程未被改动，正是下述「收敛方向」决策的效果。

---

## 批次 B｜SP claim 安全特化（性能主线）

与 handoff §15 Phase A/B 一致，非另起方案。

### B.1 单一 inner 实现

claim 记账重构为单一 inner 实现，checked 与 specialized 共用，**禁止复制算法**。

### B.2 公共 / raw 路径保留 checked lock

公共与 raw `Sequencer` 的 `next` / `try_next` 保留 `claim_lock`（`sequencer.rs:604/613` 的 `swap(Acquire)` + guard drop `store(Release)`）——它是 raw `Arc` 并发驱动 fail-closed 的唯一防线。

### B.3 不可伪造的 unique-producer capability —— 决策：方案 (a)

**背景**：现状有**两个安全的唯一 SP 构造面**——Builder 路径，与公开的 `open_single_producer_poller`（`event_poller.rs:289`，自建 fresh single sequencer 并返回唯一非 Clone producer）。而 `SimpleProducer::new`（`producer.rs:447`，`pub(crate)`，接收任意 `SequencerEnum`）与 `core.rs:181 create_producer(&self)`（其注释自陈 "callers are responsible for respecting the single-producer exclusivity invariant"）**都不编码唯一所有权**。因此"crate-private 入口 + 注释"不足以证明调用资格。

**采用方案 (a)**：引入不可伪造的内部 unique-producer capability / token，**同时覆盖 Builder 与 `open_single_producer_poller` 两个安全唯一构造面**；raw sequencer 与 DSL 永远走 checked 路径。

> **选择理由**：方案 (b)（只给 Builder）会留下一处不一致——`open_single_producer_poller` 是公开 API、同样返回唯一非 Clone producer、同样满足独占性，却因"不是 Builder 造的"而拿不到快路径。那不是安全边界，而是实现历史的偶然。方案 (a) 让"能否走快路径"由**能否证明唯一所有权**决定，而非由**从哪个函数出来**决定——这正是批次 B 的立论：把不变量交给类型系统。

**硬性禁止**：

- 不得仅按 `SequencerEnum::Single` 分支自动绕锁；
- 不得把 `SimpleProducer::new` 的 `pub(crate)` 可见性当作独占性证明。

> `CloneableProducer` 仅 multi 模式（`handle.rs:310-318` 有 `compile_fail` doctest 证明 single 模式无此方法），不在本项范围。

### B.4 正确性门禁（先于性能验收）

新路径此前不存在，既有 Miri / loom 不会自动覆盖它。必须新增：

- **checked 与 specialized 的等价性测试**：`next` / `next_n` / `try_next` / `try_next_n` 在 **wrap / capacity / backpressure / poison** 四种条件下行为一致；
- **raw `Arc<SingleProducerSequencer>` 并发驱动仍 fail-closed**（返回 `ConcurrentClaimDriver`）；
- **specialized 路径的 Miri / soundness 回归**（新增用例，非复用旧用例）；
- **编译期 UI 门禁**：`SimpleProducer` 非 Clone、非 `Sync`、单 handle 下无法取得第二个 producer；
- **构造面回归**：证明**两个授权构造面**（Builder 与 `open_single_producer_poller`）均可正确取得 capability，且 raw sequencer、DSL、以及任何未授权的内部构造路径**无法触达 specialized 快路径**；
- 既有 nested publish / poison / shutdown / loom 测试继续通过。

> **性能测试不得替代以上任何一条。**

### 证据边界

> **路径约定**：以下 `<ip>` 为 VPS IP 占位符。真实目录名含 IP，而本文件位于**公开**仓库，故按 0.1 的既定决定以占位符书写；查阅时用本地实际目录名替换。全部路径相对仓库根，且位于 **ignored** 的 `head_to_head_results/`（不在 fresh clone 中，依赖外部副本）。

| 项 | 数值 | 出处（canonical 相对路径） |
|---|---|---|
| bypass / lock（unicast） | 2.6038x，p10–p90 1.9145–3.3421，bootstrap 95% CI 2.3793–3.0655，20/20，sign-test p≈1.91e-6 | `head_to_head_results/linux_vultr_<ip>/claim_lock_ab_887ef84_vs_5cef79a_20260720/REPORT.md` |
| bypass / lock（unicast_batch） | **1.4354x**，CI 1.3712–1.5768，20/20（batch size = 10，即每 10 事件仍付一次 RMW） | 同上 `REPORT.md`；原始样本见同目录 `SUMMARY.json` 与 `bypass_unicast_batch_fork*.json` |
| bypass / lock（pipeline） | 1.5880x，CI 1.4970–1.6929，20/20 | 同上 `REPORT.md` |
| Rust vs LMAX 基线（4 场景 × 20 对） | 0.6833 / 2.5917 / 0.8582 / 0.2094 | `head_to_head_results/linux_vultr_<ip>/linux_baremetal_baseline_887ef84_20260720/REPORT.md`；逐对样本见同目录 `fork_samples.csv` |
| producer-private padded atomic | 0.9634x（复现 checked）⇒ 瓶颈是 **RMW 本身**，非锁的缓存行 | `head_to_head_results/linux_vultr_<ip>/linux_causal_3b8361e_20260720/CONCLUSIONS.md` §1；明细见同目录 `lock_matrix/REPORT.md` |
| backoff 臂 | `bypass-spin1` 0.7596x、`bypass-spin4` 0.2218x、`locked-adaptive` 1.0159x（10/20） | 同上 `CONCLUSIONS.md` §1 / `lock_matrix/REPORT.md` |
| handler 写回梯度 | W1/R ≈ 0.84–0.94、W3/R ≈ 0.86–0.89、SB/R ≈ 1.01–1.09 | 同上 `CONCLUSIONS.md` §2；明细见 `linux_causal_3b8361e_20260720/handler_gradient/REPORT.md` |
| PMU | locked-r 294.8 cycles/event、IPC 0.475；bypass-r 166.5、IPC 0.750；locked-w3 RFO-HITM 0.411447/event | 同上 `CONCLUSIONS.md` §3；明细见 `linux_causal_3b8361e_20260720/pmu_c2c/REPORT.md` 与该目录 `c2c_*.stats.txt` / `c2c_*.report.txt` |

> handoff 自述实验上界约 **1.7–2.0x**（1P/1C 臂），**"not a promised product gain"**。
> **不得**用 2.5917 × 1.4354 之类跨实验相乘预告"约 3.7x"。

---

## 批次 C｜活性与并发契约

### C.1 `into_producer` 回归测试

路径：`builder/handle.rs:101` → `builder/core.rs:86-111` `stop_consumers_keep_claims` drain gating 但**不 close 不 poison**；而 `halt()`（`core.rs:73-78`）先 `close()`，故只有 `into_producer` 这条路径裸奔。

可达性（实施期勘误）：`DisruptorHandle<_, _, MultiProducerMode>::create_producer(&self)`（`builder/handle.rs`）+ `SimpleProducer: Send` ⇒ **multi 模式的纯安全 API 即可触发**，非理论风险。single handle 没有 `create_producer`，且 `producer()` 的 `&mut` 借用不能与消费型 `into_producer(self)` 并存；因此 C.2 的 single specialized-inner 直测只证明内部活性不变量，不作为 single 安全 API 可达性证据。

需覆盖：`into_producer` + 背压 + gating removal 的组合。

### C.2 禁止把 ArcSwap guard 提出背压自旋循环

每轮 `gating_minimum()` 的 `gating_sequences.load()` 是**活性保障**：gating 被移除后返回空 vec，`get_minimum_sequence_with_default` 回退到 `next_value`，循环条件转假 → 干净退出。若把 guard 提出循环，旧 `Vec` 中已停 consumer 的 `Sequence` 被 `Arc` 续命且永不前进，而 `closed` / `poisoned` 均未置位 → **永久挂死**。

若要优化，**只允许** `arc_swap::Cache` 或周期 revalidate，且**先过活性证明 + C.1 回归**。

### C.3 WorkerPool 的 gating 水位写入改为可条件化

`consumer_engine.rs:342` 的 `consumer_sequence.set(current)` 在 CAS 重试中每次执行。**不得整行删除**：CAS 失败后 `current` 会重新 load 且可能变大，该 Release store 是在为新的 `current` 重建 gating 水位，是承重的；**仅 `current` 未变的那次重试才冗余**。

---

## 批次 D｜API 表面

> **决策：直接修改，不设 deprecation / 迁移期**（无外部使用者）。标准是"做对、做正确、做干净"。

### D.1 `handle_events_with` 命名

语义**已在** `README.md:101-106` 与 `docs/MODERNIZATION.md:62-65` 写明（同 stage 2+ mutable handler = WorkerPool 分区；广播用 `fan_out_events_with`），**不是文档缺陷**。问题是与 LMAX `handleEventsWith`（广播语义）同名异义，对迁移用户构成陷阱。

**不能机械统一重命名**——`handle_events_with` 当前承担**两个不同角色**：

| 位置 | 类型状态 | 角色 |
|---|---|---|
| `builder/fluent.rs:182`（及 `:464`） | `Empty` → `HasConsumers` | 在当前 stage 注册**第一个** mutable handler（该 stage 为 sequential） |
| `builder/fluent.rs:245` | `HasConsumers` → `Self` | 向当前 stage **追加** mutable handler，**该动作本身把 stage 转为 WorkerPool 分区** |

真正产生"分区"语义的是**第二个**，第一个是无歧义的。

**目标设计（@bearbone 已确认）**：仅重命名产生分区的那一个，使调用点自证语义。

- `handle_events_with(h)` — 保留，仅用于 stage 的**第一个** mutable handler（sequential）；
- `also_partition_with(h)` — **新名**，替代 `fluent.rs:245` 的同名重载；调用即表明"本 stage 转为 WorkerPool，每个 sequence 只由一个 handler 处理"；
- `also_partition_with_handler(h)` — 对应的 `EventHandler` 变体（替代 `fluent.rs:255` 的 `handle_events_with_handler` 追加重载）；
- `fan_out_events_with(h)` — 保留，read-only 广播；首个 handler 的 `handle_events_with_handler`（`fluent.rs:199`）保留。

**语义验收（四种组合各一测试）**：

1. single sequential handler — 每个 sequence 被唯一 handler 处理，顺序推进；
2. 2+ mutable handler（work-pool partition）— 每个 sequence **恰好**被一个 handler 处理，全体 handler 合计覆盖全部 sequence 且无重复；
3. `fan_out_events_with` broadcast — **每个** handler 观察到**全部** sequence；
4. `and_then` pipeline stage — 后继 stage 在前驱 stage 全部 handler 完成后才推进。

同步更新 `README.md:101-106` 与 `docs/MODERNIZATION.md:62-65`。

### D.2 放宽无必要的 trait bound

- `EventHandler<T>: Send + Sync` 中的 `Sync` 对 `Mutex<H>` / 单线程拥有的 handler 多属过强（`Mutex<H>` 只需 `H: Send`）；
- `T: Send + Sync + std::fmt::Debug`（`disruptor.rs` 等 10 处）**强制用户的事件类型实现 `Debug`**，而失败日志明确承诺不格式化 payload。

**验收 = compile-pass / API 回归**：实证"`Send` 非 `Sync` 的 handler"与"不实现 `Debug` 的事件类型"可用，并跑 **default / no-default / all-features** 三种 feature 组合。

### D.3 DSL 双锁 —— **风险约束，非开发项**

`disruptor.rs:86, 236-247` 的 `parking_lot::Mutex`（`single_publish_lock`）是**阻塞排队**——多线程共享 `&Disruptor` 并发 `publish_event` 时排队并全部成功；sequencer 的 `claim_lock` 是 **fail-fast**——并发 driver 得到 `ConcurrentClaimDriver` 错误。二者语义不同，**不是冗余**。

**本批次的处置：维持现状，本项列为约束而非开发任务。**

> "无外部使用者"只消除了**兼容负担**，并**没有替代语义选择**。改变它必须先在下列三种并发发布契约中选定一种，而这是产品决策、不属于本轮修正范围：
>
> | 方案 | 契约 | 代价 |
> |---|---|---|
> | 保留 DSL 阻塞排队（**当前**） | 并发 `publish_event` 排队且全部成功 | 多一把 mutex 在 DSL 热路径 |
> | 改为 fail-fast | 并发 publish 返回 `ConcurrentClaimDriver` | 现有"排队成功"用法变为随机报错 |
> | 重塑为独占 publishing surface | `&mut` 独占，编译期排除并发 | DSL 表面积与用法大改 |
>
> **约束**：在做出上述选择之前，**不得**以"减少一把锁"为由删除 `single_publish_lock`。
>
> TLS 重入标志（`disruptor.rs:89-104`）实测约 1 ns，**不是优化目标**；DSL 的真实成本是 mutex 与 translator 间接层。

---

## 批次 E｜文档与发布卫生

### E.1 `docs/DESIGN.md:567` 的 P99.9 是不实陈述

该行称 Criterion 输出 "P50, P95, P99, **P99.9**"，而 `benches/latency_comparison.rs:116-142` 只计算 mean / median / p95 / p99 / max，代码中无 p99.9 计算。

**决策**：修改该行文档以匹配现有输出，本轮不实现 p99.9。

### E.2 MSRV 从未被验证 —— 采用 pinned `1.97` CI job

准确表述：`Cargo.toml` **有** `rust-version = "1.97"`；但 **CI / workflow 中没有任何 pinned 1.97 job 或 `1.97` 字样**——全部 job 使用 `dtolnay/rust-toolchain@stable`，`rust-toolchain.toml` 亦为 moving `stable`。因此声明的 MSRV 从未被持续验证；今日字面属实纯属巧合（当前 stable 恰为 1.97.1）。

**真实的失效机制**（注意不是"stable 升到 1.98 就自动变假"）：moving-stable CI 可能引入 **1.98-only 的语法或依赖**，使 1.97 上的构建回归，而 CI 因始终跑最新 stable 而**保持全绿**。即声明**可能在无人察觉的情况下变假**。

> 仅从 `CHANGELOG.md:9` 删除承诺而保留 `rust-version = "1.97"` **技术上不闭合**——项目仍公开声明固定最低版本，只是无人验证。若确要改为"只支持当前 stable"，必须**同时**修改 `rust-version`、发布文档与每次 stable 升级的处理策略。

**处置：新增 pinned `1.97` CI job。**

---

## 批次 F｜证据与实验

### F.1 Phase 0：`--round-diagnostics` 正式 Linux 归因 run

工具**已实现**（feature `bench-round-diagnostics`，commit `c8fd64d`），但只在 macOS 做过 wiring smoke，**正式 Linux 归因从未跑过**，仓库中无 `round_batch_diagnostics.csv` / `round_producer_backpressure.csv`。

> **任何 pipeline pacing / hysteresis 产品改动之前必须先跑。** 诊断吞吐是诊断量，**不得**替代 canonical baseline。

### F.2 safe 特化落地后的 paired A/B

同一受控 Linux 会话跑 checked-vs-safe **20 对交错序配对 A/B**（unicast / unicast_batch / pipeline）。

**ship / stop 判据（事前写定）**：

- **启用 safe 路径**：B.4 全部正确性门禁通过 **且** 三个场景**各自独立**计算 paired ratio 的 bootstrap 95% CI，**三者下界均 > 1.0**，且三场景均无回退；
- **中性或负收益**：保留 checked 为默认，记录结果，**不因"方向对"而合入**；
- 对外宣称胜过 LMAX 需在同一窗口另加 Java H2H。

> **不做跨场景聚合、不取平均、不相乘、不允许"两个场景大赢补一个场景回退"。**
> **不需要**仪式性重跑 `887ef84` 已完成矩阵：配对设计本身抗漂移，且 `887ef84..f396fa2` 对 claim 热路径无实质改动（诊断由 `cfg` 隔离、`FailureRecord` 只在错误路径）。

### F.3 macOS fork-level checked-vs-safe A/B —— 决策：纳入

handoff §12 记载早期 macOS 观察为**删锁反而吞吐塌陷**，与 Linux 方向相反，且明确标注 "unresolved"。日常开发在 M1 Max。

**在启用判据中的作用（因已正式纳入，必须写明）**：safe capability 是**跨平台**产品路径，因此 Mac 结果不能只作参考。

| 受控 Mac checked-vs-safe 结果 | 处置 |
|---|---|
| 无显著回退 | 按 F.2 判据全平台启用 safe 路径 |
| **显著回退** | **停止全平台启用**，先调查原因；在查清前，二选一：仅在已验证平台启用（显式平台策略），或全平台保留 checked 为默认 |

> 不得一边说"Mac 回退不代表改动错误"，一边照常全平台启用——那会让本项失去门禁作用。"不代表改动错误"只说明**不能据此否定 Linux 结果**，不等于**可以忽略它**。

> **两类混淆边界必须分开（不要写宽）**：
>
> - **JDK 17.0.19（Linux 那轮）vs JDK 26.0.1（11 轮 macOS）** 只混淆 **Rust-vs-Java 的跨平台 H2H 比较**；
> - **不混淆同平台内部的 Rust checked-vs-safe 比值**——该比较两臂使用同一 JDK（实际上根本不涉及 JVM），故 Mac 的 checked-vs-safe 结论不受此混淆影响；
> - 早期 Mac 观察与 Linux 的其他差异（不同 commit、不同协议、fork/affinity 规程不同）**仍然存在**，故那次历史观察本身不能当作 portable 定律——这正是需要重跑一次现代受控 Mac A/B 的理由。

### F.4 ring-slot 写回：先跑 padding × handler-write 交叉对照

`head_to_head_results/linux_vultr_<ip>/linux_causal_3b8361e_20260720/CONCLUSIONS.md` §2 断言机制是 ownership transfer（依据：SB 侧缓冲无该损失、locked-w3 的 RFO-HITM 约为 locked-r 的 617 倍）；但 `docs/private/LINUX_VPS_PERFORMANCE_HANDOFF_20260720.md` §1.5 指出同一梯度**可能包含相邻槽 false sharing**，且 padding 对照**符号未知、尚未跑**。事件 32 B、两槽共享 64 B。

> **该对照完成前，不得据此改动 pipeline 语义。** 采信 handoff 的谨慎版本。

### F.5 跨语言尾延迟仍是零证据

完整仓库的 3111 个结果文件中**无任何 latency / p99 产物**；Criterion 侧只到 p99。若要主张"无 GC 优势"，需要匹配的长时 H2H、至少 p99.9 / p99.99、避免 coordinated omission，并保留 JVM GC / safepoint / JIT 日志。

---

## 明确排除项（审查提出、经复核否决，不进计划）

| # | 排除项 | 理由 |
|---|---|---|
| 1 | 全局删除 `claim_lock` | `5cef79a` 自述 unsafe、must not ship；它是 raw/shared 面 fail-closed 的唯一防线 |
| 2 | 把 ArcSwap guard 提出背压循环 | 纯安全 API 可致永久挂死（见 C.2） |
| 3 | 直接删除 DSL `single_publish_lock` | 阻塞排队 vs fail-fast 语义不同，非无损（见 D.3） |
| 4 | 采纳 fixed / adaptive backoff | 因果矩阵已测：`bypass-spin1` 0.7596x、`bypass-spin4` 0.2218x、`locked-adaptive` 1.0159x（10/20），无一通过工程门禁 |
| 5 | 整行删除 `consumer_engine.rs:342` | 承重——CAS 失败后需为新 `current` 重建 gating 水位（见 C.3） |
| 6 | 以"P0 静默丢事件 / 文档语义倒挂"处理 WorkerPool | 不成立：README 与 MODERNIZATION 已明确写明；降为 D.1 命名问题 |
| 7 | 以 MSRV 为 P1 发布阻塞 | 事实错误：当前 stable 即 1.97.1，`cargo check --all-targets` 实测通过；降为 E.2 卫生问题 |
| 8 | 跨实验倍率相乘预告收益（如 2.5917 × 1.4354 ≈ 3.7x） | 方法学错误：不同实验、不同时间窗的比值中位数不可相乘 |
| 9 | 按 `SequencerEnum::Single` 分支自动绕锁，或以 `pub(crate)` 可见性充当独占性证明 | 二者均不构成唯一所有权证明（见 B.3） |

---

## 依赖与并行规则

- **批次 0**：**0.2 已闭合**（三个 archive tag 本地 + 远端）。**0.1 的副本完整性补验尚未完成**，它只阻塞两件事——**清理 ignored evidence**、以及 **F 批的结论性测量**；**不**阻塞 A–E 的任何工作。补验通过后，0 批无剩余阻塞。
- **批次 A、C、D、E 相互独立**，可并行推进。
- **批次 B 的代码工作**不依赖任何测量；其**验收**依赖 F.2。
- **F.1 必须先于任何 pipeline pacing 产品改动。**

### 并行放行规则：挂 diff，不挂批次编号

| 可与基线测量并行 | 须等基线落盘 |
|---|---|
| A.1 Sleeping 顺序、A.2 Adapter 截断、A.3 `EventPoller` 真 try 路径、A.4 零超时去 `Instant` | 任何触及 `BusySpinWaitStrategy` / `YieldingWaitStrategy` / `wait_for_with_alert` 的改动（含 A.6 的语义统一） |

**依据**：`src/bin/h2h_rust.rs:908/1019/1146` 只实例化 `BusySpinWaitStrategy` 与 `YieldingWaitStrategy`，且走阻塞的 `wait_for_with_alert`，不经 `EventPoller` 与 timeout 路径。因此前一列的改动不在 H2H 任何路径上，与基线测量并行不会污染归因；后一列一旦改动，基线会在测量窗口内漂移。

---

## 决策记录

| 项 | 决策 |
|---|---|
| 批次 0 备份方式 | **0.2 tag 归档：已完成并验证**（本地 refs + 远端 peeled refs + `fsck`）。**0.1 外部副本：owner reported，完整性待验证**——`head_to_head_results/` 与 `docs/private/` 维持 ignored、不纳入公开仓库 |
| F.3 macOS A/B | **纳入** |
| D 批变更策略 | **直接修改**，不设 deprecation / 迁移期（无外部使用者） |
| B.3 capability 覆盖范围 | **方案 (a)**：同时覆盖 Builder 与 `open_single_producer_poller` |
| D.1 命名方案 | **同意**：仅重命名 `fluent.rs:245/255` 的同 stage 追加重载为 `also_partition_with` / `also_partition_with_handler`；首个 `handle_events_with` / `handle_events_with_handler` 与 `fan_out_events_with` 均保留 |
| D.3 本轮处置 | **同意**：本轮**维持现有 DSL 阻塞排队语义不改**，`single_publish_lock` 不删；D.3 仅作风险约束、不列为开发项 |
| A.6 `SimpleWaitStrategy::backoff` 签名 | **方案 C**：新增带默认实现的 `backoff_with_miss(&self, &mut u32)`，旧 `backoff` 标 `#[deprecated]`，**不破坏公开 API**。`backoff` **保持必需方法**——给它也加默认会与 `backoff_with_miss` 的默认互相委托，使空 `impl` 块编译通过并在运行时爆栈，等于把编译期错误降级成运行时崩溃 |
| A.6 收敛方向 | **单向 simple → full**：**不得改动 full `BusySpinWaitStrategy` / `YieldingWaitStrategy` 的实现体**。依据：`src/bin/h2h_rust.rs:908/1019/1146` 实例化的正是这两个策略并走 `wait_for_with_alert`，而 F.3 已证明 inlining 与 code layout 不可分割（一个 `#[inline(never)]` 即令中位数 0.402→1.638）。抽共享 helper 属同一风险类，DRY 收益留待 F 批完成后作独立 diff 单独测量 |
| A.6 dependent-sequence 口径 | **非 must-fix**：full 在 deps 非空时只取 `min(deps)`，adapter 取 `min(cursor, min(deps))`；正常拓扑下 deps 不超过 cursor，二者等价，adapter 只是更保守。收敛时顺手对齐即可，**不为它单独设计抽象** |
