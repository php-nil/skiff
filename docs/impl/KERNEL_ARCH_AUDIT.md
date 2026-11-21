# 内核自身架构体检（七维 · 2026-11-21 复审）

> 状态：首轮批次 A/B/C 已收口（2026-11-13 ～ 15）；2026-11-19 全面复审，2026-11-21 第三次复审（模块划分 / 职责边界 / 依赖关系 / 复用性 / 过度设计，排除 `plugins/` 插件本体）。本文件只保留结论、非问题清单、未闭环问题、搁置项与未验证项；已处理的清单随处理完即删（过程见 `docs/changes/`）。
> 范围：`crates/`（18 crate）、`xtask/`、`tools/skiff-server`；不含前端/模板/静态资源实现细节，不逐个点评官方插件；插件**体系**（manifest、签名、Supervisor、能力面划分）在范围内。
> 与既有文档的关系：[ARCH_REVIEW.md](../archive/ARCH_REVIEW.md) 是历史六轮审查（只读存档）；[KERNEL_UI_BOUNDARY_AUDIT.md](KERNEL_UI_BOUNDARY_AUDIT.md) 是内核↔UI 边界专项（web 内部重复与错误映射口径在那边）。行号易腐化，只作结论参考。
> 方法：逐 crate 读 `Cargo.toml` 与模块树、按关键词定位，关键结论核对源码；本机跑 `fmt`/`clippy --workspace`/`xtask check-channel` 编译级门禁，行为用例以 CI 为准。

## 1. 七维结论

基线事实（2026-11-21 复核）：无依赖环（`app`/`web`/`core` 单向）、全仓无 Cargo feature 开关；生产路径 `unwrap/expect` 均在不变量之后；全仓零 `TODO`/`FIXME`。

**总判断：分层与防错纪律良好；复用面的「同一判定多处拼装」已基本收口，遗留见 §3。**

**2026-11-21 复审增量结论**：依赖方向零违规（`policy`/`pkgfmt`/`wire` 隔离 `store`/`docker`、`model` 零 I/O、`sched` 不反依赖 `app`、`host` 无授权判定）；四项单一来源（wire 协议常量、TOFU 判定、`Docker::execute` 分发、RBAC 矩阵）核对通过。本轮收口：`/ui/v1` 的容器与运行环境列表改走统一 `paginate`（原三份手写 keyset 各写一遍）；运行环境容器名收口 `sites::runtime_container_name_for`（`/ui/v1/overview` 原按 `KIND_PHP` 自判「已接入」，对 Node/Rust 环境恒判「未安装」——计数口径已与读模型一致）；`web` 删除 `crate::state` 路径别名（处理器直接写 `skiff_app::*`，层归属在调用点可见）；设置页三处写入口（主题、公钥导入、撤销）改走 `authz`，不再内联 `user.can` 拦截；公钥与 TOTP 审计补真实来源 ip；清掉 1.8.1 期静态产物（`rust_embed` 按目录嵌入，孤儿会进二进制），`web/build.rs` 补孤儿 `.gz` 清理，并把 budding 版本一致性用例扩到主包/图标/启动脚本。

| 维度 | 结论 | 关键证据 |
| --- | --- | --- |
| 1 模块划分 | **基本合理，三个过热文件** | `app/src/tasks.rs` 2.5k 行、`runtime_catalog.rs` 2.0k 行、`menu.rs` 1.9k 行（2026-11-21 实测）；`sites.rs` 1.2k + `sites/` 11 文件域内拆分清晰；`store/src/db.rs` 连接+迁移+备份同文件（拆分搁置，见 §4 C6） |
| 2 职责边界 | **主线清楚，靠命名与注释维持** | `app` 自述「不含表现层」；`/ui/v1` 行装配读 `app/src/views/*`，模板与 DTO 各自映射；`web` 不再有 `state` 别名，`skiff_app::` 出现在调用点 |
| 3 依赖关系 | **无环、无 feature 分叉** | 依赖表与能力面分布表见 `docs/sections/08-engineering.md`、AGENTS.md；`app` 仅对 `tools/skiff-server` 有 dev-dependency（撤包原因端到端用例） |
| 4 复用性 | **主要重复已收口** | `run_host` → `skiff_host`；权限通配 → `Grant::permits`；路径代数 → `skiff_model::path`；签名协议 → `skiff-sign`；插件生命周期 → `app/commands/plugin.rs`；列表分页 → `ui::paginate`；运行环境容器名 → `sites::runtime_container_name_for` |
| 5 过度设计 | **轻微** | 单实现 trait 均有测试替身或依赖倒置理由；无空转发层（`web/src/menu.rs`、`crate::state` 别名均已删） |
| 6 与 UI 解耦 | **读模型已下沉，展示属性仍在 app** | `app/src/views/*` 是 SSR/JSON 共用读模型，并预拼部分中文展示串（见 §4 C10）；JSON 侧 DTO 形状独立于模板 |
| 7 健壮性 | **整体强** | 审计改同步写库 + 失败落盘兜底；`systemd-run` 超时清单元；`systemctl restart docker` 20s 总预算；`nginx -t` 10s 上限 + Drop 守卫 |

## 2. 非问题清单（下轮不要重复讨论）

| # | 候选点 | 为什么不是问题 |
| --- | --- | --- |
| N1 | 14 个 trait 里多数只有一个生产实现 | `CapabilityHost` 是依赖倒置的必要抽象（`plugin` 不能依赖 `app`，否则成环）；`Clock`/`Executor`/`TaskStore`/`Transport`/`FtpHost`/`FirewallHost` 都有测试替身；`RoleExt` 是扩展方法惯用法。**建议全部保留。** |
| N2 | 重复提交/幂等 | `JobService::submit` 的同目标唯一由 SQLite 部分唯一索引兜底并映射 `JobError::Busy`，有测试用例，不靠调用方自觉 |
| N3 | 生产代码大量 `unwrap` | 排除 `#[cfg(test)]` 后全部在不变量之后；`store`/`plugin`/`web` 热路径没有裸 `unwrap` |
| N4 | `cfg(unix)` / `cfg(target_os="linux")` 分叉风险 | 全仓无 Cargo feature；平台分支只有非 unix stub 与 linux-only 宿主能力两类，Windows/Linux CI 双向覆盖 |
| N5 | 重启后状态一致性 | 作业中断结算、插件停服、安装锁三条链路都有收尾；剩余是「业务半完成态」，契约已声明需人工核对，不是缺陷 |
| N6 | 请求超时口径 | 非流式 30 秒，SSE/WS 不挂；Docker 客户端默认 30 秒、探活 3 秒。分层正确 |
| N7 | `open_dir_nofollow` 是 `#[allow(dead_code)]` | 技术储备（作者注释明确），登记为「已实现未接入」；对外不得声明 TOCTOU 已消除 |
| N8 | `views/runtimes.rs::list` 是一行包装 | 承载「取数失败按空列表降级」的明确口径，是读模型入口，不是空转发 |
| N9 | `skiff_store::NewSite` 与 `skiff_app::sites::NewSite` 同名 | 层次不同（仓储插入行 vs 建站输入），转换在一处完成。**不改名** |
| N10 | `plugin::ipc::MAX_FRAME` 再导出 `skiff_wire::MAX_FRAME` | 有注释说明唯一来源是 wire，且是常量不是逻辑 |
| N11 | `web` 的 `tempfile` 在 `[dependencies]` | 上传暂存目录使用，非误放 |
| N12 | `app/src/db_async.rs` 自制异步桥 | `Db` 是同步 `Mutex<rusqlite::Connection>`，裸 `spawn_blocking` 无界、直接跑 worker 会卡 HTTP+kernel.sock；有界并发（4）+ 排队快败 + 嵌套自锁拒绝，全仓约 20 个真实调用点。**保留** |
| N13 | `app` 38K 行「装配层」体量大 | `AppState`+用例层定位与 08-engineering 一致，域内拆分（sites/tasks/catalog/commands）清晰；过热单文件问题走 §4 C8，不改定位 |
| N14 | `image_versions` 负缓存「无上限」 | 键是 Docker `image_id`（不可变），条目数 = 宿主用过的不同镜像数（运行环境镜像集很小）；负缓存命中窗口 60 秒；为删镜像加跨模块清键属过度接线。**不处理** |
| N15 | `sha256_hex`（identity/fs）、`Scope` 枚举（fs/plugin）、io 错误分类（fs/plugin）各两份 | 两侧都是无相互依赖的支撑层：合并要么新增共享 crate，要么反转层次（plugin→fs）。属于「抽公共件」的纯成本项，无行为收益。**保留观察，不列为待办** |
| N16 | `LoginBody` 两份（`handlers/api.rs` 3 字段 / `handlers/ui/dto.rs` 5 字段） | 两个契约不同：`/api/v1` 无 CSRF 与回跳，`/ui/v1` 必须带；合并会让其中一个接受多余字段。**不合并** |
| N17 | `web/src/util.rs` 只有 6 个纯函数 | 都是无状态转换（cookie 解析、转义、容量格式化），不是杂物间；页面拼接类逻辑不进这里 |

## 3. 未闭环问题

1. **（值得改，2026-11-19 起，进行中）内核文件写通道外仍有生产写**：当前基线 **96 处 / 13 个文件**（`xtask check-channel` 口径，只减不增）。已迁：`docker_daemon.rs`、`sites/crud.rs` 的 vhost / 默认站点 / 站点目录、`sites/runtime.rs` 的 Rust/Node 脚手架与 `entry.sh`、`panel_settings.rs` 的 `config.toml`（经 `AdminFs::write_kernel`，含启动期写回）。剩余按两类看：**内核文件**（`catalog/` 缓存、插件/升级清单等）继续迁 `write_kernel`；**宿主 nginx/php 配置**（`sites/crud.rs` 证书与克隆、`nginx.rs` 主配置候选文件）本就应直写，但必须在命令层留业务审计（`nginx.config.save` / `nginx.params.save` 已是此口径）。`AdminFs` 目前缺 `rename_kernel`，仅在确需「候选文件 → test → rename」全链路进审计时再补，不作为默认迁移目标。

## 4. 搁置项（原因与重启条件）

- **C5 `plugin → axum` 解耦**。现状：能力实现（`crates/plugin`）直接以 axum 的请求/响应类型表达结果，HTTP 形状与命令编排混在一层。不做的原因：需先定义**传输无关**的能力面返回类型，会改动所有能力方法签名与两处调用方，当前没有第二种传输或第二个消费方，纯收益无法验证。**重启条件**（任一）：① 能力面接入第二种传输（SDUI/CLI/内部 RPC）；② 出现内核之外的第二个能力消费方；③ 新增能力时因 HTTP 形状被迫在 `plugin` 里写路由判断。届时先落「能力面返回类型」设计文档，再按能力族分批迁移。
- **C6 `store/db.rs` 拆文件**。现状：迁移、查询与事务集中一个 1.2k 行文件。不做的原因：纯组织收益（零行为变化），且该文件是每次 schema 变更的冲突热点，无 schema 变更窗口里拆会产生无法用行为验证的大 diff。**重启条件**（任一）：① 下一次 schema 变更时顺带按「迁移 / 站点 / 运行环境 / 审计 / 设置」拆；② 两个以上会话同时改该文件；③ 单文件超过当前体量 1.5 倍。拆分时保持 `Db` 对外 API 不变。
- **C8 `tasks.rs` / `runtime_catalog.rs` / `menu.rs` 拆分正名**。现状：`tasks.rs` 2.5k 行混仓储适配/tick 装配/内置执行体/scheduler 能力面/`serde_json::Value` 视图五件事；`runtime_catalog.rs` 2.0k 行装运行环境机型/apk 源/端口分配/PHP 扩展四类且与 `catalog/` 撞名（应属 sites 域，如 `sites/runtime_catalog.rs`）；`app/menu.rs` 1.9k 行把声明解析、排序、偏好写库与渲染模型混在一起；「store」一词三义（skiff-store / `catalog/store.rs` 缓存 / `commands/store.rs` 商店用例）。不做的原因同 C6：纯移动的大 diff 无行为验证面。**重启条件**（任一）：① 下次改 tasks 内置执行体或运行环境目录时顺带拆；② 三文件任一再涨 20%；③ 「store」一词多义引发实际混淆事故。拆分方案：`tasks/builtins.rs` + 视图挪 `views/`（改结构体）、runtime_catalog 归位改名、`catalog/store.rs`→`catalog/cache.rs`。
- **C9（2026-11-21）`crates/web/tests/main_flow.rs` 单文件 8.0k 行 / 145 个用例（`#[test]` + `#[tokio::test]` 实测）**。现状：登录、会话、RBAC、插件安装、`/ui/v1` 各页、DTO 冻结全在一个文件里，靠 banner 注释分区。不做的原因：纯移动的大 diff，且用例结果只能由 CI 验证，本机仅能保证编得过。**重启条件**（任一）：① 下次大改用例时顺带按主题拆 `tests/main_flow/{auth,plugins,sites,ui_dto}.rs`；② 单文件超 12k 行；③ 两个以上会话同时改该文件。
- **C10（2026-11-21，规则已定）展示文案归属**。规则已写入 `AGENTS.md` §4.1：随视图结构变化的留 `app/src/views/*`，纯枚举留 `web/src/labels.rs`，带配色 class/视图语义的留 handler。现状三处（`views/*` 预拼、`labels.rs` 四类枚举、handler 状态文案）**维持不迁移**：统一任一方都要动 SSR+JSON 两个消费面，且没有行为收益可验证。**重启条件**（任一）：① 需要第二种语言 / 前端自渲染文案；② 同一文案两处改漏引发实际不一致；③ 接入第二个前端消费方。

## 5. 未验证项

1. **审计写入的实际阻塞概率**：`Db` 的 50ms 等待告警是否在真实负载下触发未采集日志；已验证的是「写库失败必落 `audit-fallback.jsonl`」兜底路径。
2. **站点域行数占比**：按文件归属统计（`sites.rs` + `sites/` 11 文件），未做函数级归属；`runtime_catalog` 算「站点域」还是「运行环境目录」待 C8 命名定夺。
3. **`web` 之外是否还有死依赖**：只核到 `skiff-sched`（已删）；`cargo-machete` 类工具未跑，其余依赖按人工抽样判断。
4. **孤儿 `.gz` 清理与二进制体积**：清理动作本身已实测（`static/budding/` 只剩 1.8.3 一套），但清掉的 2.4 MB 原始字节对 release 二进制的实际减幅未在 clean 构建下测过（本机未出包）。
