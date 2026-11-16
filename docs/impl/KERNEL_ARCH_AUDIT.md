# 内核自身架构体检（七维 · 已收口）

> 状态：**已收口**——批次 A/B/C 全部落地，C1–C4 已实机验证，C5/C6 主动搁置（重启条件见 §4）。本文件只保留结论、非问题清单、未闭环问题、搁置项与未验证项；已处理的问题清单与批次执行记录已删（过程见 `docs/changes/2026-11-13.md` ～ `2026-11-15.md`）。
> 范围：**只从内核自身角度**看 `crates/` 与 `xtask/`（17 个 crate + 1 个工具），不含前端/模板/静态资源实现细节，不逐个点评官方插件；插件**体系**（manifest、签名、Supervisor、能力面划分）在范围内。
> 与既有文档的关系：[ARCH_REVIEW.md](../archive/ARCH_REVIEW.md) 是历史六轮审查（只读存档）；[KERNEL_UI_BOUNDARY_AUDIT.md](KERNEL_UI_BOUNDARY_AUDIT.md) 是内核↔UI 边界专项。行号易腐化，只作结论参考。
> 方法：逐 crate 读 `Cargo.toml` 与模块树、按关键词定位，关键结论核对源码；未运行任何 cargo 命令，行为相关结论均标注验证方式。

## 1. 七维结论

基线事实（2026-11-15 实测）：无依赖环（`app`/`web`/`core` 单向）、全仓无 Cargo feature 开关；生产路径 `unwrap/expect` 均在不变量之后。

**总判断：分层与防错纪律良好；发现的问题已随批次 A/B/C 收口，遗留见 §3。**

| 维度 | 结论 | 关键证据 |
| --- | --- | --- |
| 1 模块划分 | **基本合理，两个过热文件** | `app/src/sites.rs` 类型/布局/安装锁/进度混放；`store/src/db.rs` 连接+迁移+备份同文件（拆分搁置，见 §4 C6） |
| 2 职责边界 | **主线清楚，边界靠注释维持** | `app` 自述「不含表现层」，但 `views/sites.rs` 曾输出中文文案与 CSS class（已上移到 `web`） |
| 3 依赖关系 | **无环、无 feature 分叉** | 依赖表与能力面分布表已随批次 A5 更新（`docs/sections/08-engineering.md`、AGENTS.md） |
| 4 复用性 | **三类真实重复已收口** | `run_host` 收口为 `skiff_host`（`crates/host`，内核与插件共用一份）；权限通配匹配收口 `Grant::permits(_wildcard)`；`nginx -t` 执行通道统一 |
| 5 过度设计 | **轻微** | `web/src/menu.rs` 仍是纯 `pub use` 兼容层（§3 问题 2）；其余单实现 trait 均有测试替身或依赖倒置理由 |
| 6 与 UI 解耦 | **读模型已下沉，展示属性已上移** | `app/src/views/*` 是 SSR/JSON 共用读模型；JSON 侧仍残留展示模型依赖（§3 问题 1） |
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

## 3. 未闭环问题

1. **（值得改）JSON 仍依赖 SSR 展示模型**：`ui/runtimes.rs`、`ui/mod.rs` 仍以 `templates::RuntimeRow` / `NetworkRowView` / `PluginView` 为输入，删/改模板字段会同时改 API 输出。修法：聚合结果上移为与模板无关的读模型（数据上移、文案下移），模板与 DTO 各自映射——与 [KERNEL_UI_BOUNDARY_AUDIT.md](KERNEL_UI_BOUNDARY_AUDIT.md) 批次 1.2 合并做。
2. **（可选）`web/src/menu.rs` 是纯 `pub use` 兼容层**：菜单领域逻辑已下移 `skiff_app::menu`，模板消费点不变；可让调用方直接用 `skiff_app::menu` 后删除。
3. **（可选）`image_versions` 负缓存无上限、不随镜像删除清理**：`app` 侧 `Mutex<HashMap<String,(Option<String>,Instant)>>`，加 TTL/上限或镜像删除时清键。

## 4. 搁置项（原因与重启条件）

- **C5 `plugin → axum` 解耦**。现状：能力实现（`crates/plugin`）直接以 axum 的请求/响应类型表达结果，HTTP 形状与命令编排混在一层。不做的原因：需先定义**传输无关**的能力面返回类型，会改动所有能力方法签名与两处调用方，当前没有第二种传输或第二个消费方，纯收益无法验证。**重启条件**（任一）：① 能力面接入第二种传输（SDUI/CLI/内部 RPC）；② 出现内核之外的第二个能力消费方；③ 新增能力时因 HTTP 形状被迫在 `plugin` 里写路由判断。届时先落「能力面返回类型」设计文档，再按能力族分批迁移。
- **C6 `store/db.rs` 拆文件**。现状：迁移、查询与事务集中一个大文件。不做的原因：纯组织收益（零行为变化），且该文件是每次 schema 变更的冲突热点，无 schema 变更窗口里拆会产生无法用行为验证的大 diff。**重启条件**（任一）：① 下一次 schema 变更时顺带按「迁移 / 站点 / 运行环境 / 审计 / 设置」拆；② 两个以上会话同时改该文件；③ 单文件超过当前体量 1.5 倍。拆分时保持 `Db` 对外 API 不变。

## 5. 未验证项

1. **审计写入的实际阻塞概率**：`Db` 的 50ms 等待告警是否在真实负载下触发未采集日志；已验证的是「写库失败必落 `audit-fallback.jsonl`」兜底路径。
2. **站点域行数占比**：按文件归属统计（6 个文件约 10.5k 行），未做函数级归属；`runtime_catalog` 算「站点域」还是「运行环境目录」待后续命名定夺。
3. **`web` 之外是否还有死依赖**：只核到 `skiff-sched`（已删）；`cargo-machete` 类工具未跑，其余依赖按人工抽样判断。
