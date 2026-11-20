# budding 接入规则（SDUI 前端渲染层 · 索引）

> 状态：**接入规则稿（P0 定案 2026-11-12）；前端已按本规则切换**——2026-11-13 壳页原生化（不再用 iframe 承载插件功能页），当前产物 `budding-1.8.3`（2026-11-20 同步上游 `1120d3`：`admin` 去外框、数据族配色走主题变量、`page` 的 `init`/`refresh`、`link`/`image` 点击触点、`instance.diagnose()`）。
> 上游事实来源：budding 仓库只读盘点（2026-11-12）；内核现状来源：`crates/web`、`crates/plugin` 与本仓库现有契约文档。
> 相关文档：[SDUI_HANDBOOK.md](SDUI_HANDBOOK.md)（框架无关的适配手册）、[URL_CONTRACT.md](../contract/URL_CONTRACT.md)、[MENU_CONTRACT.md](../contract/MENU_CONTRACT.md)、[PLUGIN_UI_CONTRACT.md](../contract/PLUGIN_UI_CONTRACT.md)。
> 分工：本文只维护 budding 相关的具体事实、契约形状与缺口；框架无关的分层与迁移阶段只在 SDUI_HANDBOOK 维护，本文引用不复述。
> **本文件是索引**：正文按 §编号拆在 [`SDUI_BUDDING_INTEGRATION/`](SDUI_BUDDING_INTEGRATION/)，编号沿用拆分前原文。

## 一页结论（摘要）

- **budding 只做渲染层**：路由、鉴权、审计、CSRF 仍在内核；壳页只负责把 schema 渲染成页面并回调宿主动作。
- **产物是版本化静态件**：`/assets/budding-<ver>.min.{js,css}` 走 `immutable` 长缓存，稳定别名 `budding.min.js` / `budding.css` 走 `no-cache`。
- **信封与动作是契约**：`{data, meta}` JSON 信封 + 动作元数据（`confirm` / `danger` / `refresh`）由内核下发，前端不自由拼 URL。
- **缺口收口在台账**：向上游提的能力缺口记 [`BUDDING_UPSTREAM_BACKLOG.md`](BUDDING_UPSTREAM_BACKLOG.md)，本文只列「还差什么」。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–2 | [01-SCOPE.md](SDUI_BUDDING_INTEGRATION/01-SCOPE.md) | 一页结论、定位与边界、产物与接入形态（离线分发、版本化资产） |
| §3 | [02-JSON_CONTRACT.md](SDUI_BUDDING_INTEGRATION/02-JSON_CONTRACT.md) | JSON 信封、DTO 字段与错误码 |
| §4 | [03-CONTRACT_RELATION.md](SDUI_BUDDING_INTEGRATION/03-CONTRACT_RELATION.md) | 与 URL / UI / 菜单 / 插件页契约的关系与补条款 |
| §5.1–5.6 | [04-BRIDGE-AUTH.md](SDUI_BUDDING_INTEGRATION/04-BRIDGE-AUTH.md) | 登录与 CSRF、401/403、ACL 快照与重新 embed、审计、表单回填、长任务与终端 |
| §5.7–5.11 | [05-BRIDGE-PAGES-A.md](SDUI_BUDDING_INTEGRATION/05-BRIDGE-PAGES-A.md) | 概览、站点与运行环境、容器、镜像/网络/卷、Compose |
| §5.12–5.20 | [06-BRIDGE-PAGES-B.md](SDUI_BUDDING_INTEGRATION/06-BRIDGE-PAGES-B.md) | 任务与作业、审计、设置、商店、插件、文件、代码/日志查看器、容器终端、日志入口 |
| §6–7 | [07-GAPS_RISKS.md](SDUI_BUDDING_INTEGRATION/07-GAPS_RISKS.md) | 能力缺口与建议、未决项与风险 |
| §8 | [08-DECISIONS.md](SDUI_BUDDING_INTEGRATION/08-DECISIONS.md) | 决策记录（D1–Dn） |
