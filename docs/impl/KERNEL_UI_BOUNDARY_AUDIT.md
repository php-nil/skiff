# 内核业务层 ↔ UI 层边界审计（SSR + `/ui/v1` · 未闭环）

> 状态：只读审计（2026-11-14）；**批次 0（拆 `ui_api.rs` → `handlers/ui/*` 及 0.2–0.8 各项）已落地**（见 `docs/changes/2026-11-14.md`）；**2026-11-21 复核**：SSR 页面已随壳页迁移全部下线（`templates.rs` 只剩错误页与插件 iframe 工作台），原「SSR 与 `/ui/v1` 两套编排」类问题随之失效，已删。本文件只保留**未闭环问题、待决策批次与未验证项**。
> 范围：`crates/web`（`/ui/v1` JSON handler、插件 iframe 工作台）、`crates/app` 命令层、`crates/plugin`/`crates/pkgfmt` 的插件 UI 边界、`crates/web/static` 的壳页与静态资源。
> 相关契约：[DESIGN.md](../DESIGN.md)（分层与依赖方向）、[URL_CONTRACT.md](../contract/URL_CONTRACT.md)、[SDUI_BUDDING_INTEGRATION.md](../sdui/SDUI_BUDDING_INTEGRATION.md)、[PLUGIN_UI_CONTRACT.md](../contract/PLUGIN_UI_CONTRACT.md)、[MENU_CONTRACT.md](../contract/MENU_CONTRACT.md)。

## 1. 依赖方向现状

**crate 级依赖不违规**：`web` 允许依赖 `app/model/store/identity/docker/plugin/pkgfmt`（`docs/sections/08-engineering.md`）。越层只发生在调用面，审计时发现的两处「绕过命令层」（`/api/v1` 重启直调 `Docker::execute`、`/ui/v1` 文件写直调 `AdminFs`）已在批次 0.6 / 0.5 收口；web 内无手写 SQL；`web` 不再设 `crate::state` 别名，上下文归属在调用点可见。

## 2. 未闭环问题

> 每条只留结论与影响；现象级证据见 git 历史（2026-11-14 审计原稿）。

1. **（S2）RPC 错误靠字符串猜内核状态码**：`PluginError` 仅 `Rejected` 映射 400、其余 500，与 RPC 路径字符串解析出的 503 语义不对称。→ 批次 1.3。
2. **（S2）DTO 冻结没有编译期/测试期约束**：字段冻结靠用例与文档约定。→ 批次 1.1。
3. **（S3）schema 运行期校验失败返回 500**：应为 400/409 + 稳定错误码。→ 批次 2.1。
4. **（S2，2026-11-19）`SiteError → AppError` 映射 4 份且口径不一**：`handlers/sites.rs::map_err`（`Field(_,inner)`→Internal）、`ui/mod.rs::site_write_err`（→BadRequest）、`ui/mod.rs::site_api_error`（递归 inner）、`jobs.rs::retry` 内联（→Internal）——同一领域错误三条 HTTP 结论。→ 批次 2.4（先定 `Field` 错误的正确语义，再在 `error.rs` 收一个 `From`）。
5. **（S3，2026-11-19）视图层残留判定**：`store.rs::load_view` 内联「已装版本高于目录可装版本则拒绝降级」（后端同判两处）→ 待移 `skiff_app::catalog` 读模型。「文案归属」作为独立问题挂 [KERNEL_ARCH_AUDIT.md](KERNEL_ARCH_AUDIT.md) §4 C10。

## 3. 待决策批次（需要先定契约）

| 批次 | 目标 | 需要定的契约 |
| --- | --- | --- |
| 1.1 | DTO 冻结机制（golden JSON / key-set 快照 / 可选 JSON Schema） | 冻结粒度、新增字段是否必须升 `api_version` |
| 1.3 | RPC 错误结构化（code + status） | `skiff-plugin`/`skiff-wire` 协议面变更 |
| 2.1 | 错误码细分：schema 校验失败改 400/409 并给稳定码 | 与商店安装侧统一 |
| 2.3 | 审计来源补全：面板/菜单设置写操作仍记 `local`（`handlers/settings.rs::apply_panel_update` / `apply_menu_update` 拿不到 `Parts`） | 把 `Parts` 传进共享入口，或改由调用方传 ip |
| 2.4 | 错误映射收敛（未闭环 4）：`error.rs` 收 `From<SiteError>`、`jobs.rs::retry` 内联改调同一映射 | `Field` 错误的 HTTP 语义（400 vs 500） |

## 4. 未验证项

1. **budding pack 漏文件时的前端行为**：`budding-boot.js` 有 `try/catch`，实际是静默降级还是控制台报错，未实测。
2. **`KERNEL_API` 版本门覆盖面**：任务门与商店安装走 `api_compatible`；直接 `.wbpkg` 上传安装没有 API 版本校验，是否有启动期兜底未验证。
3. **插件 iframe 工作台与 `/ui/v1` 的菜单口径**：无 schema 的旧插件走 `plugin_view.html` 时，三级菜单由模板侧装配、壳页由 `/ui/v1/menu` 装配，两条路径未做一致性断言。
