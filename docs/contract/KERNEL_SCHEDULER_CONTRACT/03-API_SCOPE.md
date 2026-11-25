# API 形态、权限页面、兼容、测试、决策与差距（§10–17）

> 上级索引：[KERNEL_SCHEDULER_CONTRACT.md](../KERNEL_SCHEDULER_CONTRACT.md) ｜ 拆自 `docs/contract/KERNEL_SCHEDULER_CONTRACT.md` 原 §10–17（2026-11-16 整理，正文未改）。

---

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
| `task.update` | `{id, patch}` | 只能改自己的任务；patch 只接受 `schedule` / `payload` / `timeout_secs` / `retry` / `concurrency` / `missed` / `buffer` / `enabled`，带 `kind` 直接拒绝（执行体由内核按 manifest 维护） |
| `task.delete` | `{id}` | 只能删自己的任务 |
| `task.list` | `{filter}` | 只返回自己的任务 |
| `task.run` | `{id}` | 只能触发自己的任务 |
| `task.history` | `{id, page}` | 只能看自己的任务 |
| `buffer.push` | `{id, point}` | 只能推自己的缓冲；单点 ≤ 64 KiB；限速（每任务 100 点/秒） |
| `buffer.stat` | `{id}` | 只读 |

- 权限：manifest 需声明 `scheduler` 能力与对应 verbs；未授权一律 403 + 审计 `scheduler.deny`。
- 插件任务不能指定 `kind=builtin:*`，不能跨插件调用，不能修改 `enabled` 以外的内核字段。
- `kind` 只由内核维护：manifest `[[task]]` 的 `method` 变化时，内核在升级同步里补 `kind`；
  插件 RPC 传 `patch.kind` 一律拒绝（契约只允许内核改派执行体）。
- `payload` ≤ 4 KiB（沿用 cron 现有 4 KiB 口径，`plugins/cron/src/main.rs:25,327-341`）。

### 10.3 HTTP 管理面

| 路由 | 方法 | 权限 | 说明 |
| --- | --- | --- | --- |
| `/ui/v1/tasks` | GET | 登录（viewer 可读） | 列表：任务/类型/来源/计划/状态/下次执行/缓冲占用 |
| `/ui/v1/tasks/{id}` | GET | 登录 | 详情：配置、最近 50 次、缓冲统计 |
| `/ui/v1/tasks/{id}/run` | POST | Admin + CSRF | 手动触发，审计 |
| `/ui/v1/tasks/{id}/toggle` | POST | Admin + CSRF | 启停，审计 |
| `/ui/v1/tasks` | POST | Admin + CSRF | 创建内置任务（仅允许 builtin 白名单） |
| `/ui/v1/tasks/{id}` | POST | Admin + CSRF | 修改 schedule/payload/开关 |
| `/ui/v1/tasks/{id}/delete` | POST | Admin + CSRF | 删除 |

- 深链：`/{p}/tasks/scheduled`、`/{p}/tasks/scheduled/{id}`；超管菜单归属「任务审计」一级（内核页，不占插件槽位）。
- 插件 UI 深链 `/{p}/plugins/com.skiff.cron/jobs`（cron 若仍无 schema，则由壳页补 `?legacy=1` 打开 iframe），数据来自 scheduler API。
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
- `monitor` 的 `samples.jsonl`：P1 保留历史文件只读展示；新采样由 `sample` 出点进内核缓冲，`execute` 批次**追加**到同一 JSONL 文件，超 480 行压缩回 240 行；不做旧 `samples.json` 数据合并。
- `guard` 的 `state.json` events：保留；`reconcile` 任务不依赖旧 tick。
- 站点备份：内置任务 `builtin:site-backup:<域名>`（`crates/app/src/tasks.rs`），打包走 `host.exec` 作业；站点详情页只管开关与计划。恢复（2026-11-21）同样走执行体：预检 → 恢复前快照 → 解包三步，作业目标复用同一 `builtin:site-backup:<域名>`，因此恢复与调度器打包天然互斥。

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

> 原 2026-11-05 差距快照已过期并删除：任务表 `task_runs`、内核调度（单飞/超时/冷却/重试/错过语义）、缓冲模型、`cron` 纯 UI 化、插件唤醒健康检查等均已落地，逐轮变更见 `docs/changes/`；未闭环统一记 [TEST_LEDGER.md](../../test/TEST_LEDGER.md)，不在本契约重复。
