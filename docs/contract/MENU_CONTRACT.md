# Skiff 菜单契约（Menu Contract · 索引）

> 状态：**契约 v1 · P1/P2/P3 已落地**。§11–13 的落地记录已收敛为现行差异摘要（`05-ROLLOUT.md`）；正文其余部分仍为设计口径。
> 适用：内核 Web UI、插件 manifest、插件中心、菜单设置、固定页槽位。
> 基线：`crates/pkgfmt/src/manifest.rs`、`crates/app/src/menu.rs`、`crates/web/src/handlers/ui/menu.rs`、`plugins/*/manifest.toml`。
> 契约版本：v1 · 2026-11-03（P1 同日落地，P2 同日补记）。
>
> **现役形态**：budding 后台外壳（admin + outlet）渲染菜单 DOM；已声明 `schema`（`api = "1.3"`）的插件视图由壳页原生渲染，未声明的旧视图走 `?legacy=1` 的 iframe 工作台。菜单数据由 `GET /ui/v1/menu` 下发（字段与顺序见 [`MENU_CONTRACT/02-RENDER.md`](MENU_CONTRACT/02-RENDER.md) §3），内核不再输出菜单 HTML；L3 由壳页的横向三级菜单承担（`budding-boot.js` 的 `SUBVIEW_TABS` 登记核心页，插件页由 `views` 驱动，当前页有子视图才出现）。插件 UI 交付方向改为 Schema（见 [`BUDDING_UPSTREAM_BACKLOG.md`](../sdui/BUDDING_UPSTREAM_BACKLOG.md) T7）。
> **本文件是索引**：正文按 §编号拆在 [`MENU_CONTRACT/`](MENU_CONTRACT/)，编号沿用拆分前原文。

## 四个结论（摘要）

1. **三级固定**：一级（L1）由内核固定注册表决定，插件只能声明二级（L2）；三级（L3）来自插件声明的视图。
2. **声明即数据**：`manifest.toml` 的 `[[menu]]` 是唯一声明面，内核不读插件私有文件决定菜单。
3. **设置是偏好不是权限**：显隐 / 置顶 / 排序属用户偏好，概览与会话安全页锁定，显隐对锁定项静默 no-op。
4. **槽位只对已装插件开放**：固定页（如插件中心、计划任务）的挂载点按权限矩阵与安装态下发，未安装即不渲染。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–2 | [01-MODEL_DECLARE.md](MENU_CONTRACT/01-MODEL_DECLARE.md) | 四个结论、模型与归属、`manifest` 声明契约 |
| §3–4 | [02-RENDER.md](MENU_CONTRACT/02-RENDER.md) | 渲染契约（L1/L2/L3、图标、折叠）、菜单设置与持久化 |
| §5–7 | [03-SLOTS_SECURITY.md](MENU_CONTRACT/03-SLOTS_SECURITY.md) | 固定页插件挂载点、权限与安全、兼容与迁移 |
| §8–10 | [04-VERIFY_PLAN.md](MENU_CONTRACT/04-VERIFY_PLAN.md) | 验收口径与测试矩阵、分阶段实现计划、开放问题 |
| §11–13 + 附录 | [05-ROLLOUT.md](MENU_CONTRACT/05-ROLLOUT.md) | 现行差异摘要、内置一级与默认 L2 注册表、非目标 |
