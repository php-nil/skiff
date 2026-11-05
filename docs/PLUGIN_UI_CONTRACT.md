# 插件 UI 路由与视图契约

> 本文回答一个问题：**插件的「信息 / 管理」与「功能界面」分别由谁渲染、放在哪个 URL、manifest 怎么声明。**
> 菜单树、深链键格式、用户偏好与布局不在本文重复定义，统一见 [`docs/MENU_CONTRACT.md`](MENU_CONTRACT.md)。
>
> 一句话结论：`/{p}/plugins/{id}` 是内核渲染的插件信息与管理页；插件只负责 `/{p}/plugins/{id}/<view>` 里的功能内容。

## 1. 路由分工

所有路由都在入口前缀 `/{p}`（默认 `p`，见 `config.toml`）之下。根路径 `/static`、`/plugins/...` 一律 404，插件资源不得绕过入口前缀。

| 路由 | 方法 | 渲染方 | 说明 |
|---|---|---|---|
| `/{p}/plugins` | GET | 内核 | 插件中心（已安装 / 发现 / 可更新） |
| `/{p}/plugins/{id}` | GET | 内核 | **插件信息与管理页**：身份、版本、密钥指纹、签名、沙箱、授权、菜单、运行状态、日志/事件入口、管理动作 |
| `/{p}/plugins/{id}/{view}` | GET | 内核框架 + 插件 iframe | **插件功能页**：按视图名加载 `ui/<view>.html`，内核只渲染页面标题区 + 顶部 L3 通栏，iframe 为纯内容区 |
| `/{p}/plugins/{id}/ui/{path}` | GET | 插件 | 托管静态资源（HTML/CSS/JS/图片），带 CSP，不允许目录穿越 |
| `/{p}/plugins/{id}/rpc` | POST | 内核代发 | 父页面代发插件 RPC，需要 CSRF 头与管理员权限，逐次审计 |
| `/{p}/plugins/{id}/toggle` | POST | 内核 | 启用 / 停用；`next` 只接受本站插件前缀 |
| `/{p}/plugins/{id}/remove` | POST | 内核 | 卸载；可选 `delete_data=on` |
| `/{p}/m/plugin/{id}` | GET | 内核 | 菜单默认视图 |
| `/{p}/m/plugin/{id}/{l2}` | GET | 内核 | 该 L2 的默认视图 |
| `/{p}/m/plugin/{id}/{l2}/{l3}` | GET | 内核 | 该 L3 的视图 |

约定：

- `/{p}/plugins/{id}` **不再承载 iframe**，也不再接受 `?menu=` 选择视图。
- 功能页的规范入口是 `/{p}/plugins/{id}/{view}`；菜单深链 `/{p}/m/...` 是同一视图的另一种寻址方式，二者渲染结果一致。
- 静态段 `ui` / `rpc` / `toggle` / `remove` 优先于 `{view}`，因此视图名不得使用这四个保留字（安装期校验）。
- `{view}` 只允许 `[a-z0-9][a-z0-9-]{0,31}`。

## 2. manifest 声明模型

### 2.1 字段

菜单声明要求 `api = "1.2"`。字段只在 `skiff-pkgfmt::Manifest` 定义一处：

| 位置 | 字段 | 类型 | 默认 | 语义 |
|---|---|---|---|---|
| `[[menu]]` | `view` | string | `main` | 该 L2 的默认视图；无 L3 时是唯一视图 |
| `[[menu.items]]` | `id` | string | — | L3 稳定局部 id |
| `[[menu.items]]` | `view` | string | 等于 `id` | 该 L3 的视图名 |
| `[[ui]]` | `path` | string | — | 旧包入口 / 插件中心元数据，不参与视图解析（见 §5） |
| `[[mount]]` | `ui` | string | — | 槽位内容文件，直接相对 `ui/`，不经视图映射 |

有效视图集合（`ManifestMenu::effective_views`）：

- `items` 为空 → `[view]`（单视图，隐式主视图）。
- `items` 非空 → 每个 item 的 `view`（缺省 `id`）。

视图 → 文件映射：

```text
main        → ui/main.html
{view}      → ui/{view}.html
```

### 2.2 校验与错误码

安装验包（`pkgfmt`）阶段拒绝以下情况，错误发生在安装期而不是运行时：

| 场景 | 错误码 |
|---|---|
| 视图名非法（含大写、下划线、`..`、保留字） | `MENU_VIEW_INVALID` |
| 视图名在同一插件内重复（L2 与 L3 合并计算） | `MENU_VIEW_CONFLICT` |
| `ui/<view>.html` 不存在 | `MENU_VIEW_MISSING` |
| `[[menu]]` 但 `api < 1.2` | `MENU_API_VERSION` |
| 旧包无 `[[ui]]` 却声明菜单 | `MENU_NO_UI_ENTRY` |

### 2.3 未知视图与不可见视图

- 请求的 `{view}` 不在有效视图集合内 → **404**，不回退到 `main`。
- 视图所属 L2 对当前角色不可见（`roles` / `gates`）→ **404**，不渲染后再做 403。
- 插件未启用 / 不存在 → 404。
- 有效视图存在但包内文件缺失（正常情况下安装期已拦）→ 404，不返回目录列表。

### 2.4 示例

```toml
api = "1.2"

[[ui]]
label = "示例面板"
path = "main.html"

[[menu]]
id = "panel"
parent = "tools"
label = "示例面板"
view = "main"          # 单视图 L2：ui/main.html，不渲染标签栏

[[menu]]
id = "jobs"
parent = "tools"
label = "任务"
view = "list"          # L2 默认视图（无 L3 时的唯一视图）

  [[menu.items]]
  id = "list"
  label = "任务列表"
  view = "list"        # ui/list.html

  [[menu.items]]
  id = "new"
  label = "新建任务"
  view = "new"         # ui/new.html
```

## 3. 内核信息页

### 3.1 字段清单

| 区块 | 字段 | 来源 |
|---|---|---|
| 标题 | 插件名称 + 版本 | `plugin.name` / `plugin.version` |
| 基本信息 | 标识、版本 / API、运行状态（进程 + 启用态） | DB 记录 + Supervisor |
| 身份 | 发布者 / 密钥指纹（完整 hex） | `key_id` |
| 信任 | 签名状态（已验签 / 未验签） | `trusted_key(key_id)` |
| 沙箱 | 完整沙箱 / 受限沙箱；opaque origin、无面板凭据、出网默认拒绝 | manifest `requires_full_sandbox` |
| 能力 | 声明能力面（`fs, identity, ...`） | manifest `requires` |
| 权限 | 声明权限（`fs:read` 等） | manifest `permissions` |
| 授权 | 已授权资源（`path:/proc`、`plugin:<id>`、`container:<id>` 等） | 安装时落库的 grants |
| 互操作 | 对外开放方法、允许调用的 `插件:方法` | manifest `[interop]` |
| 关联菜单 | L2/L3 标签与深链 `/{p}/m/plugin/...` | 实际可见菜单（已按角色过滤） |
| 运行 | 运行中 / 未运行 | Supervisor |
| 日志与事件 | 审计日志入口（`plugin.*`）、异常容器与日志入口 | 内核固定链接 |
| 功能入口 | 「打开功能界面」→ 默认视图 | 第一个可见 L2 的首个视图；旧包为 `main` |

### 3.2 操作清单

| 操作 | 权限 | 行为 |
|---|---|---|
| 打开功能界面 | 当前角色可见菜单 | GET 默认视图，不额外授权 |
| 启用 / 停用 | `PluginManage` | POST `toggle`，停用时先 `supervisor.stop` |
| 卸载 | `PluginManage` | POST `remove`，清理菜单/槽位与用户偏好残留，可选删除数据目录 |
| 查看日志/事件 | 对应页面权限 | 仅链接，不在信息页内嵌日志内容 |

信息页**不提供**：插件自定义 HTML、导航替代、能力临时提权、未经安装确认的授权变更。

### 3.3 权限与审计

- 信息页读取：`AuthUser` 即可；关联菜单按当前角色过滤，不可见视图不出现在列表。
- 启停 / 卸载：`PluginManage`（`PluginManagerUser` 提取器）+ CSRF。
- RPC：`PluginManage` + CSRF 头；插件停用时直接拒绝。
- 审计事件命名（已有实现，不新增未落地事件）：

| 事件 | 时机 | 结果 |
|---|---|---|
| `plugin.install` | 安装成功/失败 | `ok` / `fail` |
| `plugin.toggle` | 启停 | `ok`，detail `enabled=<bool>` |
| `plugin.uninstall` | 卸载 | `ok`，detail `delete_data=<bool>` |
| `plugin.rpc` | 每次 UI 代发 RPC | `ok` / `fail`，detail `method=<name>` |

## 4. iframe 沙箱与 CSP

功能页由内核渲染外框，插件内容在 iframe 中：

```html
<iframe sandbox="allow-scripts allow-modals allow-forms"
        src="/{p}/plugins/{id}/ui/{view}.html?nonce=...&menu=<l2-key>&tab=<view>">
```

- **不授予** `allow-same-origin`：iframe 为 opaque origin，拿不到面板 Cookie，也不能读写父页面 DOM。
- `allow-forms` 允许插件内表单提交（仍受 `form-action 'none'` 限制，不会跳出沙箱发请求）。
- `allow-scripts` 允许插件运行自身脚本；脚本只能从包内加载。
- 每次打开生成一次性 `nonce`，父页面只代发携带该 nonce 的 `skiff.rpc`。

托管响应头（`PLUGIN_UI_CSP`）：

```text
default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data:; connect-src 'none'; form-action 'none'; base-uri 'none';
object-src 'none'; frame-ancestors 'self'
```

- 禁止内联 `<script>`、外部脚本、外链样式、`fetch/XHR/WebSocket`。
- 主题只能通过 `postMessage` 跟随父面板（`skiff.theme` / `skiff.theme.request`），不读 `localStorage` 作为唯一来源。
- 高度：插件主动上报 `{ type: "skiff.height", nonce, height }`；父页面只接受带 `data-workbench-frame` 的工作台 iframe（固定槽位不受影响），把高度裁剪到 320–20000px 后写入 iframe。旧包不发消息时维持 CSS 默认 `70vh` / 最小 360px，由外层滚动。
- 加载占位：工作台 iframe 与面板使用 `--plugin-frame` 主题底色，加载阶段不出现白屏/黑屏闪烁；插件自身仍以缺省深色渲染，收到 `skiff.theme` 后切换。
- 切换 L3 或 `ui` 文件时父页面清空上报高度，等新页面重新上报。
- 跨插件跳转使用 `skiff.open`，目标必须在父页面 `data-plugins` 白名单（已启用且有 UI 入口）内。

### 4.1 页面级标题

**页面级标题（插件名 + 视图名）由内核单一提供，插件不得渲染。** 父页面已经画好标题与 L3 标签栏，插件页内再写 `<h1>插件名 · 视图名</h1>` 只会重复。

- 插件只保留**区块小标题**（`<h2>` / `<h3>`、卡片标题、表头）与功能说明文字。
- 第三方插件如确需保留自己的页面标题，必须标记 `data-plugin-title`；内核托管 HTML 时会在 `</head>` 前注入规则 `[data-plugin-title]{display:none!important}`，在 iframe 内统一隐藏。
- 内核只认显式标记，**不做**「隐藏第一个 h1」这类启发式，避免误伤区块小标题。
- 该抑制由内核在 `plugin_ui` 托管层注入，覆盖所有 `.html` 资源；CSS / JS / 图片等非 HTML 资源不注入。

## 5. 旧包兼容

| 旧包形态 | 新语义 |
|---|---|
| 只有 `[[ui]]`，无 `[[menu]]` | 合成 L2 `legacy`；默认视图 `main` 指向第一条 `[[ui]].path`（老包通常是 `ui/index.html`） |
| 多条 `[[ui]]` | 第一条 → `main`；其余 → `legacy-2`、`legacy-3`… 作为 L3，各自 `path` |
| 旧深链 `/{p}/m/plugin/{id}/legacy` | 升级为声明式后落到首个可见 L2，不 404 |
| 新包声明 `[[menu]]` | `[[ui]]` 不再参与视图解析；`[[ui]].path` 仅用于插件中心元数据 |

**何时废弃 `ui/index.html`**

1. 当前阶段（`api <= 1.1` 的包）继续支持 `ui/index.html`，内核按 `main` 回落。
2. 官方 4 插件 + testkit 已全部迁移到 `ui/<view>.html`，新包不得再以 `index.html` 作为默认视图。
3. 当内核声明不再兼容 API 1.x（`SUPPORTED_API_MAJOR` 移除 1）时，同步删除 legacy `[[ui]]` 合成与 `main → index.html` 回落；届时安装期直接拒绝旧包。

## 6. 与菜单契约的衔接

本文件**不定义**：

- L1/L2/L3 的键格式、icon 枚举、`roles` / `gates` / `order` / `pinnable`；
- 用户偏好（顺序、隐藏、固定、布局）与菜单设置页；
- 深链键 `plugin/<id>/<l2>[/<l3>]` 的拼接与清理。

上述内容一律以 [`docs/MENU_CONTRACT.md`](MENU_CONTRACT.md) 为准。本文只增加两项约束：

1. L3 的 `tab` 参数值是**视图名**，不再是任意字符串。
2. 插件功能页存在规范的 `/{p}/plugins/{id}/{view}` 路由，与 `/{p}/m/...` 深链等价。

## 7. 测试矩阵

| # | 场景 | 断言 | 层级 |
|---|---|---|---|
| U1 | 信息页字段 | 含标识、版本/API、运行状态、指纹、签名、沙箱、能力、权限、授权、互操作、关联菜单、日志入口 | 集成 |
| U2 | 信息页动作 | 含 `toggle` / `remove` 表单与 `next` 回跳；停用后无「打开功能界面」 | 集成 |
| U3 | 信息页不嵌 iframe | 无 `data-workbench-frame` | 集成 |
| U4 | 规范功能页 | `/{p}/plugins/{id}/main` 200，iframe 为 `ui/main.html` | 集成 |
| U5 | 多视图 | `/{p}/plugins/{id}/check` 200，标签栏含 `main/check/group`，`tab=check` | 集成 |
| U6 | 单视图 | 无 L3 时 200 且不渲染标签栏；深链 `/<l2>/<l2-view>` 可用 | 集成 |
| U7 | 未知视图 | 404，不回退 `main` | 集成 |
| U8 | 角色不可见视图 | 404 | 集成 |
| U9 | 旧包回落 | 只有 `[[ui]].path=index.html` 时 `/main` 加载 `ui/index.html`，无标签栏 | 集成 |
| U10 | `view` 声明校验 | 非法/重复/文件缺失在安装期失败 | 单测 |
| U11 | 槽位 | testkit `overview.actions` 渲染，`data-slot` / nonce 正确 | 集成 |
| U12 | 主题与 CSP | 插件 HTML/JS 含主题监听；CSP 无 `connect-src` 放行、无外链、无内联 script | 静态扫描 |
| U13 | 停用/卸载同步 | 菜单、槽位、功能入口消失；信息页仍可访问以恢复/卸载 | 集成 |
| U14 | 页面级标题 | 官方/测试插件 `ui/*.html` 无 `<h1>`；托管 HTML 注入 `data-kernel-title-guard`，区块小标题不受影响 | 集成/静态 |

## 8. 变更记录

| 日期 | 变更 |
|---|---|
| 2026-11-05 | 首版：`/{p}/plugins/{id}` 信息页与 `/{p}/plugins/{id}/{view}` 功能页拆分；`view` 视图模型；旧包 `ui/index.html` 回落；新增 `com.skiff.testkit` 验收插件 |
| 2026-11-05 | 页面级标题改由内核单一提供：清理 5 个插件页内标题；内核托管 HTML 注入 `data-plugin-title` 抑制规则 |
