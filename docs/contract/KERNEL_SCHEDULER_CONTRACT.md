# 内核定时任务契约（Scheduler Contract · 索引）

> 状态：已落地（2026-11-06）；任务表迁移最终编号为 schema v10，当前全局 v21。文中 v8 为设计初稿编号，以 `docs/changes/2026-11-06.md` 与代码为准。
> 关联：[KERNEL_FS_CONTRACT.md](KERNEL_FS_CONTRACT.md)（任务落盘统一走 fs 原语）、[PLUGIN_CATALOG.md](../plugin/PLUGIN_CATALOG.md)（cron/monitor/guard 分工）
> 原则：调度是内核原语，插件只注册任务与提供执行体；插件不再自带计时器。
> **本文件是索引**：正文拆在 [`KERNEL_SCHEDULER_CONTRACT/`](KERNEL_SCHEDULER_CONTRACT/)，§编号沿用原文。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–5 | [01-MODEL.md](KERNEL_SCHEDULER_CONTRACT/01-MODEL.md) | 一页结论、背景与目标、术语、任务模型、缓冲区模型（核心）、执行语义 |
| §6–9 | [02-WAKE_PERSIST.md](KERNEL_SCHEDULER_CONTRACT/02-WAKE_PERSIST.md) | 插件任务唤醒链路、持久化与保留、内置 kind 清单、与插件分工 |
| §10–17 | [03-API_SCOPE.md](KERNEL_SCHEDULER_CONTRACT/03-API_SCOPE.md) | API 形态、权限与页面、迁移兼容、测试矩阵与失败注入、决策点、范围、实现差距 |
