# 变更记录

> 上级索引：[ARCH_REVIEW.md](../ARCH_REVIEW.md) ｜ 拆自 `docs/archive/ARCH_REVIEW.md` 原 变更记录（2026-11-16 整理，正文未改）。

---

## 变更记录

| 日期 | 变更 |
|---|---|
| 2026-10-30 | 初版：按模块划分 / 职责边界 / 依赖关系 / 复用性四维审查，附 4 个结构性结论、8 项重复实现、4 个已致缺陷、6 步收敛路径 |
| 2026-10-31 | 追加 §9：P0 四项 + P1 五项的执行结果与验收证据；新增 3 个 crate（`wire`/`model`/`policy`），`plugin::trust` 收口 TOFU；测试 88 → 104 |
| 2026-10-31 | 第二轮复审 §10：上轮 9 项逐条重验全部属实（`Cidr` 下沉除外，属静默缩水）；实测基线 14,755 行 / 107 测试；新发现死依赖 31 条（含 `docker→skiff-store`）、承诺未兑现 2 项（CatchPanic、gzip 预压缩）；头部过期数字清理为初审快照标注 |
| 2026-10-31 | 「全部执行」§10.4：§10.3 行动清单 P0/P1/P2 全闭环——CatchPanic 接线、预压缩落地、`skiff-compose` 拆出、`Cidr` 下沉+非法拒启、28 条死依赖清理、`db.rs` 按聚合拆分、`resolve_container` 收口、6 项主干集成测试、`ManifestPerm` 改名与 `web-assets` 入 crate。测试 107 → 118，fmt/clippy `-D warnings` 干净 |
| 2026-10-31 | 第三轮 §11 代码优化：容器解析精确优先+歧义拒绝、WS Origin 缺失即拒、登录限流表有界、**插件上传 body 上限修复（>2MB 包此前无法安装）**、安全头去每请求解析、`Verb::parse` 由 `all()` 派生、tower-http 死特性收敛。测试 118 → 136 |
| 2026-10-31 | 第四轮 §12：§11.2 四项全部落地——exec 单连票据（30s/一次性/绑定会话与容器）、API Token scope 语义与强制（角色权限 ∩ scope）、按路由分组的请求超时（SSE/WS 排除，504 错误页）、`AppState` 下沉 `skiff-app`（kernel 不再依赖 `skiff-web`）。13 crate + xtask，测试 136 → 149 |
| 2026-11-01 | 第五轮 §13：结构收敛「全部执行」——角色矩阵统一到 `skiff_model::Permission`、路径代数统一到 `skiff_model::path`、能力面迁回 `crates/plugin`（`CapabilityHost` 解环）、值类型下沉 `model`、`AppState` 收口 + 登录用例统一、`core/kernel.rs` 拆为 `kernel/` 子模块并修正中间件顺序。根 212 项通过、plugins 29 项全绿 |
| 2026-11-02 | 第六轮 §14：全量复审（只审查不修码）。实测基线 14 crate + xtask、21,536 行 / 83 文件、215 项测试（212 通过 + 3 项既存 Windows 文件锁红，Linux CI 待复跑）、0 条死依赖。核心发现：**「授权 → 执行 → 留痕」没有唯一入口**，13 个判权点里 11 个不留越权痕迹，`restart` 有两份实现；装/卸包各写两条审计（域函数与调用方都写）；CLI 启停不停进程、不写审计；插件 UI 静态文件无认证且豁免入口前缀。新发现 10 项重复实现（均在动作编排层，判定/格式层仍单点），给出 P0 两项 / P1 三项 / P2 四项行动清单 |
| 2026-11-11 | 追加 §15 待办：async 路径上的同步 SQLite（`Db` 为 `std::sync::Mutex<Connection>`，涉及 web handler、菜单偏好、调度 tick、插件卸载、内核能力面；风险是阻塞 worker 而非死锁）。只登记不改码，`spawn_blocking` 迁移单独立项 |
| 2026-11-11 | §15 第二轮：完成 async 侧同步 SQLite 的清点与实测（release：常规语句 p99 ≤ 0.34 ms、WAL checkpoint 峰值 8–12 ms、审计裁剪 182 ms、`VACUUM INTO` 193 ms，另留 debug 对照；worker 仅 2 个），对比三种方案后选「`spawn_blocking` + 有界并发」，迁移会话中间件、内核能力面鉴权/快照、概览页与 tick 落盘，并修正 supervisor 在异步锁内写库 |
| 2026-11-11 | §15.5 归因备份争用：最坏等待等于备份窗口，属 `Db` 单连接互斥而非 `db_async` 信号量；备份/审计裁剪不经过 permit 池，单独限流无收益。剩余风险（维护窗口内整站 DB 排队、跨进程 busy 5 s）如实保留，维护操作改独立连接/分批删除列为 P1 遗留项 |
| 2026-11-12 | §15.5 第 0–2 步落地：维护车道收口 `store::maintenance`（`lane=short/maintenance` 日志、连接等待 ≥50 ms warn）；审计裁剪改 5000 行/批 + 批间让出；备份改维护专用第二连接（普通读写打开、`busy_timeout=1500ms`、只读快照、结束 PASSIVE checkpoint），保留 dest 完整性校验。release 实测：备份读者最坏等待 92.9 ms → 13.3–23.8 ms，裁剪 182 ms → 最坏 39.1–39.6 ms（窗口 646–676 ms）。rusqlite 在线备份与维护背压标志仍为遗留 P1 |
