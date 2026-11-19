# 路径与动作规则：前缀、命名、同构、方法与副作用、兼容、查询参数（§1–8）

> 上级索引：[URL_CONTRACT.md](../URL_CONTRACT.md) ｜ 拆自 `docs/contract/URL_CONTRACT.md` 原 §1–8（2026-11-16 整理，正文未改）。

---

> 本文是**新增与修改路由的唯一依据**。菜单身份（稳定键、深链）见 [`docs/contract/MENU_CONTRACT.md`](../MENU_CONTRACT.md)，
> 插件页面与 iframe 分工见 [`docs/contract/PLUGIN_UI_CONTRACT.md`](../PLUGIN_UI_CONTRACT.md)；
> 三者的关系：**菜单键稳定、URL 规范、菜单键 ≠ URL**。改 URL 不改菜单键，改菜单键不改本文。

## 1. 总原则

1. **一个资源一套路径**：壳页深链、页面内 JSON 与长连接按资源归位，不额外插 `api` 中缀（文件分组的页面与接口共用 `/{p}/files`，其余资源的 JSON 统一在 `/ui/v1/{资源}`）。
2. **层级只表达归属**：`/{前缀}/{资源}/{标识}/{子资源}`，最多四段（不含前缀与动作）。
3. **动作只出现在末段**：写操作末段必须是动词或明确的动作名，不用 `?do=`、不把动作放中间。
4. **GET 只读，POST 写入**：GET 不产生副作用；所有写入都是 POST 且带 CSRF；下载类 GET 例外见 §6。
5. **规范路径唯一**：旧路径只做兼容（§7），不并行维护两套语义；新增路由不得再产生别名。

## 2. 前缀与例外

| 范围 | 形状 | 说明 |
| --- | --- | --- |
| 页面（壳页回落） | `/{p}/...` | `p` 为安全入口前缀（`config.toml` 的 `entry_prefix`），默认 `p`，运行时可变；只命中白名单前缀（§2.1） |
| 前端 JSON 接口 | `/ui/v1/...` | 无入口前缀；会话 Cookie + CSRF；未认证返回 401；仅 `handlers::ui`（按资源拆分）；只返 JSON |
| 机器 API | `/api/v1/...` | 无入口前缀、Token/会话鉴权，版本号固定；仅 `handlers::api` |
| 探针 | `/healthz` | 无前缀、无鉴权，只回健康状态 |
| 图标 | `/favicon.ico` | 无前缀、无鉴权，返回品牌矢量图标（`image/svg+xml`，`public, max-age=86400`）；页面另用 `<link rel="icon">` 指向 `/{p}/static/favicon.svg?v=<版本>` |

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
- `/{p}/static/*`、`/ui/v1/*`、`/api/v1/*`、`/healthz`、`/favicon.ico` 不在白名单内。
- 命中白名单但前端路由不认识时，由前端渲染 404 视图；内核不回退到 `error.html`。
- 白名单变更必须同步本表、`docs/sdui/SDUI_BUDDING_INTEGRATION.md` 与 `main_flow` 测试。

## 3. 资源命名

- 资源段用**复数、小写、中划线**：`containers`、`runtimes`、`settings`、`store`、`tasks`、`jobs`。
- 标识段用资源的天然主键：容器/镜像 id、站点域名、运行环境 id（`php-8.5`）、任务 id、插件 id（含 `.`）。
- 子资源用**单数**名词：`/{p}/sites/{domain}/ssl`、`/{p}/runtimes/{id}/config`、`/{p}/files/trash`。
- 子资源下的集合仍用复数：`/{p}/runtimes/{id}/extensions`。
- 标识含 `/`、空格等字符时由路由层编码，不把多段拼进一个参数；插件 id 的 `.` 不二次编码。

## 4. 页面与页面内 JSON 的归属

页面只有**一个**入口：`GET /{p}[/{白名单前缀}[/{rest}]]` 交同一个 budding 壳页（§2.1），
**没有逐资源的页面路由**（`/{p}/sites/new`、`/{p}/containers/{id}/terminal` 之类都不是独立路由，
只是壳页回落的深链，前端路由自己认）。页面内的数据与动作统一走 `/ui/v1/{资源}`：

| 页面（壳页回落） | 页面内 JSON（`/ui/v1`） |
| --- | --- |
| `/{p}/containers` | `GET /ui/v1/containers`、`POST /ui/v1/containers` |
| `/{p}/sites` | `GET /ui/v1/sites`、`POST /ui/v1/sites` |
| `/{p}/settings` | `GET /ui/v1/settings`、`POST /ui/v1/settings/panel` |

**唯一例外是文件分组**：它由内核注册真实路由（形态门禁 `fs_capability`），页面与接口共用
`/{p}/files` 前缀，接口为 `/{p}/files/list`、`/{p}/files/write`…，**没有** `/{p}/files/api/list`
这类中缀（旧前缀 `/{p}/files/api/*` 只 308 兼容，见 §7）；回收站与配额同属该分组
（`/{p}/files/trash/*`、`/{p}/files/quotas/*`），前端直接复用这些 JSON 接口，未另建 `/ui/v1` 写入口。

同构的理由只有一个：前端拼 URL 时归属一眼可见，页面与接口不在两个前缀下分叉授权口径。

长连接与轮询落在资源段下，不带 `/ui/v1`（它们要走会话 Cookie 与入口前缀）：

| 形态 | 形状 | 示例 |
| --- | --- | --- |
| 列表 / 详情 | `GET /ui/v1/{资源}[/{id}]` | `/ui/v1/containers`、`/ui/v1/jobs/{id}` |
| 日志尾随（SSE） | `GET /{p}/{资源}/{id}/logs/stream` | `/{p}/jobs/{id}/logs/stream` |
| 终端（WS） | `POST /{p}/containers/{id}/exec/ticket` 换一次性票据，再 `GET /{p}/containers/{id}/exec?ticket=` | §6.2 |

## 5. 动作命名

### 5.1 规则

- 资源**集合**：`GET /ui/v1/{资源}` 列表；`POST /ui/v1/{资源}` 创建（**不写** `/create`，集合 POST 即创建）。
- 资源**单体**：`GET /ui/v1/{资源}/{id}` 查看；`POST /ui/v1/{资源}/{id}` 更新（路径不加 `update`；
  子资源字段各自的写入口优先，如 `POST /ui/v1/sites/{domain}/config`）。
- **动作型操作**：末段显式动词，参数放 JSON body 或查询串，不放进路径（通配动作见 §5.3）。
- `delete` 与 `remove` 分工（**按资源归属**，不按语义强弱）：
  - `delete`：内核自有实体（站点、定时任务、插件）——生命周期由 Skiff 管理。
  - `remove`：Docker 资源（镜像、网络、卷，对齐引擎 `rm`）与文件「移入回收站」
    （与 `skiff-fs` 动词 `remove` 一一对应，不表示资源本体删除）。

### 5.2 动词表（新路由只许用这些）

| 动词 | 语义 | 示例 |
| --- | --- | --- |
| `（集合 POST）` | 新建资源 | `POST /ui/v1/sites`、`/ui/v1/runtimes`、`/ui/v1/containers`、`/ui/v1/networks`、`/ui/v1/volumes` |
| `config` / `params` | 保存配置（资源已存在） | `POST /ui/v1/sites/{domain}/config`、`/ui/v1/web/params`、`/ui/v1/settings/panel` |
| `delete` | 删除内核自有实体 | `POST /ui/v1/sites/{domain}/delete`、`/ui/v1/tasks/{id}/delete`、`/ui/v1/plugins/{id}/delete` |
| `remove` | 删除 Docker 资源 / 文件入回收站 | `POST /ui/v1/images/{id}/remove`、`/ui/v1/networks/{id}/remove`、`/ui/v1/volumes/{name}/remove`、`POST /{p}/files/remove` |
| `toggle` | 启用/停用、开关 | `POST /ui/v1/plugins/{id}/toggle`、`/ui/v1/tasks/{id}/toggle` |
| `run` | 立即执行一次 | `POST /ui/v1/tasks/{id}/run` |
| `cancel` | 取消进行中的作业/配额 | `POST /ui/v1/jobs/{id}/cancel`、`/{p}/files/quotas/cancel` |
| `retry` | 重试失败作业 | `POST /ui/v1/jobs/{id}/retry` |
| `install` | 安装扩展、组件或包 | `POST /ui/v1/store/plugins/{id}/install`（返回确认数据）、`/ui/v1/web/install`、`/ui/v1/runtimes/{id}/extensions` |
| `confirm` | 落实上一步的暂存结果 | `POST /ui/v1/plugins/confirm`（凭 24 位令牌提交） |
| `restore` / `purge` / `clear` | 回收站恢复/彻底删除/清空 | `POST /{p}/files/trash/restore\|purge\|clear` |
| `pull` | 拉取镜像 | `POST /ui/v1/images/pull` |
| `validate` / `up` / `down` | Compose 校验/启动/停止 | `POST /ui/v1/compose/validate\|up\|down` |
| `probe` / `clone` / `publish` / `unbind` | 探测/克隆/发布端口/解绑站点 | `POST /ui/v1/runtimes/{id}/probe\|clone\|publish`、`/ui/v1/runtimes/{id}/sites/{domain}/unbind` |
| `batch` | 批量操作（参数在 body） | `POST /ui/v1/sites/batch` |
| `scan` / `set` | 用量扫描/设置上限（fs 配额专用） | `POST /{p}/files/quotas/scan\|set` |
| `download` | 下载产物到暂存 | 只读落盘走 GET：`GET /{p}/files/download`；升级包落暂存并记审计，故为 `POST /ui/v1/store/upgrades/{version}/download` |

### 5.3 通配动作（仅允许已有白名单）

以下四段通配由 handler 内白名单校验，新增动作必须同时改白名单与本文：

- `POST /ui/v1/containers/{id}/{action}`（start/stop/restart/kill/…）
- `POST /ui/v1/sites/{domain}/{action}`（start/stop/restart/rebuild/…）
- `POST /ui/v1/runtimes/{id}/action/{action}`
- `POST /ui/v1/compose/{project}/{action}`

通配段必须注册在静态动作之后（如 `backup` 早于 `{action}`），避免被吞。

## 6. 方法与副作用

| 方法 | 用途 | 约束 |
| --- | --- | --- |
| GET | 页面、列表、详情、下载、SSE/WS 握手 | 不产生业务副作用；`download` 只落暂存并校验摘要 |
| POST | 全部写入、动作、登出 | `/ui/v1/*` 与 `/{p}/files/*` 的写操作必须带 `x-csrf-token` 头；权限按资源判定 |
| DELETE / PUT / PATCH | **不使用** | 统一 POST + 末段动词，避免代理与表单兼容问题 |

- 写操作一律 POST 到 `/ui/v1/*` 的动词端点（如 `POST /ui/v1/sites/{domain}/delete`），没有页面级表单提交。
- 长连接：`GET /{p}/containers/{id}/logs/stream`（SSE）、`GET /{p}/jobs/{id}/logs/stream`（SSE）、`GET /{p}/containers/{id}/exec`（WS，需先 `POST /{p}/containers/{id}/exec/ticket` 换一次性票据）。
- 失败回执：`/ui/v1/*` 一律 JSON 信封（`{"error":{code,message}}`），不在 URL 里表达成败，也不做 PRG 跳转。
- `/ui/v1/*` 的写操作一律 POST + CSRF；未认证引导与登录为 `GET /ui/v1/auth/login` 与 `POST /ui/v1/auth/login`（登录用签名 CSRF token + 来源判定，CSRF 放 JSON 体 `csrf` 字段）；`POST /ui/v1/session/logout`、`settings/password`、`settings/totp/setup`、`settings/totp/enable` 为认证类入口（登录态自服务，仍校验 `x-csrf-token` 与本人会话）。
- `/ui/v1/*` 未认证返回 401 JSON，不做 303；已认证但权限不足返回 403 JSON。
- 写入口的守卫顺序固定为**先权限、后 CSRF**：无权限主体得到 `auth.forbidden`（403 JSON），不会先看到 `auth.csrf_invalid`。Docker 对象级授权（`protected` 对象）需要先解析容器，保留在 CSRF 之后，但角色门（`policy.check(actor, verb, None)`）仍在 CSRF 前。Compose 组同此顺序：Basic 档不提供 `ComposeUp/Down`，管理员在该档得到档位拒绝（`auth.forbidden`），CSRF 语义由 Lifecycle 档用例单独覆盖。

## 7. 兼容与下线

旧路径只保留**可验证的兼容**，不复制业务实现：

| 旧路径 | 方式 | 目标 / 行为 |
| --- | --- | --- |
| `/{p}/files/api/{*rest}` | 308 | `/{p}/files/{rest}`，保留方法与请求体 |

- 301 只用于 **GET 页面**；带方法的请求一律 308，禁止让写请求落到 301。
- 兼容清单只减不增。删除某条兼容前，先在 `docs/changes/` 记一次「已下线」并确认仓库内无引用。
- 测试守门：`crates/web/tests/main_flow.rs::legacy_paths_redirect_to_canonical_urls` 覆盖跳转目标。
- **不在上表的一律 404**：页面与页面内表单已全部由 budding 壳页 + `/ui/v1/*` 承载，旧 SSR 路由删掉即删，
  不做 301/308。销毁清单（每次下线追加一行，便于回溯）：
  - 2026-11-18：`/{p}/sites/env[/{*rest}]`（运行环境旧路径 20+ 条）、`/{p}/m/core/{*rest}`。
  - 2026-11-19：`GET /{p}/containers/rows`（htmx 局部片段）。
  - 2026-11-19：`/{p}/m/{*rest}` 全量删除；页面级写路由（containers / sites / runtimes / images / networks / volumes /
    compose / tasks / jobs / store / plugins / settings / audit / docker-settings）、`/{p}/logout`、`/{p}/password`、
    `/{p}/totp*`、`POST /{p}/plugins/install`（multipart 上传）一并删除。


## 8. 查询参数

- 一律**小写、单数、无下划线**；只用于只读筛选、分页、页内标签与轮询游标。
- 允许集合：`q`（关键词）、`path`（文件路径）、`tab`（页内标签）、`type`（站点/日志类型）、
  `status`（状态过滤）、`filter`（容器过滤）、`sort` / `order`、`hidden`（显示隐藏文件）、
  `page`（分页）、`after`（SSE 游标）、`refresh` / `force`（强制刷新）、`project`（Compose 项目）、
  `file` / `ini` / `port`（运行环境详情定位）、`open`（插件页打开项）。
- 认证类例外：`next`（登录后跳转）、`u`（用户名预填，兼容 `username`）。
- 写操作的参数放**请求体**：`/ui/v1/*` 用 JSON body，`/{p}/files/*` 用 JSON 或 `multipart/form-data`（上传），
  不放查询串；唯一例外是 SSE 游标 `after`。
- 新增参数必须先在本节登记；同义参数合并为一个，不新增别名。
