# 固定页槽位、权限与兼容迁移（§5–7）

> 上级索引：[MENU_CONTRACT.md](../MENU_CONTRACT.md) ｜ 拆自 `docs/contract/MENU_CONTRACT.md` 原 §5–7（2026-11-16 整理，正文未改）。

---

## 5. 固定页面的插件挂载点

### 5.1 槽位注册表

槽位由内核**封闭注册**，插件不能自造名字：

| 槽位 | 页面 | 语义 | 替换/追加 | 默认降级 |
|---|---|---|---|---|
| `overview.health` | 概览 | 顶部系统健康块 | **替换**内核默认健康块 | 内核原生健康块 |
| `overview.actions` | 概览 | 行动面板补充卡 | 追加（最多 2 个） | 不显示 |
| `overview.side` | 概览 | 右栏补充卡 | 追加（最多 1 个） | 不显示 |
| `sites.header` | 网站工作台 | 站点页顶部提示 | 追加（最多 1 个） | 不显示 |

- 新增槽位必须改内核槽位表并写契约；插件不能通过声明“发明”槽位。
- 槽位命名固定 `page.slot`，全小写、点分。

### 5.2 声明与渲染

```toml
[[mount]]
slot = "overview.health"
ui = "health.html"
height = "sm"
refresh = 60
roles = ["admin", "operator", "viewer"]
```

- 内核渲染 `<iframe src="/{p}/plugins/{id}/ui/{ui}?nonce=...&slot=overview.health">`。
- 尺寸：`sm=96px`、`md=180px`、`lg=320px`；内容超出在 iframe 内滚动，不允许撑开宿主页面。
- 刷新：`0/30/60/300` 秒；内核重设 `src`（带新 nonce）；`0` 表示只在页面打开时加载。
- 失败降级：
  - 插件未启用 / 进程未运行 / 声明 gate 不满足 → 不渲染 iframe；`overview.health` 回退内核原生块，其他槽位显示空。
  - iframe 5 秒未 `load` 或插件返回 5xx → 显示固定卡片「插件不可用」+ 重试按钮 + 「查看日志」（异常容器/插件日志）。
  - 单个槽位失败不得影响页面其他区域或整页 500。
- CSP / 安全：沿用 §3.5；iframe 沙箱不获得父页面权限；槽位内容不能读 Cookie、不能发外网。
- `overview.health` 替换时，内核仍保留「Docker 不可用」等系统级警示；插件不能隐藏系统警示。

### 5.3 monitor 最小示例

```toml
api = "1.1"

[[ui]]
label = "主机监控"
path = "index.html"

[[menu]]
id = "monitor"
parent = "tools"
label = "监控"
order = 20
icon = "activity"
view = "overview"

  [[menu.items]]
  id = "overview"
  label = "总览"
  order = 10
  view = "overview"

  [[menu.items]]
  id = "cpu"
  label = "CPU / 内存"
  order = 20
  view = "cpu"

  [[menu.items]]
  id = "disk"
  label = "磁盘"
  order = 30
  view = "disk"

[[mount]]
slot = "overview.health"
ui = "health.html"
height = "sm"
refresh = 60
roles = ["admin", "operator", "viewer"]
```

- `health.html` 是 monitor 包内新文件；没有该文件时安装校验明确失败。
- monitor 禁用后，概览自动回退内核健康块，不需要用户手动清理。

---

## 6. 权限与安全

### 6.1 角色显隐

- L2 `roles` 是**可见性上限**，不是授权来源；真正操作仍由 RPC 策略与插件 grants 判定。
- 渲染前按当前用户角色过滤；未命中角色 → 该 L2/L3 不输出 DOM，不出现“点进去 403”的入口。
- 若一个 L1 的全部 L2 因角色不可见 → 该 L1 对当前用户隐藏（`工具` 例外见 §1.4：无提供者才显示空态；`网站` 由内核固定 L2）。
- 直接访问未授权菜单 URL：返回 404（不泄露存在性）；插件 RPC 仍返回 403/策略错误。
- `roles = []`：任何用户不可见；允许但安装警告。

### 6.2 插件不能注册系统内置 id

- 插件 L2/L3 的完整键强制带 `plugin/<plugin-id>/` 前缀；内置键强制 `core/` 前缀。
- 插件 `parent` 只能是固定 L1，不能挂到 `core/<l1>/<l2>` 下；因此无法把自己的项塞进 Docker/设置的内置 L2。
- 插件 `id` 不能含 `/`；不能声明 `id = "core"` 或任何 `core/*` 键。
- 内置 L1/L2 的 label、icon、URL、顺序基准由内核固定；插件 label 不参与路由。

### 6.3 id 冲突与冒用

- 两个插件声明同一完整键：安装校验失败 `MENU_ID_CONFLICT`，不静默覆盖。
- 同一插件 L3 重复：安装失败。
- 插件 label 与内置 label 重名：允许，但 DOM 有 `data-owner-kind="plugin"`，插件中心卡片显示真实插件名；视觉上不得伪装成系统项（禁止插件提供自定义图标绕过，图标只能内置枚举）。
- 插件卸载/降级后旧键不残留（§2.5）。
- 审计：安装/拒绝/卸载/菜单偏好变更都记录；菜单声明变更随插件安装审计留存 manifest 摘要。

---

## 7. 兼容与迁移

### 7.1 当前实现基线

- `Manifest` 目前只有 `[[ui]]`（`UiEntry { label, path, icon }`）与 `deny_unknown_fields`，没有 `menu` / `mount`。
- 导航数据由 `crates/app/src/menu.rs::build` 产出、`crates/web/src/handlers/ui/menu.rs` 以 JSON 下发（`GET /ui/v1/menu`）；菜单 DOM 由 budding 运行时渲染，内核不再出模板。
- 一级固定为 概览 / 网站 / Docker / 插件 / 任务审计 / 工具 / 设置（见 `docs/impl/MENU_*`）与 `/ui/v1/menu` 的实际下发顺序。
- 插件页：详情 `/{p}/plugins/{id}` 交壳页；功能页 `/{p}/plugins/{id}/{view}` 有 schema 原生渲染，否则 `?legacy=1` 走 iframe。
- 设置拆为 `/{p}/settings/{security,menu,ui,audit}`；Docker 设置归 `/{p}/docker/settings`。

### 7.2 菜单键 → 规范 URL

菜单 `href` 一律是 budding 壳页规范路径（由 `/ui/v1/menu` 下发）。

| 菜单键 | 规范 URL | 备注 |
|---|---|---|
| `overview` | `/{p}` | L1 直达 |
| `core/sites/list` | `/{p}/sites/list` | |
| `core/sites/env` | `/{p}/sites/runtimes` | |
| `core/sites/web` | `/{p}/sites/web` | 管理专属 |
| `core/docker/containers` | `/{p}/docker/containers` | 保留 `?filter=error`；详情 `/{p}/docker/containers/{id}` |
| `core/docker/images` | `/{p}/docker/images` | |
| `core/docker/networks` | `/{p}/docker/networks` | |
| `core/docker/volumes` | `/{p}/docker/volumes` | |
| `core/docker/compose` | `/{p}/docker/compose` | |
| `core/docker/settings` | `/{p}/docker/settings` | 页面与菜单键同属 Docker 分组 |
| `core/plugins/installed` | `/{p}/plugins/installed` | |
| `core/plugins/discover` | `/{p}/plugins/discover` | |
| `core/plugins/updates` | `/{p}/plugins/updates` | |
| `core/plugins/store` | `/{p}/plugins/store` | 目录启用时才出现；确认页 `/{p}/plugins/store/confirm` |
| `core/tasks/scheduled` | `/{p}/tasks/scheduled` | 任务审计一级 |
| `core/tasks/jobs` | `/{p}/tasks/jobs` | 任务审计一级 |
| `core/tasks/audit` | `/{p}/tasks/audit` | 任务审计一级；仅 operator / admin |
| `core/files` | `/{p}/files` | 回收站 / 配额是其 L3：`/{p}/files/trash`、`/{p}/files/quotas` |
| `core/settings/security` | `/{p}/settings/security` | |
| `core/settings/menu` | `/{p}/settings/menu` | |
| `core/settings/ui` | `/{p}/settings/ui` | |
| `plugin/<id>/<l2>[/<l3>]` | `/{p}/plugins/<id>/<view>` | 有 schema 由壳页原生渲染；`?legacy=1` 才是 iframe 工作台，与 RPC 同权限（非管理者 403） |
| 旧「应用」菜单 | 解散 | cron → `任务审计`，monitor/guard 等 → `工具`；插件信息页由壳页 `/{p}/plugins/{id}` 承载 |

兼容策略：

- 内核不再有第二套页面路由：所有页面都是壳页，数据走 `/ui/v1/*`；书签一律落在上表的规范 URL。
- 插件 iframe 工作台只由显式 `?legacy=1` 打开（无随包 schema 的旧视图），正常入口始终是壳页。
- 旧「应用」菜单不再出现；`工具` 按 §1.4 渲染；`网站` 由内核固定的三个 L2 渲染。

### 7.3 官方插件应声明什么

本表是官方插件菜单归属的**唯一来源**（`manifest.toml` 为实际取值），插件目录文档只引用不复述。

| 插件 | L1 | 插件菜单（`id` / 名称） | L3 视图 | 固定页槽位 | 批次 |
|---|---|---|---|---|---|
| `com.skiff.cron` | 任务审计 `tasks` | `jobs` 任务编辑器 | 无（单视图 `list`；新建/编辑是页内弹窗，**不声明 L3**） | — | B3 |
| `com.skiff.monitor` | 工具 `tools` | `monitor` 主机监控 | 总览 / 历史采样 / 进程 / 网络（`overview`/`history`/`process`/`network`） | `overview.health` | B2 |
| `com.skiff.guard` | 工具 `tools` | `guard` 容器守护 | 容器 / 事件（`containers`/`events`） | — | B3 |
| `com.skiff.firewall` | 工具 `tools` | `firewall` 防火墙 | 防火墙规则 / fail2ban（`rules`/`fail2ban`） | — | 已上线（0.1.0，未验收） |
| `com.skiff.ftp` | 工具 `tools` | `ftp` FTP 管理 | 账号 / 运行状态（`accounts`/`runtime`） | — | 已上线（0.1.0） |
| `com.skiff.sources` | 工具 `tools` | `sources` 软件源 | 无（单视图 `main`） | — | 后续新增 |
| `com.skiff.testkit` | 工具 `tools` | `testkit` 插件架构测试台 + `testkit-single` 测试台 · 单视图 | 总览与 RPC / 能力面自检 / 菜单与槽位（`main`/`check`/`group`）；单视图 `single` 不出 L3 | `overview.actions` | 测试台 |

说明：

- 网站一级由内核自带（网站列表 / 运行环境 / Web 服务），原 `com.skiff.nginx` 已内核化，不再声明插件菜单。
- `files` 挂「网站」（站点级文件），cron 挂「任务审计」，其余插件挂「工具」，不再作为一级或二级「应用」的平铺项。
- L3 的 `view` 必须对应真实存在的 `ui/<view>.html`；没有对应视图就先不声明，避免假标签。

### 7.4 分批

1. **B1 内核先行**：注册表、manifest 解析/校验、旧路由映射、旧 `[[ui]]` 降级、L1/L2 渲染；不引入 L3 与设置页。
2. **B2 高价值路径**：内核网站（网站列表 / 运行环境 / 设置 + 运行环境安装）、monitor（工具 + `overview.health` 槽位）；同时落 user_prefs 与布局切换。
3. **B3 全量**：files/cron/guard/ftp 声明、插件中心 L2 拆分、Docker 规划页、pin/固定区与管理员默认。

---
