# async 路径上的同步 SQLite（§15）

> 上级索引：[ARCH_REVIEW.md](../ARCH_REVIEW.md) ｜ 拆自 `docs/archive/ARCH_REVIEW.md` 原 §15（2026-11-16 整理，正文未改）。

---

## 15. async 路径上的同步 SQLite（2026-11-11 登记；同日第二轮实测与迁移）

**结论**：`skiff_store::Db` 是 `std::sync::Mutex<rusqlite::Connection>` + 同步 `rusqlite`。release 实测单条
语句 **0.01–0.25 ms（p99 ≤ 0.34 ms）**，但有三类操作会把单次持锁拉到 **数十到数百毫秒**：
`purge_audit_before`（大表 DELETE，20 万行 182 ms）、`VACUUM INTO` 备份（193 ms）、以及跨进程
`busy_timeout`（5 s）。运行时只有 **2 个 worker**（`crates/core/src/main.rs:145-147`），两个同步调用
同时被拖住就会让整个 HTTP/UDS 运行时停摆。因此按「spawn_blocking + 有界并发」迁移高频入口，见 §15.3。

### 15.1 清点清单（async 上下文中的同步调用）

| 入口 | 位置 | 频次 | 单次耗时（实测/量级） | 请求热路径 | 持有哪些锁 |
| --- | --- | --- | --- | --- | --- |
| 会话加载中间件 | `crates/web/src/middleware.rs:208-227`（`verify_session` + `user_by_id`） | 每请求 1–2 条 SELECT | 0.03 ms + 0.015 ms | 是（含静态资源） | 仅 `Db` 互斥；`last_seen` 续期按 30 s 节流 |
| Bearer 鉴权提取器 | `crates/web/src/middleware.rs:283-354`（`verify_api_token` [+UPDATE] / `verify_session` / `user_by_id`） | 每个 Bearer 请求 2–3 条 | 0.03–0.06 ms | 是（API） | 仅 `Db` 互斥 |
| 页面上下文 / 菜单 | `crates/web/src/handlers/mod.rs:73,93,107` → `crates/app/src/menu.rs:1140,1157,1468,54`（`get_user_pref` + `get_setting` + 2×`list_plugins`） | 每个 HTML 页面 3–4 条，`list_plugins` ×2 | 0.47 ms/次（10 插件 × 8 KB manifest） | 是（所有页面） | 仅 `Db` 互斥；同步函数，未持有异步锁 |
| 概览页 | `crates/web/src/handlers/dashboard.rs:40-82` | 5 条读 | 合计 ~1 ms | 是 | 仅 `Db` 互斥 |
| 站点 / 运行环境页 | `crates/web/src/handlers/sites/crud.rs:48-54,507,527,629,802`；`crates/app/src/sites/{crud,runtime}.rs`（65 + 49 处） | 1–N 条，动作路径含循环 | 0.1 ms/次；N+1 时线性增长 | 是 | 仅 `Db` 互斥；TLS 写库时另持 `nginx_write_lock` |
| 设置页 | `crates/web/src/handlers/settings.rs:26,37,203-220,301,330,403` | 4–8 条 | 0.01–0.5 ms | 是 | 仅 `Db` 互斥 |
| 插件页 / 安装 / 卸载 | `crates/web/src/handlers/plugins.rs:38-784`；`crates/app/src/menu.rs:930`（按用户逐条改写） | 页面 1–3 条；装卸 5–15 条 + 逐用户 | 单条短，但装卸是批量 | 是（低频重操作） | 仅 `Db` 互斥；包体 I/O 已在 `blocking_fs` |
| 作业轮询 | `crates/web/src/handlers/jobs.rs:187-313`；SSE 循环 `:405-430`（`get` + `logs_since` 每轮） | SSE 每轮 2 条 | 0.35 ms（tail 200） | 是（长连接轮询） | 仅 `Db` 互斥 |
| 作业执行落日志 | `crates/app/src/jobs.rs:395-427`（`append_job_logs`，事务内 2+ 语句 + COUNT/SUM） | 每行 1 次，批量 `log_lines` 1 次 | 0.20 ms/行，尾部 8 ms | 是（安装/升级作业） | 仅 `Db` 互斥 |
| 调度 tick | `crates/core/src/server.rs:124-130` → `crates/app/src/tasks.rs:292-310` → `crates/sched/src/scheduler.rs:573-618` | 1 s × 任务数（每任务 `get_task` + 可能 `put_task`/`append_run`/`append_audit`） | 0.024 ms/条；N 任务线性 | 后台定时 | **`TaskService::scheduler` 的 tokio Mutex 全程持有** |
| tick 落盘（二次写） | `crates/app/src/tasks.rs:302-310,416-433` | 每 tick 每条记录/审计再写一次（`INSERT OR REPLACE` / 普通 INSERT） | 同上 | 后台定时 | 同上（在调度锁内） |
| 会话清理 | `crates/core/src/server.rs:167`（`purge_expired_sessions`） | 每 30 s 1 条 DELETE | 未实测（随过期会话数增长） | 否 | 仅 `Db` 互斥 |
| 插件鉴权（UDS） | `crates/core/src/kernel/auth.rs:43,90-98`（`list_plugins` 全表 + 失败审计） | 每个插件能力面请求 1–2 条 | 0.47 ms（`list_plugins`） | 插件 RPC 热路径 | 仅 `Db` 互斥 |
| 能力面调用 | `crates/core/src/kernel/capability.rs:50-389`（`plugin` / `plugin_config` / 审计 2–3 条） | 每请求 2–5 条 | 0.02–0.5 ms | 插件 RPC 热路径 | 仅 `Db` 互斥 |
| FTP 能力面 | `crates/core/src/kernel/ftp.rs:114-166,195-227`（授权 + 审计 + `list_sites` + 每站点 `runtime_by_id`） | 每请求 3–N 条 | `list_sites` 0.1 ms + N×0.02 ms | 插件 RPC 热路径 | 仅 `Db` 互斥 |
| 插件互调 | `crates/core/src/kernel/interop.rs:35-273`（`plugin` ×2 + `list_plugins` + 审计） | 每请求 3–5 条 | 0.5–1 ms | 插件 RPC 热路径 | 仅 `Db` 互斥 |
| 插件生命周期 | `crates/plugin/src/supervisor.rs:537-565`（状态回写 + 审计） | 启动/停止/回收各 1–2 条 | 短 | 否 | **`running` 的 tokio Mutex 在写库时仍被持有**（`set_plugin_status`） |
| 启动自检 | `crates/core/src/bootstrap.rs:35-158`（建库、迁移、settings、首个管理员） | 一次性 10+ 条 | `open+migrate(18)` 31 ms | 否（未开始服务） | 无 |
| 调度器装载 | `crates/app/src/tasks.rs:175-183` → `crates/sched/src/scheduler.rs:201,217` | 启动 2 次全量 `list_tasks` | 0.024 ms + 解析 | 否 | 无 |
| 集群同步 | — | 本仓库无集群同步入口 | — | — | — |

`crates/app/src/admin_fs.rs`（回收站/配额/审计，23 处）与 `crates/web/src/handlers/files.rs` 的全部调用
都在 `handlers::blocking_fs`（`spawn_blocking`）里执行，**不在 worker 线程上**，不计入本清单。

### 15.2 实测数据

**release 组（结论引用口径）**：Windows 开发机，2 核 4 线程，SSD，WAL，`synchronous=NORMAL`，
`cargo run --release -p skiff-store --example db_latency`（临时测量程序，跑完即删）。数据集：200 会话 /
10 插件 × 8 KB manifest / 30 站点 / 50 作业 × 200 行日志 / 20 万审计行。单次调用：

| 操作 | avg | p99 | max |
| --- | --- | --- | --- |
| `verify_session`（30 s 内不续期） | 0.013 ms | 0.019 ms | 0.55 ms |
| `audit`（单行 INSERT） | 0.049 ms | 0.076 ms | **8.35 ms**（WAL checkpoint） |
| `list_plugins`（10 × 8 KB） | 0.222 ms | 0.336 ms | 1.54 ms |
| `list_sites(30)` | 0.054 ms | 0.075 ms | 0.18 ms |
| `append_job_logs`（1 行） | 0.124 ms | 0.233 ms | **12.5 ms**（WAL checkpoint） |
| `recent_audit(200)` | 0.240 ms | 0.334 ms | 0.33 ms |
| `purge_audit_before`（约 20 万行） | — | — | **182 ms**（首次；清空后 0.05 ms） |
| `backup_to`（`VACUUM INTO`，20 万审计行） | 192.8 ms | 193.4 ms | 193.4 ms |
| `Db::open + migrate(18)`（新库） | 20.4 ms | 21.1 ms | 21.1 ms |
| 争用：备份（92 ms）期间 4 线程查询 | 5075 次查询 | — | **单次最坏等待 92.9 ms** |

**debug 组（对照，不复用于结论）**：同机同数据集、debug 构建（SQLite 为 C 实现，差异主要在 Rust
封装层，量级偏大）。单次调用：

| 操作 | avg | p99 | max |
| --- | --- | --- | --- |
| `verify_session`（30 s 内不续期） | 0.029 ms | 0.096 ms | 0.42 ms |
| `verify_session`（触发续期 UPDATE） | 0.048 ms | 0.231 ms | 0.39 ms |
| `user_by_id` | 0.015 ms | 0.026 ms | 0.42 ms |
| `audit`（单行 INSERT） | 0.055 ms | 0.152 ms | **7.9 ms**（WAL checkpoint） |
| `recent_audit(200)` | 0.455 ms | 0.730 ms | 0.73 ms |
| `list_plugins`（10 × 8 KB） | 0.471 ms | 1.464 ms | 2.39 ms |
| `list_sites(30)` | 0.105 ms | 0.230 ms | 0.88 ms |
| `scheduled_tasks`（tick 全量） | 0.024 ms | 0.050 ms | 0.13 ms |
| `scheduled_task`（主键单条） | 0.024 ms | 0.046 ms | 0.10 ms |
| `append_job_logs`（1 行） | 0.198 ms | 0.702 ms | **8.0 ms**（WAL checkpoint） |
| `job_log_tail(200)` | 0.347 ms | 0.965 ms | 0.97 ms |
| `purge_audit_before`（约 20 万行） | — | — | **260 ms** |
| `backup_to`（`VACUUM INTO`，20 万审计行） | **291 ms** | 296 ms | 296 ms |
| `Db::open + migrate(18)`（新库） | 31 ms | 35 ms | 35 ms |
| 争用：备份（43 ms）期间 4 线程查询 | 332 次查询 | — | **单次最坏等待 35.7 ms** |

**「最坏情况会阻塞运行时线程多久」**：

1. 常规读写：release p99 ≤ 0.34 ms；WAL checkpoint 偶发 8–12 ms（`audit` / 作业日志写入的 max）。
2. 大表维护：审计裁剪（20 万行 182 ms）、`VACUUM INTO`（20 万行 193 ms）与数据量近似线性；审计保留
   90 天、作业日志保留 30 天，线上库达到百万行时这两项进入**秒级**。它们已在 `spawn_blocking` 内，
   但仍占用唯一的 `Db` 互斥：期间所有迁移/未迁移的 DB 调用排在同一把锁上。
3. 跨进程争用：`busy_timeout=5 s`。CLI 的 `backup` / `doctor` / `audit` 会以第二连接打开同一库；
   一旦与在线写入撞上，**请求线程最多被卡 5 s**，且 `std::sync::Mutex::lock()` 不能被打断/超时。
4. worker 只有 2 个：上述任一长操作 + 另一个普通请求同时命中，HTTP 与 `kernel.sock` 全部停摆；
   `/healthz` 也拿不到线程。

### 15.3 方案对比与选择

| 维度 | A. `spawn_blocking` + 有界并发 | B. 专用 DB 线程 + 请求通道 | C. 连接池 + 阻塞线程池 |
| --- | --- | --- | --- |
| 内存 | 每并发一个阻塞线程（虚拟栈 2 MB，实际 RSS 小）；上限 4 可封顶 | 1 个常驻线程 + 有界队列；内存最省且可预测 | 连接数 × 页缓存（默认 2 MB）+ 线程；最高 |
| 可观测性 | permit 等待/执行耗时需自己记（本轮加日志字段）；spawn_blocking 队列深度不可见 | 队列深度天然可观测，背压清晰 | 池指标成熟，但需引入库 |
| 取消语义 | future 丢弃后阻塞任务**继续执行到结束**（审计/事务不被截断，不留半提交）；读操作浪费一次 | 请求已入队即执行；需额外取消令牌才能中断 | 借出连接上的操作无法中断，归还延迟；最易出现「取消后仍在写」的观测盲区 |
| 与现有事务边界 | 完全不改 `Db`：每条语句/事务原样 | 闭包内仍调 `Db` 同步方法，边界不变，但需替换全局句柄 | 事务边界不变，但多连接下 `BEGIN IMMEDIATE` 的 busy 语义变化 |
| 新依赖 | 无（tokio 已在 `app/web/core`） | 无（std mpsc + tokio oneshot） | 需要池实现（新 crate 或自研） |
| 改造面 | 中：逐个 async 调用点包一层，可分批 | 大：所有 async 侧必须改走服务对象，sync 侧（CLI/bootstrap）保留直连，两套入口易漂移 | 大：替换 `Db` 构造与全部调用点 |
| 实际收益 | 直接消除「阻塞 worker」；单写者模型不要求多连接 | 与 A 相当，额外消除锁竞争等待 | 针对「连接不足」，而实测瓶颈是单写者串行，收益低 |

**选 A**。理由：唯一实际风险是「阻塞 worker」，A 正好消除它；语义零变化、零新依赖、可按入口分批
回归；B 的收益与 A 相近但要替换全局 `Db` 句柄，回归面大；C 解决的瓶颈不存在，还要新依赖。

### 15.4 本轮实施范围与边界

已迁移（`skiff_app::db_async`：`spawn_blocking` + 全局 `Semaphore(4)`；permit 随闭包移动、在闭包结束时
释放，取消不会提前归还。排队最多等 2 s，超时返回 `Busy` 并打 warn，不静默堆积；闭包内再投递会返回
`Nested` 被拒绝，避免占着 permit 等 permit 自锁）：

1. `web` 会话中间件：`load_session` 与 `AuthUser` 的 Bearer 路径（每请求必经）；
2. `core/kernel`：插件鉴权 `uid_to_plugin`、`capability::config_get` / `identity_self`、`ftp::context`
   的站点快照（N+1 整体一次投递）；
3. `web/handlers/dashboard.rs` 的 5 条读（合并为一次投递）；
4. `app/tasks.rs::tick_once` 的运行记录/审计落盘：移出 `scheduler` 锁后再投递（锁内只留调度状态机）。

未迁移（记录为后续，不在本轮）：

- `menu` / `page_ctx`：同步函数被所有页面渲染复用，改 async 需动全部 handler 签名；
- `jobs` 日志逐行写入：需把作业执行器整体改为 async 感知；
- `app/sites/**` 的同步 helper：位于动作编排深处，先按 §14 收口动作层再迁；
- `purge_audit_before` / `backup_to` 这类秒级长操作：不属于线程池问题，已按 §15.5 的维护车道单独处理
  （分批删除 + 第二连接快照），不经过 `db_async` 的 permit。

**边界**：`std::sync::MutexGuard` 不是 `Send`，不会出现「持锁跨 await」型死锁；本轮同时修正
`supervisor.rs:537-545` 在 `running` 异步锁内写库的问题（先释放再写）。所有迁入 `spawn_blocking`
的闭包都是自包含语句/事务，取消只会让结果无人接收，不会留下半提交。

### 15.5 备份争用归因与剩余风险

§15.2 里「备份 92 ms 期间 5075 次查询、最坏单次等待 92.9 ms」**不是 `db_async` 的信号量排队**，而是
**同一个 `Db` 互斥被整段持有**。判定依据：

1. 该测量程序用 `std::thread` 直接调 `Db::verify_session` / `Db::backup_to`，全程没有 tokio、没有
   `db_async`、没有 permit，等待只可能发生在 `Db.conn` 的 `std::sync::Mutex` 上；
2. `Db::backup_to`（当日位于 `crates/store/src/db.rs`，现为
   `crates/store/src/db/maintenance.rs`）先取 `self.conn` 守卫，再在守卫内执行 `VACUUM INTO`，
   守卫直到函数结束才释放——备份窗口 = 持锁窗口；
3. 因此最坏等待约等于整个备份时长，是 SQLite 单连接串行化的直接结果，`spawn_blocking` 只能保证
   等待期间**不占 worker**，不能缩短等待本身。

现状核对（**下表为归因当日状态**；维护车道已于 2026-11-12 按下文「实施结果」落地）：

| 操作 | 调用路径 | 是否走 `db_async` / 占 permit | 持锁表现 |
| --- | --- | --- | --- |
| 站点/数据库备份（`backup_to`） | `tasks.rs::db_vacuum` / `site_backup`，内置任务在独立 `spawn_blocking` 里执行 | **否**（不占 `db_async` permit） | 整个 `VACUUM INTO` 期间独占 `Db` 互斥 |
| 审计裁剪（`purge_audit_before`） | `tasks.rs::audit_prune`，同上 | **否** | 整条 DELETE 期间独占 `Db` 互斥 |
| CLI `skiff backup` | 独立进程（`core/src/commands.rs:461`） | 不适用 | 跨进程 SQLite 锁，服务侧最多等 `busy_timeout` 5 s |

结论：**给维护操作单独的低并发额度没有收益**——它限的是 `db_async` 的 permit，而这两类操作根本不
经过该池子；即使给它们单独 1 个额度，普通请求仍会在 `Db` 互斥上等满整个维护窗口。真正能缩短等待的
办法只有一条：**让维护操作不再持共享连接**。

**实施结果（2026-11-12，第 0–2 步）**：维护车道收口到 `crates/store/src/db/maintenance.rs`。

1. **可观测**：`Db::conn_lane(lane)` 在拿到锁后记录等待，≥50 ms 打 warn（`lane=short|maintenance`）；
   维护方法统一打 `lane="maintenance"` + `op` / `duration_ms` / `rows` / `bytes`；`db_async` 模块文档
   写明「覆盖范围只到 `short` 车道」，加大 permit 对维护窗口无效。
2. **审计裁剪分批**：5000 行/批、每批一个自动提交事务、批间让出 5 ms；累计删除数仍是返回值与
   `rows` 日志，调用方（`tasks.rs` 的 `audit_prune`）语义不变。
3. **备份走维护专用第二连接**：普通读写方式打开——**不要改成 `SQLITE_OPEN_READ_ONLY`**，WAL 库的
   只读连接仍要读写同目录 `-shm` / `-wal`，缺写权限会 `SQLITE_CANTOPEN`（约束写在代码注释里）；
   `busy_timeout=1500ms`（主连接 5 s 是面向带外 CLI 进程的），争用即失败上抛，不与它叠加；该连接
   只发读语句与 `VACUUM INTO`，备份后在同一连接上跑一次 `PRAGMA wal_checkpoint(PASSIVE)`。
   dest≠源、dest 完整性校验、失败清理保持原样。
4. **串行性**：维护写操作仍全部走主连接，并由进程内静态 `Mutex` 串行；备份与审计裁剪不会并行，
   第二连接只读快照，不引入第二写者。

实测（release，Windows 动态内存虚拟机 4.92 GB / 2C4T，200,000 审计行，4 个读线程并发
`recent_audit(50)`，直接运行已构建二进制以避开闸门编译争用；**Linux 测试机尚未复测**）：

| 操作 | 改造前（§15.2） | 改造后 |
| --- | --- | --- |
| 备份 | 备份期间 5075 次查询，最坏单次等待 92.9 ms（= 持锁窗口） | 快照 22.2 MB，耗时 381 / 391 / 411 ms；读者 p99 7.7–11.8 ms、最坏 13.3–23.8 ms |
| 审计裁剪 | 182 ms，最坏等待 182 ms | 维护窗口 646–676 ms；读者 p50 0.18–0.35 ms、p99 21.2–22.6 ms、最坏 39.1–39.6 ms |

读等待从「等于维护窗口」降到「单批 / 单次 checkpoint 的 I/O 抖动」量级（约 4–7x）；代价是裁剪总窗口
因批次与让出变长约 3.6x，备份后多一次 PASSIVE checkpoint。以上只代表本机这一组样本，不外推生产。

**剩余风险（更新）**：

- 备份不再占 `Db` 互斥，但 `VACUUM INTO` 仍要读全库；期间跨进程 CLI 写库会与它争 SQLite 锁，
  维护侧 1.5 s 后直接失败上抛（有意不排队）。
- `purge_audit_before` 的写仍在主连接上：单批内仍会短暂独占互斥，批量越小等待越短、总窗口越长。
- 维护操作仍由内置任务低峰调度；百万行级库的窗口继续线性放大，届时需重新评估。

**遗留（P1，本轮不做）**：rusqlite `backup` feature 的在线备份（可增量、可限定步数）与「维护背压标志」。
