# 内核自身架构体检（七维 · 2026-11-25 第六轮复审）

> 状态：首轮批次 A/B/C 已收口（2026-11-13 ～ 15）；2026-11-19 全面复审，2026-11-21 第三轮（复用面收口 + 静态产物清理）、第四轮（模块划分 / 职责边界 / 依赖关系 / 复用性 / 过度设计）、第五轮（§3 内核文件写通道收口）、第六轮（11-24~25 新增代码复扫 + 五维全量复核）。本文件只保留结论、非问题清单、未闭环问题、搁置项与未验证项；已处理的清单随处理完即删（过程见 `docs/changes/`）。
> 范围：`crates/`（18 crate）、`xtask/`、`tools/skiff-server`；不含前端/模板/静态资源实现细节，不逐个点评官方插件；插件**体系**（manifest、签名、Supervisor、能力面划分）在范围内。
> 与既有文档的关系：[ARCH_REVIEW.md](../archive/ARCH_REVIEW.md) 是历史六轮审查（只读存档）；[KERNEL_UI_BOUNDARY_AUDIT.md](KERNEL_UI_BOUNDARY_AUDIT.md) 是内核↔UI 边界专项（web 内部重复与错误映射口径在那边）。行号易腐化，只作结论参考。
> 方法：逐 crate 读 `Cargo.toml` 与模块树、按关键词定位，关键结论核对源码；本机跑 `fmt`/`clippy --workspace`/`xtask check-channel` 编译级门禁，行为用例以 CI 为准。

## 1. 七维结论

基线事实（2026-11-21 第四轮复核）：无依赖环（`app`/`web`/`core` 单向）、全仓无 Cargo feature 开关；生产路径 `unwrap/expect` 均在不变量之后；全仓零 `TODO`/`FIXME`。

**总判断：分层与防错纪律良好；复用面的「同一判定多处拼装」已基本收口，内核文件写通道已收口（唯一例外与重启条件见 §4 C15）。**

**2026-11-21 第四轮复审增量结论**：五维全量复扫（18 crate + 官方插件体系），分层禁令与四项单一来源**全部复核通过**，未发现新的绕过点。本轮收口三处**功能性缺陷**：FTP 插件站点身份与内核 `SITE_UID` 冲突（1000 vs 201000，导致站点文档根属主与容器内 PHP 进程不一致）、`/ui/v1/files` 的新建目录/改名漏归还站点身份（与遗留 `/{p}/files` 行为分叉）、loopback 旁路采信客户端可伪造的审计身份头；商店清单验签改走 `skiff-sign` 唯一实现；清理死代码与空抽象（`app/attention.rs` 195 行、`sched::Executor`、`plugin::FirewallHost` + 内核 `KernelHost`、`runtime_catalog` 三个零调用函数、`web/handlers/sites/crud.rs`）净 −364 行；重复实现收口（暂存文件清理两份合一、运行环境机型清单双份合一、`web` 层三处本地时间格式化合一）。新增搁置项 C11–C14（`commands/file.rs` 转发层根因、`web` 两套文件处理器并存、`dto.rs` 请求/响应混装 + 全局导出、`site_page.rs` 手写 Schema）。

**2026-11-23 第五轮复核增量结论**：五维全量复扫（逐 crate 读 `Cargo.toml` + 通道/分层禁令源码核查），依赖关系、通道唯一、分层禁令与第四轮**完全一致，无新违规**；全仓 13 处 `#[allow(dead_code)]` 均有正当理由，无真死代码。收口 11-22 前端迁移批次引入的复用缺口：`reload_page()` / `or_dash()` 各三处重复定义收敛到 `schema_kit`（净 −24 行，补两例单测）；`site_page.rs` 已改用 `schema_kit` 构件（C14 部分改善）。新增体量观察：`routes.rs` 1128 行、`dto.rs` 1257 行、`main_flow.rs` 9106 行（C8/C9 阈值未触发但在涨）。同日收口 C12（两套文件处理器并存 → 单一路径 `/ui/v1/files/*`，见 §4 C12）。

**2026-11-25 第六轮复审增量结论**：重点是 11-24~25 新增代码（`runtime_index.rs` / `AuditCategory` 审计分类 / 菜单「任务审计」一级重组 / `skiff-server runtimes`·`sync` 补同步）+ 五维全量复核。结论：分层禁令与四项单一来源**全部复核通过，无新违规**；`app`/`core` 直写扫描命中全部位于 `#[cfg(test)]`（含 `runtime_catalog.rs` 内 `sh -n` 测试）；`RUNTIME_INDEX_KINDS` 单一来源在 `skiff-server` 的 `build.rs`，`sync`/`runtimes`/`check` 均导入；审计分类的命名空间清单单一来源在 `skiff_model::AuditCategory`（`store` 的 SQL 过滤与 web 下拉都从这里取，SQL 侧只持有命名空间表达式编码，非第二份清单）。收口一处归属偏差：`AuditCategory::label` 的中文文案原放在 `skiff-model`，违反 AGENTS.md §4.1「纯枚举文案留 `web/src/labels.rs`」，已迁到 `crate::labels::audit_category`（行为不变，`fmt` + 定向 clippy 通过）。过热文件行数复核均未触发 C8/C9 重启条件：`tasks.rs` 2692（+8%）、`runtime_catalog.rs` 2110（+5%）、`menu.rs` 1922（+1%）、`routes.rs` 1171（+3.8%）、`dto.rs` 1296（+3%）；`main_flow.rs` 8745 行较第五轮回落。


| 维度 | 结论 | 关键证据 |
| --- | --- | --- |
| 1 模块划分 | **基本合理，三个过热文件** | `app/src/tasks.rs` 2.5k 行、`runtime_catalog.rs` 2.0k 行、`menu.rs` 1.9k 行（2026-11-21 实测）；`sites.rs` 1.2k + `sites/` 11 文件域内拆分清晰；`store/src/db.rs` 连接+迁移+备份同文件（拆分搁置，见 §4 C6） |
| 2 职责边界 | **主线清楚，靠命名与注释维持** | `app` 自述「不含表现层」；`/ui/v1` 行装配读 `app/src/views/*`，模板与 DTO 各自映射；`web` 不再有 `state` 别名，`skiff_app::` 出现在调用点 |
| 3 依赖关系 | **无环、无 feature 分叉** | 依赖表与能力面分布表见 `docs/sections/08-engineering.md`、AGENTS.md；`app` 仅对 `tools/skiff-server` 有 dev-dependency（撤包原因端到端用例） |
| 4 复用性 | **主要重复已收口** | `run_host` → `skiff_host`；权限通配 → `Grant::permits`；路径代数 → `skiff_model::path`；签名协议 → `skiff-sign`（2026-11-21 第四轮补上 `catalog/verify.rs` 这处绕过）；插件生命周期 → `app/commands/plugin.rs`；列表分页 → `ui::paginate`；运行环境容器名 → `sites::runtime_container_name_for`；暂存文件清理 → `commands/incoming.rs`；本地时间格式化 → `web/util.rs::fmt_local_time` |
| 5 过度设计 | **轻微** | 单实现 trait 均有测试替身或依赖倒置理由；无空转发层（`web/src/menu.rs`、`crate::state` 别名均已删）。2026-11-21 第四轮清掉三处：`sched::Executor`（只有自己测试实现，文档还误称 core 有）、`plugin::FirewallHost`（`dispatch` 明确不使用该参数）、`runtime_catalog` 三个零调用函数 |
| 6 与 UI 解耦 | **读模型已下沉，展示属性仍在 app** | `app/src/views/*` 是 SSR/JSON 共用读模型，并预拼部分中文展示串（见 §4 C10）；JSON 侧 DTO 形状独立于模板 |
| 7 健壮性 | **整体强** | 审计改同步写库 + 失败落盘兜底；`systemd-run` 超时清单元；`systemctl restart docker` 20s 总预算；`nginx -t` 10s 上限 + Drop 守卫；loopback 旁路不采信客户端身份头（2026-11-21 第四轮修复） |

## 2. 非问题清单（下轮不要重复讨论）

| # | 候选点 | 为什么不是问题 |
| --- | --- | --- |
| N1 | 单实现 trait | `CapabilityHost` 是依赖倒置的必要抽象（`plugin` 不能依赖 `app`，否则成环）；`Clock`/`TaskStore`/`Transport`/`FtpHost` 都有测试替身；`RoleExt` 是扩展方法惯用法；`NginxControl` 的测试替身用于站点启停路径。**建议保留。**（2026-11-21 第四轮删掉两个例外：`sched::Executor` 只有自己的测试实现、`plugin::FirewallHost` 的参数在 `dispatch` 里明确不使用——它们不是「有测试替身」，而是「没有生产使用方」） |
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
| N18 | `plugins/common` 只 175 行有效代码（占插件代码 2.3%），共享率看着偏低 | 是**有意识的克制**：各插件的能力调用与策略判定刻意各写各的，「免得抽象层把『谁调了什么能力』藏起来（审计要看得见）」。合并要么新增共享 crate，要么反转层次（plugin→fs）。**不合并** |
| N19 | `plugins/ftp` 依赖 `skiff-plugin-common` 却直接用 `skiff_model` 常量 | `common` 已 re-export `skiff_model::SITE_UID/SITE_GID`，走 `common` 是为了不新增依赖边（`common` 本就依赖 `skiff-model`）。**保持现状** |
| N20 | `crates/model` 名叫 model 却含 `path.rs` 授权代数、`exec.rs` 二进制风险分类、`verb.rs` 动词→权限映射 | `AGENTS.md` §4.1 明确「路径授权代数只在 `skiff_model::path`」是有意设计；`model` 零 workspace 内部依赖，做纯类型层与纯策略层皆可，但它同时持有 `Permission` 矩阵与 `Verb` 表，二者必须同源。**不改定位** |
| N21 | `crates/model/src/scope.rs`（API token scope）与 `crates/fs/src/subject.rs`、`crates/plugin/src/fsops.rs`（路径 scope）同名 | 三个是不同域的概念（token 权限范围 vs 文件系统路径范围），前者已被 `web` 用来做 scope 闸门；命名撞车但语义无歧义风险（各自模块内限定使用）。**不改名** |
| N22 | `crates/store/src/db/` 14 个子模块的拆分依据不完全一致 | 9 个严格按表、5 个按业务域聚合从属表（`jobs`/`plugins`/`runtimes`/`sites`/`tasks`）、1 个按执行车道拆（`maintenance`，长持锁操作需要与短事务车道分开表达）。每处不一致都有明确领域理由。**不强行统一** |
| N23 | `crates/core/src/commands.rs` 含站点属主对齐等业务实现 | `core` 是可执行 crate（无 lib target），CLI 命令天然在此；`sites_chown_index` 的递归遍历 + 属主对齐规则与 `app/src/sites/layout.rs` 的口径一致，迁移收益低于跨 crate 暴露成本。**暂不迁移** |
| N24 | `crates/sdk/src/bin/skiff-plugin.rs` 的 `pack`/`stamp` 与 `crates/pkgfmt` 的打包能力重叠 | SDK CLI 面向插件作者（脚手架 + 一键打包），`pkgfmt` 是内核侧库；SDK 复用 `pkgfmt` 的实现而非复制。**不迁移** |
| N25 | `skiff-server` 的 `HttpUpstream`（`upstream.rs`，代理 + Bearer 挑战 + 30s 超时）与 `app` 商店目录客户端的 `UreqTransport`（`catalog/fetch.rs`，条件请求 + 自定义 CA + 3s 建连）各封装一份 ureq | 两处需求形状不同（上游版本抓取 vs 条件 GET 目录拉取），分属发布工具与内核装配层；合并要么新增共享 crate，要么让 `skiff-server` 反向依赖 `skiff-app`。与 N15 同类的纯成本项。**保留观察，不列为待办** |

## 3. 未闭环问题

**无。** 原「内核文件写通道外仍有生产写」已于 2026-11-21 收口：基线 **95 处 / 14 文件 → 2 处 / 1 文件**，剩下两处都属 C15（流式下载），不是待办。

- 迁移面：站点脚手架与文档根、证书成对写入与回滚、vhost 增删改与启停改名、默认站点投影、`nginx.rs` 主配置与其 `nginx -t` 候选、日志轮转、商店缓存（`catalog/store.rs`）、暂存包清扫与升级下载目录、`daemon.json` 目录、内置任务（数据库快照）备份目录、运行环境目录/PHP 配置/克隆复制。
- `AdminFs` 补齐原语：`write_kernel_mode`（按位落盘；非 Unix 视权限位为成功）、`write_if_changed_kernel`（内容一致不写、不刷 mtime）、`rename_kernel`（两段式，只用于目标本不该存在的开关动作）、`copy_kernel`（保留权限位）、`remove_tree_kernel`（含 Windows 占用重试）。
- 闸门升级：`xtask check-channel` 从「总数只减不增」改成**白名单 + 按文件计数**——未登记文件出现直写、登记文件处数上涨、或迁移后未下调登记，三种情况都判失败。

## 4. 搁置项（原因与重启条件）

- **C5 `plugin → axum` 解耦**。现状：能力实现（`crates/plugin`）直接以 axum 的请求/响应类型表达结果，HTTP 形状与命令编排混在一层。不做的原因：需先定义**传输无关**的能力面返回类型，会改动所有能力方法签名与两处调用方，当前没有第二种传输或第二个消费方，纯收益无法验证。**重启条件**（任一）：① 能力面接入第二种传输（SDUI/CLI/内部 RPC）；② 出现内核之外的第二个能力消费方；③ 新增能力时因 HTTP 形状被迫在 `plugin` 里写路由判断。届时先落「能力面返回类型」设计文档，再按能力族分批迁移。
- **C6 `store/db.rs` 拆文件**。现状：迁移、查询与事务集中一个 1.2k 行文件。不做的原因：纯组织收益（零行为变化），且该文件是每次 schema 变更的冲突热点，无 schema 变更窗口里拆会产生无法用行为验证的大 diff。**重启条件**（任一）：① 下一次 schema 变更时顺带按「迁移 / 站点 / 运行环境 / 审计 / 设置」拆；② 两个以上会话同时改该文件；③ 单文件超过当前体量 1.5 倍。拆分时保持 `Db` 对外 API 不变。
- **C8 `tasks.rs` / `runtime_catalog.rs` / `menu.rs` 拆分正名**。现状：`tasks.rs` 2.5k 行混仓储适配/tick 装配/内置执行体/scheduler 能力面/`serde_json::Value` 视图五件事；`runtime_catalog.rs` 2.0k 行装运行环境机型/apk 源/端口分配/PHP 扩展四类且与 `catalog/` 撞名（应属 sites 域，如 `sites/runtime_catalog.rs`）；`app/menu.rs` 1.9k 行把声明解析、排序、偏好写库与渲染模型混在一起；「store」一词三义（skiff-store / `catalog/store.rs` 缓存 / `commands/store.rs` 商店用例）。不做的原因同 C6：纯移动的大 diff 无行为验证面。**重启条件**（任一）：① 下次改 tasks 内置执行体或运行环境目录时顺带拆；② 三文件任一再涨 20%；③ 「store」一词多义引发实际混淆事故。拆分方案：`tasks/builtins.rs` + 视图挪 `views/`（改结构体）、runtime_catalog 归位改名、`catalog/store.rs`→`catalog/cache.rs`。（2026-11-23 补充观察：11-22 新增 `web/handlers/ui/routes.rs` 1128 行、`dto.rs` 1257 行，职责单一但偏大；若任一再涨 20% 提前触发拆分，同走「顺带拆」口径。2026-11-25 第六轮复核：tasks 2692 / runtime_catalog 2110 / menu 1922 / routes 1171 / dto 1296，均未触发 20% 阈值。）
- **C9（2026-11-21）`crates/web/tests/main_flow.rs` 单文件 8.0k 行 / 145 个用例（`#[test]` + `#[tokio::test]` 实测）**。现状：登录、会话、RBAC、插件安装、`/ui/v1` 各页、DTO 冻结全在一个文件里，靠 banner 注释分区。不做的原因：纯移动的大 diff，且用例结果只能由 CI 验证，本机仅能保证编得过。**重启条件**（任一）：① 下次大改用例时顺带按主题拆 `tests/main_flow/{auth,plugins,sites,ui_dto}.rs`；② 单文件超 12k 行；③ 两个以上会话同时改该文件。
- **C10（2026-11-21，规则已定）展示文案归属**。规则已写入 `AGENTS.md` §4.1：随视图结构变化的留 `app/src/views/*`，纯枚举留 `web/src/labels.rs`，带配色 class/视图语义的留 handler。现状三处（`views/*` 预拼、`labels.rs` 四类枚举、handler 状态文案）**维持不迁移**：统一任一方都要动 SSR+JSON 两个消费面，且没有行为收益可验证。**重启条件**（任一）：① 需要第二种语言 / 前端自渲染文案；② 同一文案两处改漏引发实际不一致；③ 接入第二个前端消费方。
- **C11（2026-11-21 新增）`app/commands/file.rs` 446 行纯转发层**。现状：22 个 DTO + 22 个一行函数（`WriteFile{user,path,content}` → `fs.write(user,path,content)`，零信息量），存在的唯一理由是 `actions.rs` 的 `ActionSpec.command` 是**字符串**（注释明说「不参与编译期绑定」），需要 `NAMES` 常量表来满足动作漂移测试。根因是动作表用字符串指向命令，而非编译期绑定。**重启条件**（任一）：① 动作表改为「类型 → 命令」的编译期绑定（`ActionSpec` 泛型化或按资源分派枚举）；② 该文件行数再涨 30%；③ 新增资源组时发现必须同时维护 `NAMES` 字符串表。届时删除该层并把 `actions.rs` 的漂移测试改为直接校验 `admin_fs` 方法名。
- **C12（2026-11-21 新增；2026-11-23 已收口）`web` 层两套文件处理器并存**。已下线遗留 `/{p}/files/*`（`handlers/files.rs`，22 路由）与 `/{p}/files/api/*` 308 兼容，全部 JSON 接口收口到 `/ui/v1/files/*`（`handlers/ui/files.rs`）：补齐 upload / download（流式）/ trash×4 / quotas×4 共 10 个 handler，统一 `{data,...}` 信封与 `fs_api_error` 语义错误码；前端 `budding-boot.js` 配额/上传/下载调用改走 `/ui/v1`，回收站动作 URL 改 `${api}`；形态门禁 `fs_capability` 随子路由迁移（挂在 `/ui/v1/files` 子路由）；`tree` 端点无消费方直接丢弃。页面 `/{p}/files*` 改由 SPA 壳页回落承接，权限改在 `/ui/v1` 数据端点判定（viewer 拿 200 壳页 + 403 数据）。`main_flow.rs` 同步更新断言并新增「受限形态 403」用例。
- **C13（2026-11-21 新增）`web/handlers/ui/dto.rs` 请求/响应混装 + `pub use dto::*` 全局导出**。现状：119 个类型里 57 个是请求体、62 个是响应 DTO，同文件混装；`ui/mod.rs:56` 的 `pub use dto::*` 把 119 个类型全部提升到 `handlers::ui` 作用域，配合 16 处 `#[allow(unused_imports)] use super::*;` 形成隐式全局命名空间（读者要反查才知道 `SiteItem` 来自 `dto.rs`）。**重启条件**（任一）：① DTO 形状变更频率明显上升、请求/响应 diff 互相干扰；② 新增消费方需要按契约子集导出。
- **C14（2026-11-21 新增）`web/handlers/ui/site_page.rs` 手写 SDUI Schema**。现状：askama 模板退场后，站点详情页的页面结构由 Rust 侧拼装（含按钮可见性判断 `overview_actions` 与状态驱动的按钮切换）。（2026-11-23：已改用 `schema_kit` 构件拼装，不再纯手写 `json!`，C14 部分改善；但单文件仍 914 行，可读性问题未完全解决。）逻辑仍在服务端（权限判定可信），但可读性/可重构性显著低于模板。**重启条件**（任一）：① 站点页要改版或加 tab；② 引入 SDUI Schema DSL builder（`sites/sdui/` 有相关规划时）。
- **C15（2026-11-21）`catalog/mod.rs` 流式下载的正文落盘与发布替换**。现状：包正文用 `std::fs::write` 整块落盘、再用 `std::fs::rename` 发布（`store_blob`），是内核文件写通道白名单里唯一的两处。不做的原因：`skiff-fs` 通道单次写上限 256 KiB（`MAX_REQUEST_BYTES`）且没有流式/分块接口，包可达数十 MB；发布又必须是一次性 `rename`（通道改名对已存在目标是先删后移，会给「已校验产物」多出一个目标不存在的窗口）。两处都在验签与摘要核对之后，失败即整包丢弃，不产生半成品。**重启条件**（任一）：① 通道提供流式/分块写接口；② 单包体积上限收紧到 256 KiB 以内；③ 下载链路改为「先落独立临时目录、再整树搬运」。

- **C16（2026-11-24 新增）`docs/sdui/SDUI_BUDDING_INTEGRATION/06-BRIDGE-PAGES-B.md` 超体积红线**。现状：31.8 KB / 244 行（行数合规、字节超 `AGENTS.md` §5.2 的 30 KB），内容是同主题 9 个批次的桥接契约（§5.12–5.20），单行极长（字段表与端点清单）。不做的原因：按 §5.2 拆成同名目录属纯文件搬迁的大 diff，跨文档引用（`01-SCOPE` / `08-DECISIONS` / 契约 §4.5）要同步改，与当轮行为改动无关且无法用测试验证。**重启条件**（任一）：① 下次改该文件正文结构时先拆（`06-BRIDGE-PAGES-B/` 存 §5.12–5.17 与 §5.18–5.20 两册，原文件降为索引）；② 字节再涨 20%（约 38 KB）；③ 新增 §5.21 及以后批次。

## 5. 未验证项

1. **审计写入的实际阻塞概率**：`Db` 的 50ms 等待告警是否在真实负载下触发未采集日志；已验证的是「写库失败必落 `audit-fallback.jsonl`」兜底路径。
2. **站点域行数占比**：按文件归属统计（`sites.rs` + `sites/` 11 文件），未做函数级归属；`runtime_catalog` 算「站点域」还是「运行环境目录」待 C8 命名定夺。
3. **`web` 之外是否还有死依赖**：只核到 `skiff-sched`（已删）；`cargo-machete` 类工具未跑，其余依赖按人工抽样判断。
4. **孤儿 `.gz` 清理与二进制体积**：清理动作本身已实测（`static/budding/` 只剩 1.8.7 一套），但清掉的 2.4 MB 原始字节对 release 二进制的实际减幅未在 clean 构建下测过（本机未出包）。

