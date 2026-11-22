# SDUI / budding 索引

> 正文在 [`sdui/`](sdui/)。

## 现状口径（共性）

- **P0/P1 已落地，P2 未实施**：budding 壳页 + `/ui/v1` 数据面已接入（当前基线 `budding-1.8.5`，含 `redirect` 组件与 `diagnose()`）；除 [SDUI_HANDBOOK.md](sdui/SDUI_HANDBOOK.md) 外均为规则稿/台账，上线形态以 [PLUGIN_UI_CONTRACT.md](contract/PLUGIN_UI_CONTRACT.md) 与 URL 契约为准。
- **分工**：接入边界、DTO 字段与错误码见 [SDUI_BUDDING_INTEGRATION.md](sdui/SDUI_BUDDING_INTEGRATION.md)；组件/动作用法与宿主采用约定见 [BUDDING_INTERACTION_CAPABILITIES.md](sdui/BUDDING_INTERACTION_CAPABILITIES.md)；上游缺口台账**只记未关闭项**，见 [BUDDING_UPSTREAM_BACKLOG.md](sdui/BUDDING_UPSTREAM_BACKLOG.md)。
- 全局口径决策（文案 key、时间 RFC3339、分页、CSP 等，D8–D13）钉在集成稿 §8 决策记录，不散落。

## 子文档

| 子文档 | 管什么 |
| --- | --- |
| [SDUI_HANDBOOK.md](sdui/SDUI_HANDBOOK.md) | SDUI 资源 / 动作 / 迁移阶段（设计稿，P0/P1 已落地；分 3 册） |
| [SDUI_BUDDING_INTEGRATION.md](sdui/SDUI_BUDDING_INTEGRATION.md) | 接入边界、DTO 字段与错误码（分 8 册） |
| [BUDDING_INTERACTION_CAPABILITIES.md](sdui/BUDDING_INTERACTION_CAPABILITIES.md) | 组件 / 动作用法与宿主采用约定 |
| [BUDDING_UPSTREAM_BACKLOG.md](sdui/BUDDING_UPSTREAM_BACKLOG.md) | 向上游提能力缺口的台账（只记未关闭项） |
