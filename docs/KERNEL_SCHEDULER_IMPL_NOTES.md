# 内核调度器底层实现说明（`skiff-sched`）

> 状态：P1 底层能力已实现，**未接 SQLite、未接插件、未接 Web**。
> 契约：`docs/KERNEL_SCHEDULER_CONTRACT.md`
> 代码：`crates/sched`（包名 `skiff-sched`，已在根 `Cargo.toml` members 登记）
> 依赖边界：只依赖 `chrono` / `serde` / `serde_json` / `thiserror` / `skiff-cron`；
> 不依赖 `skiff-store` / `skiff-docker` / `skiff-wire` / `skiff-web`，未引 tokio。

## 1. 契约关键结论

### 1.1 站得住的设计

- **任务级事实源**替代「manifest cron → 插件 tick」：调度单位从插件变成任务，才能表达超时、重试、并发、缓冲与来源级联。
- **采样与执行解耦**是这套模型的核心价值：高频采样只写内核内存，低频执行批量消费，插件不再持有计时器。
- **水位回执协议**（`persist / keep / discard / partial`）方向正确：未回执默认 `keep` 是保守且安全的；水位越界按协议失败、缓冲不动，可以避免「执行体报错但数据已被误删」。
- **三层内存配额**（单点 / 单任务 / 全局）配溢出策略与降级，能在内存失控前给出确定性行为，而不是静默扩张。
- **单飞 + 超时冷却 + 仅瞬时错误重试**：避免同一任务堆叠、避免超时后立即重入、避免业务错误被无意义重试。

### 1.2 落地时容易踩的坑

1. **1s tick 不能同步执行执行体**。契约写的是 1s 调度循环，但插件 RPC 可能跑几十秒。必须保证 `tick` 只出票、执行放到独立任务；否则所有任务会被最慢的执行体串行阻塞。本 crate 的「出票 / 结算」两段式就是为此设计。
2. **超时无法强杀执行体**。超时只释放单飞锁并进入冷却，晚到的结果必须丢弃。上层要以 `run_id` + `snapshot_id` 作为幂等键；本 crate 已拒绝同一票据重复结算，但「旧结果晚到」需要上层按 `run_id` 丢弃。
3. **`retain_across_ticks=false` 容易写错**。契约要求「回执后无论 verdict，未被取走的点清空」，清空边界必须是**快照时刻的 tail**，不能用当前 tail，否则会误删执行期间新采样的点。本项目按前者实现。
4. **全局 64 MiB / 128 MiB 与「降级」的表述有歧义**。§4.5 表格说全局超限「先丢最旧再拒收」，正文又说「全局超限时暂停 `source=plugin` 且 `overflow=reject` 的采样，直到回落到 90%」。实现把两者拆成软限（默认 64 MiB，触发降级）与硬限（默认 128 MiB，触发腾挪/拒收），需要产品侧确认这一解释。
5. **插件配额 16 MiB / 64 MiB 未区分口径**。实现把 16 MiB 当作默认运营配额（超出即拒收新采样），64 MiB 只作为配置校验上限。
6. **DST 与时区**。`chrono::Local` 对不存在的本地时刻返回 `None`（cron 跳过该次），对重复时刻取较早；跨时区部署必须显式设置 `TZ`，否则同一表达式在不同机器上触发时间不同。
7. **错过计算不能依赖 `last_run.at`**。任务失败、跳过、手动触发都会污染 `last_run`。实现用独立的 `next_boundary / next_due` 游标计算，时钟回拨时也不推进游标。
8. **历史与审计写库频率**。每次执行一条 `task_runs`、每条审计一条记录，全量同步写 SQLite 会成为高频任务的瓶颈；接线时应批量提交，并落实 30 天 / 500 条保留策略。
9. **运行态只在内存**。进程重启后 `running` 会回到 `idle`；接线后需要在启动时按 `scheduled_tasks.state` 做一次状态校正。
10. **缓冲不落盘的代价**。P1 接受重启丢缓冲，但必须每次启动对「声明了 buffer 的任务」记一次 `buffer.lost_on_restart`，避免数据静默消失。

## 2. crate 结构与公开 API

### 2.1 模块

| 文件 | 职责 |
| --- | --- |
| `src/model.rs` | 任务模型、字段级校验、生命期/运行态状态机、补丁与过滤 |
| `src/buffer.rs` | 单任务内存缓冲、三种溢出、四种回执、水位与计量、告警信号 |
| `src/schedule.rs` | cron / interval 求值、抖动、本地时区与 DST 处理 |
| `src/clock.rs` | `Clock` 抽象、`SystemClock`、`ManualClock` |
| `src/exec.rs` | 执行体 trait、票据、批次、回执、错误分类、运行记录 |
| `src/scheduler.rs` | 调度状态机、tick 出票、结算、配额与降级、审计 |
| `src/store.rs` | `TaskStore` 抽象 + 内存实现 |
| `src/audit.rs` | 审计动作/主体/结果/事件结构 |
| `src/error.rs` | `TaskError` |

### 2.2 模型

- `TaskId`：`[A-Za-z0-9._:-]{1,96}`。
- `TaskKind`：`Builtin { name }` / `Plugin { id, method }`；字符串形式 `builtin:<name>`、`plugin:<id>:<method>`；`plugin` 方法禁止 `scheduler`、`task.*`、`buffer.*`（自调防线）。
- `TaskSource`：`Site / Docker / Plugin / System`，字符串形式 `site:<x>`、`docker:<x>`、`plugin:<id>`、`system`。
- `Schedule`：`Cron { expr, tz, jitter_secs }` / `Interval { secs, align }`；interval 最小 5s，jitter 0–600s。
- `TaskSpec`：`enabled / payload(≤4 KiB) / timeout(1–600s，默认 30) / retry / concurrency / missed / buffer / created_by / created_at / updated_at`。
  - `retry.max ≤ 10`，表示**额外重试次数**；`missed=all` 仅允许内置任务且 `retry.max=0`。
- `Task`：`spec + state(active/blocked/disabled_by_system) + blocked_reason + last_run`。
- `TaskState`：`idle / queued / running / blocked / failed / disabled`，带合法迁移表。

### 2.3 缓冲

```rust
Buffer::new(cfg: BufferConfig, aggregate_rule: Option<AggregateRule>) -> Self
Buffer::push(&mut self, at: i64, data: Value, now: i64) -> PushReport
Buffer::snapshot(&mut self, max_batch_bytes: usize) -> Snapshot
Buffer::commit(&mut self, snapshot_id: u64, verdict: Option<Verdict>, now: i64)
    -> Result<CommitOutcome, BufferError>
Buffer::stat(&self) -> BufferStat
Buffer::estimate_bytes(data: &Value) -> usize   // 序列化字节 + 32 B/点
```

- 单点上限 64 KiB；单任务容量默认 4 MiB、上限 16 MiB；单次批次按 `max_batch_bytes`（默认 512 KiB）最旧优先截断。
- 溢出 `drop_oldest / aggregate / reject`；`aggregate` 无规则时降级为 `drop_oldest`。
- 高水位 80%（每 10 分钟最多一次）、`overflow`、`recovered`、`point_too_large` 以 `BufferSignal` 返回。
- `commit` 先校验 `snapshot_id` 与水位，再变更缓冲；校验失败保留 pending，缓冲一字节不动。

### 2.4 执行体与票据

```rust
pub trait Executor {
    fn sample(&mut self, ctx: &SampleContext) -> Result<Option<Value>, ExecError>;
    fn execute(&mut self, ctx: &RunContext, batch: &Batch) -> Result<ExecOutput, ExecError>;
}
```

- `RunContext` 带 `run_id / task_id / kind / source / trigger / attempt / started_at / snapshot_id / missed_count / payload / cancelled`。
- `Verdict`：`Persist { persisted_upto }`、`Keep`、`Discard`、`Partial { persisted_upto }`；`None` 等价 `Keep`。
- `ErrorClass`：`spawn / timeout / unavailable / invalid_params / executor_failed / protocol / cancelled`，前三种为瞬时错误。
- `RunRecord`：开始/结束/耗时/结果/错误类与详情/`request_id`/`consumed_upto`/`output_bytes`/`missed_count`。

### 2.5 调度器

```rust
Scheduler::in_memory(cfg, clock) -> Result<Scheduler, TaskError>
Scheduler::new(store, cfg, clock) -> Result<Scheduler<S>, TaskError>

register(spec, now, clock) -> Result<Task, TaskError>   // (kind, source) 幂等 upsert
update(id, patch, now, clock) -> Result<Task, TaskError>
set_enabled(id, enabled, now, clock) -> Result<Task, TaskError>
delete(id, now) -> Result<(), TaskError>
push(id, value, now) -> Result<PushReport, TaskError>
tick(clock, gate) -> TickReport { runs, samples, records, audits }
run_now(id, actor, now, gate) -> Result<RunTicket, TaskError>
finish(run_id, Result<ExecOutput, ExecError>, now) -> Result<FinishOutcome, TaskError>
timeout(run_id, now) -> Result<RunRecord, TaskError>
cancel(id) -> bool
apply_sample(ticket, Result<Option<Value>, ExecError>, now) -> Result<Option<PushReport>, TaskError>
```

只读视图：`list / get / history / audits / task_state / sample_failures / is_degraded / global_bytes / plugin_bytes / buffer_stat`。

`PluginGate` 抽象插件可用性（`Ready / Disabled / NotInstalled / ApiMismatch`），生产实现由接线层基于 Supervisor 提供。

## 3. 与契约的差异

| # | 契约 | 本轮实现 | 说明 |
| --- | --- | --- | --- |
| D1 | 内核内置 1s `tokio::time::interval` 循环 | 只提供 `tick` 原语，由上层驱动 | 不引 tokio；便于注入时间做演算测试；1s 节拍由接线层保证 |
| D2 | `Scheduler` trait（`&self` + `RunHandle`） | `Scheduler<S>` 结构体（`&mut self` + `RunTicket` 两段式） | 运行态与缓冲都在内存，必须可变；出票/结算避免同步阻塞 |
| D3 | `scheduled_tasks / task_runs / task_state_changes` 三张表 | `TaskStore` trait + `MemoryTaskStore`，未落 SQLite | 按任务边界要求，`crates/store/src/db.rs` 未改动；状态变化并入 `task.blocked` / `task.run` 审计 |
| D4 | `history(id, page)` 分页 | `history(id, limit)` 倒序 | 分页由 Web/仓储层在 SQL 上做 |
| D5 | `push` 返回 `Result<(), TaskError>` | 返回 `PushReport`（含 `outcome / signals / net_bytes`） | 让调用方知道拒收原因与丢点计量 |
| D6 | 插件按需 ensure / 健康等待 / 熔断 / `task_pinned` | `PluginGate` 只返回状态；ensure/health 在执行体适配层 | 复用 `Supervisor` 的职责不搬进纯逻辑 crate |
| D7 | 插件只能创建 `plugin:<self>:`、`source` 由内核注入、每插件 ≤20 条、推送限速 100 点/秒 | 未实现 | 属于 `scheduler` RPC 能力面与权限层；模型层仅拒绝自调方法名 |
| D8 | `last_error` 作为任务字段展示 | 落在 `RunRecord.error_class/error_detail/request_id`；`Task.last_run` 只保留结果摘要 | 避免任务行频繁写；失败详情走执行历史 |
| D9 | 全局「超限」的单一阈值 | 软限（默认 64 MiB）触发降级；硬限（默认 128 MiB）先腾挪再拒收 | 契约表格与正文对「降级/超限」表述不一致，按两者拆分 |
| D10 | 单插件 16 MiB / 64 MiB | 16 MiB 为拒收配额；64 MiB 仅配置校验上限 | 同上，契约未区分运营配额与配置上限 |
| D11 | `aggregate` 按 `payload.aggregate` 合并任意点 | 仅合并数值字段（`sum/last/max/min`），非数值窗口降级为丢最旧 | 避免对结构不同的点做隐式语义猜测 |
| D12 | 停机/崩溃恢复与状态校正 | 未实现 | 缓冲 P1 不落盘；重启后由接线层校正 `running` 状态 |
| D13 | `delete` 无时间参数 | `delete(id, now)` | 审计需要准确时间戳，补一个 `now` |

## 4. 为接 SQLite / Web 预留的 trait 边界

### 4.1 持久化：`TaskStore`

```rust
pub trait TaskStore {
    fn list_tasks(&self) -> Vec<Task>;
    fn get_task(&self, id: &str) -> Option<Task>;
    fn put_task(&mut self, task: Task) -> Result<(), TaskError>;
    fn delete_task(&mut self, id: &str) -> Result<Option<Task>, TaskError>;
    fn append_run(&mut self, run: RunRecord) -> Result<(), TaskError>;
    fn list_runs(&self, task_id: &str, limit: usize) -> Vec<RunRecord>;
    fn append_audit(&mut self, event: AuditEvent) -> Result<(), TaskError>;
    fn list_audits(&self, limit: usize) -> Vec<AuditEvent>;
}
```

映射建议：

| 结构 | 表 | 字段 |
| --- | --- | --- |
| `TaskSpec` | `scheduled_tasks` | `schedule_json / payload_json / retry_json / buffer_json` 直接序列化；`state / blocked_reason / last_run_json` 跟 `Task` 走 |
| `RunRecord` | `task_runs` | 一一对应；`consumed_upto` 建议加列 |
| `AuditEvent` | `audit` | `action = action_str()`，`actor = as_string()`，`target / outcome / detail` |

约束：`skiff-sched` **不反向依赖** `store`。SQLite 适配器建议放在 `crates/core`（已同时依赖 store 与 sched），或在 `crates/store` 中新增 `sched_store.rs` 并让 store 依赖 sched——前者不会把调度语义漏进存储层，推荐前者。

### 4.2 执行体适配：`Executor`

- 内置任务：在 `crates/core` 实现 `Executor`，直接调用 fs/backup 等能力。
- 插件任务：把 `execute` 适配为 `Supervisor::ensure + health(≤5s) + interop:call`；失败映射：
  - 拉起失败 → `ErrorClass::Spawn`、`ErrorPhase::Ensure`
  - 健康等待超时 → `Spawn` / `Health`
  - RPC 超时 → 由调度层 `timeout()` 记录 `Timeout`；RPC 返回错误 → `ExecutorFailed/InvalidParams`
  - 回执缺字段/越界 → `Protocol`
- 同步 trait 意味着长调用必须放到阻塞池（`tokio::task::spawn_blocking`）或独立执行线程；`tick` 循环只做出票与结算。

### 4.3 插件可用性：`PluginGate`

由 `crates/core` 基于 Supervisor 实现：已安装/已启用/manifest API 兼容 → `Ready`；否则返回对应 blocked 原因。本 crate 只在状态变化时写一次 `task.blocked` 审计。

### 4.4 Web 管理面

- 路由与模板不在本 crate；需要的只读数据已全部暴露：`list / get / history / audits / buffer_stat / task_state / global_bytes`。
- 写操作对应 `register/update/set_enabled/delete/run_now`；权限矩阵（viewer/operator/admin）与 CSRF 在 Web 层实现。
- `run_now` 用 `AuditActor` 区分管理员/插件/系统，审计由 scheduler 内部写 `task.trigger`。

## 5. 后续接线顺序

1. **表结构（schema v8）**：由当前负责 `crates/store/src/db.rs` 的会话落地 `scheduled_tasks` / `task_runs` 与索引；本 crate 不改。
2. **仓储适配器**：实现 `TaskStore`（建议放 `crates/core` 的适配模块），补齐启动加载、状态校正、`buffer.lost_on_restart`、保留清理。
3. **tick 装配**：在 `crates/core/src/server.rs` 起 1s 循环（与 30s 巡检分离）：`tick → 对每个 RunTicket spawn_blocking 执行 → finish/timeout → 批量写 run/audit`。
4. **执行体与插件门**：实现内置 `Executor`，再把插件 `sample/execute` 适配到 Supervisor + interop；接通 `PluginGate`。
5. **内置任务**：`site-backup`、`db-vacuum`（P1），`cert-renew` 等只注册禁用占位。
6. **Web/UI**：任务列表/详情/历史/手动触发/启停；viewer 只读、Admin + CSRF 写；最后移除 manifest `cron` 解析与 `run_cron`。
7. **插件迁移**：`cron` 降为 UI、`monitor`/`guard` 注册任务；此步必须在上面的内核链路验收通过后做。

## 6. 已知风险

- **执行体晚到**：超时后插件仍可能完成并返回；接线层必须以 `run_id` 丢弃晚到结果，不能只看任务 id。
- **单飞只在内存**：崩溃重启后 `running` 状态需要启动校正，否则任务会长期处于「运行中」。
- **内存估算偏乐观**：计量按 `serde_json` 字节 + 32 B/点，未计 `VecDeque` 容量与分配器碎片；全局 128 MiB 是近似值，建议接线时留 10% 余量。
- **审计写入放大**：每次执行至少一条 run + 一条 audit；高频任务需批量提交，避免 SQLite 写锁成为瓶颈。
- **降级只在本进程**：`is_degraded` 是进程内状态，不做跨进程/跨机协调（契约本身也是单机模型）。
- **`aggregate` 语义有限**：仅数值字段可合并；结构化采样点（如容器列表）不适用，超限会退化为丢最旧。
- **DST 依赖宿主时区**：`Clock::to_local/local_to_epoch` 基于 `chrono::Local`；容器部署必须显式 `TZ`，否则 cron 结果随环境漂移。
- **全局软/硬限解释待产品确认**（见 §3 D9/D10）；若产品要求「达到 64 MiB 即腾挪拒收」，只需调整 `SchedulerConfig` 的默认值与判定顺序。

## 7. 本轮验证

```text
cargo fmt --all                                              # 通过
cargo build -p skiff-sched --locked                          # 通过
cargo test  -p skiff-sched --locked                          # 45 passed; 0 failed
cargo clippy -p skiff-sched --all-targets --locked -- -D warnings   # 通过
```

测试分布：`buffer` 15、`model` 8、`schedule` 4、`scheduler` 16、`store` 1、`clock` 1。

覆盖点：

- 缓冲容量与三种溢出（丢最旧 / 聚合 / 拒绝）、聚合无规则降级、单点 >64 KiB、批次截断、跨 tick 保留开关、高水位/恢复。
- 四种回执与未回执默认 `keep`、`persist` 水位删除、`partial` 部分删除、`discard` 全清、水位越界协议失败且缓冲不动、同一快照禁止重复结算。
- 单飞跳过、`queue_one` 单槽位、超时 + 60s 冷却、错过 `skip/once/all`（含 `max_catch_up` 与 `missed_count`）。
- 手动触发与忙碌拒绝、并发任务互不阻塞、禁用任务不调度与重新启用。
- 仅瞬时错误重试与退避、`attempt` 递增。
- 全局软限降级（暂停插件 reject 采样）与回落恢复、全局硬限对 `drop_oldest` 任务先行腾挪。
- 注入时间下的 cron 触发、本地时区换算、DST 跳变日跳过不存在的本地时刻。
- 时钟回拨告警与 >600s 暂停/恢复、插件 gate blocked 与恢复。
- `Executor` 两段式接口从票据到回执的完整往返。
