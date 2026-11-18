# 内核自身架构体检（七维 · 2026-11-19 复审）

> 状态：首轮批次 A/B/C 已收口（2026-11-13 ～ 15）；**2026-11-19 全面复审并二次复核**（模块划分 / 职责边界 / 依赖关系 / 复用性 / 过度设计，排除 `plugins/` 插件本体）。本文件只保留结论、非问题清单、未闭环问题、搁置项与未验证项；已处理的清单随处理完即删（过程见 `docs/changes/`）。
> 范围：`crates/`（18 crate）、`xtask/`、`tools/skiff-server`；不含前端/模板/静态资源实现细节，不逐个点评官方插件；插件**体系**（manifest、签名、Supervisor、能力面划分）在范围内。
> 与既有文档的关系：[ARCH_REVIEW.md](../archive/ARCH_REVIEW.md) 是历史六轮审查（只读存档）；[KERNEL_UI_BOUNDARY_AUDIT.md](KERNEL_UI_BOUNDARY_AUDIT.md) 是内核↔UI 边界专项（web 内部重复与错误映射口径在那边）。行号易腐化，只作结论参考。
> 方法：逐 crate 读 `Cargo.toml` 与模块树、按关键词定位，关键结论核对源码；本机只跑 `fmt`/`clippy`/`xtask check` 编译级门禁，行为用例以 CI 为准。

## 1. 七维结论

基线事实（2026-11-19 复核）：无依赖环（`app`/`web`/`core` 单向）、全仓无 Cargo feature 开关；生产路径 `unwrap/expect` 均在不变量之后。

**总判断：分层与防错纪律良好；首轮问题已随批次 A/B/C 收口，本轮小项已清理，遗留见 §3。**

**2026-11-19 复审增量结论**：依赖方向零违规（`policy`/`pkgfmt`/`wire` 隔离 `store`/`docker`、`model` 零 I/O、`sched` 不反依赖 `app`、`host` 无授权判定）；四项单一来源均核对通过（wire 协议路由常量零泄漏、TOFU 只在 `plugin::trust::resolve`、`Docker::execute` 唯一分发、RBAC 矩阵只在 `model::rbac`）；cron→sched→app/tasks 无重复计算。本轮已顺手收口：`web/src/menu.rs` 兼容层删除（web 直接消费 `skiff_app::menu`）、路径代数改调 `skiff_model::path`（policy / plugin / web / fs）、`app` 内 `unix_now` 三份合为 `time_fmt`。同日二次复核后又收口：`/ui/v1` 行装配上移 `app/src/views/*`、插件生命周期下沉 `app/src/commands/plugin.rs`、签名协议抽 `skiff-sign`、`check-channel` 补计数形态并重定基线、支撑层错误补 `std::error::Error`、菜单 URL 形状常量入 `skiff_model::url`、删除零调用的内核 `mirror_probe.rs`。剩余新债见 §3（内核文件写通道外仍有批量生产写，已迁首批）。

| 维度 | 结论 | 关键证据 |
| --- | --- | --- |
| 1 模块划分 | **基本合理，三个过热文件** | `app/src/tasks.rs` 2.4k 行、`runtime_catalog.rs` 2.0k 行、`menu.rs` 1.9k 行；`sites.rs` 1.1k + `sites/` 12 文件域内拆分清晰；`store/src/db.rs` 连接+迁移+备份同文件（拆分搁置，见 §4 C6） |
| 2 职责边界 | **主线清楚，边界靠注释维持** | `app` 自述「不含表现层」；`/ui/v1` 行装配已改读 `app/src/views/*`，模板与 DTO 各自映射 |
| 3 依赖关系 | **无环、无 feature 分叉** | 依赖表与能力面分布表已随批次 A5 更新（`docs/sections/08-engineering.md`、AGENTS.md） |
| 4 复用性 | **主要重复已收口** | `run_host` 收口为 `skiff_host`；权限通配匹配收口 `Grant::permits(_wildcard)`；路径代数收口 `skiff_model::path`（含 `fs::protected_hit`）；签名协议收口 `skiff-sign`；插件生命周期收口 `app/commands/plugin.rs`；菜单兼容层已删 |
| 5 过度设计 | **轻微** | 单实现 trait 均有测试替身或依赖倒置理由；本轮删掉 `web/src/menu.rs` 纯 `pub use` 层后，未发现新的空转发 |
| 6 与 UI 解耦 | **读模型已下沉，展示属性已上移** | `app/src/views/*` 是 SSR/JSON 共用读模型；JSON 侧 DTO 形状已独立（`handlers/ui/dto.rs`）并直接消费 app 读模型 |
| 7 健壮性 | **整体强** | 审计改同步写库 + 失败落盘兜底；`systemd-run` 超时清单元；`systemctl restart docker` 20s 总预算；`nginx -t` 10s 上限 + Drop 守卫 |

## 2. 非问题清单（下轮不要重复讨论）

| # | 候选点 | 为什么不是问题 |
| --- | --- | --- |
| N1 | 14 个 trait 里多数只有一个生产实现 | `CapabilityHost` 是依赖倒置的必要抽象（`plugin` 不能依赖 `app`，否则成环）；`Clock`/`Executor`/`TaskStore`/`Transport`/`FtpHost`/`FirewallHost` 都有测试替身；`RoleExt` 是扩展方法惯用法。**建议全部保留。** |
| N2 | 重复提交/幂等 | `JobService::submit` 的同目标唯一由 SQLite 部分唯一索引兜底并映射 `JobError::Busy`，有测试用例，不靠调用方自觉 |
| N3 | 生产代码大量 `unwrap` | 排除 `#[cfg(test)]` 后约 30 处，全部在不变量之后；`store`/`plugin`/`web` 热路径没有裸 `unwrap` |
| N4 | `cfg(unix)` / `cfg(target_os="linux")` 分叉风险 | 全仓无 Cargo feature；平台分支只有非 unix stub 与 linux-only 宿主能力两类，Windows/Linux CI 双向覆盖 |
| N5 | 重启后状态一致性 | 作业中断结算、插件停服、安装锁三条链路都有收尾；剩余是「业务半完成态」，契约已声明需人工核对，不是缺陷 |
| N6 | 请求超时口径 | 非流式 30 秒，SSE/WS 不挂；Docker 客户端默认 30 秒、探活 3 秒。分层正确 |
| N7 | `open_dir_nofollow` 是 `#[allow(dead_code)]` | 技术储备（作者注释明确），登记为「已实现未接入」；对外不得声明 TOCTOU 已消除 |
| N8 | `views/runtimes.rs::list` 是一行包装 | 承载「取数失败按空列表降级」的明确口径，是读模型入口，不是空转发 |
| N9 | `skiff_store::NewSite` 与 `skiff_app::sites::NewSite` 同名 | 层次不同（仓储插入行 vs 建站输入），转换在一处完成。**不改名** |
| N10 | `plugin::ipc::MAX_FRAME` 再导出 `skiff_wire::MAX_FRAME` | 有注释说明唯一来源是 wire，且是常量不是逻辑 |
| N11 | `web` 的 `tempfile` 在 `[dependencies]` | 上传暂存目录使用，非误放 |
| N12 | `app/src/db_async.rs` 自制异步桥 | `Db` 是同步 `Mutex<rusqlite::Connection>`，裸 `spawn_blocking` 无界、直接跑 worker 会卡 HTTP+kernel.sock；有界并发（4）+ 排队快败 + 嵌套自锁拒绝，全仓约 20 个真实调用点。**保留** |
| N13 | `app` 37K 行「装配层」体量大 | `AppState`+用例层定位与 08-engineering 一致，域内拆分（sites/tasks/catalog/commands）清晰；过热单文件问题走 §4 C8，不改定位 |
| N14 | `image_versions` 负缓存「无上限」 | 键是 Docker `image_id`（不可变），条目数 = 宿主用过的不同镜像数（运行环境镜像集很小）；负缓存命中窗口 60 秒；为删镜像加跨模块清键属过度接线。**不处理** |
| N15 | `sha256_hex`（identity/fs）、`Scope` 枚举（fs/plugin）、io 错误分类（fs/plugin）各两份 | 两侧都是无相互依赖的支撑层：合并要么新增共享 crate，要么反转层次（plugin→fs）。属于「抽公共件」的纯成本项，无行为收益。**保留观察，不列为待办** |

## 3. 未闭环问题

1. **（值得改，2026-11-19，进行中）内核文件写通道外仍有生产写**：`AdminFs::write_kernel` 的唯一客户此前是 `docker_daemon.rs`。本轮已迁首批：`sites/crud.rs` 的 vhost / 默认站点 / 站点目录、`sites/runtime.rs` 的 Rust/Node 环境脚手架与 `entry.sh`。剩余主要热点：`panel_settings.rs` 写 config.toml 仍走 `atomic::write_atomic`（零 fs 审计）、`sites/crud.rs` 证书与克隆（`spawn_blocking` 内，需把 `AdminFs` 以可克隆句柄传入）、`nginx.rs` 主配置（候选文件 → test → rename 流程，缺 rename 原语）。修法：继续分批迁 `write_kernel`/`remove_kernel`，基线随迁随减（当前 95，只减不增；理由是记录，见 AGENTS.md 通道唯一）。

## 4. 搁置项（原因与重启条件）

- **C5 `plugin → axum` 解耦**。现状：能力实现（`crates/plugin`）直接以 axum 的请求/响应类型表达结果，HTTP 形状与命令编排混在一层。不做的原因：需先定义**传输无关**的能力面返回类型，会改动所有能力方法签名与两处调用方，当前没有第二种传输或第二个消费方，纯收益无法验证。**重启条件**（任一）：① 能力面接入第二种传输（SDUI/CLI/内部 RPC）；② 出现内核之外的第二个能力消费方；③ 新增能力时因 HTTP 形状被迫在 `plugin` 里写路由判断。届时先落「能力面返回类型」设计文档，再按能力族分批迁移。
- **C6 `store/db.rs` 拆文件**。现状：迁移、查询与事务集中一个 1.1k 行文件。不做的原因：纯组织收益（零行为变化），且该文件是每次 schema 变更的冲突热点，无 schema 变更窗口里拆会产生无法用行为验证的大 diff。**重启条件**（任一）：① 下一次 schema 变更时顺带按「迁移 / 站点 / 运行环境 / 审计 / 设置」拆；② 两个以上会话同时改该文件；③ 单文件超过当前体量 1.5 倍。拆分时保持 `Db` 对外 API 不变。
- **C8（2026-11-19）`tasks.rs` / `runtime_catalog.rs` / `menu.rs` 拆分正名**。现状：`tasks.rs` 2.4k 行混仓储适配/tick 装配/内置执行体/scheduler 能力面/`serde_json::Value` 视图五件事；`runtime_catalog.rs` 2.0k 行装运行环境机型/apk 源/端口分配/PHP 扩展四类且与 `catalog/` 撞名（应属 sites 域，如 `sites/runtime_catalog.rs`）；`app/menu.rs` 1.9k 行把声明解析、排序、偏好写库与渲染模型混在一起；「store」一词三义（skiff-store / `catalog/store.rs` 缓存 / `commands/store.rs` 商店用例）。不做的原因同 C6：纯移动的大 diff 无行为验证面。**重启条件**（任一）：① 下次改 tasks 内置执行体或运行环境目录时顺带拆；② 三文件任一再涨 20%；③ 「store」一词多义引发实际混淆事故。拆分方案：`tasks/builtins.rs` + 视图挪 `views/`（改结构体）、runtime_catalog 归位改名、`catalog/store.rs`→`catalog/cache.rs`。

## 5. 未验证项

1. **审计写入的实际阻塞概率**：`Db` 的 50ms 等待告警是否在真实负载下触发未采集日志；已验证的是「写库失败必落 `audit-fallback.jsonl`」兜底路径。
2. **站点域行数占比**：按文件归属统计（`sites.rs` + `sites/` 12 文件约 12k 行），未做函数级归属；`runtime_catalog` 算「站点域」还是「运行环境目录」待 C8 命名定夺。
3. **`web` 之外是否还有死依赖**：只核到 `skiff-sched`（已删）；`cargo-machete` 类工具未跑，其余依赖按人工抽样判断。
