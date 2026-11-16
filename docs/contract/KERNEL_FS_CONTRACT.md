# 内核文件系统原语契约（FS Contract · 索引）

> 状态：已落地（插件侧 `fs` 能力面 + 管理员「文件管理」第一步，2026-11-07；其余管理员操作面按 §3 逐步接入）
> 关联：[KERNEL_SCHEDULER_CONTRACT.md](KERNEL_SCHEDULER_CONTRACT.md)（任务落盘走本契约）、[SITE_RUNTIME_CONTRACT.md](SITE_RUNTIME_CONTRACT.md)（站点目录结构）
> 原则：**写入必须经内核**；插件 UI 可以留在外面，但所有宿主文件操作都受授权、审计、回收站与配额约束。路径解析唯一例外见 [KERNEL_FS_IMPL_NOTES.md](../impl/KERNEL_FS_IMPL_NOTES.md) §2.2.1。
> **本文件是索引**：正文拆在 [`KERNEL_FS_CONTRACT/`](KERNEL_FS_CONTRACT/)，§编号沿用原文。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–3 | [01-MODEL.md](KERNEL_FS_CONTRACT/01-MODEL.md) | 一页结论、背景与现状、授权模型、操作面 |
| §4–7 | [02-AUDIT_TRASH_QUOTA.md](KERNEL_FS_CONTRACT/02-AUDIT_TRASH_QUOTA.md) | 审计、回收站、可选写前快照、容量与配额 |
| §8–11 | [03-SITE_API.md](KERNEL_FS_CONTRACT/03-SITE_API.md) | 与站点的关系、页面与展示、API 形态、迁移与兼容 |
| §12–16 | [04-SCOPE_GAPS.md](KERNEL_FS_CONTRACT/04-SCOPE_GAPS.md) | 测试矩阵、决策点、P1/P2 范围、与现有实现的差距 |
