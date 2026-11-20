# 插件唤醒、持久化、内置 kind 与插件分工（§6–9）

> 上级索引：[KERNEL_SCHEDULER_CONTRACT.md](../KERNEL_SCHEDULER_CONTRACT.md) ｜ 拆自 `docs/contract/KERNEL_SCHEDULER_CONTRACT.md` 原 §6–9（2026-11-16 整理，正文未改）。

---

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
- 插件版本/API 不匹配：`blocked(api_mismatch)`，提示升级内核或插件；不尝试降级调用。兼容判定为「主版本一致、次版本不高于内核」，与商店安装共用 `skiff_pkgfmt::manifest::api_compatible`（当前内核 `1.3`）。

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
- 审计：保留 90 天，由内置 `audit-prune` 每日按时间窗清理（见 FS 契约 §4.3）。
- 删除任务时历史保留 7 天便于回溯，随后异步清理；站点级联删除除外（立即清理）。

### 7.4 落盘目标

- 内置任务：内核直接写 `data_dir` 下约定目录（如 `backups/sites/<domain>/`），通过 fs 原语审计。
- 插件任务：执行体只能通过 fs 通道写；内核不代写插件数据。
- 缓冲区不落盘（P1）；执行体若需「先落盘再回执」，自行调用 fs 写入并返回 `persist`。

## 8. 内置 kind 清单

| kind | source | 默认 schedule | payload | 产出 | P1 |
| --- | --- | --- | --- | --- | --- |
| `builtin:site-backup` | `site:<domain>` | 默认 `0 3 * * *` + jitter 120s；间隔/时间可在站点详情调整 | `{keep:7, include:[index,conf,ssl]}` | `backups/sites/<domain>/<YYYYMMDD-HHmmss>/` + `manifest.json` | ✅ |
| `builtin:cert-renew` | `site:<domain>` | `0 4 * * *` | `{}` | 证书续期（Let’s Encrypt） | ⛔ P2 占位 |
| `builtin:db-vacuum` | `system` | `0 3 * * 0` | `{keep:7}` | `backups/db/skiff-<date>.db`（`VACUUM INTO`） | ✅ |
| `builtin:audit-prune` | `system` | `0 5 * * *` | `{}` | 删除 `audit.ts < now-90d`，详情记 `deleted=<n>` | ✅ |
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
| `monitor` | manifest `cron` 每 5 分钟 `tick`，插件内环形缓冲 `samples.json` | 注册 `plugin:com.skiff.monitor:sample`：`sample_interval=60s` 进内核缓冲，`schedule=*/5` 执行批量落盘；插件只实现 `sample`/`execute` 两个方法；落盘为 `samples.jsonl` 追加 |
| `guard` | 每分钟 `tick` 全量列举容器 | 注册 `plugin:com.skiff.guard:reconcile`，每分钟对账；插件内无定时器 |
| `files` | 无定时任务 | 保持无任务；文件操作走 fs 原语 |

- `cron` UI 的「目标插件/方法」列表仍由内核 `interop` 能力面提供，不再由 cron 插件自行维护允许列表。
- 插件方法仍受四道闸（`interop:call` + `plugin:` 授权 + consumes/provides + 自调禁令），调度器不绕过。
- **manifest `[[task]]` 声明**（D8 已落地）：插件用 `key` / `method` / `expr` 或 `interval_secs` /
  `sample_method` / `sample_interval_secs` / `capacity_bytes` / `max_batch_bytes` / `timeout_secs` /
  `enabled` 描述「内核按什么节奏驱动我」；内核启动与 Web 安装/升级后按声明种入
  `plugin:<id>:<key>`，已存在则不覆盖（保留管理员改过的调度与启停）。插件内**不得**再有
  定时器；动态任务（站点备份等）仍由内核内置任务承担。
- `monitor` 落盘口径：`sample` 只更新 `data/prev.json` 差值基线；`execute` 把内核批次以
  JSON Lines **追加**到 `data/samples.jsonl`（一行一点，60s 采样），行数超过 480 才压缩为最近
  240 行（≈4 小时），统一走 fs 通道。追加写与文件大小无关，不再每次整文件重写。写失败返回错误 →
  内核记失败且不产生回执，缓冲保留到下一轮重试；容量 4 MiB，溢出按 `drop_oldest`。
  `history` 逐行解析，崩溃留下的半行/坏行跳过。
- `guard` 本轮只落周期对账；契约原定的「docker events 为主」尚无 events 能力面，列为 P2 差距，
  不以插件自建事件循环替代。
