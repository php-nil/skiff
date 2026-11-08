# 内核定时任务契约（Scheduler Contract）

> 状态：已落地（2026-11-06）；任务表迁移最终编号为 schema v10，当前全局 v11。文中 v8 为设计初稿编号，以 `docs/changes/2026-11-06.md` 与代码为准。
> 关联：`docs/KERNEL_FS_CONTRACT.md`（任务落盘统一走 fs 原语）、`docs/PLUGIN_CATALOG.md`（cron/monitor/guard 分工）
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

### 5.4 失败语义

```jsonc
"last_error": {
  "class": "spawn|timeout|unavailable|invalid_params|executor_failed|protocol",
  "message": "…",
  "phase": "ensure|health|call|verdict",
  "plugin_id": "com.skiff.monitor",
  "request_id": "req-…",
  "attempt": 2
}
```

- 失败分类决定是否重试：`spawn/timeout/unavailable` 可重试；`invalid_params/protocol/executor_failed` 不重试（业务失败由执行体自担）。
- 重试按 `retry.backoff_secs` 序列执行，超过 `retry.max` 停止并记最终失败。
- 失败不自动停用任务（除插件缺失/停用，见 §6）；连续失败 5 次在列表显示「异常」徽章，不静默。

### 5.5 超时与取消

- 默认 `timeout_secs=30`（`skiff_sched::DEFAULT_TIMEOUT_SECS`），上限 600s。
- 超时后内核记 `timeout`、释放逻辑锁、进入冷却；插件 RPC 无法强杀，可能仍在后台完成，其返回值丢弃。
- 取消是协作式的：手动停用/删除任务时，内核标记取消位；执行体可查询 `_task.cancelled`，但不保证即时中断。

## 6. 插件任务唤醒链路

```
到点
 │
 ├─ 任务 enabled? ──否─► 跳过（不发审计，除非状态刚变化）
 ├─ 插件已安装? ──否─► state=blocked(reason=not_installed)，不重试
 ├─ 插件已启用? ──否─► state=blocked(reason=disabled)，恢复启用后自动回 active
 ├─ manifest API 与内核兼容? ──否─► state=blocked(reason=api_mismatch)
 ├─ ensure 拉起（失败计数/退避/熔断沿用 Supervisor）
 ├─ 等待 socket 就绪 ≤5s ──超时─► fail(class=spawn, phase=health)
 └─ 调用执行体（timeout_secs）─► 记录结果
```

- 复用 `Supervisor::ensure/call`（`crates/plugin/src/supervisor.rs:121-189`）；熔断（连续 3 次启动失败、退避 0.5s/2s/8s）不重复实现。
- 等待就绪以「socket 可连接 + 首次 ping 成功」为准，超时 5s；P1 若 Supervisor 没有显式健康探针，需要在 `ensure` 内补一个轻量 ping（属于实现，不在本文档展开）。
- 插件被停用：任务不删除，`state=blocked`；每轮只检查状态，不重复写审计（状态变化时写一次）。
- 插件版本/API 不匹配：`blocked(api_mismatch)`，提示升级内核或插件；不尝试降级调用。

### 6.1 常驻与保活

- 只要存在 `enabled` 且声明 `buffer` 的任务，内核标记该插件 `task_pinned`；30s 回收器跳过它，避免每 30s 冷启动。
- 无缓冲任务保持 `on-demand`，按 `idle_timeout_sec` 回收。
- `task_pinned` 是调度器状态，不改 manifest；插件卸载/全部任务停用后自动解除。
- 代价：常驻插件占内存；列表页展示「因任务常驻」标识，管理员可关闭任务释放。

## 7. 持久化与保留

### 7.1 表

| 表 | 关键字段 | 说明 |
| --- | --- | --- |
| `scheduled_tasks` | `id, kind, source, schedule_json, payload_json, enabled, state, timeout_secs, retry_json, concurrency, missed, buffer_json, created_by, created_at, updated_at, last_run_json` | 唯一事实源；缓冲不落库 |
| `task_runs` | `id, task_id, trigger, attempt, started_at, finished_at, result, error_class, error_detail, duration_ms, request_id, output_bytes` | 执行历史；按 task 分页查询 |
| `task_state_changes`（可并入 audit） | `task_id, from_state, to_state, reason, at` | 状态迁移可追溯 |

- 索引：`task_runs(task_id, started_at desc)`、`scheduled_tasks(source)`、`scheduled_tasks(state, enabled)`。
- 外键：`source=site:<domain>` 的任务在站点删除时级联删除（应用层显式删 + 历史清理）；`runtime_id` 删除保护同理。

### 7.2 审计事件

沿用 `audit` 表（`crates/store/src/model.rs:28-38`），动作命名：

| action | actor | target | 说明 |
| --- | --- | --- | --- |
| `task.create` | `admin:<user>` / `plugin:<id>` | task id | 创建/注册 |
| `task.update` | 同上 | task id | 修改调度/参数/开关（detail 只记变更字段名） |
| `task.delete` | 同上 | task id | 删除 |
| `task.trigger` | `admin` / `system` | task id | 手动触发 |
| `task.run` | `system` | task id | 每次执行结果（ok/fail/timeout/skipped_busy） |
| `task.blocked` | `system` | task id | 进入 blocked 及原因 |
| `buffer.high_watermark / overflow / recovered` | `system` | task id | 缓冲治理 |

- 审计只记元数据，不记 payload 内容（payload 可能含站点/容器标识，仍属敏感面）。
- 高频采样**不逐点审计**，只按 §4.5 阈值审计；执行历史单独落 `task_runs`，与安全审计分离。

### 7.3 保留与清理

- `task_runs`：默认保留 30 天或每任务 500 条，先到为准；由内置 `db-vacuum` 或 `task-runs-prune` 清理。
- 审计：沿用现有审计保留策略（若未实现，P1 补 90 天清理，见 FS 契约 §5）。
- 删除任务时历史保留 7 天便于回溯，随后异步清理；站点级联删除除外（立即清理）。

### 7.4 落盘目标

- 内置任务：内核直接写 `data_dir` 下约定目录（如 `backups/sites/<domain>/`），通过 fs 原语审计。
- 插件任务：执行体只能通过 fs 通道写；内核不代写插件数据。
- 缓冲区不落盘（P1）；执行体若需「先落盘再回执」，自行调用 fs 写入并返回 `persist`。

## 8. 内置 kind 清单

| kind | source | 默认 schedule | payload | 产出 | P1 |
| --- | --- | --- | --- | --- | --- |
| `builtin:site-backup` | `site:<domain>` | `30 3 * * *` + jitter 120s | `{keep:7, include:[index,conf,ssl]}` | `backups/sites/<domain>/<YYYYMMDD-HHmmss>/` + `manifest.json` | ✅ |
| `builtin:cert-renew` | `site:<domain>` | `0 4 * * *` | `{}` | 证书续期（Let’s Encrypt） | ⛔ P2 占位 |
| `builtin:db-vacuum` | `system` | `0 3 * * 0` | `{keep:7}` | `backups/db/skiff-<date>.db`（`VACUUM INTO`） | ✅ |
| `builtin:metrics-rollup` | `system` | `*/5 * * * *` | `{window:300}` | 内核指标聚合表 | ⛔ P2（依赖 monitor 迁移） |
| `builtin:metrics-prune` | `system` | `0 2 * * *` | `{keep_days:7}` | 清理聚合/原始指标 | ⛔ P2 |
| `builtin:task-runs-prune`（建议新增） | `system` | `0 2 * * *` | `{keep_days:30}` | 清理 `task_runs` | ✅（可并入 db-vacuum） |

约束：

- `site-backup` 由站点页「备份」开关创建/删除，开关幂等；任务 `source=site:<domain>`，站点删除时级联删除任务与备份目录策略见 FS 契约 §9。
- `cert-renew` 只注册禁用占位，UI 显示「P2 未实现」，不伪造续期成功。
- 内置任务不允许插件创建/修改；管理员只能改 schedule/payload/开关。
- 备份产物不得直接写进站点 `index/`（避免被 Web 暴露）；默认写 `data_dir/backups/sites/`，站点删除时按保留策略处理。

## 9. 与插件分工

| 插件 | 变更前 | 变更后 |
| --- | --- | --- |
| `cron` | 持有 `jobs.json`、自己匹配表达式、自己调用目标插件 | 纯管理 UI：任务增删改查、历史、手动触发；全部走内核 scheduler API；不再有计时器与本地任务文件 |
| `monitor` | manifest `cron` 每 5 分钟 `tick`，插件内环形缓冲 `samples.json` | 注册 `plugin:com.skiff.monitor:sample`：`sample_interval=60s` 进内核缓冲，`schedule=*/5` 执行批量落盘；插件只实现 `sample`/`execute` 两个方法 |
| `guard` | 每分钟 `tick` 全量列举容器 | 注册 `plugin:com.skiff.guard:reconcile`，每分钟对账；插件内无定时器 |
| `files` | 无定时任务 | 保持无任务；文件操作走 fs 原语 |

- `cron` UI 的「目标插件/方法」列表仍由内核 `interop` 能力面提供，不再由 cron 插件自行维护允许列表。
- 插件方法仍受四道闸（`interop:call` + `plugin:` 授权 + consumes/provides + 自调禁令），调度器不绕过。
- **manifest `[[task]]` 声明**（D8 已落地）：插件用 `key` / `method` / `expr` 或 `interval_secs` /
  `sample_method` / `sample_interval_secs` / `capacity_bytes` / `max_batch_bytes` / `timeout_secs` /
  `enabled` 描述「内核按什么节奏驱动我」；内核启动与 Web 安装/升级后按声明种入
  `plugin:<id>:<key>`，已存在则不覆盖（保留管理员改过的调度与启停）。插件内**不得**再有
  定时器；动态任务（站点备份等）仍由内核内置任务承担。
- `monitor` 落盘口径：`sample` 只更新 `data/prev.json` 差值基线；`execute` 把内核批次写
  `data/samples.json`（环形 240 点，60s 采样 ≈ 4 小时），统一走 fs 通道。写失败返回错误 →
  内核记失败且不产生回执，缓冲保留到下一轮重试；容量 4 MiB，溢出按 `drop_oldest`。
- `guard` 本轮只落周期对账；契约原定的「docker events 为主」尚无 events 能力面，列为 P2 差距，
  不以插件自建事件循环替代。

## 10. API 形态

### 10.1 内核内部 trait

```rust
trait Scheduler {
    fn register(&self, task: NewTask) -> Result<Task, TaskError>;   // 内置按 (kind, source)、插件按 id 幂等 upsert
    fn update(&self, id: &str, patch: TaskPatch) -> Result<Task, TaskError>;
    fn delete(&self, id: &str) -> Result<(), TaskError>;
    fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), TaskError>;
    fn run_now(&self, id: &str, actor: Actor) -> Result<RunHandle, TaskError>;
    fn list(&self, filter: TaskFilter) -> Vec<Task>;
    fn history(&self, id: &str, page: Page) -> Vec<TaskRun>;
    fn buffer_stat(&self, id: &str) -> BufferStat;
    fn push(&self, id: &str, point: Value) -> Result<(), TaskError>; // 插件自行采样时用
}
```

### 10.2 插件侧 RPC（能力面 `scheduler`）

| method | 参数 | 限制 |
| --- | --- | --- |
| `task.register` | `{local_key, method, schedule, payload, timeout, retry, concurrency, missed, buffer}` | 只能创建 `kind=plugin:<self>:<method>`；`source=plugin:<self>`；每插件 ≤ 20 条 |
| `task.update` | `{id, patch}` | 只能改自己的任务；不可改 kind/source |
| `task.delete` | `{id}` | 只能删自己的任务 |
| `task.list` | `{filter}` | 只返回自己的任务 |
| `task.run` | `{id}` | 只能触发自己的任务 |
| `task.history` | `{id, page}` | 只能看自己的任务 |
| `buffer.push` | `{id, point}` | 只能推自己的缓冲；单点 ≤ 64 KiB；限速（每任务 100 点/秒） |
| `buffer.stat` | `{id}` | 只读 |

- 权限：manifest 需声明 `scheduler` 能力与对应 verbs；未授权一律 403 + 审计 `scheduler.deny`。
- 插件任务不能指定 `kind=builtin:*`，不能跨插件调用，不能修改 `enabled` 以外的内核字段。
- `payload` ≤ 4 KiB（沿用 cron 现有 4 KiB 口径，`plugins/cron/src/main.rs:25,327-341`）。

### 10.3 HTTP 管理面

| 路由 | 方法 | 权限 | 说明 |
| --- | --- | --- | --- |
| `/{p}/tasks` | GET | 登录（viewer 可读） | 列表：任务/类型/来源/计划/状态/下次执行/缓冲占用 |
| `/{p}/tasks/{id}` | GET | 登录 | 详情：配置、最近 20 次、缓冲统计 |
| `/{p}/tasks/{id}/history` | GET | 登录 | 分页历史 |
| `/{p}/tasks/{id}/run` | POST | Admin + CSRF | 手动触发，审计 |
| `/{p}/tasks/{id}/toggle` | POST | Admin + CSRF | 启停，审计 |
| `/{p}/tasks` | POST | Admin + CSRF | 创建内置任务（仅允许 builtin 白名单） |
| `/{p}/tasks/{id}` | POST | Admin + CSRF | 修改 schedule/payload/开关 |
| `/{p}/tasks/{id}/delete` | POST | Admin + CSRF | 删除 |

- 深链：`/{p}/m/core/tasks`、`/{p}/m/core/tasks/{id}`；超管菜单新增「任务」一级入口（内核页，不占插件槽位）。
- 插件 UI 深链保持 `/{p}/m/plugin/com.skiff.cron/jobs`，由 cron 插件渲染，数据来自 scheduler API。
- 所有写操作带 CSRF、审计；错误内联返回，不吞异常。

## 11. 权限与页面

### 11.1 权限矩阵

| 操作 | Viewer | Operator | Admin | 插件（自身作用域） |
| --- | --- | --- | --- | --- |
| 查看任务/历史 | ✅ | ✅ | ✅ | ✅（仅自己） |
| 手动触发 | ❌ | ❌ | ✅ | ✅（仅自己） |
| 启停/修改/删除 | ❌ | ❌ | ✅ | ✅（仅自己） |
| 创建内置任务 | ❌ | ❌ | ✅ | ❌ |
| 查看缓冲统计 | ✅ | ✅ | ✅ | ✅（仅自己） |

> 推荐保持保守：任务会触发写盘/外部调用，Operator 不给手动触发，避免运维误操作放大。

### 11.2 页面要素

- 列表：名称、kind badge（内置/插件）、source、计划（cron/间隔）、状态（active/blocked/异常/停用）、下次执行、最近结果、缓冲占用条。
- 详情：配置只读展示 + 可编辑字段；「立即执行」；最近执行表（时间/触发/耗时/结果/错误类）；缓冲区（点数/字节/丢弃数/水位）；插件状态与跳转插件管理页。
- 空态：无任务时给出「到插件页注册任务」或「创建站点备份」的行动入口。
- blocked 任务必须有原因与修复建议（启用插件/升级/重新安装）。

## 12. 迁移与兼容

- 测试期，允许**直接重建**：新增 schema v8 三张表；不迁移 `jobs.json`。
- 插件升级时删除旧的 `plugins/cron/data/jobs.json`；cron UI 首次打开显示「任务已迁移至内核，请重新创建」空态（P1 可提供一次性导入按钮，但默认不做）。
- 现有 manifest `cron = [...]` 字段：**已移除 `run_cron`**；字段仅保留解析兼容，内核不再读取、不再产生第二套执行路径。
- manifest `[[task]]`：内核在启动装配与 Web 安装/升级成功后按声明种任务；插件卸载（或 manifest 升级后不再声明）时回收 `source=plugin:<id>` 的孤儿任务。种子任务以 `created_by=system:manifest` 标记，只回收这类「内核声明种入」的任务，插件自己经 RPC 建的任务不动。
- `monitor` 的 `samples.json`：P1 保留历史文件只读展示；新采样由 `sample` 出点进内核缓冲，`execute` 批次写回同一环形文件；不做旧数据合并。
- `guard` 的 `state.json` events：保留；`reconcile` 任务不依赖旧 tick。
- 站点备份：仓库内**未发现现有 `site_backup` 实现**（全仓搜索无 Rust 命中），因此无需收敛逻辑；站点页新增开关即可。

## 13. 测试矩阵与失败注入

| 类别 | 用例 | 期望 |
| --- | --- | --- |
| 模型 | kind/id/source 非法；插件越权创建 builtin；内置重复 (kind, source) upsert；插件同 id 重复 | 拒绝并给出字段级错误；内置合法重复只更新；插件任务身份是 id 里的 `local_key`，同一 `(kind, source)` 允许多条 |
| 调度 | cron 5 段；interval 4s 拒绝；jitter；本地时区；非法表达式 blocked | 不 panic；不刷审计 |
| 缓冲 | 容量上限；三种 overflow；retain false；partial 越界水位；单点 >64 KiB；全局 128 MiB | 丢弃/拒收计数正确；审计一次；越界回执按协议失败 |
| 执行 | 单飞跳过；queue_one；missed skip/once/all；时钟回拨 | `skipped_busy/missed_count/clock_skew` 记录正确 |
| 失败 | 插件停用/缺失/API 不匹配；拉起失败熔断；执行超时；执行体返回错误 | blocked 原因准确；超时不重入；重试只对瞬时错误 |
| 内置 | site-backup 开关幂等；站点删除级联；db-vacuum 产物与保留 | 无孤儿任务/文件；备份不落 Web 根 |
| 审计/权限 | 创建/修改/触发/失败审计；viewer 只读；插件跨作用域拒绝 | 审计字段完整；403 且审计 deny |
| 页面 | 列表/详情/历史/深链；blocked 原因展示 | 200；深链命中同一数据源 |
| 失败注入 | 插件进程 kill -9；RPC 慢响应；缓冲洪泛；磁盘写满；DB 忙 | 状态可见、可恢复；不丢审计；不无限重试 |

## 14. 决策点（推荐）

| # | 问题 | 推荐答案 | 理由 |
| --- | --- | --- | --- |
| D1 | 调度器放内核还是留在 cron 插件 | **内核** | 统一事实源/审计/权限；插件计时器会重复造轮子且无法保活 |
| D2 | 采样由谁驱动 | **内核按 `sample_interval` 调插件 `sample`，缓冲在内核** | 插件无计时器；缓冲跨插件重启不丢；内存可统一限额 |
| D3 | 大缓冲如何交给执行体 | **按 `max_batch_bytes` 切批（默认 512 KiB），最旧优先** | 避免一帧超 UDS 上限；执行体无需分页协议 |
| D4 | 默认 overflow | **`drop_oldest`；审计关键任务显式 `reject`** | 指标可丢、审计不可丢；默认不应静默拒收 |
| D5 | 错过执行 | **默认 skip；内置备份 `once`** | 避免重启后补跑风暴；备份补一次有意义 |
| D6 | 默认并发 | **`skip` + 记录 `skipped_busy`** | 不堆叠、不打爆插件；需要排队再显式 `queue_one` |
| D7 | 缓冲任务是否保活插件 | **标记 `task_pinned`，回收器跳过** | 常驻采样避免冷启动；代价可见、可关闭 |
| D8 | 插件任务注册方式 | **manifest `[[task]]` 声明 + 运行期 RPC 兜底** | 声明式可审计、随包走；动态任务（站点备份）走内置 |
| D9 | `cron` 插件的兼容 | **直接切换，不迁移 `jobs.json`** | 测试期无正式环境；保留兼容会长期供养两套调度 |

## 15. P1 / P2 范围

### P1（必须真跑通）

- schema v8：`scheduled_tasks` / `task_runs`（`task_state_changes` 可并入 audit）。
- 内核 1s 调度循环；cron + interval；单飞/超时/重试/错过语义。
- 插件任务注册（manifest + `scheduler` RPC）与按需唤醒、blocked 原因。
- 缓冲模型：内存计量、三种溢出、批次交付、四种回执、全局上限与告警。
- `cron` 插件改 UI；`monitor`/`guard` 迁移到任务模型。
- 内置 `site-backup`（站点页开关 + 站点删除级联）、`db-vacuum`。
- 管理页列表/详情/历史/手动触发；viewer 只读、写操作 Admin + CSRF + 审计。
- 测试矩阵覆盖模型、缓冲、执行、失败、内置、权限、深链。

### P2

- `cert-renew`（Let’s Encrypt）、`metrics-rollup`、`metrics-prune`（依赖指标迁移到内核）。
- 缓冲区 spill 落盘与重启恢复。
- 分布式/多机协调、任务依赖编排、优先级抢占。
- 通知渠道（邮件/Webhook）与任务 SLA 报表。

## 16. 与现有实现的差距

| 差距 | 现状 | 需要补什么 |
| --- | --- | --- |
| 无任务表 | ✅ 已落地：`scheduled_tasks`/`task_runs`（schema v12） | 保持 |
| 调度粒度是插件 | ✅ 已落地（2026-11-08）：`run_cron` 已删，manifest `[[task]]` 由内核种入 `scheduled_tasks`，1s 循环驱动 | 保持；新增周期能力必须走任务模型，不得恢复插件计时器 |
| 无缓冲 | ✅ 已落地：monitor 由内核 `sample` 出点进缓冲，`execute` 收批次回 `persist` | 保持；`task_pinned` 未实现，常驻采样仍依赖 `sample_interval < 回收阈值` |
| 无重试/重入/错过语义 | 仅 30s 超时 + 一次调用 | 单飞锁、retry、missed、冷却、错误分类 |
| 无执行历史 | 仅 `last` 一个结果（`jobs.json`） | `task_runs` + 分页查询 |
| 审计字段不足 | `audit` 仅 7 列，无 request id/耗时 | 任务审计走 `task_runs` + audit 双写；补 request id |
| 插件唤醒无健康等待 | `Supervisor::ensure` 拉起后直接拿 socket（`crates/plugin/src/supervisor.rs:121-189`） | ensure 内加 ≤5s ping/health；失败分类映射 |
| 常驻控制缺失 | 回收器无条件 `reap_idle`（`crates/core/src/server.rs:120`） | `task_pinned` 标记与回收豁免 |
| 无内置备份 | 全仓无 `site_backup` 实现；站点删除整目录删除（`crates/app/src/sites.rs:2523-2558`） | 内置备份任务、站点开关、产物目录与保留、删除级联 |
| `cron` 插件自带计时器 | ✅ 已落地：`cron` 纯 UI；`monitor`/`guard` 删除 `tick` 与 manifest `cron`，改内核任务 | 保持；UI 不再提供「立即巡检」等绕过调度器的直调入口 |

## 17. 变更记录

| 日期 | 变更 |
| --- | --- |
| 2026-11-05 | 首版评审稿：任务模型、缓冲模型、执行语义、内置任务、插件分工、API、迁移与测试矩阵 |
| 2026-11-08 | `monitor`/`guard` 迁移完成：删除 `run_cron` 与 manifest `cron` 执行路径，改为 manifest `[[task]]` 声明 + 内核种入；monitor 走 `sample`→缓冲→`execute` 批量落盘，guard 走 `reconcile` 每分钟对账；插件 UI 只读 |
