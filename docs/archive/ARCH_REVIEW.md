# 架构与组织审查（MVP 代码 · 索引）

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：代码架构审查导航 ｜ 部署与验收：[DEPLOY_TEST.md](../test/DEPLOY_TEST.md)
> **只读存档**：结论按当时基线成立，行号、行数与测试数均已随改动腐化，读代码以符号名为准。与现行文档冲突时一律以现行文档为准。
> 正文在 [`ARCH_REVIEW/`](ARCH_REVIEW/)，按轮次分册，**§编号沿用拆分前原文**。

## 一页结论（摘要）

- 六轮审查都围绕同一组维度：**模块划分 / 职责边界 / 依赖关系 / 复用性**；每轮都附 `文件:行号`，推断项显式标「推断」。
- 结构性缺陷的根因集中在两处：**判定逻辑散落在 handler**（后收敛到 `skiff-policy`）与**能力面实现散落**（后收敛到 `crates/plugin` + `skiff-model` 值类型）。
- P0–P2 行动清单已全部执行；第六轮（§14）是最后一次全量复审，其后只剩 §15 的 async 同步 SQLite 一条专题。
- **当前基线不看本文**：crate 数与依赖方向见 [§8 工程结构](../sections/08-engineering.md)，实测见 [TEST_ENV.md](../test/TEST_ENV.md)。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–9 | [01-REVIEW.md](ARCH_REVIEW/01-REVIEW.md) | 初审（2026-10-30）：现状地图、依赖、逐 crate 边界、重复实现、真实缺陷、目标结构、行动清单、P0/P1 执行结果 |
| §10 | [02-ROUND2.md](ARCH_REVIEW/02-ROUND2.md) | 第二轮复审：模块划分 / 职责边界 / 依赖关系 / 复用性 |
| §11–13 | [03-ROUND3-5.md](ARCH_REVIEW/03-ROUND3-5.md) | 第三轮代码优化、第四轮补做项、第五轮结构收敛 |
| §14 | [04-ROUND6.md](ARCH_REVIEW/04-ROUND6.md) | 第六轮全量复审（含 §14.0 当轮实测基线） |
| §15 | [05-ASYNC-SQLITE.md](ARCH_REVIEW/05-ASYNC-SQLITE.md) | async 路径上的同步 SQLite：登记、实测与迁移 |
| 变更记录 | [06-CHANGELOG.md](ARCH_REVIEW/06-CHANGELOG.md) | 本文的变更记录 |
