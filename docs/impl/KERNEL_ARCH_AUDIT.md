# 内核自身架构体检（七维审计）

> 状态：只留**未处理**项——七维结论、非问题清单、未闭环、搁置项、未验证项。已处理条目随处理完即删，过程与判据见 `docs/changes/`。最近复核 2026-11-28。**编号不回收**：`C` 系列收口即删故有跳号，`N` 系列一旦登记永久占用（防重复讨论）。
> 范围：`crates/`（18 crate）、`xtask/`、`tools/skiff-server`；不含前端/模板/静态资源实现细节，不逐个点评官方插件，但插件**体系**（manifest、签名、Supervisor、能力面划分）在范围内。
> 关系：[ARCH_REVIEW.md](../archive/ARCH_REVIEW.md) 是历史六轮审查（只读存档）；[KERNEL_UI_BOUNDARY_AUDIT.md](KERNEL_UI_BOUNDARY_AUDIT.md) 是内核↔UI 边界专项。
> 方法：逐 crate 读 `Cargo.toml` 与模块树、按关键词定位并核对源码；本地只跑编译级门禁（`fmt` / `clippy` / `xtask check-channel`），行为用例以 CI 为准；行数与体量一律取 **HEAD**。

## 1. 七维结论

基线事实：无依赖环（`app`/`web`/`core` 单向）、全仓无 Cargo feature 开关、生产路径 `unwrap/expect` 均在不变量之后、零 `TODO`/`FIXME`、未闭环问题 **0**。

**总判断：分层与防错纪律良好。** 四项单一来源（插件↔内核协议 `skiff-wire`、TOFU `plugin::trust::resolve`、动词→Docker `Docker::execute`、角色→权限 `skiff_model::Permission`）与分层禁令在最近三轮全量复扫中零违规；内核文件写通道已收口（`xtask check-channel` 白名单 0 文件 0 处）；能力面 HTTP 形状全部归位 `core`，`plugin` 不依赖 HTTP 框架；两个易复发的「第二份清单」已固化单一来源：`RUNTIME_INDEX_KINDS` 在 `skiff-server` 的 `build.rs`，审计分类命名空间在 `skiff_model::AuditCategory`。

| 维度 | 结论 | 关键证据 |
| --- | --- | --- |
| 1 模块划分 | **合理，无过热文件** | `menu/` 目录 7 文件（2169 行，最大 `decl.rs` 623 行）；`tasks`/`runtime_catalog`/`runtime`/`crud`/`menu` 均已拆分；`ui/dto/` 按域拆 13 文件（最大 235 行）；`ui/*_page.rs` 已全部改用 `schema_kit` 共享构件；`store/src/db.rs` 已拆为连接/迁移驱动/DDL/用例 |
| 2 职责边界 | **主线清楚，靠命名与注释维持** | `app` 不含表现层；`/ui/v1` 行装配读 `app/src/views/*`；`web` 无 `state` 别名 |
| 3 依赖关系 | **无环、无 feature 分叉** | 依赖表见 `docs/sections/08-engineering.md`；`app` 仅对 `tools/skiff-server` 有 dev-dependency |
| 4 复用性 | **主要重复已收口** | 宿主机执行→`skiff_host`；权限通配→`Grant::permits`；路径代数→`skiff_model::path`；签名协议→`skiff-sign`；插件生命周期→`app/commands/plugin.rs`；列表分页→`ui::paginate`；UI 输出端六件→`ui/schema_kit.rs`；百分号编码→`web/util.rs::encode_uri_component`；容量格式化→`web/util.rs::human_size` |
| 5 过度设计 | **轻微** | 单实现 trait 都有测试替身或依赖倒置理由；无空转发层 |
| 6 与 UI 解耦 | **读模型已下沉，展示属性仍在 app** | `app/src/views/*` 是 SSR/JSON 共用读模型；JSON DTO 形状独立于模板 |
| 7 健壮性 | **整体强** | 审计同步写库 + 失败落盘兜底；`systemd-run` 超时清单元；`systemctl restart docker` 20s 总预算；`nginx -t` 10s 上限 + Drop 守卫；loopback 旁路不采信客户端身份头 |

## 2. 非问题清单（下轮不要重复讨论）

| # | 候选点 | 判定 |
| --- | --- | --- |
| N1 | 单实现 trait（`CapabilityHost`/`Clock`/`TaskStore`/`Transport`/`FtpHost`/`RoleExt`/`NginxControl`） | 依赖倒置必要抽象或有测试替身，保留 |
| N2 | `JobService::submit` 重复提交 | SQLite 部分唯一索引兜底，有用例 |
| N3 | 生产代码大量 `unwrap` | 排除 `#[cfg(test)]` 后全部在不变量之后 |
| N4 | `cfg(unix)` / `cfg(target_os="linux")` 分叉 | 只有非 unix 桩与 linux-only 宿主能力两类，双向 CI 覆盖 |
| N5 | 重启后状态一致性 | 三条收尾链路都有，剩余业务半完成态契约已声明 |
| N6 | 请求超时口径 | 非流式 30s、SSE/WS 不挂；Docker 30s、探活 3s |
| N7 | `open_dir_nofollow` 带 `#[allow(dead_code)]` | 技术储备，已实现未接入 |
| N8 | `views/runtimes.rs::list` 是一行包装 | 承载降级口径，读模型入口 |
| N9 | `skiff_store::NewSite` 与 `skiff_app::sites::NewSite` 同名 | 层次不同，转换在一处完成，不改名 |
| N10 | `plugin::ipc::MAX_FRAME` 再导出 `skiff_wire::MAX_FRAME` | 注释指明唯一来源是 wire |
| N11 | `web` 的 `tempfile` 在 `[dependencies]` | 上传暂存目录使用，非误放 |
| N12 | `app/src/db_async.rs` 自制异步桥 | 有界并发 4 + 排队快败 + 嵌套自锁拒绝，全仓约 20 调用点 |
| N13 | `app` 38K 行装配层体量大 | 与 08-engineering 定位一致，域内拆分清晰 |
| N14 | `image_versions` 负缓存「无上限」 | 键是不可变 Docker image_id，镜像集很小；为删镜像加跨模块清键属过度接线 |
| N15 | `sha256_hex`/`Scope`/io 错误分类各两份 | 两侧无依赖的支撑层，合并成本过高，保留观察 |
| N16 | `LoginBody` 两份（api.rs 3 字段 / session.rs 5 字段） | 契约不同（CSRF/回跳），合并让一方接受多余字段 |
| N17 | `web/src/util.rs` 只有 6 个纯函数 | 无状态转换，页面拼接类逻辑不进这里 |
| N18 | `plugins/common` 仅 175 行，共享率偏低 | **有意识克制**：审计要看得见谁调了什么能力 |
| N19 | `plugins/ftp` 依赖 `common` 却直接用 `skiff_model` 常量 | `common` 已 re-export，走它是为了不新增依赖边 |
| N20 | `model` 含 path.rs/exec.rs/verb.rs 非纯数据 | 路径代数只在 model 是 AGENTS.md §4.1 有意设计；model 零内部依赖 |
| N21 | `model/src/scope.rs` 与 `fs/src/subject.rs`/`plugin/src/fsops.rs` scope 同名 | token scope vs 文件系统路径 scope，无语义歧义 |
| N22 | `store/src/db/` 子模块拆分依据不一致 | 每处不一致都有领域理由（按表/按业务域/按执行车道） |
| N23 | `core/src/commands.rs` 含站点属主对齐等业务 | core 是可执行 crate（无 lib target），CLI 命令天然在此 |
| N24 | `sdk/src/bin/skiff-plugin.rs` pack/stamp 与 pkgfmt 重叠 | SDK CLI 面向插件作者，pkgfmt 是内核侧库；复用而非复制 |
| N25 | `skiff-server::HttpUpstream` 与 `app::UreqTransport` 各封装一份 ureq | 需求形状不同（上游版本 vs 条件 GET），分属发布工具与内核装配层 |
| N26 | 展示文案分三处（views 预拼 / labels 纯枚举 / handler 状态文案） | **已决策口径**（AGENTS.md §4.1），统一无行为收益；仅当出现第二消费方或改漏时才立项 |
| N27 | 13 处 `#[allow(dead_code)]` | 全是平台分支或测试替身，逐处有注释 |
| N28 | 能力面错误类型只带 u16 状态码 | 原因枚举本来就存在（FsError 6 变体 / FtpError·FirewallError 各 3 变体），status() 只是 HTTP 投影 |
| N29 | `plugin/firewall.rs`/`core/kernel/firewall.rs` 全方法返回 400 | **冻结决定兑现**：保留路由让旧调用方拿到可读的「已迁移」而非 404 |
| N30 | `commands/store.rs` 与 `commands/upgrade.rs` 各有一个 `cleanup_stale_incoming` | 目录与扩展名不同，两行适配 + 业务日志；合并加三参数更绕 |
| N31 | `handlers/ui/mod.rs` 的 `#![allow(unused_imports)]` + 子模块 `use super::*` | ui prelude 写法，40 子模块头部省十几行；若 ui 子模块降到 10 个以内再改 |
| N32 | `routes.rs` 的 `fn enc` 是 `encode_uri_component` 一行包装 | 局分别名让调用侧读作 `enc(value)`，不是第二份实现 |

## 3. 搁置项（原因与重启条件）

**条件只认可判定的事实**：数字阈值，或已发生的事件（出现第二个消费方、接入第二种传输、实际事故）。满足任一条即做，不必等复审；既改不动条件也不打算做的，直接写「不做」并说清代价。行数取**工作区实测并标注日期**。

| 搁置项 | 当前实测（2026-11-28） | 触发线 | 重启条件 |
| --- | --- | --- | --- |
| **C16** 标识符词法 `[A-Za-z0-9._-]` 七份副本 | — | — | ① `sched` 因其它原因需要 `skiff_model`；② 该词法规则需第二次修改 |

> **已完成项（已删除）**：C8 `menu.rs` 拆分 → ✅ 2026-11-28 完成（2018 行 → `menu/` 目录 7 文件：`mod.rs` 447 / `decl.rs` 623 / `prefs.rs` 206 / `render.rs` 187 / `settings.rs` 194 / `workbench.rs` 410 / `mount.rs` 102，行为零改动，对外 pub 接口完全兼容）；C8 `routes.rs` 正名 → ✅ 2026-11-28 完成（`handlers/ui/routes.rs` 1250 行 → `shell.rs`；模块声明 `mod routes` → `mod shell`、`.merge(routes::router())` → `.merge(shell::router())`；`ui/mod.rs` 的 `pub use dto::*` 降级为 `pub(crate) use dto::*`——crate 内部子模块照常从父级 re-export 取 dto 类型，外部 crate 不再能从 `handlers::ui::SiteItem` 等路径拿到 dto 类型；shell.rs 显式引 `use super::dto::MenuViewData`，budding.rs 测试 include_str 路径同步更新 9 处）；C17 `sites/runtime.rs`/`sites/crud.rs` 拆分 → ✅ 2026-11-28 完成（拆为 `runtime/{mod,jobs,php}` + `crud/{mod,vhost,tls}`，行为零改动，附带修复 6 个已存在的路径错误）；C14 `site_page.rs` SDUI Schema 拆分 → ✅ 2026-11-28 完成（898 行 → `site_page/` 目录 6 文件：`site_page.rs` 237 / `overview.rs` 150 / `config.rs` 140 / `domains.rs` 52 / `ssl.rs` 113 / `backup.rs` 158，按 tab 拆分，`site_btn`/`site_write_action` 提升为 `pub(crate)` 供子模块复用，对外 `pub` 接口 `site_page_schema` / `sites_page_schema` / `site_create_fields` 完全不变，调用方 `sites.rs`、`sites_extra_page.rs` 无需改路径）；C8 已完成部分：`tasks/builtins.rs` 从 tasks.rs 拆出、`runtime_catalog.rs` 归位 `sites/runtime_catalog.rs`、`catalog/store.rs`→`catalog/cache.rs`（消弭「store」三义）。

## 4. 未验证项

1. **审计写入的实际阻塞概率**：`Db` 的 50ms 等待告警在真实负载下是否触发未采集日志；已验证的是「写库失败必落 `audit-fallback.jsonl`」兜底。
2. **站点域行数占比**：只按文件归属统计过，未做函数级归属；`runtime_catalog` 已归位 `sites/runtime_catalog.rs`，现属站点域。
3. **`web` 之外是否还有死依赖**：只核到 `skiff-sched`（已删）；`cargo-machete` 类工具未跑，其余按人工抽样判断。
