# 内核自身架构体检（七维审计）

> 状态：只留**未处理**项——七维结论、非问题清单、未闭环、搁置项、未验证项。已处理条目随处理完即删，过程与判据见 `docs/changes/`。最近复核 2026-11-28（UI 输出端重复构件收口 + 体量表数字校准 + 两个未登记的大文件补录）。**编号不回收**：`C` 系列收口即删故有跳号，`N` 系列一旦登记永久占用（防重复讨论）。
> 范围：`crates/`（18 crate）、`xtask/`、`tools/skiff-server`；不含前端/模板/静态资源实现细节，不逐个点评官方插件，但插件**体系**（manifest、签名、Supervisor、能力面划分）在范围内。
> 关系：[ARCH_REVIEW.md](../archive/ARCH_REVIEW.md) 是历史六轮审查（只读存档）；[KERNEL_UI_BOUNDARY_AUDIT.md](KERNEL_UI_BOUNDARY_AUDIT.md) 是内核↔UI 边界专项。
> 方法：逐 crate 读 `Cargo.toml` 与模块树、按关键词定位并核对源码；本地只跑编译级门禁（`fmt` / `clippy` / `xtask check-channel`），行为用例以 CI 为准；行数与体量一律取 **HEAD**（工作区读数会把未提交批次算成增长）。

## 1. 七维结论

基线事实：无依赖环（`app`/`web`/`core` 单向）、全仓无 Cargo feature 开关、生产路径 `unwrap/expect` 均在不变量之后、零 `TODO`/`FIXME`。

**总判断：分层与防错纪律良好。** 四项单一来源（插件↔内核协议 `skiff-wire`、TOFU `plugin::trust::resolve`、动词→Docker `Docker::execute`、角色→权限 `skiff_model::Permission`）与分层禁令在最近三轮全量复扫中零违规；内核文件写通道已收口，唯一例外是 §4 C15；能力面 HTTP 形状全部归位 `core`，`plugin` 不依赖 HTTP 框架（§2 N28）。两个易复发的「第二份清单」已固化单一来源：`RUNTIME_INDEX_KINDS` 在 `skiff-server` 的 `build.rs`（`sync`/`runtimes`/`check` 共用），审计分类命名空间在 `skiff_model::AuditCategory`（`store` 的 SQL 只存命名空间表达式编码）。

| 维度 | 结论 | 关键证据 |
| --- | --- | --- |
| 1 模块划分 | **基本合理，剩余一个过热文件** | `menu.rs` 2018 行（阈值见 §4 C8）；`tasks.rs` 2616 / `sites/runtime_catalog.rs` 2210 / `sites/runtime/{mod 1117, jobs 1528, php 385}` / `sites/crud/{mod 1743, vhost 619, tls 418}` 均已拆分；`ui/routes.rs` 1250 行在阈值内；`ui/dto/` 按域拆 13 文件（最大 235 行）；`ui/*_page.rs` 已全部改用 `schema_kit` 共享构件（2026-11-28）；`store/src/db.rs` 已拆为「连接 139 / 迁移驱动 94 / DDL 621 / 用例 361」 |
| 2 职责边界 | **主线清楚，靠命名与注释维持** | `app` 不含表现层；`/ui/v1` 行装配读 `app/src/views/*`，模板与 DTO 各自映射；`web` 无 `state` 别名 |
| 3 依赖关系 | **无环、无 feature 分叉** | 依赖表见 `docs/sections/08-engineering.md`；`app` 仅对 `tools/skiff-server` 有 dev-dependency（撤包原因端到端用例） |
| 4 复用性 | **主要重复已收口** | 宿主机执行 → `skiff_host`；权限通配 → `Grant::permits`；路径代数 → `skiff_model::path`；签名协议 → `skiff-sign`；插件生命周期 → `app/commands/plugin.rs`；列表分页 → `ui::paginate`；运行环境容器名 → `sites::runtime_container_name_for`；暂存清理 → `commands/incoming.rs`；本地时间 → `web/util.rs::fmt_local_time`；**UI 输出端六件 → `ui/schema_kit.rs`**（JSON Pointer 取数 `text`/`flag`/`num`/`rows`、写请求信封 `post_request`、指标卡 `metric_card`）、百分号编码 → `web/util.rs::encode_uri_component`、容量格式化 → `web/util.rs::human_size`（2026-11-28） |
| 5 过度设计 | **轻微** | 单实现 trait 都有测试替身或依赖倒置理由；无空转发层 |
| 6 与 UI 解耦 | **读模型已下沉，展示属性仍在 app** | `app/src/views/*` 是 SSR/JSON 共用读模型（预拼的中文串归属见 §2 N26），JSON DTO 形状独立于模板 |
| 7 健壮性 | **整体强** | 审计同步写库 + 失败落盘兜底；`systemd-run` 超时清单元；`systemctl restart docker` 20s 总预算；`nginx -t` 10s 上限 + Drop 守卫；loopback 旁路不采信客户端身份头 |

## 2. 非问题清单（下轮不要重复讨论）

| # | 候选点 | 为什么不是问题 |
| --- | --- | --- |
| N1 | 单实现 trait | `CapabilityHost` 是依赖倒置的必要抽象（`plugin` 不能依赖 `app`，否则成环）；`Clock`/`TaskStore`/`Transport`/`FtpHost` 有测试替身；`RoleExt` 是扩展方法惯用法；`NginxControl` 的替身用于站点启停路径。删掉的都是「没有生产使用方」而非「有测试替身」的那类。**保留** |
| N2 | 重复提交/幂等 | `JobService::submit` 的同目标唯一由 SQLite 部分唯一索引兜底并映射 `JobError::Busy`，有用例，不靠调用方自觉 |
| N3 | 生产代码大量 `unwrap` | 排除 `#[cfg(test)]` 后全部在不变量之后；`store`/`plugin`/`web` 热路径无裸 `unwrap` |
| N4 | `cfg(unix)` / `cfg(target_os="linux")` 分叉 | 全仓无 Cargo feature；平台分支只有非 unix 桩与 linux-only 宿主能力两类，Windows/Linux CI 双向覆盖 |
| N5 | 重启后状态一致性 | 作业中断结算、插件停服、安装锁三条链路都有收尾；剩余是「业务半完成态」，契约已声明需人工核对 |
| N6 | 请求超时口径 | 非流式 30 秒，SSE/WS 不挂；Docker 客户端默认 30 秒、探活 3 秒 |
| N7 | `open_dir_nofollow` 带 `#[allow(dead_code)]` | 技术储备（作者注释明确），登记为「已实现未接入」；对外不得声明 TOCTOU 已消除 |
| N8 | `views/runtimes.rs::list` 是一行包装 | 承载「取数失败按空列表降级」的口径，是读模型入口，不是空转发 |
| N9 | `skiff_store::NewSite` 与 `skiff_app::sites::NewSite` 同名 | 层次不同（仓储插入行 vs 建站输入），转换在一处完成。**不改名** |
| N10 | `plugin::ipc::MAX_FRAME` 再导出 `skiff_wire::MAX_FRAME` | 注释指明唯一来源是 wire，且是常量不是逻辑 |
| N11 | `web` 的 `tempfile` 在 `[dependencies]` | 上传暂存目录使用，非误放 |
| N12 | `app/src/db_async.rs` 自制异步桥 | `Db` 是同步 `Mutex<rusqlite::Connection>`，裸 `spawn_blocking` 无界、直接跑 worker 会卡 HTTP + kernel.sock；现为有界并发（4）+ 排队快败 + 嵌套自锁拒绝，全仓约 20 个调用点。**保留** |
| N13 | `app` 38K 行「装配层」体量大 | `AppState` + 用例层定位与 08-engineering 一致，域内拆分（sites/tasks/catalog/commands）清晰；过热单文件走 §4 C8，不改定位 |
| N14 | `image_versions` 负缓存「无上限」 | 键是不可变的 Docker `image_id`，条目数 = 宿主用过的镜像数（镜像集很小）；负缓存窗口 60 秒；为删镜像加跨模块清键属过度接线。**不处理** |
| N15 | `sha256_hex`（identity/fs）、`Scope` 枚举（fs/plugin）、io 错误分类（fs/plugin）各两份 | 两侧是无相互依赖的支撑层，合并要么新增共享 crate、要么反转层次（plugin→fs）。纯成本项。**保留观察** |
| N16 | `LoginBody` 两份（`handlers/api.rs` 3 字段 / `ui/dto/session.rs` 5 字段） | 契约不同：`/api/v1` 无 CSRF 与回跳，`/ui/v1` 必须带；合并会让其中一个接受多余字段。**不合并** |
| N17 | `web/src/util.rs` 只有 6 个纯函数 | 都是无状态转换（cookie 解析、转义、容量格式化）；页面拼接类逻辑不进这里 |
| N18 | `plugins/common` 只 175 行有效代码，共享率偏低 | **有意识的克制**：各插件的能力调用与策略判定刻意各写各的，「免得抽象层把『谁调了什么能力』藏起来」（审计要看得见）。**不合并** |
| N19 | `plugins/ftp` 依赖 `common` 却直接用 `skiff_model` 常量 | `common` 已 re-export `SITE_UID`/`SITE_GID`，走 `common` 是为了不新增依赖边。**保持现状** |
| N20 | `crates/model` 名叫 model 却含 `path.rs` 授权代数、`exec.rs` 二进制风险分类、`verb.rs` 动词→权限映射 | 路径代数只在 `skiff_model::path` 是 AGENTS.md §4.1 的有意设计；`model` 零 workspace 内部依赖，且 `Permission` 矩阵与 `Verb` 表必须同源。**不改定位** |
| N21 | `model/src/scope.rs`（token scope）与 `fs/src/subject.rs`、`plugin/src/fsops.rs`（路径 scope）同名 | 不同域概念（token 权限范围 vs 文件系统路径范围），前者已被 `web` 用作 scope 闸门；各自模块内限定使用，无语义歧义。**不改名** |
| N22 | `store/src/db/` 子模块的拆分依据不完全一致 | 多数按表、少数按业务域聚合从属表（`jobs`/`plugins`/`runtimes`/`sites`/`tasks`）、`maintenance` 按执行车道拆（长持锁需与短事务车道分开表达）。每处不一致都有领域理由。**不强行统一** |
| N23 | `core/src/commands.rs` 含站点属主对齐等业务实现 | `core` 是可执行 crate（无 lib target），CLI 命令天然在此；`sites_chown_index` 的规则与 `app/src/sites/layout.rs` 一致，迁移收益低于跨 crate 暴露成本。**暂不迁移** |
| N24 | `sdk/src/bin/skiff-plugin.rs` 的 `pack`/`stamp` 与 `pkgfmt` 打包能力重叠 | SDK CLI 面向插件作者，`pkgfmt` 是内核侧库；SDK 复用其实现而非复制。**不迁移** |
| N25 | `skiff-server::HttpUpstream`（代理 + Bearer 挑战 + 30s）与 `app` 的 `UreqTransport`（条件请求 + 自定义 CA + 3s 建连）各封装一份 ureq | 需求形状不同（上游版本抓取 vs 条件 GET 目录拉取），分属发布工具与内核装配层；合并要么新增共享 crate，要么让 `skiff-server` 反向依赖 `skiff-app`。与 N15 同类。**保留观察** |
| N26 | 展示文案分三处（`views/*` 预拼、`labels.rs` 纯枚举、handler 状态文案） | **已决策的口径**而非待办：规则在 AGENTS.md §4.1，三处归属各有理由，统一到任一方都要同时动 SSR 与 JSON 两个消费面且无行为收益。只有出现「第二种语言 / 前端自渲染文案」「同一文案两处改漏」「接入第二个前端消费方」之一才重新立项 |
| N27 | 13 处 `#[allow(dead_code)]` | 全是平台分支或测试替身所需：`cfg_attr(not(unix), …)` 的非 Unix 桩（`core/kernel`、`plugin/{capability,fsops,interop,ftp,supervisor}`）、`#[cfg_attr(not(test), …)]` 的测试专用件、`fs/lock.rs` 的技术储备（即 N7）、`web/{asset_rules,templates}.rs` 的常量。逐处有注释。**不清理** |
| N28 | 能力面错误类型只带 `u16` 状态码，像丢了「原因」 | 原因枚举本来就存在：`FsError` 是 6 变体（`Forbidden`/`NotFound`/`Conflict`/`ReadOnly`/`NoSpace`/`Io`），经 `CapError::Fs(FsError)` 原样交到 `core`，`status()` 只是同一处的 HTTP 投影；`FtpError`/`FirewallError` 各 3 变体与 403/503/500 一一对应，越权另由 `is_denied()` 判。第二种传输出现时直接匹配变体即可。**不是待办** |
| N29 | `plugin/firewall.rs`（63 行）与 `core/kernel/firewall.rs`（151 行）在能力面全方法统一返回 400 | **冻结决定的兑现**（`docs/plugin/HOST_EXEC_DESIGN.md` §五）：`CAP_FIREWALL_CALL` 与 SDK 的 `firewall_call` 仍在导出面，保留路由让旧调用方拿到可读的「已迁移」400 而非 404；授权与部署形态判定放在拒绝之前，越权照样落 `deny` 审计。ftp 面同理只留只读查询。**不删** |
| N30 | `commands/store.rs` 与 `commands/upgrade.rs` 各有一个 `cleanup_stale_incoming` | 同名但目录与扩展名不同（`.incoming/*.wbpkg|part` vs `upgrade/incoming/*.tar.gz|part`），都是对 `commands/incoming.rs::cleanup_expired` 的两行适配加一条业务日志。合并就要加 `exts`/`dir`/日志文案三个参数，读起来比两份五行包装更绕。**保留** |
| N31 | `handlers/ui/mod.rs` 的 `#![allow(unused_imports)]` + 子模块 `use super::*` | 这是 ui 模块的「prelude」写法：共享 import（`Value`/`json`/`AppState`/`Arc`）集中在 mod.rs，各子模块 glob 取用。代价是这几个类型在 ui 内的未使用导入不会被发现，收益是 40 个子模块头部不重复十几行。改成显式导入是 40 文件的大 diff 且无行为收益。**保留**（若将来 ui 子模块降到 10 个以内再改） |
| N32 | `routes.rs` 的 `fn enc` 是 `util::encode_uri_component` 的一行包装 | 保留是因为该文件的语义单位是「query 片段」（`kv`/`query` 都调它），局分别名让调用侧读作 `enc(value)` 而非 28 字符的长名；与 C16 的「跨 crate 词法副本」性质不同（这里不是第二份实现）。**保留** |

## 3. 未闭环问题

**无。** `xtask check-channel` 白名单已**彻底清空**（2026-11-27 C15 收口）——0 文件 0 处，app/core 全经 `AdminFs::*_kernel` 或 `skiff_host`。

## 4. 搁置项（原因与重启条件）

**条件只认可判定的事实**：数字阈值，或已发生的事件（出现第二个消费方、接入第二种传输、实际事故）。满足任一条即做，不必等复审；既改不动条件也不打算做的，直接写「不做」并说清代价。行数取**工作区实测并标注日期**（有未提交批次时即提交后的值，比 HEAD 更接近下轮起点）。

| 体量类条目 | 工作区实测（2026-11-28，含未提交批次） | 触发线 | 归属 |
| --- | --- | --- | --- |
| `app/src/tasks.rs` | 2616 行 | 3139 行（+20%） | C8 ✅ |
| `app/src/sites/runtime_catalog.rs` | 2210 行 | 2652 行（+20%） | C8 ✅ |
| `app/src/menu.rs` | 2018 行 | 2422 行（+20%） | C8 |
| `web/handlers/ui/routes.rs` | 1250 行 | 1375 行（+10%） | C8 |
| `app/src/catalog/cache.rs` | 122 行 | 146 行（+20%） | C8 ✅ |

- **C8 `tasks.rs` / `runtime_catalog.rs` / `menu.rs` 拆分正名（含 web 侧 `routes.rs`）**。**已完成项**：① `tasks/builtins.rs` 从 tasks.rs 拆出（内置执行体：备份/DB 整理/回收站清理/审计清理），tasks.rs 2849→2616 行，builtins.rs 263 行；② `runtime_catalog.rs` 归位改名 `sites/runtime_catalog.rs`，lib.rs 移除模块声明，`use crate::runtime_catalog` 全仓改为 `crate::sites::runtime_catalog`（app 内 8 文件 + web crate 4 文件 + labels.rs）；③ `catalog/store.rs`→`catalog/cache.rs`，消弭「store」三义（skiff-store / catalog 缓存 / commands/store.rs 商店用例）之一。clippy 全过。**剩余项**：`menu.rs` 拆分（声明解析/排序/偏好写库/渲染模型）；`ui/mod.rs` 取消 `pub use dto::*` + 调用点显式 `dto::<域>::Xxx`（与 `routes.rs` 正名同批）。**重启条件**：menu.rs ≥2422 行或 routes.rs ≥1375 行。
- **C14 `web/handlers/ui/site_page.rs` 手写 SDUI Schema**。askama 模板退场后，站点详情页结构由 Rust 侧拼装（含 `overview_actions` 与状态驱动的按钮切换）；逻辑在服务端是对的（权限判定可信），可读性低于模板。**已做的微改进**：2026-11-27 消除本地重复辅助函数 5 处（`dash`→`kit::or_dash`、`text_line`、`write_card`、`flex`、`API_BASE`）；2026-11-28 把 `metric_card` 上移 `schema_kit`（三页共用一份）。site_page.rs 实测 **898 行**（原 917）。保留 `site_write_action` / `site_btn` 两件站点专属。**重启条件**（任一）：① 站点页改版或加 tab；② 引入 SDUI Schema DSL builder；③ 单文件 ≥1078 行（+20%，2026-11-28 实测 898）。
- **C16 标识符词法 `[A-Za-z0-9._-]` 七份副本**。`sched/src/model.rs::is_token`、`sched/src/buffer.rs` 内联、`pkgfmt/src/manifest.rs::is_manifest_token`、`pkgfmt/src/schema.rs` 内联、`app/src/tasks.rs::valid_plugin_id`、`app/src/tasks.rs` 内联（`local_key`）、`app/src/sites/vhost.rs` 内联（站点目录 slug）。同一词法规则跨三个 crate 各写一遍：改一处漏一处就会造成「安装时接受的 id，调度时被判非法」。**不做的原因**：`sched` 目前只依赖 `skiff-cron`，收敛到 `skiff_model` 需要新增 `sched → model` 依赖边，属 AGENTS.md §4.2 的「跨模块依赖调整」，须先定依赖方向而不该由复审顺带决定。**重启条件**（任一）：① `sched` 因其它原因需要 `skiff_model`；② 该词法规则需要第二次修改。
- **C17 ~~`sites/runtime.rs`（3103）/ `sites/crud.rs`（2862）拆分~~** ✅ **已完成 2026-11-28**。两个文件是 `app` 里最大的生产文件，此前从未登记体量阈值。`runtime.rs` 一个 `impl AppState` 块装五件事（运行时生命周期、作业视图、PHP 扩展安装、PHP 配置读写、配置目标路径）；`crud.rs` 装站点读写 + vhost 渲染 + TLS 快照。**实际执行**：① `runtime.rs` → `runtime/{mod.rs, jobs.rs, php.rs}`（1117 / 1528 / 385 行）；② `crud.rs` → `crud/{mod.rs, vhost.rs, tls.rs}`（1743 / 619 / 418 行）。纯移动，行为零改动。**附带修复**：原 crud.rs/runtime.rs 中 `crate::runtime_catalog` 路径错误（应为 `crate::sites::runtime_catalog`），baseline 即有 6 个编译错误；18 个私有方法因跨文件调用改为 `pub(crate)`。**配套改动**：`crates/web/tests/main_flow/ops.rs` 的 `include_str!` 改为 `concat!` 拼接三文件。fmt + clippy 全仓零错误零警告。

## 5. 未验证项

1. **审计写入的实际阻塞概率**：`Db` 的 50ms 等待告警在真实负载下是否触发未采集日志；已验证的是「写库失败必落 `audit-fallback.jsonl`」兜底。
2. **站点域行数占比**：只按文件归属统计过（`sites.rs` + `sites/` 11 文件），未做函数级归属；`runtime_catalog` 已归位 `sites/runtime_catalog.rs`，现属站点域（运行环境目录）。
3. **`web` 之外是否还有死依赖**：只核到 `skiff-sched`（已删）；`cargo-machete` 类工具未跑，其余按人工抽样判断。
