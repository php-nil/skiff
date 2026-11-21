# 验收口径与分阶段计划（§8–10）

> 上级索引：[MENU_CONTRACT.md](../MENU_CONTRACT.md) ｜ 拆自 `docs/contract/MENU_CONTRACT.md` 原 §8–10（2026-11-16 整理，正文未改）。

---

## 8. 验收口径与测试矩阵

### 8.1 契约级断言

| # | 场景 | 断言 | 层级 |
|---|---|---|---|
| C1 | 合法 manifest `[[menu]]` | schema 解析成功；L2/L3 稳定键正确 | 单测 |
| C2 | `parent` 非固定 L1 | 安装拒绝 `MENU_PARENT_UNKNOWN` | 单测/集成 |
| C3 | 插件声明 `core/...` 或重复键 | 拒绝 `MENU_ID_CONFLICT`，无覆盖 | 单测 |
| C4 | L3 深度超限 / 数量超限 | 拒绝 `MENU_DEPTH_EXCEEDED` / `MENU_TOO_MANY_ITEMS` | 单测 |
| C5 | `view` 非法/文件不存在/同插件重复 | 拒绝 `MENU_VIEW_INVALID` / `MENU_VIEW_MISSING` / `MENU_VIEW_CONFLICT` | 单测 |
| C6 | 旧包只有 `[[ui]]` | 合成 `plugin/<id>/legacy`；旧 nginx 包兼容映射到 `sites`，其余映射到 `tools` | 单测/集成 |
| C7 | 网站 L1 的内核固定 L2 | 始终出现网站列表 / 运行环境 / Web 服务，三项均 200，无 404 | 集成 |
| C8 | 0 个工具提供者 | 工具 L1 显示空态 | 集成 |
| C9 | 插件未启用 | 其 L2 不出现在导航；插件中心可启用 | 集成 |
| C10 | 角色不匹配 | 菜单不渲染；直接访问菜单 URL 404 | 集成 |
| C11 | 顺序偏好 | 用户顺序生效；新项按合并算法插入 | 单测/集成 |
| C12 | 隐藏偏好 | 隐藏项不渲染，但保留排序位置 | 集成 |
| C13 | 布局切换 | 同一菜单键/URL 集合，`data-layout` 变化，结构不变 | 集成 |
| C14 | 深链 | `/{p}/m/...` 命中正确 L1/L2/L3 当前态 | 集成 |
| C15 | L3 无 JS | iframe query 含 `menu`/`tab`，`tab` = 视图名，插件可渲染 | 集成/浏览器 |
| C16 | 槽位失败 | monitor 禁用后概览回退原生健康块；页面 200 | 集成 |
| C17 | 卸载清理 | 插件删除后菜单消失；`user_prefs` 无该前缀 | 单测/集成 |
| C18 | 悬停 | hover 不展开；click 才展开；Escape 关闭并还焦点 | 浏览器 |
| C19 | 窄屏 | ≤860px 折叠；无 hover 浮层；L3 横向滚动 | 浏览器 |
| C20 | CSP | 菜单页无内联脚本/外链；插件 iframe CSP 仍收紧 | 集成/静态扫描 |

### 8.2 需要补的单测/集成测试方向

- `manifest` 单测：新增字段解析、所有错误码、旧 `[[ui]]` 合成。
- 纯函数注册表测试：`builtin + plugin declarations → effective tree`，覆盖 role/gate/hidden/order/merge/uninstall。
- Web 集成：用临时 DB 种插件记录 + manifest，GET `/{p}`、`/{p}/m/...`，断言 DOM 与当前态。
- 设置集成：POST 菜单偏好（CSRF）→ 重新 GET 断言顺序/隐藏/布局；越权用户不能改别人。
- 槽位集成：插件禁用/模拟加载超时 → 回退块；`overview.health` 不破坏原生数据块。
- 浏览器：桌面上下/左右、窄屏、键盘、hover 不展开、Escape、焦点归还、L3 切换后 iframe URL。
- 安全：恶意 label（HTML）、外部 icon、`target.ui` 越界、伪造 `core` 键、重复键、越权角色。

口径沿用仓库规则：本地只跑 `cargo fmt --all` 与 `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`（编译级），用例结果以 CI 的 `cargo test --workspace --locked` 为准（`AGENTS.md` §4.3）。

---

## 9. 分阶段实现计划

### P1：注册表与声明层（不改视觉交互大框架）

- 扩展 `skiff-pkgfmt::Manifest`：`[[menu]]` / `[[menu.items]]` / `[[mount]]` 结构 + 校验 + 错误码。
- 内核新增内置菜单注册表（六 L1、Docker/插件/设置内置 L2；规划项标 `route_available=false`）。
- 用注册表替换原 `handlers::nav()` 的硬编码（已落地：现由 `skiff_app::menu` 产出、`GET /ui/v1/menu` 下发）；保留旧 `active` → 新键映射；旧 `[[ui]]` 降级。
- 产出：现有页面仍可打开；导航结构变为六 L1；工具空态可验收；网站内核固定 L2 可验收；所有契约单测通过。
- 风险：`deny_unknown_fields` 导致旧内核不识别新包（预期）；当前「应用」入口消失需同步测试与文档；旧 active 映射漏项会丢当前态。

### P2：L3、设置与槽位

- 新增 `user_prefs` 表（schema v2）与菜单设置页 `/settings/menu`。
- 内核 L3 标签栏 + 规范 URL + iframe `menu/tab` + `skiff.menu` 消息。
- 上下/左右布局（同一 DOM，`data-layout`），窄屏折叠与交互修正（点击展开、无 hover 打开）。
- monitor 等官方插件声明 L2+L3；monitor 声明 `overview.health` 槽位；概览页接入槽位失败降级。
- 产出：工具 → 主机监控 → L3 切换 → iframe 内容；排序/隐藏/布局保存生效；概览可显示 monitor 健康槽位。
- 风险：iframe 状态保持与无 JS 降级；用户偏好合并；槽位加载超时的视觉抖动；CSP 回归。

### P3：全量迁移与规划项

- files/cron/guard/monitor/ftp 声明迁移；插件中心 L2 拆分；旧「应用」代码删除。
- Docker 概览 / 编排 / 设置 页面与内置 L2（编排无后端前保持 `route_available=false`，不出现死链）。
- pin/固定区、管理员实例默认、菜单设置细节（拖拽、恢复默认、预览）。
- 产出：7 个官方插件全部走新契约；旧 `[[ui]]` 仅作兼容；契约测试矩阵全绿。
- 风险：官方插件 UI 需配合 `tab`/`menu` query，否则 L3 是空标签；Docker 编排缺少后端可能导致长期隐藏项；用户偏好迁移与清理遗漏。

---

## 10. 开放问题（最多 3 条）

### Q1. 工具在 0 提供者时，默认显示空态还是自动隐藏？

- 说明：`网站` 已内核化、L2 固定，不再参与本问题；本问题只针对插件支撑的 `工具`。
- 推荐：**默认显示空态**（固定一级集合不变，空态给出「去插件中心」），用户可在菜单设置里隐藏。
- 影响：导航始终是六项；测试要覆盖空态；与旧「应用 0 项隐藏」规则相反，需确认。
- 若选自动隐藏：六项固定会退化为动态集合，与「一级固定」冲突。

### Q2. L3 切换是同一 iframe 内消息切换，还是每个 L3 独立 iframe/页面？

- 推荐：**同一 iframe + `tab` query + `skiff.menu` 消息**；`ui` 不同时才换 `src`。
- 理由：保留插件内部状态、避免每次切标签重建 iframe、深链仍可用。
- 影响：插件 UI 必须监听 `skiff.menu` 或读 query；如果插件坚持多 HTML 文件，也能通过 `ui` 字段工作，但状态不保留。

### Q3. 菜单偏好只做按用户，还是管理员需要全局策略/锁定？

- 推荐：**P2 只做按用户 + 全局默认布局**；不做强制模板与锁定。
- 理由：改动面最小、符合“高度自定义”，避免引入权限模型外的“策略层”。
- 影响：每个用户可自行隐藏/排序；若以后企业需要统一模板，再增加 `menu.policy` 与管理员锁定，属于 P4。

---
