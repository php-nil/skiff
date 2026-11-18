# 官方插件集规划：竞品对照、首批选型与目录落地（索引）

> 状态：**首批已落地（2026-11-01）**，现为 7 个官方插件 + testkit（`sources` 属后续新增，不在首批选型内，实现见 [`plugins/README.md`](../../plugins/README.md)）｜ 日期：2026-11-01
> 关联：[§1.4 分层边界](../sections/01-requirements.md)、[§3.3 原子能力](../sections/03-architecture.md)、[§4 扩展体系](../sections/04-plugin-package.md)、[§9 里程碑](../sections/09-milestones.md)
> 落地细节（目录约定、构建自检、装机授权方法）见 [`plugins/README.md`](../../plugins/README.md)，此处不重复。
> 数字纪律：竞品能力只做定性对照，未核实的具体数量不写；本仓库现状结论均附代码位置，便于复核。
> **本文件是索引**：正文拆在 [`PLUGIN_CATALOG/`](PLUGIN_CATALOG/)，§编号沿用原文。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–3 | [01-OVERVIEW.md](PLUGIN_CATALOG/01-OVERVIEW.md) | 一页结论、能力现状核对（选型硬约束）、竞品对照、官方插件清单 |
| §4 | [02-PLUGINS.md](PLUGIN_CATALOG/02-PLUGINS.md) | 6 个官方插件的实现与前置（权限、能力面、站点目录、依赖） |
| §5–7 | [03-LAYOUT_PLAN.md](PLUGIN_CATALOG/03-LAYOUT_PLAN.md) | 目录落地位置、实施顺序（含进度）、风险与未决 |
