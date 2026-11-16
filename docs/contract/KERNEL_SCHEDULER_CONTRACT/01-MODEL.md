# 一页结论、目标、术语、任务模型与缓冲区（§0–5）

> 上级索引：[KERNEL_SCHEDULER_CONTRACT.md](../KERNEL_SCHEDULER_CONTRACT.md) ｜ 拆自 `docs/contract/KERNEL_SCHEDULER_CONTRACT.md` 原 §0–5（2026-11-16 整理，正文未改）。

---

> 状态：已落地（2026-11-06）；任务表迁移最终编号为 schema v10，当前全局 v11。文中 v8 为设计初稿编号，以 `docs/changes/2026-11-06.md` 与代码为准。
> 关联：`docs/contract/KERNEL_FS_CONTRACT.md`（任务落盘统一走 fs 原语）、`docs/plugin/PLUGIN_CATALOG.md`（cron/monitor/guard 分工）
> 原则：调度是内核原语，插件只注册任务与提供执行体；插件不再自带计时器。

## 0. 一页结论

1. 内核新增**唯一任务事实源** `scheduled_tasks`（schema v8）与 1s 调度循环；现有「manifest `cron` 触发插件 `tick`」收敛进该模型。
2. 任务按 `kind` 分为内置（`builtin:<name>`）与插件（`plugin:<plugin_id>:<method>`）；`source` 标明作用域（站点/容器/插件/系统）。
3. 核心能力是**内核内存缓冲区**：插件只在被调用时返回采样点，内核按 `sample_interval` 驱动采样、按 `schedule` 驱动执行；执行体拿到只读批次，返回 `persist / keep / discard / partial`。
4. 内存有三级上限（单点 / 单任务 / 全局），超限按策略丢最旧或拒绝写入，并产生一次审计 + 指标告警，不静默扩张。
5. 执行语义固定：最小间隔 5s、单飞（重入跳过并记录）、错过默认跳过（内置任务可配一次补跑）、超时 30s 默认、仅瞬时错误重试。
6. 插件在到点前按需拉起；插件不存在/被停用/API 不匹配时任务进入 `blocked` 并记录原因，不伪造执行。
7. `cron` 插件降级为纯管理 UI；`monitor` 注册采样任务；`guard` 以 docker events 为主、定时任务做周期对账。
8. P1 落 `site-backup`、`db-vacuum`；`cert-renew`、`metrics-rollup`、`metrics-prune` 只注册为禁用占位，明确标注「P2 未实现」。

## 1. 背景与目标

### 1.1 现状

| 事实 | 位置 | 影响 |
| --- | --- | --- |
| 内核每 30s 巡检一次，按 manifest 的 `cron` 表达式调用插件 `tick` | `crates/core/src/server.rs:111-202` | 分辨率 30s；调度单位是「插件」而不是「任务」，无法表达超时/重试/并发/缓冲 |
| `cron` 插件自己持有 `jobs.json`、自己判断表达式、自己调 `interop:call` | `plugins/cron/src/main.rs:1-12,189-253` | 计时器在插件里；任务事实源在插件私有文件；内核无法统一审计与保活 |
| `monitor` 靠 manifest `cron = ["*/5 * * * *"]` 被调 `tick` 后落盘 `samples.json` | `plugins/monitor/manifest.toml:13-14`、`plugins/monitor/src/main.rs:6,119-144` | 采样与落盘耦合在同一 tick；两次执行之间没有内核缓冲 |
| `guard` 每分钟 `tick` 全量列出容器 | `plugins/guard/src/main.rs:3,123` | 有 docker events 时仍做全量对账，缺少「事件为主、定时对账」的分层 |
| 审计表只有 `actor/ip/action/target/result/detail` | `crates/store/src/model.rs:28-38` | 没有 request id、耗时、重试次数，任务失败定位弱 |

### 1.2 目标

- 内核提供**任务注册 + 调度 + 缓冲 + 执行语义 + 历史审计**五件套，任何插件都能声明任务而不自带计时器。
- 常驻采样与低频执行解耦：高频采样进内存缓冲，低频执行批量消费。
- 内置任务（站点备份、数据库维护等）与插件任务使用同一模型、同一审计、同一权限面。
- 失败可定位、可重试、可手动触发、可观察。

### 1.3 非目标

- 不执行任意 shell；插件任务只能调用插件自己声明的方法。
- 不提供分布式调度、跨机协调、优先级抢占。
- P1 不做缓冲区落盘（进程重启即丢，见 §4.6）。
- 不实现 SSL 自动申请、应用商店、数据库创建等业务能力。

## 2. 术语

| 术语 | 含义 |
| --- | --- |
| 任务（task） | 一条调度配置 + 一个执行体 + 一组语义参数 |
| 执行体（executor） | `builtin:<name>` 的内核实现，或 `plugin:<id>:<method>` 的插件 RPC |
| 采样体（sampler） | 可选；内核按 `sample_interval` 调用的插件方法，返回值进入缓冲 |
| 批次（batch） | 一次执行从缓冲取出的只读数据切片 |
| 水位（watermark） | 缓冲点单调递增序号；执行体回执用它声明已消费到哪 |
| 触发（trigger） | `schedule` / `manual` / `catch_up` / `retry` |

## 3. 任务模型

```jsonc
{
  "id": "builtin:site-backup:example.com",   // 全局唯一，见 §3.1
  "kind": "builtin:site-backup",             // 或 plugin:com.skiff.monitor:sample
  "source": "site:example.com",              // site:<domain> | docker:<container> | plugin:<id> | system
  "schedule": { "type": "cron", "expr": "30 3 * * *", "tz": "local", "jitter_secs": 120 },
  // 或 { "type": "interval", "secs": 60, "align": "epoch" }
  "enabled": true,
  "payload": { "keep": 7 },                  // ≤ 4 KiB，执行体私有
  "timeout_secs": 300,
  "retry": { "max": 2, "backoff_secs": [30, 120], "on": ["timeout", "spawn", "unavailable"] },
  "concurrency": "skip",                     // skip | queue_one
  "missed": "skip",                          // skip | once | all
  "buffer": null,                            // 见 §4
  "created_by": "admin",
  "created_at": 1760000000,
  "updated_at": 1760000000,
  "state": "active",                         // active | blocked | disabled_by_system
  "last_run": { "at": 1760000000, "result": "ok", "duration_ms": 12000, "trigger": "schedule" }
}
```

### 3.1 `id` 与 `kind` 语法

- `id`：`[A-Za-z0-9._:-]{1,96}`，全局唯一；建议内置任务 `builtin:<name>:<scope>`，插件任务 `plugin:<plugin_id>:<local_key>`。
- `kind`：
  - `builtin:<name>`：内核实现，插件不可创建；清单见 §8。
  - `plugin:<plugin_id>:<method>`：`plugin_id` 必须等于执行体所在插件；`method` 为 `[A-Za-z0-9._-]{1,64}`。
- 插件只能创建/修改 `kind` 以 `plugin:<self_id>:` 开头的任务；禁止自调（`method` 不能是调度器自身入口）。
- 同一 `plugin_id + method + source` 只允许一条任务；重复注册按更新处理，避免插件重启堆任务。

### 3.2 `source` 与级联

| source | 语义 | 级联 |
| --- | --- | --- |
| `site:<domain>` | 绑定站点 | 删除站点时删除其任务与历史；有运行中的任务先取消 |
| `docker:<container>` | 绑定容器 | 容器删除后任务转 `blocked`，保留历史 |
| `plugin:<id>` | 插件自用 | 卸载插件（删除数据）时级联删除任务 |
| `system` | 内核自用 | 不随业务对象删除 |

> 推荐：`source` 由内核根据创建入口注入，调用方不可伪造；这与现有 `_interop.caller` 的防伪造口径一致（`plugins/cron/src/main.rs:88-98`）。

### 3.3 `schedule`

- `cron`：5 段（分 时 日 月 周），沿用 `crates/cron` 的 `expr_matches`；时区固定宿主本地，P1 不提供时区选择。
- `interval`：`secs ≥ 5`，`align=epoch` 表示按整秒对齐；低于 5s 拒绝创建（不给特例）。
- `jitter_secs`：0–600，用于站点备份等避免整点堆叠。
- 校验时机：创建、修改、启用、每次加载；非法表达式任务直接 `blocked` 并记录原因，不 panic、不刷审计。

## 4. 缓冲区模型（核心）

### 4.1 生命周期

```
采样（可选）        写缓冲              到点执行                回执
sample RPC  ──►  ring/append  ──►  只读批次（≤ max_batch） ──► persist/keep/discard/partial
  (内核驱动)       (内核内存)          (调用执行体)             (内核清理/保留)
```

- **谁采样**：内核按 `buffer.sample_interval_secs` 调用任务的 `sample_method`（若声明）；插件不持有计时器。
- **谁缓冲**：内核持有，按任务隔离；插件进程重启不丢缓冲（缓冲不在插件里）。
- **谁消费**：到 `schedule` 时内核取最旧的一批点，序列化后交给执行体。

### 4.2 配置

```jsonc
"buffer": {
  "sample_method": "sample",       // 可选；为空表示插件自行 push
  "sample_interval_secs": 30,      // ≥ 1；P1 全局最小 1s
  "capacity_bytes": 4194304,       // 默认 4 MiB，上限 16 MiB
  "overflow": "drop_oldest",       // drop_oldest | aggregate | reject
  "retain_across_ticks": true,     // true=未被消费的点保留；false=每次执行后清空
  "aggregate_window_secs": 60,     // overflow=aggregate 时按窗口聚合
  "max_batch_bytes": 524288        // 单次交给执行体的上限，默认 512 KiB
}
```

- 单点上限 64 KiB（序列化后）；超限拒收并记 `buffer.point_too_large`。
- `overflow=drop_oldest`：丢最旧点，累计 `dropped_points/dropped_bytes`。
- `overflow=aggregate`：同一窗口内按 `payload.aggregate`（`sum|last|max|min`）合成一个点；无聚合规则时降级为 `drop_oldest`。
- `overflow=reject`：新采样点直接拒收，返回 `buffer_full`；用于审计类不可丢数据。
- `retain_across_ticks=false`：执行体回执后无论 verdict，未被取走的点清空；用于「每次窗口独立」的指标。

### 4.3 执行体回执

执行体返回：

```jsonc
{ "verdict": "persist", "persisted_upto": 1024, "detail": "写入 24 点" }
// verdict: persist | keep | discard | partial
```

| verdict | 内核动作 | 典型场景 |
| --- | --- | --- |
| `persist` | 删除批次内 `seq ≤ persisted_upto` 的点 | 正常落盘 |
| `keep` | 批次点全部保留（快照副本丢弃） | 下游暂不可用，下次再试 |
| `discard` | 删除批次内全部点（到批次水位） | 数据已过期/无价值 |
| `partial` | 只删除 `seq ≤ persisted_upto`，其余保留 | 部分成功、逐批推进 |

约束：

- 回执必须带 `persisted_upto`（`persist/partial` 必填），且只能落在本批次范围内；越界按协议错误处理，任务记 `fail`，缓冲不动。
- 未返回 verdict 视为 `keep`（保守，不丢数据）。
- `persist` 的语义是「执行体已把数据写到最终位置或已确认接收」，内核不校验落盘结果；落盘由内置实现或插件通过 fs 通道完成（见 FS 契约）。

### 4.4 只读快照

- 批次以 JSON 数组交付，元素含 `seq`、`at`、`data`；调用期间内核持任务读锁，执行体不能修改内核缓冲。
- 超过 `max_batch_bytes` 时按最旧优先切批，剩余点视为隐式 `partial`，下一轮继续；执行体不需要感知分页，只需按批处理。
- 快照不包含其他任务的点；不同任务互不可见。

### 4.5 内存计量与上限

| 层级 | 默认 | 上限 | 超限行为 |
| --- | --- | --- | --- |
| 单点 | — | 64 KiB | 拒收，审计 `buffer.point_too_large` |
| 单任务 | 4 MiB | 16 MiB | 按 `overflow` 处理 |
| 单插件 | 16 MiB | 64 MiB | 新采样拒收，插件任务按 `reject` 降级 |
| 全局 | 64 MiB | 128 MiB | 先对 `drop_oldest` 任务丢最旧，再拒收新点；写一条 warn 审计 |

- 计量口径：`serde_json` 序列化字节 + 32 B/点管理开销；采样点写入时累加，消费时扣减。
- 告警：达到 80% 产生一次 `buffer.high_watermark` 审计（每任务每 10 分钟最多一次，带滞后）；达到 100% 产生 `buffer.overflow`；恢复后产生 `buffer.recovered`。
- 降级：全局超限时暂停 `source=plugin` 且 `overflow=reject` 的采样，直到回落到 90%；内置任务不暂停。
- 缓冲仅内存：内核重启/崩溃后清空并记 `buffer.lost_on_restart`（P1 接受；P2 可选落盘 WAL，见 §14）。

### 4.6 跨 tick 与跨重启

- `retain_across_ticks=true` 是默认：执行体没消费的点继续累积，直到容量上限。
- 跨重启不保留；P2 可选「spill 到 `data/scheduler/buffers/<task>.jsonl`」，按容量与频率开关，默认关闭（写放大与磁盘配额成本高）。

## 5. 执行语义

### 5.1 调度循环与最小间隔

- 内核新增 1s 调度循环（`tokio::time::interval(1s)`），与现有 30s 巡检分离；30s 巡检继续负责插件回收、会话清理、站点日志轮转。
- `interval` 最小 5s；`cron` 最小粒度 1 分钟；采样最小 1s。
- 不允许「低于阈值就特事特办」：所有高频任务都走同一循环与同一单飞/超时逻辑。

### 5.2 错过执行

- 计算 `missed = floor((now - last_scheduled) / period) - 1`（首次启用不补跑）。
- `missed=skip`（默认）：只按当前时刻执行一次，记录 `missed_count`。
- `missed=once`：补跑一次，随后对齐当前周期；内置 `site-backup` 推荐该值。
- `missed=all`：逐次补跑，上限 10 次，剩余计入 `missed_count`；仅内置、且 `retry` 关闭时允许。
- 时钟回拨：若 `now < last_run.at`，不执行、记 `clock_skew` 审计；超过 10 分钟回拨则暂停该任务至时钟恢复。

### 5.3 重入保护

- 默认 `concurrency=skip`：同一任务单飞；上一轮未结束时本轮记 `skipped_busy`，**不排队、不堆叠**。
- `concurrency=queue_one`：只保留一个待执行槽；已有待执行时新触发仍记 `skipped_busy`。
- 执行结束（成功/失败/超时）才释放单飞锁；超时后进入 `grace_secs=60` 冷却，冷却期内触发一律跳过，避免插件实际仍在跑时重入。
- **超时后的重入封锁**：超时结算释放单飞后，若插件执行体仍未返回（内核继续等待回执而不丢弃调用），该任务进入 `reentry_blocked` 状态；此后任何调度/手动触发都立即失败并返回错误码 `reentry_blocked`，直到**迟到回执到达**或**插件进程重启导致调用报错**才解除。冷却期与重入封锁是两道独立闸门：冷却按时间，封锁按回执。

### 5.4 失败语义

```jsonc
"last_error": {
  "class": "spawn|timeout|unavailable|reentry_blocked|invalid_params|executor_failed|protocol",
  "message": "…",
  "phase": "ensure|health|call|verdict",
  "plugin_id": "com.skiff.monitor",
  "request_id": "req-…",
  "attempt": 2
}
```

- 失败分类决定是否重试：`spawn/timeout/unavailable/reentry_blocked` 可重试；`invalid_params/protocol/executor_failed` 不重试（业务失败由执行体自担）。`reentry_blocked` 属瞬时：回执到达即恢复，重试需按 `retry.backoff_secs` 留出等待窗口，避免在封锁期空转。
- 重试按 `retry.backoff_secs` 序列执行，超过 `retry.max` 停止并记最终失败。
- 失败不自动停用任务（除插件缺失/停用，见 §6）；连续失败 5 次在列表显示「异常」徽章，不静默。

### 5.5 超时与取消

- 默认 `timeout_secs=30`（`skiff_sched::DEFAULT_TIMEOUT_SECS`），上限 600s。
- 超时后内核记 `timeout`、释放逻辑锁、进入冷却；插件 RPC 无法强杀，可能仍在后台完成，其返回值丢弃。**但内核不丢弃执行体**：继续等待该次调用返回，返回前同一任务按 §5.3 以 `reentry_blocked` 拒绝重入。
- 取消是协作式的：手动停用/删除任务时，内核标记取消位；执行体可查询 `_task.cancelled`，但不保证即时中断。
