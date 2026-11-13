# URL 契约（页面与接口路径唯一依据）

> 本文是**新增与修改路由的唯一依据**。菜单身份（稳定键、深链）见 [`docs/MENU_CONTRACT.md`](MENU_CONTRACT.md)，
> 插件页面与 iframe 分工见 [`docs/PLUGIN_UI_CONTRACT.md`](PLUGIN_UI_CONTRACT.md)；
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
| 前端 JSON 接口 | `/ui/v1/...` | 无入口前缀；会话 Cookie + CSRF；未认证返回 401；仅 `handlers::ui_api`；只返 JSON |
| 机器 API | `/api/v1/...` | 无入口前缀、Token/会话鉴权，版本号固定；仅 `handlers::api` |
| 探针 | `/healthz` | 无前缀、无鉴权，只回健康状态 |
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
- `/{p}/static/*`、`/{p}/m/*`、`/ui/v1/*`、`/api/v1/*`、`/healthz` 不在白名单内。
- 命中白名单但前端路由不认识时，由前端渲染 404 视图；内核不回退到 `error.html`。
- 白名单变更必须同步本表、`docs/SDUI_BUDDING_INTEGRATION.md` 与 `main_flow` 测试。

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

## 7. 兼容与下线

旧路径只保留**可验证的兼容**，不复制业务实现：

| 旧路径 | 方式 | 目标 / 行为 |
| --- | --- | --- |
| `/{p}/files/api/{*rest}` | 308 | `/{p}/files/{rest}`，保留方法与请求体 |
| `/{p}/docker/settings` | 308 | `/{p}/settings/docker` |
| `/{p}/sites/env` | 301 | `/{p}/runtimes`（保留查询串） |
| `/{p}/sites/env/{id}` | 301 | `/{p}/runtimes/{id}`（保留查询串） |
| `/{p}/sites/env/{id}/...` 的各 POST 动作 | 别名 | 与 `/{p}/runtimes/{id}/...` 同一 handler |
| `/{p}/sites/env/add`、`/{p}/runtimes/add` | 别名 | `POST /{p}/runtimes/create` |
| `/{p}/store/plugins/{id}/install` 的 GET | 303 | 回 `/{p}/store` 并提示「安装必须用页面按钮」；安装仅接受 POST |
| `/{p}/images/{id}/remove`、`/{p}/networks/{id}/remove`、`/{p}/volumes/{name}/remove`、`/{p}/plugins/{id}/remove`、`/{p}/settings/keys/{id}/remove` | 别名 | 对应 `/delete` |
| `/{p}/settings/keys`（集合 POST） | 别名 | `POST /{p}/settings/keys/create` |
| `/{p}/m/{*rest}` | 内部转发 | 菜单深链，不重定向 |

- 301 只用于 **GET 页面**；带方法的请求一律 308 或同 handler 别名，禁止让表单 POST 落到 301。
- 别名清单只减不增。删除某条别名前，先在 `docs/changes/` 记一次「已下线」并确认仓库内无引用。
- 测试守门：`crates/web/tests/main_flow.rs::legacy_paths_redirect_to_canonical_urls` 覆盖跳转目标；
  新增兼容项必须加进该测试。

## 8. 查询参数

- 一律**小写、单数、无下划线**；只用于只读筛选、分页、页内标签与轮询游标。
- 允许集合：`q`（关键词）、`path`（文件路径）、`tab`（页内标签）、`type`（站点/日志类型）、
  `status`（状态过滤）、`filter`（容器过滤）、`sort` / `order`、`hidden`（显示隐藏文件）、
  `page`（分页）、`after`（SSE 游标）、`refresh` / `force`（强制刷新）、`project`（Compose 项目）、
  `file` / `ini` / `port`（运行环境详情定位）、`open`（插件页打开项）。
- 认证类例外：`next`（登录后跳转）、`u`（用户名预填，兼容 `username`）。
- 写操作的参数放**请求体**（表单字段），不放查询串；唯一例外是 SSE 游标 `after`。
- 新增参数必须先在本节登记；同义参数合并为一个，不新增别名。

## 9. 现有路由清单（按资源）

> 只列规范路径；别名见 §7。`{p}` 为入口前缀。

| 资源 | 路径 |
| --- | --- |
| 概览 | `GET /{p}`、`GET /{p}/`（尾斜杠 302 到根） |
| 会话 | `GET|POST /{p}/login`、`POST /{p}/logout`、`POST /{p}/password`、`POST /{p}/totp`、`POST /{p}/totp/enable` |
| 容器 | `GET /{p}/containers`、`/{p}/containers/{id}` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/docker/containers[/{id}]`）；`GET /{p}/containers/{id}/terminal` 为 budding 终端页（复用既有 exec 票据 + WS）；`GET /{p}/containers/{id}/logs` 为 budding 日志页（尾部快照，JSON 等价 `GET /ui/v1/containers/{id}/logs?tail=`）；`GET /containers/{id}/logs/stream`（SSE，仅旧 SSR 详情页跟随）；`GET /containers/rows`、`/containers/new`（壳页原生表单）；`POST /containers/create`、`/{id}/limits`、`/{id}/{action}`、`/{id}/exec/ticket`；`GET /{id}/exec`；JSON 等价 `POST /ui/v1/containers`、`/containers/{id}/limits`、`/containers/{id}/{action}` |
| 镜像 | `GET /{p}/images` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/docker/images`）；`POST /images/pull`、`/images/{id}/delete`；JSON 等价 `POST /ui/v1/images/pull`、`/images/{id}/remove` |
| 网络 | `GET /{p}/networks` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/docker/networks`）；`POST /networks/create`、`/networks/{id}/delete`；JSON 等价 `POST /ui/v1/networks`、`/networks/{id}/remove` |
| 卷 | `GET /{p}/volumes` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/docker/volumes`）；`POST /volumes/create`、`/volumes/{name}/delete`；JSON 等价 `POST /ui/v1/volumes`、`/volumes/{name}/remove` |
| Compose | `GET /{p}/compose` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/docker/compose`）；`POST /compose/validate`、`/up`、`/down`、`/{project}/{action}`；JSON 等价 `POST /ui/v1/compose/validate`、`/ui/v1/compose/up`、`/ui/v1/compose/down`、`/ui/v1/compose/{project}/{action}` |
| 站点 | `GET /{p}/sites`、`/sites/{domain}`、`/sites/new` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/sites/list[/{domain}]`）；`GET /sites/{domain}/logs` 默认交壳页日志查看器（旧 SSR 仅 `?legacy=1`）；`GET /sites/web` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/sites/web`）；`POST /sites/create`、`/batch`、`/{domain}/config/save`、`/{domain}/domains`、`/{domain}/ssl`、`/{domain}/default`、`/{domain}/backup`、`/{domain}/{action}`、`/{domain}/delete`、`/web/config/main`、`/web/config/params`；JSON 等价 `POST /ui/v1/sites`、`/sites/batch`、`/sites/{domain}/{action}`、`/sites/{domain}/default`、`/sites/{domain}/domains`、`/sites/{domain}/config`、`/sites/{domain}/ssl`、`/sites/{domain}/backup`、`/sites/{domain}/delete`、`/web/config`、`/web/params`、`GET /ui/v1/sites/{domain}/logs?part=&tail=&q=`、`GET /ui/v1/web`、`GET /ui/v1/web/logs` |
| 运行环境 | `GET /{p}/runtimes`、`/runtimes/{id}`、`/runtimes/{id}/logs` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/sites/env[/{id}]`）；`/runtimes/{id}/logs` 为原生日志页（JSON 等价 `GET /ui/v1/runtimes/{id}/logs?tail=`），旧详情抽屉改用 `GET /runtimes/{id}/logs.json`；`GET /runtimes/jobs`、`/{id}/health`、`/{id}/job`；写操作 JSON 等价 `POST /ui/v1/runtimes`、`/runtimes/images`、`/runtimes/{id}/config`、`/node-config`、`/port`、`/clone`、`/config-file`、`/action/{action}`、`/extensions`、`/extensions/batch`、`/extensions/{key}/install|uninstall`、`/pecl`、`/probe`、`/sites/{domain}/unbind`；详情聚合 `GET /ui/v1/runtimes/{id}[/?file=key]`、目录 `GET /ui/v1/runtimes/catalog`、镜像参数 `GET /ui/v1/runtimes/images` |
| 文件 | `GET /{p}/files` 已交付 budding 原生页（平铺目录 + 面包屑，不做树形；旧 SSR 保留在 `GET /{p}/m/core/files`）；`GET /files/view?path=` 为 budding 代码查看器；`/files/list`、`/files/read`、`/files/tree`、`/files/download`；`POST /files/write`、`/mkdir`、`/rename`、`/remove`、`/upload`、`/copy`、`/chmod`、`/chown`、`/archive`、`/extract`；JSON 等价接口 `GET /ui/v1/files`、`GET /ui/v1/files/read`、`POST /ui/v1/files/mkdir`、`/rename`、`/remove` |
| 回收站 | `GET /{p}/files/trash` 已交付 budding 壳页（`FILE_TRASH_READ`；旧 SSR 保留在 `GET /{p}/m/core/files/trash`）；`GET /files/trash/list`；`POST /files/trash/restore`、`/purge`、`/clear`（前端直接复用这些 JSON 接口，CSRF 头与审计不变，未另建 `/ui/v1` 写入口） |
| 配额 | `GET /{p}/files/quotas` 已交付 budding 壳页（`FILE_QUOTA_READ`；旧 SSR 保留在 `GET /{p}/m/core/files/quotas`）；`GET /files/quotas/list`；`POST /files/quotas/set`、`/cancel`、`/scan`（同上，直接复用既有 JSON 接口） |
| 插件 | `GET /{p}/plugins`、`/plugins/{id}` 已交付 budding 壳页（旧 SSR 列表 `/m/core/plugins/{installed\|discover\|updates}`、信息页 `/m/core/plugins/info/{id}` 保留但不再作为新入口）；`GET /{p}/plugins/{id}/{view}` 由壳页渲染随包 schema，`GET /{p}/plugins/{id}/schema/{view}` 返回该视图的 schema JSON（`PluginManage`，与 RPC 同权限）；`/plugins/{id}/ui/{path}` 与显式 `?legacy=1` 工作台保留兼容；旧表单 `POST /plugins/install`、`/confirm`、`/{id}/toggle`、`/{id}/delete`、`/{id}/rpc` 保留，JSON 等价 `GET /ui/v1/plugins`、`GET /ui/v1/plugins/{id}`、`POST /ui/v1/plugins/{id}/toggle`、`POST /ui/v1/plugins/{id}/delete` |
| 商店 | `GET /{p}/store` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/plugins/store`，仅作无 JS 兜底）；旧表单入口 `POST /store/plugins/{id}/install`、`GET /store/upgrades/{version}/download` 保留；JSON 等价接口 `GET /ui/v1/store`、`POST /ui/v1/store/plugins/{id}/install`、`POST /ui/v1/store/upgrades/{version}/download`、`POST /ui/v1/plugins/confirm` |
| 设置 | `GET /{p}/settings` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/settings/security`，仅作无 JS 兜底）；`GET /{p}/settings/menu`、`/{p}/settings/docker` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/settings/menu`、`/{p}/m/core/docker/settings`）；`POST /settings/menu`、`/settings/docker`、`/settings/keys/create`、`/settings/keys/{id}/delete`、`/settings/panel`（JSON 等价接口 `POST /ui/v1/settings/panel`、`/settings/password`、`/settings/totp/setup`、`/settings/totp/enable`、`/settings/keys`、`/settings/keys/{id}`、`/settings/menu`、`/settings/docker`） |
| 审计 | `GET /{p}/audit` 已交付 budding 原生只读列表（关键字 + 签名 keyset 游标）；旧 SSR 保留在 `GET /{p}/m/core/settings/audit`（最近 200 条）；JSON 等价接口 `GET /ui/v1/audit?q=&limit=&cursor=`；`AuditRead` 权限不变，viewer 直接访问 403、菜单不可见 |
| 定时任务 | `GET /{p}/tasks`、`/tasks/{id}` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/tasks[/{id}]`）；`POST /tasks/{id}/toggle`、`/run`、`/delete`；JSON 等价 `POST /ui/v1/tasks/{id}/toggle`、`/run`、`/delete` |
| 任务中心 | `GET /{p}/jobs`、`/jobs/{id}` 已交付 budding 壳页（旧 SSR 保留在 `GET /{p}/m/core/jobs[/{id}]`）；`GET /jobs/{id}/logs` 已交付 budding 日志查看器（消费既有增量 JSON，不建 SSE）；`GET /jobs/{id}/logs/stream`；`POST /jobs/{id}/cancel`、`/retry`；JSON 等价 `POST /ui/v1/jobs/{id}/cancel`、`/retry` |
| 菜单深链 | `GET /{p}/m/{rest}` |
| 前端 JSON（UI） | `GET /ui/v1/auth/login`、`POST /ui/v1/auth/login`（未认证）、`GET /ui/v1/session`、`GET /ui/v1/menu`、`GET /ui/v1/overview`、`GET /ui/v1/sites`、`GET /ui/v1/sites/{domain}`、`POST /ui/v1/sites`、`POST /ui/v1/sites/batch`、`POST /ui/v1/sites/{domain}/{action}`、`POST /ui/v1/sites/{domain}/default`、`POST /ui/v1/sites/{domain}/domains`、`POST /ui/v1/sites/{domain}/config`、`POST /ui/v1/sites/{domain}/ssl`、`POST /ui/v1/sites/{domain}/backup`、`POST /ui/v1/sites/{domain}/delete`、`GET /ui/v1/runtimes`、`GET /ui/v1/runtimes/{id}`、`GET /ui/v1/runtimes/catalog`、`GET /ui/v1/runtimes/images`、`POST /ui/v1/runtimes/images`、`POST /ui/v1/runtimes`、`POST /ui/v1/runtimes/{id}/action/{action}`、`POST /ui/v1/runtimes/{id}/config`、`POST /ui/v1/runtimes/{id}/node-config`、`POST /ui/v1/runtimes/{id}/port`、`POST /ui/v1/runtimes/{id}/clone`、`POST /ui/v1/runtimes/{id}/extensions`、`POST /ui/v1/runtimes/{id}/extensions/batch`、`POST /ui/v1/runtimes/{id}/extensions/{key}/{op}`、`POST /ui/v1/runtimes/{id}/pecl`、`POST /ui/v1/runtimes/{id}/config-file`、`POST /ui/v1/runtimes/{id}/probe`、`POST /ui/v1/runtimes/{id}/sites/{domain}/unbind`、`GET /ui/v1/web`、`GET /ui/v1/web/logs`、`POST /ui/v1/web/config`、`POST /ui/v1/web/params`、`POST /ui/v1/compose/validate`、`POST /ui/v1/compose/up`、`POST /ui/v1/tasks/{id}/toggle`、`POST /ui/v1/tasks/{id}/run`、`POST /ui/v1/tasks/{id}/delete`、`POST /ui/v1/jobs/{id}/cancel`、`POST /ui/v1/jobs/{id}/retry`、`GET /ui/v1/containers`、`GET /ui/v1/containers/{id}`、`POST /ui/v1/containers`、`POST /ui/v1/containers/{id}/limits`、`POST /ui/v1/containers/{id}/{action}`、`GET /ui/v1/containers/{id}/logs`、`GET /ui/v1/runtimes/{id}/logs`、`GET /ui/v1/images`、`POST /ui/v1/images/pull`、`POST /ui/v1/images/{id}/remove`、`GET /ui/v1/networks`、`POST /ui/v1/networks`、`POST /ui/v1/networks/{id}/remove`、`GET /ui/v1/volumes`、`POST /ui/v1/volumes`、`POST /ui/v1/volumes/{name}/remove`、`GET /ui/v1/compose`、`POST /ui/v1/compose/down`、`POST /ui/v1/compose/{project}/{action}`、`GET /ui/v1/tasks`、`GET /ui/v1/tasks/{id}`、`GET /ui/v1/jobs`、`GET /ui/v1/jobs/{id}`、`GET /ui/v1/settings`、`GET /ui/v1/settings/menu`、`POST /ui/v1/settings/menu`、`GET /ui/v1/settings/docker`、`POST /ui/v1/settings/docker`、`POST /ui/v1/settings/panel`、`POST /ui/v1/settings/password`、`POST /ui/v1/settings/totp/setup`、`POST /ui/v1/settings/totp/enable`、`POST /ui/v1/settings/keys`、`POST /ui/v1/settings/keys/{id}`、`GET /ui/v1/audit`、`GET /ui/v1/files`、`GET /ui/v1/files/read`、`POST /ui/v1/files/mkdir`、`POST /ui/v1/files/rename`、`POST /ui/v1/files/remove`、`GET /ui/v1/sites/{domain}/logs`、`GET /ui/v1/store`、`POST /ui/v1/store/plugins/{id}/install`、`POST /ui/v1/plugins/confirm`、`GET /ui/v1/plugins`、`GET /ui/v1/plugins/{id}`、`POST /ui/v1/plugins/{id}/toggle`、`POST /ui/v1/plugins/{id}/delete`、`POST /ui/v1/session/logout`（其余资源按实现前逐条登记：认证、站点、运行时、Docker、文件、任务、作业、插件、设置、审计） |
| 壳页回落 | `GET /{p}` 与 `GET /{p}/{白名单前缀}[/{rest}]`（仅 `Accept: text/html`；白名单见 §2.1） |
| 静态资源 | `GET /{p}/static/{path}` |
| API v1 | `POST /api/v1/session`；`GET /api/v1/docker/containers`、`/docker/containers/{id}`、`/plugins`、`/plugins/ui`；`POST /api/v1/docker/containers/{id}/restart` |

## 10. 变更流程

1. 新路由先在本表登记资源、方法与动作，再写代码；不允许「先实现后补文档」。
2. 改已有路径必须同时提供兼容（§7）或明确下线记录，并更新引用它的模板、静态资源与测试。
3. 代码侧守门：`crates/web/src/assets.rs` 校验静态资源 API 基址；`main_flow.rs` 校验旧路径跳转。
4. 文档侧引用：`MENU_CONTRACT.md` 的旧路由映射表只描述**菜单键 → URL**，URL 形状以本文为准。

## 11. 变更记录

| 日期 | 变更 |
| --- | --- |
| 2026-11-12 | 新增 `/ui/v1` JSON 命名空间例外（无前缀、Cookie + CSRF、未认证 401、仅 `handlers::ui_api`、只返 JSON）；`/ui/` 其它形态一律 404；新增 §2.1 SPA 壳页回落白名单（仅 `GET` + `Accept: text/html` + 命中页面前缀；写请求永不回落）；§6 补 `/ui/v1` 方法与认证例外；§9 登记 `/ui/v1` 与壳页回落。D13 定案：history 路由，地址栏保持 `/{p}/sites/xxx`。 |
| 2026-11-13 | 第一批只读页迁移：`GET /{p}/sites`、`/{p}/sites/{domain}`、`/{p}/runtimes`、`/{p}/runtimes/{id}` 改交付 budding 壳页，旧 SSR 保留在 `GET /{p}/m/core/sites/list[/{domain}]` 与 `/{p}/m/core/sites/env[/{id}]`；`/ui/v1` 新增 `GET /sites`、`GET /sites/{domain}`、`GET /runtimes`（未认证 401、viewer 读站点、运行环境仅管理员）。 |
| 2026-11-13 | 第二批容器页迁移：`GET /{p}/containers`、`/{p}/containers/{id}` 改交付 budding 壳页，旧 SSR 保留在 `GET /{p}/m/core/docker/containers[/{id}]`；`/ui/v1` 新增 `GET /containers`、`GET /containers/{id}`、`POST /containers/{id}/{action}`（CSRF 走 `x-csrf-token`，动作与旧 htmx 路径同策略同审计）。 |
| 2026-11-13 | 第三批镜像/网络/卷列表迁移：`GET /{p}/images`、`/{p}/networks`、`/{p}/volumes` 改交付 budding 壳页，旧 SSR 保留在 `GET /{p}/m/core/docker/images|networks|volumes`；`/ui/v1` 新增三个只读列表接口，写操作本批未迁移。 |
| 2026-11-13 | 第四批编排页迁移：`GET /{p}/compose` 改交付 budding 壳页，旧 SSR 保留在 `GET /{p}/m/core/docker/compose`；`/ui/v1` 新增 `GET /compose`、`POST /compose/down`、`POST /compose/{project}/{action}`（编辑/校验/部署仍走旧版页面）。 |
| 2026-11-13 | 第五批任务与作业只读迁移：`GET /{p}/tasks`、`/{p}/tasks/{id}`、`/{p}/jobs`、`/{p}/jobs/{id}` 改交付 budding 壳页，旧 SSR 保留在 `GET /{p}/m/core/tasks|jobs[/{id}]`；`/ui/v1` 新增四个只读接口，启停/取消/重试/日志流仍走旧版。 |
| 2026-11-13 | 第六批审计迁移受阻：后端只有 `recent_audit(limit)`，无关键字/游标查询；按红线不自行新增 SQL，`GET /{p}/audit` 先交付 budding 占位壳页，旧 SSR 保留在 `GET /{p}/m/core/settings/audit`，待审计查询落地后再补 JSON 接口与翻页。 |
| 2026-11-13 | 第七批设置页迁移：`GET /{p}/settings` 改交付 budding 壳页，旧 SSR 保留在 `GET /{p}/m/core/settings/security`；`/ui/v1` 新增 `GET /settings` 与 `POST /settings/panel`（复用旧表单校验/落盘/审计），口令 / TOTP / 公钥增删仍走旧页。 |
| 2026-11-13 | 第八批商店页迁移：`GET /{p}/store` 改交付 budding 壳页，旧 SSR 保留在 `GET /{p}/m/core/plugins/store`；新增 `GET /ui/v1/store`、`POST /ui/v1/store/plugins/{id}/install`、`POST /ui/v1/plugins/confirm`，下载验签与安装收尾分别复用 `store::prepare`、`plugins::verify_for_confirm` + `commit_install`；升级包下载仍走旧页 GET。 |
| 2026-11-13 | 第九批插件页迁移：`GET /{p}/plugins`、`/{p}/plugins/{id}` 改交付 budding 壳页，旧 SSR 列表保留在 `/{p}/m/core/plugins/{installed\|discover\|updates}`、信息页新增深链 `/{p}/m/core/plugins/info/{id}`；`/{p}/plugins/{id}/{view}` 与 `/ui/{path}` 的 iframe 槽位不变；新增 `GET /ui/v1/plugins`、`GET /ui/v1/plugins/{id}`、`POST /ui/v1/plugins/{id}/toggle`、`POST /ui/v1/plugins/{id}/delete`（复用 `apply_toggle` / `apply_remove`）。 |
| 2026-11-13 | 第十批文件管理/终端：`GET /{p}/files` 改交付 budding 占位壳页（依赖 T1/T2/T5，终端需另立 WS 契约），旧 SSR 保留在 `GET /{p}/m/core/files`；`/files/*` 接口与回收站/配额子页不变，受限形态仍由 fs 能力面门禁 403。 |
| 2026-11-13 | 插件功能页原生化（第一批 guard）：`api = "1.3"` 插件可在 `[[menu]]` / `[[menu.items]]` / `[[mount]]` 声明 `schema`（`ui/schema/<view>.json`，安装期静态校验）；新增 `GET /{p}/plugins/{id}/schema/{view}`（`PluginManage`，与 `POST /{id}/rpc` 同权限），`GET /{p}/plugins/{id}/{view}` 改由壳页加载 schema 渲染；`/{p}/m/plugin/...` 旧工作台同权限收口为 `PluginManage`；旧 SSR/`ui/{path}` 与 `?legacy=1` 保留。 |
| 2026-11-13 | 后台外壳接入：壳页由「单页卡片」改为 `admin` 外壳 + `outlet`，菜单由新增的 `GET /ui/v1/menu` 下发（投影内核菜单树）；前端路由 path 直接带 `/{p}` 前缀且不设 `routerBase`；新增壳内路由 `/{p}/store/confirm`。 |
| 2026-11-13 | 第十一批设置页补齐：`/ui/v1` 新增 `POST /settings/password`（轮换会话并回传新 `csrf_token`）、`/settings/totp/setup`、`/settings/totp/enable`、`/settings/keys`、`/settings/keys/{id}`（公钥两项仅 Admin，审计 `settings.key.import` / `settings.key.remove`）；设置页口令 / TOTP / 公钥不再依赖旧 SSR，旧页仅作无 JS 兜底。 |
| 2026-11-13 | 第十二批文件管理原生化：`GET /{p}/files` 改为 budding 原生平铺目录（面包屑，无树形）；`/ui/v1` 新增 `GET /files`、`POST /files/mkdir`、`POST /files/rename`、`POST /files/remove`（CSRF，删除进回收站）；上传仍走既有 `POST /{p}/files/upload` 分片接口（前端按 `upload_chunk_bytes` 切片），旧 SSR 仅保留回收站 / 配额页。 |
| 2026-11-13 | 第十三批代码/日志查看器：`/{p}/files/view?path=`（代码查看器）、`/{p}/jobs/{id}/logs`（作业日志查看器）新增；`/{p}/sites/{domain}/logs` 默认交壳页，旧 SSR 仅 `?legacy=1`；`/ui/v1` 新增 `GET /files/read`、`GET /sites/{domain}/logs?part=&tail=&q=`；日志按需加载，不建 SSE。 |
| 2026-11-13 | 第十四批容器终端：新增 `GET /{p}/containers/{id}/terminal` budding 页；通道复用既有 `POST /{p}/containers/{id}/exec/ticket` 与 `GET /{p}/containers/{id}/exec?ticket=`（票据 30 秒、一次即废、绑定会话与容器，Origin 严格校验，审计沿用 `Verb::Exec`），不新建 WS 契约、不新增权限来源。 |
| 2026-11-13 | 第十六批日志入口收口：`GET /{p}/containers/{id}/logs` 由旧 SSR SSE 页改为 budding 日志页（尾部快照 + 刷新），SSE 流移至 `GET /{p}/containers/{id}/logs/stream`（仅旧 SSR 详情页跟随）；新增 `GET /{p}/runtimes/{id}/logs` budding 日志页，旧详情抽屉改用 `GET /{p}/runtimes/{id}/logs.json`；`/ui/v1` 新增 `GET /containers/{id}/logs?tail=`（`ContainerLogs`/`DockerRead`）与 `GET /runtimes/{id}/logs?tail=`（Admin）；无新增长连接契约。 |
| 2026-11-13 | 第十七批 Docker 写操作原生化：`GET /{p}/containers/new` 改交付壳页（原生表单，`ContainerCreate` 门禁不变）；`/ui/v1` 新增 `POST /containers`、`POST /containers/{id}/limits`、`POST /images/pull`、`POST /images/{id}/remove`、`POST /networks`、`POST /networks/{id}/remove`、`POST /volumes`、`POST /volumes/{name}/remove`，全部复用既有权限矩阵、CSRF 头、审计与 `docker_cmd` 业务入口；旧 SSR 写路径保留但不再作为新前端入口。 |
| 2026-11-13 | 第十八批站点写操作原生化：`GET /{p}/sites/new` 改交付壳页（`SITE_CREATE` 门禁不变）；`/ui/v1` 新增 `POST /sites`、`POST /sites/batch`、`POST /sites/{domain}/{action}`、`POST /sites/{domain}/default`、`POST /sites/{domain}/domains`、`POST /sites/{domain}/config`、`POST /sites/{domain}/ssl`、`POST /sites/{domain}/backup`、`POST /sites/{domain}/delete`，全部复用 `SITE_*` 动作、CSRF 头、审计与 `commands::site`；详情响应新增 `config` / `backup` / `can_manage`，列表新增 `can_admin`（只增不改）。 |
| 2026-11-13 | 第十九批 Compose 原生化 + 壳页未认证一致性修复：`/ui/v1` 新增 `POST /compose/validate`（只解析校验）与 `POST /compose/up`（校验后部署），复用 `planned_project` / `validate_specs` / `compose_up` 与既有权限审计；`{p}/compose` 壳页补编辑、校验、部署表单。修复 SPA 回落未认证分支：命中白名单的 GET 不再先看 `Accept`，与 `/{p}/files` 一致 303 到登录页；已认证非 HTML `Accept` 仍 404，写请求仍不回落。 |
| 2026-11-13 | 第二十批任务与作业写操作原生化：`/ui/v1` 新增 `POST /tasks/{id}/toggle`、`/run`、`/delete` 与 `POST /jobs/{id}/cancel`、`/retry`，复用 `TASK_*` / `JOB_*` 动作、CSRF 头、审计与既有命令层；作业列表新增 `can_admin`（只增不改）。旧 SSR 写路径保留但不再作为新前端入口。 |
| 2026-11-13 | 第二十一批文件回收站/配额原生化：`GET /{p}/files/trash`、`/{p}/files/quotas` 改交付 budding 壳页，入口门禁仍是 `FILE_TRASH_READ` / `FILE_QUOTA_READ`，旧 SSR 留在 `GET /{p}/m/core/files/trash|quotas`；前端直接复用既有 `/files/trash/{list,restore,purge,clear}` 与 `/files/quotas/{list,set,cancel,scan}` JSON 接口（CSRF 头、审计、`commands::file` 不变），未新增 `/ui/v1` 写入口；`GET /ui/v1/files` 新增 `can_admin`（只增不改）。 |
| 2026-11-13 | 第二十二批设置菜单/Docker 设置原生化：`GET /{p}/settings/menu`、`/{p}/settings/docker` 改交付 budding 壳页，旧 SSR 留在 `GET /{p}/m/core/settings/menu`、`/{p}/m/core/docker/settings`；`/ui/v1` 新增 `GET|POST /settings/menu`（复用 `menu::apply_action` 与同一审计）与 `GET|POST /settings/docker`（复用 `docker_settings::apply_settings` 的落盘/回滚/审计）；菜单导航 href 改指规范路径，`/{p}/docker/settings` 的 308 兼容不变。 |
| 2026-11-13 | 第二十三批运行环境配置/安装/扩展原生化：`/ui/v1` 新增 `GET /runtimes/{id}[?file=]` 详情聚合、`GET /runtimes/catalog`、`GET|POST /runtimes/images`，以及 `POST /runtimes`、`/{id}/action/{action}`、`/config`、`/node-config`、`/port`、`/clone`、`/extensions`、`/extensions/batch`、`/extensions/{key}/{op}`、`/pecl`、`/config-file`、`/probe`、`/sites/{domain}/unbind`；全部复用 `runtime_cmd`、既有动作权限、CSRF 头与审计，旧表单路由保留；`/{p}/runtimes/{id}` 保持壳页，旧 SSR 详情留在 `GET /{p}/m/core/sites/env/{id}`。 |
| 2026-11-13 | 第二十四批 Web 服务页原生化：`GET /{p}/sites/web` 改交付 budding 壳页（Admin-only，旧 SSR 留在 `GET /{p}/m/core/sites/web`）；`/ui/v1` 新增 `GET /web`（状态/安装进度/主配置/包含与站点配置/性能参数/模块）、`GET /web/logs`（按需读取尾部）、`POST /web/config` 与 `POST /web/params`（复用抽出后的 `nginx::apply_main` / `apply_params`，`nginx -t`、重载、审计不变）；菜单 href 改指 `/{p}/sites/web`。 |
| 2026-11-13 | 第二十五批旧 SSR 回落收口：插件菜单键 `core/plugins/discover`、`core/plugins/updates` 登记为原生路由 `/{p}/plugins/discover`、`/{p}/plugins/updates`（页内标签同路径，旧 `?tab=` 兼容），不再退回内核 href；`/ui/v1` 新增 `POST /store/upgrades/{version}/download`（仅 Admin + `x-csrf-token`，与旧表单 GET 共用 `store::apply_upgrade_download` 的公钥校验、暂存、sha256 与 `upgrade.download` 审计），商店页不再用旧 SSR 链接承接升级包下载/表单确认。旧 SSR 路由与模板全部保留。 |
| 2026-11-13 | 插件功能页原生化（第二批 monitor）：`/{p}/plugins/com.skiff.monitor/{overview,history,process,network}` 改由壳页加载随包 schema；`processes` / `network` RPC 新增只读 `all` 参数（一次取回当前快照，供表格本地排序/筛选/分页），旧分页参数与返回字段不变；`GET /{p}/plugins/{id}/schema/{view}` 与 `POST /{id}/rpc` 权限不变；`overview.health` 槽位仍由旧页承载。 |
| 2026-11-13 | 插件功能页原生化（第三批 firewall）：`/{p}/plugins/com.skiff.firewall/{rules,fail2ban}` 改由壳页加载 schema；写操作仍只经 `POST /{id}/rpc`（CSRF + `PluginManage` + 审计），表单值由壳页合并、数值由插件归一化；`skiffPluginView.query` 只承载标量并拼到当前插件页 query（如 `?jail=sshd`），不产生新路由。 |
| 2026-11-13 | 插件功能页原生化（第四批 ftp）：`/{p}/plugins/com.skiff.ftp/{accounts,runtime}` 改由壳页加载 schema；账号行「重置密码」用 `?login=<name>` 预填页内下拉，不新增路由；所有写操作仍只经 `POST /{id}/rpc`，可选防火墙插件缺失时防护接口返回原因/空列表。 |
| 2026-11-13 | 插件功能页原生化（第五批 cron）：`/{p}/plugins/com.skiff.cron/list` 改由壳页加载 schema；列表行「编辑」用 `?id=<task-id>` 预填同一表单，不新增路由；执行/启停/删除/创建/更新仍只经 `POST /{id}/rpc`；旧 SSR 与 `?legacy=1` 预设编辑器保留。 |
