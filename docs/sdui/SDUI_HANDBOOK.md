# SDUI 内核适配手册（JSON Schema 驱动的服务端 UI · 索引）

> 状态：**设计稿；P0/P1 已落地（2026-11-10），P2 未实施**（最近核对 2026-11-13）。本文描述目标、契约与推进方式；落地范围以 §P0/P1 状态为准。
> 前置：SDUI 框架由外部提供，其组件集、事件模型与布局能力尚未确定（见 §6）。
> 关系：插件页路由与 iframe 边界见 [PLUGIN_UI_CONTRACT.md](../contract/PLUGIN_UI_CONTRACT.md)，菜单声明模型见 [MENU_CONTRACT.md](../contract/MENU_CONTRACT.md)，分层红线见 [AGENTS.md](../../AGENTS.md) §4.1 与 [03-architecture.md](../sections/03-architecture.md) §3.1。本文不重复上述结论，只回答「内核要交出什么、保留什么、怎么迁」。
> **本文件是索引**：正文拆在 [`SDUI_HANDBOOK/`](SDUI_HANDBOOK/)，§编号沿用原文。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–1 | [01-COUPLING.md](SDUI_HANDBOOK/01-COUPLING.md) | 一页结论、现状耦合清单（渲染 / 数据 / 动作三处的具体耦合点） |
| §2–3 | [02-LAYERS.md](SDUI_HANDBOOK/02-LAYERS.md) | 目标分层、迁移路径（分阶段切换与回退） |
| §4–6 + 附录 | [03-MANUAL.md](SDUI_HANDBOOK/03-MANUAL.md) | 立即可以做的优化、适配手册、风险与待定、代表页面对照、与现有文档的分工 |
