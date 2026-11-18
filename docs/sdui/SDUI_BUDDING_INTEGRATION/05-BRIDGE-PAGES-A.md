# 桥接：概览、站点、容器、镜像与 Compose（§5.7–5.11）

> 上级索引：[SDUI_BUDDING_INTEGRATION.md](../SDUI_BUDDING_INTEGRATION.md) ｜ 拆自 `docs/sdui/SDUI_BUDDING_INTEGRATION.md` 原 §5.7–5.11（2026-11-16 整理，正文未改）。

---

### 5.7 概览页（第四批，只读）

**页面与数据**

- `GET /{p}` 交付 budding 壳（未认证 303 到登录页，已登录渲染概览）；SSR 概览仍保留在 `GET /{p}/m/overview` 菜单深链，直到插件概览槽位迁入 budding。
- 壳启动脚本先取 `GET /ui/v1/session` 与 `GET /ui/v1/menu`，按菜单生成路由表后渲染 admin 外壳；地址栏路径命中路由时加载对应页面，未适配页面渲染统一占位（§2.3.1）。
- 概览的 `[[mount]]` 固定页插件槽位不再走 iframe：`health_plugin` 非空时壳页对其调 `snapshot` RPC，用卡片渲染 CPU / 内存 / 负载 / 运行时长；插件缺失、未启用或无 `PluginManage` 时渲染原因卡片（`health_reason` 或 RPC `error.message`），不整页失败、也不静默缺块（见 §4.4）。
- 数据只复用 `dashboard::load_overview` 的既有查询（Docker 容器列表 + 插件/站点/运行环境/审计），**没有新增聚合 SQL**；本批不返回列表、不做写操作、不做图表、不做实时刷新。
- 图表位按 D14–D16 未定，页面用 `alert` 明确占位（「D14–D16 尚未定案」），不引入任何图表库。

**字段表（冻结，只增不改）**

`GET /ui/v1/overview` → `{ "data": ... }`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `collected_at` | string | RFC3339 UTC 单字段（D9） |
| `docker.status.code` | string | `available` / `unavailable` |
| `docker.status.label` | string | 中文文案「可用」/「不可用」（D8 双字段） |
| `docker.reason` | string | 不可用时为宿主给出的原因，可用时为空串 |
| `containers.running` | number | 运行中容器数 |
| `containers.total` | number | 容器总数 |
| `sites.total` | number | 站点总数 |
| `sites.abnormal` | number | `status != running` 的站点数 |
| `plugins.total` | number | 插件总数 |
| `plugins.enabled` | number | 已启用数 |
| `plugins.disabled` | number | 停用数（`total - enabled`） |
| `runtimes.total` | number | 运行环境总数 |
| `runtimes.abnormal` | number | 有容器但非 running 的数量 |
| `runtimes.uninstalled` | number | 尚无容器的数量 |
| `health_plugin` | string \| null | `overview.health` 槽位的提供者（已启用插件的 id）；无提供者为 `null` |
| `health_reason` | string | 无可用提供者时的可读原因（已停用 / 未安装声明槽位的插件）；有提供者时为空串 |

错误码沿用既有信封：未认证 `auth.unauthenticated`（401）、数据库不可用 `dependency.unavailable`（503）。本批没有列表，因此不涉及 `limit`/`cursor`（D10）。

**渲染映射**

- `docker.status.code != available` → 顶部 warning `alert`（标题用 `label`，正文用 `reason`）。
- 四张计数卡片：容器 / 网站 / 插件 / 运行环境；文案由前端 schema 生成，数字来自接口。
- 健康卡片：`health_plugin` 存在时取 `snapshot`，超过阈值（`cpu_high` / `mem_high`）追加 warning `alert`；`health_plugin` 为空或取数失败时都渲染「系统状态」原因卡片（空值用 `health_reason`，失败用 RPC `error.message`），不隐藏其它卡片。
- 「趋势图表」卡片内固定 `alert` 占位，禁止在占位里暗示已选定图表库。

---

### 5.8 站点与运行环境（第一批，只读）

**页面与数据**

- `GET /{p}/sites`、`/{p}/sites/{domain}`、`/{p}/sites/new`、`/{p}/runtimes`、`/{p}/runtimes/{id}` 交付 budding 壳页；旧 SSR 已随 `/{p}/m/core/*` 深链删除（2026-11-18，直达 404）。站点写操作已原生化，运行环境的安装/配置/扩展仍待后续批次。
- 列表复用 `list_sites` / `list_runtimes` / `runtime_views` 与页面 `site_view` / `runtime_row` 展示模型，**没有新增 SQL**；`q` 关键字过滤与旧列表页同源（域名 / 目录 / 接入方式 / 别名）。
- `GET /ui/v1/runtimes` 与旧运行环境页同权限：Admin（`authz::authorize_for`），viewer 返回 403（`auth.forbidden`）。
- 站点写操作：`POST /ui/v1/sites`、`/{domain}/{action}`（start/stop）、`/{domain}/default`、`/{domain}/domains`、`/{domain}/config`、`/{domain}/ssl`、`/{domain}/backup`、`/{domain}/delete`、`/sites/batch`；全部复用 `SITE_*` 动作与 `skiff_app::commands::site`，CSRF 走 `x-csrf-token`，不新增权限来源。

**字段表（冻结，只增不改）**

`GET /ui/v1/sites?limit=50&cursor=&q=` → `{ "data": { items, total, next_cursor, can_admin } }`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `items[].domain` | string | 主域名 |
| `items[].root_dir` | string | 站点目录（整站备份单元） |
| `items[].index_dir` | string | 文档根（文件能力授权根） |
| `items[].site_type` | string | `runtime` / `static` / `proxy` |
| `items[].type_label` | string | 中文类型文案 |
| `items[].runtime_id` / `runtime_label` / `runtime_name` | string | 仅 runtime；记录缺失时为空串 |
| `items[].runtime_shared_hint` | string | 共享环境提示，无提示为空串 |
| `items[].proxy_target` / `proxy_websocket` | string / bool | 仅 proxy |
| `items[].endpoint` | string | 列表「接入方式」列 |
| `items[].status` | `{code,label}` | D8；code 为 `running` / `stopped` / `error` 等 |
| `items[].https` | `{enabled,force_https,label,class}` | 证书/HTTPS 徽标 |
| `items[].is_default` | bool | 默认站点 |
| `items[].aliases` | string[] | 附加域名 |
| `items[].created_at` | string | RFC3339 UTC 单字段（D9） |
| `total` | number | 过滤后的总数 |
| `next_cursor` | string \| null | 站点游标 `created_at:id`（键集分页，D10）；末页为 null |
| `can_admin` | bool | 是否具备 Admin（新建站点、批量启停入口显隐） |

`GET /ui/v1/sites/{domain}` → `{ "data": ... }`：上面全部列表字段 + 下列字段：

| 字段 | 类型 | 说明 |
|---|---|---|
| `ssl` | object | `enabled` / `force_https` / `cert_exists` / `cert_path` / `key_path` / `subject` / `issuer` / `expires` |
| `runtime` | object \| null | 绑定运行环境快照：`id` / `name` / `kind{code,label}` / `version` / `state{code,label}` / `site_count` |
| `runtime_down` | bool | 绑定环境未运行或 socket 缺失 |
| `runtime_alert` | string | 不可用时的中文原因，正常为空串 |
| `config` | object | `path` / `content` / `active` / `error`；读取失败时 `error` 非空、其余为空 |
| `backup` | object | `available` / `enabled` / `task_id` / `last_text` / `next_text` |
| `can_manage` | bool | 是否具备 Admin（详情页写操作入口显隐） |

`GET /ui/v1/runtimes?limit=50&cursor=&q=` → 同上信封：

| 字段 | 类型 | 说明 |
|---|---|---|
| `items[].id` / `name` / `container_name` | string | 实例标识与容器名 |
| `items[].kind` | `{code,label}` | `php` / `node` / `go` / `rust` 及中文 |
| `items[].version` / `version_display` / `version_note` / `image` | string | 目标版本、展示版本、说明与镜像 |
| `items[].state` | `{code,label}` | 实时状态；`code` 为 `running` / `stopped` / `missing` / `error` 等 |
| `items[].state_class` | string | `ok` / `warn` / `danger` / `muted` |
| `items[].drift` | bool | 期望运行但容器未运行 |
| `items[].site_count` | number | 绑定站点数 |
| `items[].container_state` | string | 底层容器实测状态 |
| `items[].endpoint_short` / `endpoint_full` | string | 列表短标识 / 完整接入方式 |
| `items[].implemented` / `is_default` | bool | 类型已接入 / 默认版本 |
| `items[].created_at` / `updated_at` | string | RFC3339 UTC（D9） |
| `items[].job` | object \| null | `state` / `label` / `step` / `progress` / `error` / `active` / `failed` / `started_at` / `updated_at` |
| `next_cursor` | string \| null | 运行环境游标为上一页末项 `id`（D10） |

错误码沿用既有信封：未认证 `auth.unauthenticated`（401）、CSRF 失败 `auth.csrf_invalid`（403）、viewer 写站点 `auth.forbidden`（403）、站点不存在 `not_found`（404）、域名/配置字段错误 `shape.invalid`（400）、域名冲突 `conflict`（409）、Web 服务未就绪或数据库不可用 `dependency.unavailable`（503）。

**渲染映射**

- 站点列表：`table`（`source` 游标分页、`paging: 'cursor'`），列 = 域名 / 类型 / 接入 / 状态 / HTTPS / 运行环境 / 创建时间；行点击 `navigate` 到 `/{p}/sites/{domain}`；`can_admin` 时提供「新建站点」与批量启停卡片。
- 站点详情：运行环境不可用时顶部 warning `alert`；「站点信息」+「站点操作」（启停/默认/删除）+「vhost 配置」+「附加域名」+「证书 / HTTPS」+「站点备份」卡片。删除用 `prompt` 要求输入域名，服务端同样校验 `confirm`；配置 textarea 预填当前内容，保存失败直接展示字段错误。
- 新建站点：`/{p}/sites/new` 隐藏路由 + 原生表单；运行环境下拉来自 `GET /ui/v1/runtimes`，无就绪实例时保持空选项并由后端拒绝。
- 运行环境列表：`table`，列 = 类型 / 实例 / 版本 / 说明 / 状态 / 站点数 / 接入；安装与配置仍链回旧版页面。
- **D10 翻页**：1.8.0 的 `table` 支持游标分页（`paging: 'cursor'` + `cursorPath` + `loadMore`），与内核 `{items, next_cursor}` 直接对齐；本页与运行环境、容器、镜像/网络/卷、任务、作业列表统一改为 `source` 取数 + 底部「加载更多」，页面级只额外读一次 `?limit=1` 的元数据（`can_admin` / `docker_available`）。

### 5.9 容器（第二批）

**页面与数据**

- `GET /{p}/containers`、`/{p}/containers/{id}` 交付 budding 壳页；旧 SSR 已随 `/{p}/m/core/*` 深链删除（2026-11-18）。
- 列表复用 `Docker::list_containers` 与 `handlers::container_view`，`filter=error` 与旧页面同一判定（`container_abnormal`），不新增 SQL。
- 详情复用 `resolve_container` + `inspect_container`；读权限仍是 `policy.check(ContainerInspect)`，不新增权限规则。
- 启停/重启/删除：`POST /ui/v1/containers/{id}/{action}`，CSRF 走 `x-csrf-token` 头；策略判定、deny 审计与 `docker_cmd::container_action` 与旧 htmx 路径完全同源。
- 新建容器：`GET /{p}/containers/new` 交付壳页原生表单（`ContainerCreate` 门禁仍在页面入口），提交走 `POST /ui/v1/containers`；参数解析复用 `containers::spec_from_form`，`validate_spec` 与 `docker_cmd::container_create` 不变。
- 资源限制编辑：`POST /ui/v1/containers/{id}/limits`，沿用 `ContainerUpdate` 策略、`containers::limits_from_form`（空串 = 0 = 清除）与 `docker_cmd::container_update`；详情页直接给表单，不再跳旧 SSR。

**字段表（冻结，只增不改）**

`GET /ui/v1/containers?limit=50&cursor=&filter=error` → `{ "data": { items, total, next_cursor, filter, docker_available, docker_reason, can_operate, can_delete, can_admin } }`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `items[].id` / `short_id` / `name` | string | 容器完整 id、短 id、名称 |
| `items[].image` | string | 展示用镜像名（与旧表格同样截断 60 字符） |
| `items[].state` | `{code,label}` | Docker 状态码 + 中文（D8） |
| `items[].status` | string | Docker 状态描述（截断 60 字符） |
| `items[].project` | string | compose 项目名，非 compose 为空串 |
| `items[].protected` | bool | 受保护容器（禁止删除） |
| `items[].unprotected_kernel` | bool | 内核托管但缺保护标签的存量容器 |
| `filter` | string | 原样回显（空串或 `error`） |
| `docker_available` / `docker_reason` | bool / string | 与旧页面同口径的可用性与原因 |
| `can_operate` / `can_delete` | bool | 当前角色是否可启停 / 删除 |
| `can_admin` | bool | 是否具备 `DockerLifecycle`（新建容器入口显隐；与 `can_delete` 同源） |
| `next_cursor` | string \| null | 上一页末项 `id`（D10） |

`GET /ui/v1/containers/{id}` → `{ "data": ... }`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `container` | object | 上面 `items[]` 的全部字段 |
| `image` / `cmd` / `health` | string | 镜像、启动命令（空格连接）、健康检查状态 |
| `created_at` | string | Docker 返回的 RFC3339 UTC 原值（D9） |
| `ports[]` | object[] | `host` / `container` / `protocol` |
| `mounts[]` | object[] | `kind` / `source` / `target` / `read_only` |
| `networks` | string[] | 接入网络名 |
| `labels[]` | object[] | `key` / `value`，按 key 排序 |
| `limits` | object | 表单原始值：`cpu_shares` / `cpu_period` / `cpu_quota` / `memory_limit_mb` / `memory_swap_mb` / `pids_limit` |
| `limits_effective[]` | object[] | `label` / `value`，与旧详情「当前生效」同一文案 |
| `can_exec` / `can_operate` / `can_delete` | bool | 当前角色的动作能力 |
| `docker_reason` | string | Docker 不可用原因，正常为空串 |

`POST /ui/v1/containers/{id}/{action}`（`start` / `stop` / `restart` / `kill` / `delete|remove`）→ `{ "data": { id, name, action, removed } }`。错误信封沿用全局：未认证 `auth.unauthenticated`（401）、CSRF 失败 `auth.csrf_invalid`（403）、策略拒绝 `auth.forbidden`（403）、容器不存在 `not_found`（404）、Docker 不可达 `dependency.unavailable`（503）、未知动作 `shape.invalid`（400）。

`POST /ui/v1/containers`，body 与旧表单同名：`image`（必填）、`name`、`ports`、`env`、`binds`、`volumes`、`restart`、`cpu_shares` / `cpu_period` / `cpu_quota`、`memory_limit_mb` / `memory_swap_mb`、`pids_limit`、`networks`（string[]）→ `{ "data": { id } }`。`ports` / `env` / `binds` / `volumes` / `networks` 前端按行或逗号拆成文本后由后端同一解析器处理。

`POST /ui/v1/containers/{id}/limits`，body 六个字符串字段（`cpu_shares` / `cpu_period` / `cpu_quota` / `memory_limit_mb` / `memory_swap_mb` / `pids_limit`），空串按旧表单语义转 0（清除）→ `{ "data": { id, name, updated } }`。

**渲染映射**

- 列表：`table`（`source` 游标分页、`paging: 'cursor'`），列 = 名称 / ID / 镜像 / 状态 / 状态描述 / Compose 项目；行点击 `navigate` 到 `/{p}/containers/{id}`。`filter=error` 由 URL 查询参数并入 `source.params`。
- 详情：按块拆分「基本信息 / 端口 / 挂载 / 网络 / 资源限制 / 标签」六张 `card`；`can_operate` 时按状态给启动或重启+停止按钮，`can_delete` 时给删除按钮与资源限制表单（删除前 `window.confirm`）；动作成功后刷新当前页，删除后回列表。
- 新建容器：`/{p}/containers/new` 隐藏路由 + 原生 `form`，字段与旧表单同名；创建成功后跳转 `/{p}/containers/{id}`，失败展示统一错误信封文案。
- **D10 翻页**同 5.8：列表由表格 `source` 按 `next_cursor` 追加，底部「加载更多」。

### 5.10 镜像 / 网络 / 卷（第三批，读写）

- `GET /{p}/images`、`/{p}/networks`、`/{p}/volumes` 交付 budding 壳页；旧 SSR 已随 `/{p}/m/core/*` 深链删除（2026-11-18）。
- 复用 `list_images` / `list_networks` / `list_volumes` 与旧页面的 `ResourceRow` / `NetworkRowView` 展示模型，不新增 SQL。
- 写操作已原生化：`POST /ui/v1/images/pull`、`POST /ui/v1/images/{id}/remove`、`POST /ui/v1/networks`、`POST /ui/v1/networks/{id}/remove`、`POST /ui/v1/volumes`、`POST /ui/v1/volumes/{name}/remove`；逐项复用 `ImagePull` / `ImageRemove` / `NetworkCreate` / `NetworkRemove` / `VolumeCreate` / `VolumeRemove` 策略与 `docker_cmd` 审计。
- 网络名/卷名的字符集判定抽到 `resources::validate_name`，旧表单与 `/ui/v1` 共用一份。
- 写操作的错误信封（与容器/编排一致，按状态码分流、不做文案匹配）：重名对象——Docker 回 **409**——→ **409 `domain.conflict`**，`message` 用 Docker 原文（如 `network with name x already exists`）；入参形状不合法 → 400 `shape.invalid`；Docker 不可达 → 503 `dependency.unavailable`。`DockerError` 只为 304 与 409 保留结构化变体（`NotModified` / `Conflict`），其余状态码仍按 `Api(String)` 透出。

**字段表（冻结，只增不改）**

三个列表统一信封：`{ "data": { items, total, next_cursor, docker_available, docker_reason, can_admin } }`；`next_cursor` 为上一页末项 `id`（D10）。

`GET /ui/v1/images` 的 `items[]`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | string | 镜像完整 id（sha256） |
| `name` | string | 标签列表（逗号连接）；无标签为 `<none> (短id)` |
| `meta` | string | 人类可读大小（与旧页面同一 `human_size`） |
| `warn` | string | 旧页面预留提示位，当前为空串 |

`GET /ui/v1/networks` 的 `items[]`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` / `name` | string | 网络 id 与名称 |
| `driver` / `scope` | string | 驱动与作用域 |
| `ipam_driver` / `subnet` / `gateway` | string | 原样文案，未知显式「未知」/「—」 |
| `internal` / `attachable` | string | 「内部/外部」「可附加/不可附加/未知」 |
| `container_count` | string | 「N 个」或「未知」 |
| `containers_known` | bool | Docker 是否返回了容器列表 |
| `containers[]` | object[] | `name` / `ipv4` / `ipv6` / `mac` |
| `builtin` | bool | Docker 预定义网络 |
| `can_remove` | bool | 当前网络是否允许删除 |

`GET /ui/v1/volumes` 的 `items[]`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` / `name` | string | 卷名 |
| `meta` | string | `driver → mountpoint` |
| `warn` | string | 宿主绑定语义时为「宿主绑定语义」，否则空串 |

错误与容器列表一致：未认证 `auth.unauthenticated`（401）、Docker 不可达时列表仍 200 但 `docker_available=false` 并给原因。

**渲染映射**

- 三个页面共用 `table`（`source` 游标分页、`paging: 'cursor'`）：镜像列 = 镜像/大小/提示；网络列 = 名称/驱动/范围/子网/网关/容器/内部外部；卷列 = 名称/驱动与挂载点/提示。
- Docker 不可用时顶部 warning `alert`；列表底部按 `next_cursor` 显示「加载更多」。
- `can_admin` 为真时追加「拉取镜像 / 创建网络 / 创建卷」表单与行内「删除」（网络行按钮按 `can_remove` 显隐）；删除前 `window.confirm`，成功后刷新当前页。viewer 保持只读并提示需要管理员权限。

### 5.11 编排 Compose（第四批）

- `GET /{p}/compose` 交付 budding 壳页；旧 SSR 已随 `/{p}/m/core/*` 深链删除（2026-11-18）。
- 列表复用 `compose::status_of`（容器标签归组，不持久化 compose 源文件），不新增 SQL；项目按名称排序。
- 项目级启停/重启与 down 已提供 JSON 动作，复用现有 `compose_project_action` / `compose_down` 与逐容器权限判定、审计。
- 编辑/校验/部署已原生化：`POST /ui/v1/compose/validate` 只解析与 `validate_specs`，不触达 Docker；`POST /ui/v1/compose/up` 校验后走 `compose_up`。源文本不持久化。

**字段表（冻结，只增不改）**

`GET /ui/v1/compose?project=` → `{ "data": { projects, services, selected_project, docker_available, docker_reason, can_admin } }`

| 字段 | 类型 | 说明 |
|---|---|---|
| `projects[].name` | string | 项目名（来自 `io.skiff.project` 或 compose 标签） |
| `projects[].managed` | bool | 是否为内核托管项目 |
| `projects[].services` / `running` | number | 服务总数 / 运行中数量 |
| `services[].service` / `container` / `id` | string | 服务名、容器名、容器 id |
| `services[].state` | `{code,label}` | 容器状态（D8） |
| `services[].status` / `image` | string | 状态描述与镜像 |
| `services[].managed` | bool | 容器是否带内核保护标签 |
| `services[].logs` | string | 选中项目时最近 100 行日志（未选中为空串） |
| `selected_project` | string | 请求的 `project` 参数原样回显 |
| `docker_available` / `docker_reason` | bool / string | 与旧页面同口径；`docker_reason` 在列表读取失败时给错误文本 |
| `can_admin` | bool | 是否具备 `DockerLifecycle`（编辑/部署表单显隐） |

`POST /ui/v1/compose/{project}/{action}`（`start` / `stop` / `restart`）→ `{ "data": { project, action, done, failed } }`；`failed` 含策略拒绝与命令失败。

`POST /ui/v1/compose/validate`，body `{ "project": "...", "compose": "..." }` → `{ "data": { project, services, networks, volumes, valid } }`；只做解析、项目名归一与 `validate_specs`，不访问 Docker。

`POST /ui/v1/compose/up`，同一 body → `{ "data": { project, failed, services } }`；`failed > 0` 如实回传（命令层审计落 `partial`）。

`POST /ui/v1/compose/down`，body `{ "project": "...", "remove_volumes": false }` → `{ "data": { project, remove_volumes, report } }`，`report` 为既有命令返回的逐容器结果。

错误：未认证 `auth.unauthenticated`（401）、CSRF `auth.csrf_invalid`（403）、无编排权限 `auth.forbidden`（403）、未知动作 `shape.invalid`（400）、Docker 不可达 `dependency.unavailable`（503）。

**渲染映射**

- 项目表格：项目 / 归属 / 服务数 / 运行中，行内按钮启动、停止、重启、清理（清理前确认，卷默认保留）。
- `can_admin` 时追加「编辑 / 校验 / 部署」卡片：项目名（必填，不能为 `skiff` / `sites`）、compose 文本域、操作下拉（仅校验 / 校验并部署）。校验结果以弹窗展示服务顺序、网络与卷；部署后刷新列表。viewer 只看到权限提示。
- 服务日志暂由旧页展示（日志查看器待 T3）。
