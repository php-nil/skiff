# 桥接：任务、审计、设置、商店、插件、文件与终端（§5.12–5.20）

> 上级索引：[SDUI_BUDDING_INTEGRATION.md](../SDUI_BUDDING_INTEGRATION.md) ｜ 拆自 `docs/sdui/SDUI_BUDDING_INTEGRATION.md` 原 §5.12–5.20（2026-11-16 整理，正文未改）。

---

### 5.12 任务与作业（第五批，只读）

- `GET /{p}/tasks`、`/{p}/tasks/{id}`、`/{p}/jobs`、`/{p}/jobs/{id}` 交付 budding 壳页；旧 SSR 已随 `/{p}/m/core/*` 深链删除（2026-11-18）。
- 列表/详情复用 `TaskService::list_views` / `detail_view` 与 `Jobs::list` / `tail` / `detail_of`，不新增 SQL。
- 写操作已原生化：`POST /ui/v1/tasks/{id}/toggle|run|delete`、`POST /ui/v1/jobs/{id}/cancel|retry`；分别复用 `TASK_TOGGLE|RUN|DELETE`、`JOB_CANCEL|RETRY` 与既有命令层审计。SSE 实时日志仍待上游通道。

**字段表（冻结，只增不改）**

`GET /ui/v1/tasks?q=&limit=50&cursor=` → `{ "data": { items, total, next_cursor, can_admin } }`

| 字段 | 类型 | 说明 |
|---|---|---|
| `items[].id` / `name` | string | 任务 id / 名称 |
| `items[].expr` / `next_text` | string | 调度表达式与下次运行展示文案 |
| `items[].source` / `target` / `method` | string | 来源、目标、执行方式 |
| `items[].enabled` | bool | 是否启用 |
| `items[].state_label` / `state_class` | string | 状态文案 / 配色 class |
| `items[].last_text` / `last_class` | string | 最近结果文案 / 配色 |
| `items[].builtin` | bool | 内置任务 |
| `next_cursor` | string \| null | 上一页末项任务 id（D10） |

`GET /ui/v1/tasks/{id}` → `{ "data": { task, payload, runs, can_admin } }`；`payload` 为原样 JSON 对象，`runs[]` 字段 `trigger` / `started_text` / `duration` / `result` / `result_class` / `detail` / `missed`。

`GET /ui/v1/jobs?limit=50&cursor=` → `{ "data": { items, total, next_cursor, can_admin } }`

| 字段 | 类型 | 说明 |
|---|---|---|
| `items[].id` / `title` | string | 作业 id / 标题 |
| `items[].kind_label` / `target_label` | string | 类型与目标文案 |
| `items[].status_label` / `status_class` / `phase_label` | string | 状态与阶段 |
| `items[].progress` | number | 进度百分比；`indeterminate` 表示不确定进度 |
| `items[].duration` / `created_by` | string | 耗时与提交人 |
| `items[].created_at` | string | RFC3339 UTC（D9） |
| `items[].active` | bool | 是否进行中 |

`GET /ui/v1/jobs/{id}` → `{ "data": ... }`：作业元数据（同上）+ `created_at` / `started_at` / `finished_at`（RFC3339 UTC，D9，未开始/结束为 null）+ `cancellable` / `cancel_requested` / `created_by_role` / `requester_ip` / `exit_code` / `trigger` / `error_class` / `error_detail` / `result` / `params` / `retry_of` / `can_cancel` / `can_retry` / `lookback` / `logs`（首屏尾巴纯文本，`[HH:MM:SS] 行`）/ `can_admin`。

错误：未认证 401、CSRF 失败 403、viewer 写操作 403（`auth.forbidden`）、任务/作业不存在 404、任务服务不可用 503、内置任务删除与不可重试类型 400（`shape.invalid`）。

写接口：`POST /ui/v1/tasks/{id}/toggle`（body `{enabled}`）、`/run`、`/delete`；`POST /ui/v1/jobs/{id}/cancel`（→ `{id,outcome,message}`，`outcome` 为 `cancelled_now|requested|terminal`）、`/retry`（→ `{id,retry_of}`）。

**渲染映射**

- 任务列表：表格（任务/方式/目标/调度/下次运行/状态/最近结果），行点击进详情；`can_admin` 时行内提供「切换启停 / 立即运行」（删除放详情页，便于按内置任务显隐）。
- 任务详情：信息、参数、最近执行表格三块 + 任务操作卡片（启停 / 立即运行 / 删除）。
- 作业列表：表格（作业/标题/类型/目标/状态/耗时/提交人/创建时间），行点击进详情。
- 作业详情：元数据、参数、结果、日志四条 `card`；`can_cancel` / `can_retry` 时给「请求取消 / 重试作业」，取消前确认，重试成功跳新作业详情；日志用 `code` 组件，实时流仍待 SSE 通道。

### 5.13 审计（第六批占位 → 第十五批落地）

- `GET /{p}/audit` 交付 budding 壳页，页面为原生只读审计列表；旧 SSR 已随 `/{p}/m/core/*` 深链删除（2026-11-18）；权限口径不变（viewer 403）。
- 权限两处同源：页面与 `GET /ui/v1/audit` 都按 `Permission::AuditRead` 判定（viewer 403，菜单不可见），不新增权限来源。
- 查询：`GET /ui/v1/audit?q=&limit=&cursor=`，落 `{ items, keyword, has_more, next_cursor }`。
  - 关键字非空时对 actor / ip / action / target / detail 做 ASCII 不区分大小写的子串匹配（`instr(lower(...))`，`%`/`_` 按字面量处理）。
  - 排序 `(ts DESC, id DESC)`，走既有 `idx_audit_ts(ts DESC, id DESC)`（schema v19），**未改表、未加索引**；关键字只做行内过滤。
  - 游标是 `app::cursor::CursorCodec<AuditScope>` 签名游标（HMAC，密钥复用持久化 CSRF 派生密钥，签名域 `cursor:audit:` 隔离），载荷为上一页末项 `(ts,id)` 并与筛选指纹绑定：换关键字沿用旧游标 400、篡改 400、旧版本 400。`limit` 默认 50、上限 200。
  - 字段冻结：`id` / `ts`（RFC3339 UTC，D9）/ `actor` / `ip` / `action` / `target` / `result` / `detail`（截断 200 字符）。
- 前端：关键字表单 + `table` 列表 + 「下一页（更早记录）」按钮；翻页是向更早的 keyset 前进，不提供随机跳页。
- 列表渲染：`table` 固定 `perPage: -1`，把宿主给出的本页全部行渲染出来；审计页再加 `virtual` + `height: 420` 定高内部滚动。服务端游标取本页第 `limit` 条的 `(ts,id)`，与本页最后一条实际渲染行一致，不再出现「本页 50 条只显示 10 条、翻页跳过 40 条」。其余本地列表（运行环境配置/扩展、站点解绑、Compose 项目、菜单、回收站、配额、商店）同规则，不与组件默认每页 10 条错位。
- 本地验证：`main_flow` 覆盖 viewer 403、关键字命中、翻页游标、换关键字与篡改 400；无头浏览器实测页面渲染、`?q=` 过滤与接口层游标翻页（10 条验收数据内，后翻 id 递减、换关键字 400）。

### 5.14 设置（第七批 + 第十一批补齐）

- `GET /{p}/settings` 交付 budding 壳页；旧 SSR 已随 `/{p}/m/core/*` 深链删除（2026-11-18）；口令 / TOTP 写接口保留，壳页经 `/ui/v1` 调用。
- 写操作全部迁到 `/ui/v1`，复用旧链路的校验、落盘、审计与错误文案，CSRF 走 `x-csrf-token`：
  - 面板配置 `POST /ui/v1/settings/panel` → `settings::apply_panel_update`（端口 / 安全入口 / TLS）。
  - 口令 `POST /ui/v1/settings/password` → argon2 校验旧口令、写新散列、`rotate_password_session` 轮换会话（响应回传新 `csrf_token` 并重设会话 Cookie）、删除首启 `initial-admin.txt`。错误文案沿用旧页：旧口令错「当前口令不正确」、两次不一致「两次输入的新口令不一致」。
  - TOTP 两步：`POST /ui/v1/settings/totp/setup` 生成并暂存密钥（未启用态），`POST /ui/v1/settings/totp/enable` 校验动态码后启用并一次性返回 10 条备份码。错误文案沿用旧页：「动态码不正确，请确认设备时间后重试」。
  - 公钥 `POST /ui/v1/settings/keys`（导入）与 `POST /ui/v1/settings/keys/{id}`（撤销）→ `decode_public_key` + `trust_key` / `remove_trusted_key`，审计事件 `settings.key.import` / `settings.key.remove`，仅 Admin。
- 强改密期间的中间件放行只服务旧 SSR，不改动；新前端失败一律按 JSON 信封回显，不 PRG。
- 口令框直接声明原生 `type: 'password'`（`autocomplete`：当前口令 `current-password`，新口令 `new-password`）。
- `GET /ui/v1/settings` 返回 `panel` / `account` / `keys` / `can_admin` / `flash`；公钥仅对管理员返回，非管理员为空数组。

**字段表（冻结，只增不改）**

`GET /ui/v1/settings` → `{ "data": { panel, account, keys, can_admin, flash } }`

| 字段 | 类型 | 说明 |
|---|---|---|
| `panel.listen_addr` / `listen_port` | string / number | 当前监听地址与端口 |
| `panel.entry_prefix` | string | 当前入口前缀（不含 `/`） |
| `panel.tls_mode` | string | `auto` / `on` / `off`（与旧页面三态同口径） |
| `panel.tls_cert` / `tls_key` | string | 证书 / 私钥路径（未配置为空串） |
| `panel.config_path` | string | `config.toml` 路径 |
| `panel.plaintext_behind_proxy` | bool | 可信反代终止 TLS 形态 |
| `panel.initial_admin_pending` | bool | 首启口令文件仍在 |
| `account.totp_enabled` / `must_change_password` | bool | 账户安全状态（只读） |
| `account.totp_pending_secret` / `totp_pending_uri` | string | 已生成但未启用的 TOTP 密钥与 `otpauth://` 绑定串；无暂存密钥时为空串 |
| `keys[]` | array | `{key_id, fingerprint_short, label}`；仅管理员返回，viewer 为空 |
| `can_admin` | bool | 是否可编辑面板配置 |
| `flash.notice` / `flash.error` | string | 本次读取消费的会话提示（刷新不重放） |

`POST /ui/v1/settings/panel`，body `{listen_addr, listen_port, entry_prefix, tls_mode, tls_cert, tls_key}`（字符串）→ `{ "data": { "message": "..." } }`；校验失败 400 `shape.invalid`，无权限 403 `auth.forbidden`，CSRF 失败 403 `auth.csrf_invalid`。

其余写动作同形（均为 POST + `x-csrf-token`）：

| 路径 | body | 成功返回 |
|---|---|---|
| `/ui/v1/settings/password` | `{old_password, new_password, confirm_password}` | `{message, csrf_token}`，并下发新会话 Cookie |
| `/ui/v1/settings/totp/setup` | 无 | `{secret, otpauth_uri, message}` |
| `/ui/v1/settings/totp/enable` | `{code}` | `{backup_codes[], message}`（备份码仅此一次） |
| `/ui/v1/settings/keys` | `{public_key, label}` | `{key_id, message}`；仅 Admin |
| `/ui/v1/settings/keys/{id}` | 无 | `{message}`；仅 Admin |
| `/ui/v1/settings/menu` | `{action, key?, layout?, width?, max?}` | `{message}`；偏好只写当前账号，动作与旧表单同源（`up`/`down`/`hide`/`show`/`pin`/`unpin`/`layout`/`width`/`reset`） |
| `/ui/v1/settings/docker` | `{registry_mirror, image_prefix}` | `{message}`；仅 Admin，落盘/回滚/审计与旧表单同源 |

**菜单设置与 Docker 设置（第二十二批）**

- `GET /{p}/settings/menu`、`GET /{p}/settings/docker` 交付 budding 壳页；旧 SSR 已随 `/{p}/m/core/*` 深链删除（2026-11-18）。
- 菜单导航的 href 改指向规范路径 `/{p}/settings/menu`、`/{p}/settings/docker`，不再以 `/m/core/...` 作为产品入口；`/{p}/docker/settings` 的 308 兼容不变。
- `GET /ui/v1/settings/menu` → `{layout, width, max_width, groups[{key,label,items[{key,label,href,hidden,pinned,pinnable,plugin,icon}]}]}`；读对登录用户开放（偏好按账号隔离）。页面用「布局按钮 + 内容宽度表单 + 分组表格」，行内「↑ / ↓ / 显隐 / 固定」，底部「恢复默认」（确认后 `action=reset`）。
- `GET /ui/v1/settings/docker` → `{registry_mirror, image_prefix, can_admin}`；读对登录用户开放，`can_admin=false` 时页面只读提示。写操作仅 Admin，失败按 JSON 信封返回（不 PRG），成功刷新页面。

**运行环境详情（第二十三批）**

- `/runtimes` 列表新增「安装运行环境」表单（类型来自 `GET /ui/v1/runtimes/catalog`，版本可留空）与「镜像参数」表单（`GET|POST /ui/v1/runtimes/images`）。
- `/runtimes/{id}` 改为真实原生页（原占位与旧版链接删除）：`GET /ui/v1/runtimes/{id}` 一次返回运行环境行、PHP 参数、Node 配置、端口、扩展清单、配置文件列表、当前选中的配置文件内容（`?file=key`）、绑定站点与 `config_mounts_stale`。
- 写操作：生命周期按钮（start/stop/restart/rebuild/sync/delete）走 `POST /ui/v1/runtimes/{id}/action/{action}`，有绑定站点时前端先确认；`POST .../probe` 只回显实测版本；PHP 参数、Node 配置、端口、配置文件、PECL、克隆、解绑各有独立端点，全部复用 `runtime_cmd` 与既有动作权限。
- 扩展表行内「安装 / 卸载」调用 `POST /ui/v1/runtimes/{id}/extensions/{key}/{op}`；批量端点 `POST .../extensions/batch` 已登记，当前页面按单项操作。
- 详情聚合仍是 Admin-only（与旧页同权限）；viewer 直接请求 403，菜单不可见。

**Web 服务页（第二十四批）**

- `GET /{p}/sites/web` 改交付 budding 壳页（Admin 权限，viewer 403）；旧 SSR 已随 `/{p}/m/core/*` 深链删除（2026-11-18）。
- `GET /ui/v1/web` → `{status, install, config{main_path,main_content,include_path,include_content,sites[],params}, modules, can_admin}`，与旧页同一数据源（`nginx_status`、`site_install_progress`、`nginx_config_view`、`nginx_modules`）。
- `GET /ui/v1/web/logs?load=&kind=&lines=&level=&status=&q=&offset=`：未 `load` 时不触碰文件，返回读取范围、状态码分布、截断标记与行数据。
- 页面拆三级子视图（2026-11-17）：`/{p}/sites/web` 状态、`/config` 配置、`/logs` 日志、`/modules` 模块，每次只渲染当前视图（详见 `01-SCOPE.md` §2.3.1「三级子视图登记」）；日志视图进入即按查询参数取数（默认访问日志 200 行），「加载日志」把筛选条件带回 `?type=&lines=&level=&status=&q=`（不再用 `?logs=1`）。
- 写操作：`POST /ui/v1/web/config`（主配置，`nginx -t` + reload）与 `POST /ui/v1/web/params`（性能参数）；SSR 表单与 JSON 共用 `nginx::apply_main` / `apply_params`，审计事件 `nginx.config.save` / `nginx.params.save` 不变。失败返回 400 并保留错误文本，不伪装成功。

### 5.15 商店（第八批）

- `GET /{p}/store` 交付 budding 壳页；旧 SSR 商店页已删除（2026-11-18）；目录状态、四类条目与升级包入口由 `/{p}/store` 壳页读 `/ui/v1/store` 渲染。
- `GET /ui/v1/store?refresh=1` 与旧页面共用 `store::load_view`：同一目录快照、同一可安装版本判定与升级提示。
- 安装分两步，均复用旧链路与审计：
  1. `POST /ui/v1/store/plugins/{id}/install` → 调用 `store::prepare`（下载、摘要/大小、验签、版本与降级校验），返回权限确认数据；审计动作与旧表单一致（`store.install`）。
  2. `POST /ui/v1/plugins/confirm` → 调用 `plugins::verify_for_confirm` + `commit_install`（同一信任判定、授权落库、暂存包清理），与旧确认页同一条收尾路径。
- 进度：下载验签是单次同步请求，前端只给「正在下载并验签」占位提示，不引入 SSE/轮询。
- 升级包下载（第二十五批）：`POST /ui/v1/store/upgrades/{version}/download`（Admin + `x-csrf-token`）与旧表单 GET 共用 `store::apply_upgrade_download`——同一目录启用/公钥判定、同一暂存与 sha256 校验、同一 `upgrade.download` 审计；成功返回 `{version,arch,channel,source,sha256,size,path,reused}`。商店页在 `upgrade.can_download` 时给出原生下载按钮，不再跳旧页。

**字段表（冻结，只增不改）**

`GET /ui/v1/store` → `{ "data": { state, message, channel, seq, fetched_at, source, allow_unsigned, key_configured, can_install, total, sections[], upgrade } }`

| 字段 | 类型 | 说明 |
|---|---|---|
| `state` | `{code,label}` | `disabled` / `fresh` / `cached` / `offline` / `rejected` |
| `message` | string | 目录状态说明（与旧页面提示条同一文案） |
| `channel` / `seq` / `fetched_at` / `source` | string | 目录元信息；空值由前端显示 `—` |
| `allow_unsigned` / `key_configured` | bool | 开发态放行 / 固定公钥已配置 |
| `can_install` | bool | 插件分区是否给安装入口 |
| `total` | number | 四类条目合计 |
| `sections[]` | array | `{title, hint, installable, items[]}`，固定四项顺序：插件 / 应用模板 / 运行环境 / 升级通道 |
| `sections[].items[]` | array | `{name, version, source, updated_at, compatibility, note, warn, installable, action, install_note}` |
| `upgrade` | object | `{current, arch, available, channel, min_from, changelog_url, note, can_download}` |

`POST /ui/v1/store/plugins/{id}/install` body `{version?}` → `{ "data": { token, id, name, version, api, key_id, signed, permissions[], resources[], interop_provides[], interop_consumes[], requires[], egress[], channel, source } }`

`POST /ui/v1/plugins/confirm` body `{token, grant[], resource[], extra_resource}` → `{ "data": { id, redirect_to } }`

错误码：目录未启用 404 `lookup.not_found`；未配公钥 / 包校验或版本失败 400 `shape.invalid`；无 `PluginManage` 403 `auth.forbidden`；CSRF 失败 403 `auth.csrf_invalid`。

### 5.16 插件（第九批）

- `GET /{p}/plugins`、`GET /{p}/plugins/{id}` 交付 budding 壳页；旧 SSR 列表与信息页已随 `/{p}/m/core/*` 删除（2026-11-18）；数据走 `/ui/v1/plugins`。
- 菜单键 `core/plugins/discover`、`core/plugins/updates` 映射为原生路由 `/{p}/plugins/discover`、`/{p}/plugins/updates`，页内以「发现 / 可更新」标签呈现；页内标签链接同路径，旧 `?tab=` 继续兼容（第二十五批）。
- **插件功能页原生化**：`api = "1.3"` 起视图可声明随包 schema（`ui/schema/<name>.json`，安装期静态校验）；`/{p}/plugins/{id}/{view}` 由壳页加载 `GET /{p}/plugins/{id}/schema/{view}` 后原生渲染，数据与写操作经 `skiffPluginRpc` 走既有 `POST /rpc`（CSRF + 权限 + 审计）。未声明 schema 的旧视图仍走旧 SSR 工作台（显式 `?legacy=1`）。功能页、schema 与 RPC 权限统一为 `PluginManage`（§4.4）。
- 插件详情页把声明的功能页以纯文本列出，并说明原生 schema 渲染与旧 SSR 兼容关系。
- 启停 / 卸载分别复用 `plugins::apply_toggle`、`plugins::apply_remove`（停进程、审计、任务回收、菜单偏好清理语义不变）。
- 上传安装包（发现页）与 RPC 桥仍未原生渲染，也不从新页面跳旧页；商店安装已原生可用。

**字段表（冻结，只增不改）**

`GET /ui/v1/plugins` → `{ "data": { plugins[], enabled_count, disabled_count, can_admin, allow_unsigned, reduced_allowed } }`

`plugins[]`：`{id, name, version, api, status:{code,label}, enabled, key_id, signed, reduced, running, requires, permissions, grants, ui_path, takedown_note}`

`GET /ui/v1/plugins/{id}` → `{ "data": { plugin, grants[{kind,value}], interop_provides[], interop_consumes[], menus[{label,href,icon,children[{label,href,icon}]}], entry_href, can_admin, allow_unsigned, reduced_allowed } }`

`POST /ui/v1/plugins/{id}/toggle` → `{ "data": { id, enabled } }`

`POST /ui/v1/plugins/{id}/delete` body `{delete_data?}` → `{ "data": { id, redirect_to } }`

错误码：插件不存在 404 `lookup.not_found`；无 `PluginManage` 403 `auth.forbidden`；CSRF 失败 403 `auth.csrf_invalid`。

### 5.17 文件管理（第十一批）

- `GET /{p}/files` 交付 budding 原生页：**平铺目录 + 面包屑，不做目录树**（负责人定案：禁止树形；原 F3 缺口按降级口径关闭）。
- `GET /ui/v1/files?path=` 返回 `{path,parent,roots[],entries[],truncated,max_edit_bytes,upload_chunk_bytes}`；条目 `{name,path,dir,symlink,size,mtime,mode,owner,nlink,link_target}`，`mtime` 为 RFC3339 UTC 或 `null`（D9）。默认路径是 Web 根；授权沿用 `FILE_BROWSE`（仅 Admin），路径硬拒 / 配额 / 审计仍由 `AdminFs` + `skiff-fs` 单一来源决定。
- 写动作：`POST /ui/v1/files/mkdir`（`FILE_WRITE`）、`POST /ui/v1/files/rename`（`FILE_WRITE`）、`POST /ui/v1/files/remove`（`FILE_REMOVE`，进回收站并回传 `trash_id`）；均要求 `x-csrf-token`，失败按 `AppError` 信封返回。
- 列表用 `virtual-list`（`itemHeight: 52`、`height: 560`）虚拟滚动；条目操作是 budding 按钮：打开 / 下载 / 重命名 / 删除。下载仍走既有 `GET /{p}/files/download`。
- 上传走宿主 `PlatformAdapter.upload`：前端按服务端下发的 `upload_chunk_bytes`（256 KiB）切片，逐片 POST 既有 `/{p}/files/upload?path=&offset=`（同一授权、同一审计），按字节上报进度，组件负责进度、取消与重试。**宿主必须同时注入 `platform.request`**：上游 `createPlatformAdapter` 只在提供 `request` 时保留 `upload`。
- 终端不在本批：仍待 WS 契约与 terminal Pack（待第 4 组）。
- 旧 SSR（浏览 / 回收站 / 配额）已删除（2026-11-18）；三个 L3 均为壳页，数据走 `/files/*` 与 `/ui/v1/files*`。
- 回收站与配额已原生化（第二十一批）：`GET /{p}/files/trash`、`/{p}/files/quotas` 交付壳页，入口门禁仍是 `FILE_TRASH_READ` / `FILE_QUOTA_READ`；数据与写操作**直接复用**既有 `GET /files/trash/list`、`POST /files/trash/{restore,purge,clear}`、`GET /files/quotas/list`、`POST /files/quotas/{set,cancel,scan}`（`x-csrf-token`、审计、`commands::file` 不变），未新增 `/ui/v1` 写入口。
- 回收站表（路径 / 大小 / 删除时间 / 剩余天数）行内「恢复 / 彻底删除」，顶部「清空回收站 / 刷新」；恢复冲突（目标已存在）二次确认 `overwrite`。配额页顶部「设置配额」表单（路径 + 容量/文件数上限，至少一项），配额表（路径 / 已用 / 上限 / 统计）行内「重新统计 / 取消配额」。`GET /ui/v1/files` 只新增 `can_admin`（只增不改），`can_admin` 时才显示两个入口。

### 5.18 代码查看器与日志查看器（第十三批）

- 代码查看器：文件管理里点文本文件进入 `GET /{p}/files/view?path=`（budding 隐藏路由），数据来自 `GET /ui/v1/files/read?path=`（`FILE_READ`，仅 Admin）。响应 `{path,text,size,truncated,max_edit_bytes}`；渲染用 `code` 组件（1.8.0 起渲染器类型名为 `code`，旧 `code-viewer` 已不再注册；文本字段为 `modelValue`，旧 `code` 字段组件不读取；只读、行号、复制、固定高 560），语言按扩展名映射；疑似二进制（含 NUL）只提示不渲染。纯前端高亮，不引 WASM。
- 代码 Pack：入库 `budding-code-1.8.1-r2.min.js`（89,982 B，取自 `dist/packs/code.mjs`，自包含 ESM；字节自 1.8.1 起未变），启动脚本在 `embed` 前用 `BuddingSDK.registerPack('code', …?v=<ver>)` 注册；渲染器类型名同为 `code`，文本字段为 `modelValue`。未注册或加载失败时 `code` 组件退化为纯文本。CSP 仍是 `script-src 'self'`，动态 import 同源可用。
- 站点日志：`GET /{p}/sites/{domain}/logs` 默认交壳页；旧 SSR 仅接受 `?legacy=1`。数据来自 `GET /ui/v1/sites/{domain}/logs?part=access|error&tail=200&q=`，复用 `AppState::site_logs`（尾部 2MB 窗口、最新在前），权限与旧页一致（登录即可读）。页面用 `code` 组件定高 560，`tail` 可选（上限 2000）、关键字只过滤该窗口，访问/错误日志切换与刷新都是页面级按需拉取，不建 SSE。
- 作业日志：`GET /{p}/jobs/{id}/logs` 同路径按 `Accept` 分流——浏览器导航（含 `text/html`）交 budding 日志页，脚本请求（无 `text/html`）仍返回既有 JSON（`?after=` 游标、同权限、同审计），按 `after` 游标最多取 10 页后停止，页面按需加载、不建 SSE；作业详情页首屏日志也改为 `code` 组件，并保留旧详情入口用于取消 / 重试。
- 站点详情页新增「查看访问 / 错误日志」入口；旧 SSR 日志页不再从面板导航。

### 5.19 容器终端（第十四批）

- 路由：`GET /{p}/containers/{id}/terminal`（budding 隐藏路由），入口在容器详情的「打开终端」。**不使用 iframe，也不自建 xterm**。
- 通道完全复用既有契约，本轮无内核改动：
  - `POST /{p}/containers/{id}/exec/ticket`（`AdminUser` + `x-csrf-token` + `Verb::Exec` 策略）签发单连票据，绑定「当前会话 + 容器」，30 秒有效、一次即废；
  - `GET /{p}/containers/{id}/exec?ticket=` 升级 WS，`Origin` 严格校验；
  - WS 的 start / fail / end 继续写 `Verb::Exec` 审计。
- 前端：页面先取票再渲染 `terminal` 组件；`wsUrl` 由 `location.protocol` 推出 `ws://` / `wss://`，`resizeFormat: 'none'`（内核 exec 只透传 stdin，不解析 resize JSON），`reconnect: false` + 手动「重新连接（重新取票）」。
- Pack：入库 `budding-terminal-1.8.1-r2.min.js`（395,014 B，取自 `dist/packs/terminal.mjs`，字节自 1.8.1 起未变，自带 xterm 与样式注入，无裸依赖），`BuddingSDK.registerPack('terminal', …?v=<ver>)`。
- **验证边界**：本机（Windows、无 Docker）只验证了路由、壳页与取票失败时的错误页；WS 真连与容器内 `/bin/sh` 交互必须在 Linux 测试机验收，未验前不得记为已完成。

### 5.20 日志入口收口（第十六批）

- 容器日志：`GET /{p}/containers/{id}/logs` 从旧 SSR SSE 页改为 budding 日志页（尾部快照 + 刷新，不做实时流）；SSE 流只保留在 `GET /{p}/containers/{id}/logs/stream`，供旧 SSR 详情页 `data-src` 跟随。新增 `GET /ui/v1/containers/{id}/logs?tail=`（策略同旧 SSE：`Verb::ContainerLogs` → `DockerRead`；`tail` 默认 500、上限 2000），容器详情新增「查看日志」入口，旧版链接收窄为「资源限制编辑」。
- 运行环境日志：新增 `GET /{p}/runtimes/{id}/logs` budding 日志页与 `GET /ui/v1/runtimes/{id}/logs?tail=`（Admin，复用 `AppState::runtime_logs`）；旧详情抽屉的快照接口移到 `GET /{p}/runtimes/{id}/logs.json`（`logs_url` 已同步），让 `/logs` 可以交壳页；运行环境列表行可点进详情占位页，占位页提供「查看运行日志」与「旧版配置 / 安装 / 扩展」两个明确入口。
- 前端统一用 `code` 组件（`language: 'plaintext'`、固定高度、行号、可复制），刷新走整页重载；不新增 SSE/WS 契约，也不在前端造分页或流式缓冲。
- `?legacy=1` 现状：仅插件 iframe 工作台与站点日志旧 SSR 两处保留（前者是插件嵌入兼容，后者是无 JS 兜底）；本轮未新增 legacy 分支。
- 验证：`main_flow` 覆盖「页面交壳 + JSON 未认证 401 / 无 Docker 503 / viewer 403 / 实例不存在 400」；无头浏览器实测三个页面（容器日志错误页、运行环境日志错误页、运行环境详情占位页）均无 iframe、无脚本错误。

---
