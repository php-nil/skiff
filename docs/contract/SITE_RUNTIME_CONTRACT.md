# Skiff 网站运行环境与建站契约（SITE_RUNTIME_CONTRACT · 索引）

> 状态：**设计契约 v1（2026-11-05）；P1 实现补充见 §11（2026-11-06）；Node.js 接入见 §4.3（2026-11-09）**。
> 2026-11-09：Node.js 已在内核接入（运行环境目录、安装/启停/重建、站点绑定与反代）；Go 仍未实现，已从 `runtime_catalog::RUNTIME_KINDS` 与新建弹窗摘除。
> 基线：`crates/store/src/db.rs`（schema v21）、`crates/app/src/sites.rs`、`crates/app/src/sites/network.rs`、`crates/app/src/runtime_catalog.rs`、`crates/web/src/handlers/sites.rs`、`crates/web/templates/site_env.html`、`docs/contract/MENU_CONTRACT.md`。
> 2026-11-15：运行环境改为**专用 bridge + 内核静态容器地址**，宿主不再出现运行环境端口；站点 vhost 只反代到容器地址；对外发布是显式开关（`site_runtimes.publish_port`，默认关闭）。
> 可点预览：[`docs/archive/ui/site-runtime-preview.html`](../archive/ui/site-runtime-preview.html)。
> **本文件是索引**：正文按 §编号拆在 [`SITE_RUNTIME_CONTRACT/`](SITE_RUNTIME_CONTRACT/)，编号沿用拆分前原文。

## 五条结论（摘要）

1. **一站一目录**：`/www/sites/<域名>/{index,log,ssl,conf}` 承载文档根、日志、证书与 vhost；备份单目录 = 备份整站。
2. **运行环境是独立实体**：站点绑定运行环境，运行环境管容器与地址；PHP / Node.js / Rust 走同一套生命周期，不再有 per-site 进程。
3. **网络专用化**：站点容器只接 `skiff-sites`（首选 `172.18.0.0/16`，占用时按候选序列回退），地址落 `site_runtimes.container_ip`，vhost 只反代 `<container_ip>:<port>`。
4. **对外发布是开关**：默认不发布宿主端口；`publish_port` 显式填写才发布。
5. **健康与维护态**：守护每 3 秒巡检，容器退出或地址不可达时 vhost 切 503 维护页，恢复后自动写回。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–2 | [01-MODEL.md](SITE_RUNTIME_CONTRACT/01-MODEL.md) | 五条结论、领域模型、数据与目录（含网段回退与静态池） |
| §3–5 | [02-CREATE_PAGE.md](SITE_RUNTIME_CONTRACT/02-CREATE_PAGE.md) | 建站流程差异（静态 / PHP / Node.js）、运行环境管理页、与 Docker 容器页边界 |
| §6–7 | [03-ROUTES_SCOPE.md](SITE_RUNTIME_CONTRACT/03-ROUTES_SCOPE.md) | 路由、权限与审计；P1 / P2 范围 |
| §8–10 | [04-VERIFY_IMPACT.md](SITE_RUNTIME_CONTRACT/04-VERIFY_IMPACT.md) | 测试矩阵、变更影响面、需要拍板的决策、状态文案与空态 |
| §11–13 | [05-P1-NOTES.md](SITE_RUNTIME_CONTRACT/05-P1-NOTES.md) | P1 实现补充、运行环境拆分与安装器本地化、PHP 配置文件直接编辑 |
