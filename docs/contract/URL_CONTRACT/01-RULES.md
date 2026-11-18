# 路径与动作规则：前缀、命名、同构、方法与副作用、兼容、查询参数（§1–8）

> 上级索引：[URL_CONTRACT.md](../URL_CONTRACT.md) ｜ 拆自 `docs/contract/URL_CONTRACT.md` 原 §1–8（2026-11-16 整理，正文未改）。

---

> 本文是**新增与修改路由的唯一依据**。菜单身份（稳定键、深链）见 [`docs/contract/MENU_CONTRACT.md`](../MENU_CONTRACT.md)，
> 插件页面与 iframe 分工见 [`docs/contract/PLUGIN_UI_CONTRACT.md`](../PLUGIN_UI_CONTRACT.md)；
> 三者的关系：**菜单键稳定、URL 规范、菜单键 ≠ URL**。改 URL 不改菜单键，改菜单键不改本文。

## 1. 总原则

1. **一个资源一套路径**：页面、页面内 JSON、表单提交共用同一资源前缀，不额外插 `api` 中缀。
2. **层级只表达归属**：`/{前缀}/{资源}/{标识}/{子资源}`，最多四段（不含前缀与动作）。
3. **动作只出现在末段**：写操作末段必须是动词或明确的动作名，不用 `?do=`、不把动作放中间。
4. **GET 只读，POST 写入**：GET 不产生副作用；所有写入都是 POST 且带 CSRF；下载类 GET 例外见 §6。
5. **规范路径唯一**：旧路径只做兼容（§7），不并行维护两套语义；新增路由不得再产生别名。

## 2. 前缀与例外

| 范围 | 形状 | 说明 |
| --- | --- | --- |
| 页面 / 页面内接口 / 表单 | `/{p}/...` | `p` 为安全入口前缀（`config.toml` 的 `entry_prefix`），默认 `p`，运行时可变 |
| 前端 JSON 接口 | `/ui/v1/...` | 无入口前缀；会话 Cookie + CSRF；未认证返回 401；仅 `handlers::ui`（按资源拆分）；只返 JSON |
| 机器 API | `/api/v1/...` | 无入口前缀、Token/会话鉴权，版本号固定；仅 `handlers::api` |
| 探针 | `/healthz` | 无前缀、无鉴权，只回健康状态 |
| 图标 | `/favicon.ico` | 无前缀、无鉴权，返回品牌矢量图标（`image/svg+xml`，`public, max-age=86400`）；页面另用 `<link rel="icon">` 指向 `/{p}/static/favicon.svg?v=<版本>` |
| 菜单深链 | `/{p}/m/{...}` | 由 `handlers::menu::dispatch` 内部转发到同一 handler，不是第二套页面 |

- 代码中一律用 `state.cfg.entry_prefix` 或模板 `ctx.prefix` 拼接，禁止硬编码 `/p`。
- 根路径 `/static`、`/plugins/...`、`/files/...` 一律 404，不能绕过入口前缀。
- `/ui/v1/` 与 `/api/v1/` 并列；`/ui/` 下其它形态一律 404，不得作为静态资源或页面旁路。

### 2.1 SPA 壳页回落白名单

仅当 `GET` 且路径命中下表前缀（按**路径段边界**匹配，`/{p}/sitesfoo` 不算命中 `/{p}/sites`）时进入页面分支：未认证请求一律 303 到 `/{p}/login?next=...`（与 `/{p}/files` 等注册路由一致，**不先看 `Accept`**）；已认证请求仍需 `Accept: text/html` 才返回壳页，否则 404。

| 前缀 | 说明 |
| --- | --- |
| `/{p}` | 概览根（仅精确匹配） |
| `/{p}/sites` | 站点 |
| `/{p}/containers`、`/{p}/images`、`/{p}/networks`、`/{p}/volumes`、`/{p}/compose` | Docker |
| `/{p}/runtimes` | 运行环境 |
| `/{p}/files` | 文件管理 |
| `/{p}/plugins` | 插件（功能页由壳页按随包 schema 原生渲染；旧 iframe 仅 `?legacy=1` 兼容） |
| `/{p}/store` | 商店 |
| `/{p}/settings` | 设置 |
| `/{p}/audit` | 审计 |
| `/{p}/tasks` | 定时任务 |
| `/{p}/jobs` | 任务中心 |

- 白名单之外仍返回 404（HTML 或 JSON 由 `Accept` 决定），不得回落壳页。
- `POST` 等写请求**永不回落**，即使 `Accept: text/html`。
- `/{p}/static/*`、`/{p}/m/*`、`/ui/v1/*`、`/api/v1/*`、`/healthz`、`/favicon.ico` 不在白名单内。
- 命中白名单但前端路由不认识时，由前端渲染 404 视图；内核不回退到 `error.html`。
- 白名单变更必须同步本表、`docs/sdui/SDUI_BUDDING_INTEGRATION.md` 与 `main_flow` 测试。

## 3. 资源命名

- 资源段用**复数、小写、中划线**：`containers`、`runtimes`、`settings`、`store`、`tasks`、`jobs`。
- 标识段用资源的天然主键：容器/镜像 id、站点域名、运行环境 id（`php-8.5`）、任务 id、插件 id（含 `.`）。
- 子资源用**单数**名词：`/{p}/sites/{domain}/ssl`、`/{p}/runtimes/{id}/config`、`/{p}/files/trash`。
- 子资源下的集合仍用复数：`/{p}/runtimes/{id}/extensions`。
- 标识含 `/`、空格等字符时由路由层编码，不把多段拼进一个参数；插件 id 的 `.` 不二次编码。

## 4. 页面与页面内 JSON 同构

页面 `/{p}/files` 与其接口 `/{p}/files/list`、`/{p}/files/write` 同前缀；
**不再有** `/{p}/files/api/list` 这类中缀。理由：前端 `apiUrl()` 只需拼 `/{p}/files`，
页面与接口的归属关系一眼可见，也避免「页面在 A、接口在 B」的授权口径分叉。

同一资源的页面、片段（`rows`）、轮询（`jobs`、`job`）、SSE 都放在该资源下：

| 形态 | 形状 | 示例 |
| --- | --- | --- |
| 列表页 | `GET /{p}/{资源}` | `/{p}/containers` |
| 局部片段 | `GET /{p}/{资源}/rows` | `/{p}/containers/rows` |
| 新建表单页 | `GET /{p}/{资源}/new` | `/{p}/sites/new`、`/{p}/containers/new` |
| 详情页 | `GET /{p}/{资源}/{id}` | `/{p}/sites/{domain}` |
| 轮询/长连接 | `GET /{p}/{资源}[/{id}]/jobs`、`/logs`、`/logs/stream` | `/{p}/runtimes/jobs`、`/{p}/jobs/{id}/logs/stream` |

## 5. 动作命名

### 5.1 规则

- 资源**集合**：`GET` 列表；`POST` 创建（`/create` 或集合 POST，二者等价，新路由优先显式动词）。
- 资源**单体**：`GET` 查看；`POST` 更新（路径不加 `update`，如 `POST /{p}/settings/docker`）。
- **动作型操作**：末段显式动词，参数放表单或查询串，不放进路径（除通配动作，见 §5.3）。
- `delete` 与 `remove` 分工：
  - `delete`：删除资源本体（镜像、网络、卷、插件、密钥、任务）。
  - `remove`：文件系统语义的「移入回收站」，与 `skiff-fs` 动词 `remove` 一一对应，不表示资源删除。

### 5.2 动词表（新路由只许用这些）

| 动词 | 语义 | 示例 |
| --- | --- | --- |
| `create` | 新建资源 | `POST /{p}/sites/create`、`/{p}/runtimes/create`、`/{p}/networks/create` |
| `save` | 保存配置（资源已存在） | `POST /{p}/sites/{domain}/config/save`、`/{p}/sites/web/config/main` |
| `delete` | 删除资源本体 | `POST /{p}/images/{id}/delete`、`/{p}/plugins/{id}/delete` |
| `toggle` | 启用/停用、开关 | `POST /{p}/plugins/{id}/toggle`、`/{p}/tasks/{id}/toggle` |
| `run` | 立即执行一次 | `POST /{p}/tasks/{id}/run` |
| `cancel` | 取消进行中的作业/配额 | `POST /{p}/jobs/{id}/cancel`、`/{p}/files/quotas/cancel` |
| `retry` | 重试失败作业 | `POST /{p}/jobs/{id}/retry` |
| `install` / `uninstall` | 安装/卸载扩展或包 | `POST /{p}/runtimes/{id}/extensions/{key}/install` |
| `restore` / `purge` / `clear` | 回收站恢复/彻底删除/清空 | `/{p}/files/trash/restore|purge|clear` |
| `pull` | 拉取镜像 | `POST /{p}/images/pull` |
| `validate` / `up` / `down` | Compose 校验/启动/停止 | `/{p}/compose/validate|up|down` |
| `probe` / `clone` / `unbind` | 探测/克隆/解绑 | `/{p}/runtimes/{id}/probe|clone`、`.../sites/{domain}/unbind` |
| `batch` | 批量操作（参数在表单） | `POST /{p}/sites/batch` |
| `scan` / `set` | 用量扫描/设置上限（fs 配额专用） | `/{p}/files/quotas/scan|set` |
| `download` | 下载产物/升级包（GET，只读） | `GET /{p}/store/upgrades/{version}/download` |

### 5.3 通配动作（仅允许已有白名单）

以下三段通配由 handler 内白名单校验，新增动作必须同时改白名单与本文：

- `POST /{p}/containers/{id}/{action}`（start/stop/restart/kill/…）
- `POST /{p}/sites/{domain}/{action}`（start/stop/restart/rebuild/…）
- `POST /{p}/runtimes/{id}/action/{action}`
- `POST /{p}/compose/{project}/{action}`

通配段必须注册在静态动作之后（如 `backup` 早于 `{action}`），避免被吞。

## 6. 方法与副作用

| 方法 | 用途 | 约束 |
| --- | --- | --- |
| GET | 页面、列表、详情、下载、SSE/WS 握手 | 不产生业务副作用；`download` 只落暂存并校验摘要 |
| POST | 全部写入、动作、表单提交 | 必须带 `csrf` 表单字段或头；Admin 权限按资源判定 |
| DELETE / PUT / PATCH | **不使用** | 统一 POST + 末段动词，避免代理与表单兼容问题 |

- 删除确认页（`GET /{p}/sites/{domain}/delete`）与执行（`POST` 同一路径）同路径，靠方法区分。
- 长连接：`GET /{p}/containers/{id}/logs/stream`（SSE，仅旧 SSR 详情页跟随）、`/{p}/containers/{id}/exec`（WS，需先 `POST .../exec/ticket` 换票）；`GET /{p}/containers/{id}/logs` 已是 budding 页面（尾部快照）。
- 失败回执：写操作 303 回页面，错误用 flash（`?` 不携带结果消息），不在 URL 里表达成败。
- `/ui/v1/*` 的写操作一律 POST + CSRF；未认证引导与登录为 `GET /ui/v1/auth/login` 与 `POST /ui/v1/auth/login`（登录用签名 CSRF token + 来源判定，CSRF 放 JSON 体 `csrf` 字段）；`POST /ui/v1/session/logout`、`session/password`、`session/totp`、`session/totp/enable` 为认证类例外，语义与 `/{p}/logout`、`/{p}/password`、`/{p}/totp`、`/{p}/totp/enable` 一一对应。
- `/ui/v1/*` 未认证返回 401 JSON，不做 303；已认证但权限不足返回 403 JSON。
- 写入口的守卫顺序固定为**先权限、后 CSRF**：无权限主体得到 `auth.forbidden`（SSR 为 403 页），不会先看到 `auth.csrf_invalid`。Docker 对象级授权（`protected` 对象）需要先解析容器，保留在 CSRF 之后，但角色门（`policy.check(actor, verb, None)`）仍在 CSRF 前。Compose 组同此顺序：Basic 档不提供 `ComposeUp/Down`，管理员在该档得到档位拒绝（`auth.forbidden`），CSRF 语义由 Lifecycle 档用例单独覆盖。

## 7. 兼容与下线

旧路径只保留**可验证的兼容**，不复制业务实现：

| 旧路径 | 方式 | 目标 / 行为 |
| --- | --- | --- |
| `/{p}/files/api/{*rest}` | 308 | `/{p}/files/{rest}`，保留方法与请求体 |
| `/{p}/docker/settings` | 308 | `/{p}/settings/docker` |
| `/{p}/store/plugins/{id}/install` 的 GET | 303 | 回 `/{p}/store` 并提示「安装必须用页面按钮」；安装仅接受 POST |
| `/{p}/images/{id}/remove`、`/{p}/networks/{id}/remove`、`/{p}/volumes/{name}/remove`、`/{p}/plugins/{id}/remove`、`/{p}/settings/keys/{id}/remove` | 别名 | 对应 `/delete` |
| `/{p}/settings/keys`（集合 POST） | 别名 | `POST /{p}/settings/keys/create` |
| `/{p}/m/plugin/{*rest}` | 内部转发 | 插件工作台，内核渲染功能页与 schema |
| `/{p}/m/overview` | SSR 页面 | 概览页暂保留 SSR（直到插件槽位迁完） |

- 301 只用于 **GET 页面**；带方法的请求一律 308 或同 handler 别名，禁止让表单 POST 落到 301。
- 别名清单只减不增。删除某条别名前，先在 `docs/changes/` 记一次「已下线」并确认仓库内无引用。
- 测试守门：`crates/web/tests/main_flow.rs::legacy_paths_redirect_to_canonical_urls` 覆盖跳转目标；
  新增兼容项必须加进该测试。
- **旧路由下线记录（2026-11-18）**：budding 壳页已全部覆盖资源面板，旧路由注册直接删除、访问返回 404，不做 301/308 重定向。
  下线清单：`/{p}/sites/env[/{*rest}]`（运行环境旧路径 20+ 条路由）、`/{p}/m/core/{*rest}`（SSR 深链 dispatch 分支）。
  仍维持兼容的路径见上表：`/{p}/files/api/*`、`/{p}/docker/settings`、`/{p}/store/plugins/{id}/install` GET、`remove`→`delete` 别名、
  `settings/keys` POST 别名、`/{p}/m/plugin/*` 插件工作台。

## 8. 查询参数

- 一律**小写、单数、无下划线**；只用于只读筛选、分页、页内标签与轮询游标。
- 允许集合：`q`（关键词）、`path`（文件路径）、`tab`（页内标签）、`type`（站点/日志类型）、
  `status`（状态过滤）、`filter`（容器过滤）、`sort` / `order`、`hidden`（显示隐藏文件）、
  `page`（分页）、`after`（SSE 游标）、`refresh` / `force`（强制刷新）、`project`（Compose 项目）、
  `file` / `ini` / `port`（运行环境详情定位）、`open`（插件页打开项）。
- 认证类例外：`next`（登录后跳转）、`u`（用户名预填，兼容 `username`）。
- 写操作的参数放**请求体**（表单字段），不放查询串；唯一例外是 SSE 游标 `after`。
- 新增参数必须先在本节登记；同义参数合并为一个，不新增别名。
