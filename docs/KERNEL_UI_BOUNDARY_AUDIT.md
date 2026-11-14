# 内核业务层 ↔ UI 层边界审计（SSR + `/ui/v1`）

> 状态：**现行审计稿**（2026-11-14，只读审计，未改任何代码）。
> 范围：`crates/web`（SSR handler + `/ui/v1` JSON handler）、`crates/app` 命令层、`crates/plugin`/`crates/pkgfmt` 的插件 UI 边界、`crates/web/static` 的壳页与静态资源。
> 结论口径：每条问题都给「现象（文件:行号）→ 为什么是问题 → 影响面」；未确认的写「未验证」，不当作事实。
> 相关契约：[DESIGN.md](DESIGN.md)（分层与依赖方向）、[URL_CONTRACT.md](URL_CONTRACT.md) §2/§6/§9、[SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md) §3/§4.4/§5、[PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md)、[MENU_CONTRACT.md](MENU_CONTRACT.md)、[DEV_ENV.md](DEV_ENV.md) §4。

## 0. 一页结论

| 指标 | 实测值 |
| --- | --- |
| 问题总数 | **18** 条 |
| 分级分布 | **S1（阻碍长期维护）4 条**；**S2（一致性/可维护性风险）10 条**；**S3（风格/低风险）4 条** |
| `ui_api.rs` 体量 | **5,642 行**、**95 个 `pub async fn` handler**、`lib.rs` 中 **88 条 `/ui/v1` 路由**、覆盖 **15 类资源** |
| `/ui/v1` 专项集成测试 | `crates/web/tests/main_flow.rs` 中 **42 个 `ui_*` 用例**、约 175 处 `/ui/v1` 引用 |

**最重要的三条结论：**

1. **`ui_api.rs` 已经是巨型模块**：95 个 handler 与全部 DTO、字段映射、分页、错误转换堆在一个文件里，任何单资源改动都要求通读整个文件，review 与合并冲突成本持续上升（问题 1）。
2. **JSON 路径依赖 SSR 的内部实现，而不是共享的命令层输入**：`/ui/v1` 把 JSON body 先压回 `HashMap<String, Vec<String>>` 表单，再调用 SSR 的表单解析函数（`new_form`、`spec_from_form`、`limits_from_form`），JSON 契约实际上被 SSR 表单字段名绑架（问题 2）；运行环境列表还直接把 SSR 展示结构 `templates::RuntimeRow` 转成 JSON（问题 3）。
3. **同一操作存在两套编排**：站点列表、运行环境详情、改密、TOTP、公钥导入/撤销都是 SSR 与 `/ui/v1` 各写一遍编排，其中改密/TOTP 已出现分支差异（错误文案、是否写审计、空密钥校验）（问题 4）。

**若只能先做三件事**：拆 `ui_api.rs`（纯移动，零契约变化）→ 去掉「JSON → 表单 → SSR parser」中间层 → 抽 settings 账户安全与站点/运行环境读取的共享函数。三项都不改变对外契约。

---

## 1. 依赖方向现状（先澄清一个容易误判的点）

**crate 级依赖不违规。** 工程契约的依赖表中，`web` 允许依赖 `app/model/store/identity/docker/plugin/pkgfmt`（[08-engineering.md](sections/08-engineering.md#L51-L67)）。因此「`crates/web` 依赖 `skiff-store` / `skiff-docker`」本身不是问题。

**真正的越层在调用面**，分三类：

| 类型 | 是否存在 | 证据 |
| --- | --- | --- |
| web 里手写 SQL | **不存在** | 全 crate 检索 `SELECT/INSERT/UPDATE/DELETE/conn()/query_row/prepare` 无命中 |
| 绕过 `app` 命令层直接 `Docker::execute` | **存在 1 处** | [api.rs](file:///c:/dev/skiff/crates/web/src/handlers/api.rs#L112-L146) |
| 绕过 `app` 命令层直接调 `AdminFs` | **存在 1 组** | [ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L5393-L5571) |

`Docker::execute` 本身**不做**权限判定（[client.rs](file:///c:/dev/skiff/crates/docker/src/client.rs#L845-L862)），对象级 `policy.check` 在调用方；所以绕过命令层不会直接造成越权，但会绕过「审计随命令产生」的约定。

---

## 2. 问题清单

### S1 — 阻碍长期维护

#### 问题 1：`ui_api.rs` 是「什么都往里塞」的巨型模块

**现象**
- 文件 [ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs) 共 **5,642 行**，含 **95 个 `pub async fn`**、**105 个 `pub struct`**（DTO 与 handler 混排）。
- 路由全部集中在 [lib.rs](file:///c:/dev/skiff/crates/web/src/lib.rs#L169-L420) 的 `ui_json` 分组，88 条路由连续堆叠，无子 `Router` 分层。
- 资源覆盖 15 类：auth/session、menu、overview、sites、runtimes、containers、images/networks/volumes、compose、tasks、jobs、audit、plugins、store、settings、web(nginx)、files。

**为什么是问题**
- 单文件承担 DTO 定义、查询编排、分页、枚举映射、错误翻译、审计调用六种职责；`mod tests` 只有 2 个用例（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L5847-L5884)），其余验证全靠集成测试。
- 任何一个资源改字段都要在 5,600 行里定位；多会话并行时该文件是必然的冲突热点（与仓库「提交按逻辑拆分」的纪律相冲突）。

**影响面**：所有 `/ui/v1` 消费者（budding 壳与 `static/budding/budding-boot.js`）；后续新增页面只能继续往同一文件追加。

#### 问题 2：JSON 路径依赖 SSR 表单解析层

**现象**
- 站点新建：`SiteCreateBody` → `site_create_form` 压成表单 map → `handlers::sites::new_form(&form)` → `CreateSite`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L797-L845)）。
- 容器新建：`ContainerCreateBody` → `create_form` → `handlers::containers::spec_from_form`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L2874-L2934)）。
- 容器资源限制：`ContainerLimitsBody` → `limits_form` → `handlers::containers::limits_from_form`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L2958-L2996)）。

**为什么是问题**
- 命令层入参本是类型化 DTO（`commands::site::CreateSite`、`ContainerSpec`），JSON 路径却先降级为字符串表单再解析回来：类型信息丢失、`?` 字段缺失会被静默当成空串。
- `new_form`、`spec_from_form`、`limits_from_form` 是 SSR 表单语义的实现细节；SSR 改名/改校验时 JSON 契约随之改变，而 `docs/SDUI_BUDDING_INTEGRATION.md` 的字段表不会同步提醒。
- 结果：两条传输路径共享的是「表单字段名」这个隐式契约，而不是共享命令层。

**影响面**：站点创建、容器创建、容器限额三类写操作的 JSON 稳定性；budding 表单一旦与 SSR 表单字段名分叉，错误会以「参数为空」而非「字段不合法」出现。

#### 问题 3：JSON DTO 直接依赖 SSR 展示模型

**现象**
- `runtimes` 调用 `handlers::sites::runtime_row`（返回 `templates::RuntimeRow`）再逐字段转成 `RuntimeItem`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L1217-L1272)）。
- `network_item(v: crate::templates::NetworkRowView)`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L3061-L3088)）。
- `plugin_item(view: crate::templates::PluginView)`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L4238-L4259)）。

**为什么是问题**
- 依赖方向在 web crate 内部反了：JSON 层依赖 SSR 的展示模型，而不是依赖领域/查询结果。模板字段为渲染服务（文案、CSS class、截断），却成为 API 的输入。
- 改 SSR 文案或删模板字段会同时改 API 输出；API 字段冻结（SDUI §3.7「字段只增不改」）无法靠编译期保证。

**影响面**：运行环境、网络、插件三类列表/详情的 JSON 稳定性；模板重构会被迫带上 API 兼容评估。

#### 问题 4：同一操作两套编排（已分叉或高概率分叉）

**现象（逐条对照）**

| 操作 | SSR 实现 | JSON 实现 | 现状 |
| --- | --- | --- | --- |
| 站点列表读取 | [sites/crud.rs](file:///c:/dev/skiff/crates/web/src/handlers/sites/crud.rs#L47-L124) `render_sites` | [ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L476-L538) `sites` | 各自做 `list_sites + list_runtimes + runtime_views + cert_ready + 过滤 + 分页`，view model 不同（`SiteView` vs `SiteItem`） |
| 运行环境详情 | [sites/runtime.rs](file:///c:/dev/skiff/crates/web/src/handlers/sites/runtime.rs#L316) | [ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L1747-L1908) | 数据源同为 `state.runtime_view` 等，但聚合逻辑（sites 过滤、extensions 组装、port_editable）各写一遍 |
| 改密 | [auth.rs](file:///c:/dev/skiff/crates/web/src/handlers/auth.rs#L249-L320) | [ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L5133-L5197) | 校验顺序、`blocking_fs`、轮换会话、删首启口令文件四步逐行重复 |
| TOTP 启用 | [auth.rs](file:///c:/dev/skiff/crates/web/src/handlers/auth.rs#L346-L376) | [ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L5230-L5266) | **已分叉**：SSR 对空密钥返回「请先生成 TOTP 密钥」并拒绝；JSON 不校验空密钥，错误变成「动态码不正确」；JSON 额外写 `auth.totp.enable` 审计（ip 硬编码 `"local"`），SSR 不写 |
| 公钥导入/撤销 | [settings.rs](file:///c:/dev/skiff/crates/web/src/handlers/settings.rs#L289-L340) | [ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L5277-L5334) | decode、`key_id_of`、`trust_key`、审计四步重复；两处审计 ip 都写死 `"local"` |

**为什么是问题**
- 业务结论不一致时没有单一裁定处；TOTP 已是实例（同一账号在两条路径受到不同校验）。
- 修复一个 bug 要改两处，且没有测试强制两处等价。

**影响面**：站点/运行环境读模型、账户安全写操作；后续任何「SSR 与 JSON 同源」承诺都难以兑现。

---

### S2 — 一致性与可维护性风险

#### 问题 5：写入口的权限/CSRF 校验顺序不统一

**现象**
- SSR 用提取器守门：`AdminUser` / `PluginManagerUser`（[middleware.rs](file:///c:/dev/skiff/crates/web/src/middleware.rs#L486-L519)），权限与动作注册表对齐有单测（[authz.rs](file:///c:/dev/skiff/crates/web/src/authz.rs#L99-L125)）。
- JSON 大多手写 `check_csrf_header` + `authorize`，但顺序不一致：`store_install` 先 CSRF 后权限（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L4563-L4571)）；`settings_key_import` 先权限后 CSRF（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L5283-L5286)）。
- 插件类写入口不走动作注册表，直接 `require_permission(PluginManage)`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L4358-L4359)、[4384-4385](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L4384-L4385)、[4707-4708](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L4707-L4708)）。
- Docker 资源写接口走第三条路：`state.cfg.policy.check(actor, Verb, obj)`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L3317-L3326)），这是动词权限矩阵的正规入口，但入口声明形式仍是「提取器 / `authz::authorize` / `policy.check` / `require_permission`」四种并存。
- 另有 `handlers::sites::require_admin` 作为 `Permission::Admin` 的第二个名字（[sites.rs](file:///c:/dev/skiff/crates/web/src/handlers/sites.rs#L425-L427)），JSON 侧大量使用。

**为什么是问题**
- 同一个「无权限 + CSRF 缺失」请求，不同端点返回的先后错误不同（403 CSRF vs 403 权限）；客户端重试语义随之不同。
- 两套门禁表述（提取器 / 手写守卫）让「权限矩阵唯一来源」只剩判定函数一级，入口声明级仍是多份。

**影响面**：所有 `/ui/v1` 写端点与 SSR 写端点的错误体验；新增端点靠人记忆选择写法。

#### 问题 6：审计口径有缺口与硬编码来源

**现象**
- TOTP 启用：JSON 写审计、SSR 不写（见问题 4）。
- 公钥导入/撤销：两条路径都用 `"local"` 作为 ip（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L5296-L5303)、[settings.rs](file:///c:/dev/skiff/crates/web/src/handlers/settings.rs#L302-L309)），丢失真实客户端来源。
- `/api/v1` 重启容器：`audit_or_log` 由 handler 手写（[api.rs](file:///c:/dev/skiff/crates/web/src/handlers/api.rs#L131-L143)），与 `commands::docker` 内的审计是两套代码。

**为什么是问题**
- 同一动作在审计表里的动作名、来源列、是否有记录取决于入口，事后取证会出现「SSR 做过但查不到」。
- ip 写死 `"local"` 使安全事件无法定位来源。

**影响面**：审计可信度（`/ui/v1/audit` 与 SSR 审计页展示同一张表）。

#### 问题 7：插件 UI 静态资源没有登录门禁

**现象**
- `GET /{p}/plugins/{id}/ui/{*path}` 的 handler 只取 `State + Path`，无 `AuthUser`（[plugins.rs](file:///c:/dev/skiff/crates/web/src/handlers/plugins.rs#L843-L890)）。
- 同插件的 schema 接口要求 `PluginManagerUser`（[plugins.rs](file:///c:/dev/skiff/crates/web/src/handlers/plugins.rs#L897-L914)），RPC 同样（[plugins.rs](file:///c:/dev/skiff/crates/web/src/handlers/plugins.rs#L777-L783)）。
- `load_session` 不强制登录，只注入 `CurrentUser`（[middleware.rs](file:///c:/dev/skiff/crates/web/src/middleware.rs#L187-L283)）。

**为什么是问题**
- 未登录可直接读取插件 `ui/` 下的 HTML/JS（是否含敏感数据取决于插件实现）。schema 需要 PluginManage 而静态页不需要，同一插件出现两种权限口径。
- **注意**：这是否为设计意图，文档未见明确说明 → **未验证**；但代码事实是「无认证提取器」。

**影响面**：插件 UI 的暴露面；若未来插件 UI 内嵌 token/配置，会直接外泄。

#### 问题 8：RPC 错误靠字符串猜内核状态码

**现象**
- `kernel_http_status` 在 SDK 错误文案里找固定片段「内核返回 HTTP 」，再解析数字（[plugins.rs](file:///c:/dev/skiff/crates/web/src/handlers/plugins.rs#L741-L748)），据此映射 HTTP 状态（[plugins.rs](file:///c:/dev/skiff/crates/web/src/handlers/plugins.rs#L758-L770)）。

**为什么是问题**
- 协议语义（错误形状）由 `skiff-wire` / `skiff-plugin` 拥有；web 层用字符串匹配复原它，SDK 改一句文案就把 503 压成 400。
- 这正是 AGENTS 红线「UDS 协议只在 `skiff-wire`」的精神边界：虽然没改协议，但把协议解释逻辑放到了传输层。

**影响面**：插件 RPC 的错误码稳定性；`fail2ban` 缺失等依赖类失败的 503 语义。

#### 问题 9：插件版本目录布局知识重复

**现象**
- web 侧自行拼 `data_dir/plugins/{id}/versions/{active_version}/ui[/schema]`（[plugins.rs](file:///c:/dev/skiff/crates/web/src/handlers/plugins.rs#L854-L861)、[922-930](file:///c:/dev/skiff/crates/web/src/handlers/plugins.rs#L922-L930)）。
- 同类拼接也出现在 [supervisor.rs](file:///c:/dev/skiff/crates/plugin/src/supervisor.rs#L696)、[official.rs](file:///c:/dev/skiff/crates/plugin/src/official.rs#L197)、[install.rs](file:///c:/dev/skiff/crates/plugin/src/install.rs#L330)。
- `plugin_root` 只覆盖到插件根（[install.rs](file:///c:/dev/skiff/crates/plugin/src/install.rs#L91-L93)），没有 `version_dir()` / `ui_dir()`。

**为什么是问题**
- 布局变更（如增加 `current` 符号链接）需要改 4 处；web 侧拼错只会表现为 404，不会编译失败。

**影响面**：插件安装、启动、UI/schema 托管三处一致性。

#### 问题 10：`/api/v1` 重启容器绕过命令层

**现象**
- [api.rs](file:///c:/dev/skiff/crates/web/src/handlers/api.rs#L112-L146) 直接 `state.docker().execute(Verb::Restart, ...)` 并手写审计；而 SSR/JSON 走 `commands::docker::container_action`（[containers.rs](file:///c:/dev/skiff/crates/web/src/handlers/containers.rs#L694)、[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L2814-L2825)）。

**为什么是问题**
- 命令层注释明确「审计随命令产生，handler 不再代写」（[commands/docker.rs](file:///c:/dev/skiff/crates/app/src/commands/docker.rs#L1-L8)），这里正好相反。
- 一旦命令层调整（例如重启后等待健康），`/api/v1` 不会跟随。

**影响面**：`/api/v1` 机器 API 的语义一致性（当前只有这一个端点，扩散成本低）。

#### 问题 11：`/ui/v1` 文件写操作绕过 `commands::file`

**现象**
- JSON 侧直接 `fs.mkdir/rename/remove/read/list`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L5393-L5571)）。
- SSR 侧全部经 `commands::file`（[files.rs](file:///c:/dev/skiff/crates/web/src/handlers/files.rs#L451-L573)）。

**为什么是问题**
- 短期行为等价：授权、路径代数、回收站、审计都在 `AdminFs`/`skiff-fs` 内，审计 sink 在 [admin_fs.rs](file:///c:/dev/skiff/crates/app/src/admin_fs.rs#L369-L409)，所以**不是安全漏洞**。
- 但「单一入口」已被绕过：命令层若增加前置校验或 DTO 变更，JSON 路径不跟随；两条路径的错误映射也开始分叉（`fs_api_error` 手写状态映射，[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L5376-L5388)）。

**影响面**：文件管理后续演进；属于「现在等价、将来分叉」的典型。

#### 问题 12：DTO 冻结没有编译期约束

**现象**
- 全部 DTO 内联在 `ui_api.rs`；大量响应是匿名 `json!` 对象（例如站点列表 [ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L526-L533)、容器列表 [1402-1414](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L1402-L1414)、插件列表 [4273-4280](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L4273-L4280)）。
- 冻结口径仅存在于文档（SDUI §3.7「字段只增不改」，[SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md#L279-L289)）与集成测试断言。
- 无 JSON Schema / golden 文件 / 字段集合快照测试。

**为什么是问题**
- 删除字段或改名不会编译失败，只能靠测试与 review；匿名 `json!` 连字段类型都没有声明。
- `SiteDetailData` 使用 `#[serde(flatten)]`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L584-L595)），字段集合被拆分到两个结构，冻结审计更难。

**影响面**：前端契约漂移风险；budding 升级时的兼容判断。

#### 问题 13：插件菜单视图排序规则重复

**现象**
- `plugin_menu_views` 自行按 `(order, label, id)` 排序并去重（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L5711-L5751)）。
- 同一排序规则在 `skiff_app::menu` 已有两处（[menu.rs](file:///c:/dev/skiff/crates/app/src/menu.rs#L391)、[397](file:///c:/dev/skiff/crates/app/src/menu.rs#L397)）。

**为什么是问题**
- 菜单顺序是可见契约；两处规则一旦不一致，SSR 导航与 budding 路由表顺序不同。

**影响面**：菜单/插件视图顺序。

#### 问题 18：错误信封单一来源，但插件错误映射不对称

**现象**
- 正向：`ErrorEnvelope` / `ErrorCode` 是单一定义（[error.rs](file:///c:/dev/skiff/crates/web/src/error.rs#L34-L107)、[156-192](file:///c:/dev/skiff/crates/web/src/error.rs#L156-L192)）；SSR 走 `IntoResponse` 渲染 HTML（117-153），JSON 走 `ApiError` 输出同一 `envelope()`（184-191），并有单测锁定映射（277-337）。权限矩阵也只来自 `skiff_model::Permission`（[authz.rs](file:///c:/dev/skiff/crates/web/src/authz.rs#L20-L31)、[mod.rs](file:///c:/dev/skiff/crates/web/src/handlers/mod.rs#L374-L387)）。
- 不对称：`PluginError` 只有 `Rejected` 映射成 400，其余一律 `Internal` 500（[error.rs](file:///c:/dev/skiff/crates/web/src/error.rs#L229-L236)）；而 RPC 路径又用字符串解析把内核 503 找回来（问题 8）。同一类「插件依赖不可用」在 `/ui/v1/plugins/{id}/rpc` 是 503，在安装/升级/卸载接口是 500 且 `retryable=false`。

**为什么是问题**
- 前端无法用统一的 `code`/`retryable` 判断「稍后重试」；错误语义取决于走哪个端点。
- 插件 crate 的错误类型没有区分「依赖不可用 / 拒绝 / 内部错误」，这是 `skiff-plugin` 的职责缺口，web 层只能猜。

**影响面**：所有插件写接口与 RPC 的错误体验；`main_flow` 对 503 的断言目前只覆盖 RPC。

---

### S3 — 风格/低风险

#### 问题 14：压缩跳过列表在两个文件里手工同步

**现象**
- [build.rs](file:///c:/dev/skiff/crates/web/build.rs#L22-L34) 的跳过扩展名列表，与 [assets.rs](file:///c:/dev/skiff/crates/web/src/assets.rs#L471-L487) 测试里的 `ALREADY_COMPRESSED` 常量重复，注释明说「保持一致」。

**影响**：新增二进制资源时容易只改一处，导致测试误报或漏生成 `.gz`。

#### 问题 15：budding pack 产物没有存在性断言

**现象**
- `assets.rs` 只声明主包与图标版本（[assets.rs](file:///c:/dev/skiff/crates/web/src/assets.rs#L28-L44)）。
- 启动脚本用 `data-budding-version` 拼 `budding-code-<ver>.min.js` / `budding-terminal-<ver>.min.js`（[budding-boot.js](file:///c:/dev/skiff/crates/web/static/budding/budding-boot.js#L1036-L1052)）。
- 没有测试断言这些 pack 文件随当前 `BUDDING_VERSION` 存在；`shell_html` 测试只覆盖主包/图标（[spa.rs](file:///c:/dev/skiff/crates/web/src/handlers/spa.rs#L206-L226)）。

**影响**：升级主包但漏更新 pack 文件名时，浏览器只会在运行时 404（是否静默降级为纯文本 → **未实测**）。

#### 问题 16：同一日志查询在 SSR 与 JSON 的参数名不同

**现象**
- SSR `GET /{p}/sites/web` 用 `?type=error`（[nginx.rs](file:///c:/dev/skiff/crates/web/src/handlers/nginx.rs#L63-L91)）。
- JSON `GET /ui/v1/web/logs` 用 `?kind=error`（[ui_api.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui_api.rs#L2674-L2705)）。

**影响**：前端要从旧页迁参数时容易传错，且文档字段表需记两套。

#### 问题 17：schema 运行期校验失败返回 500

**现象**
- 安装期已校验，运行期再校验一次是合理的；但失败返回 `AppError::Internal`（[plugins.rs](file:///c:/dev/skiff/crates/web/src/handlers/plugins.rs#L940-L942)）。

**影响**：包内容被替换属于「包完整性/客户端问题」，500 会诱导前端提示「服务器错误」；与商店安装侧 400 的口径不一致。

---

## 3. `ui_api.rs` 拆分方案

### 3.1 目标结构（只移动，不改行为）

```
crates/web/src/handlers/ui/
├── mod.rs          // 只做 Router 组装（按资源 merge），导出 handlers 给 lib.rs
├── auth.rs         // login_config / login_submit / session / logout
├── menu.rs         // menu（保留 plugin_menu_views，后续改为共享 helper）
├── overview.rs     // overview
├── sites.rs        // 列表/详情/写操作/site_logs
├── runtimes.rs     // 列表/详情/创建/配置/扩展/端口/克隆/日志
├── containers.rs   // 列表/详情/日志/动作/创建/限额
├── resources.rs    // images / networks / volumes
├── compose.rs      // compose 列表/校验/up/down/项目动作
├── tasks.rs        // tasks + task 写操作
├── jobs.rs         // jobs + cancel/retry
├── audit.rs        // audit
├── plugins.rs      // 插件列表/详情/toggle/delete/confirm
├── store.rs        // store 列表/安装/升级下载
├── settings.rs     // settings + panel/menu/docker/password/totp/keys
├── web.rs          // nginx 状态/日志/配置
├── files.rs        // 文件浏览/读/写
└── dto.rs          // 所有 #[derive(Serialize/Deserialize)] 的 wire 类型
```

- `dto.rs` 只放「wire 形状」，禁止放业务函数；每个资源模块 `use super::dto::*`。
- 每个资源模块导出 `pub fn router(p: &str) -> Router<Arc<AppState>>`，`ui/mod.rs` 负责 `merge`；`lib.rs` 的 `ui_json` 分组收缩为一行 `.merge(handlers::ui::router())`。
- 保持所有 handler 函数名与路径字符串不变，`tests/main_flow.rs` 的路由调用无需改动（测试走 HTTP，不引用模块路径）。

### 3.2 路由注册怎么改

当前 [lib.rs](file:///c:/dev/skiff/crates/web/src/lib.rs#L171-L420) 的 88 条 `.route()` 平移进各资源模块的 `router()`；`lib.rs` 只保留：

```rust
let ui_json = handlers::ui::router()
    .layer(/* 现有超时与状态层不变 */);
```

注意顺序约束不变：`ui_json` 仍在 `.layer(request_timeout)` 之前、`streaming` 之前合并（[lib.rs](file:///c:/dev/skiff/crates/web/src/lib.rs#L866-L888)）；不要在任何子 Router 上重复挂 `client_info`/`load_session`。

### 3.3 拆分的验收方式（行为不变的证据）

1. `tools\bin\cargo.cmd check -p skiff-web --tests` 全绿（只移动，不应新增编译错误）。
2. `crates/web/tests/main_flow.rs` 的 42 个 `ui_*` 用例全绿，重点：`ui_session_returns_identity_csrf_and_prefix`、`ui_sites_and_runtimes_lists_use_frozen_fields_and_keyset_cursor`、`ui_files_mkdir_rename_remove_roundtrip`、`ui_store_install_and_confirm_keep_csrf_and_permission_semantics`。
3. 路由表字符串 diff 为空（可用 `git diff` 人工核对，或临时加一条「路由数量 = 88」的断言测试）。
4. `tools\bin\cargo.cmd fmt` 无 diff。

---

## 4. 分批执行顺序

> 每批一个可独立验证的目标；「零契约」批次不改变 URL、状态码、错误码、响应字段与审计动作名。

### 批次 0（现在就能动手，零对外契约变化）

| # | 目标 | 改动文件 | 行为不变的保证 |
| --- | --- | --- | --- |
| 0.1 | 拆 `ui_api.rs` 为 `handlers/ui/*`，DTO 集中到 `dto.rs` | `crates/web/src/handlers/ui_api.rs` → `handlers/ui/**`；[lib.rs](file:///c:/dev/skiff/crates/web/src/lib.rs#L169-L420) 只保留 merge | 纯移动；42 个 `ui_*` 集成测试 + 路由字符串不变 |
| 0.2 | 去掉「JSON → 表单 → SSR parser」：站点/容器/限额直接构造类型化 DTO | `handlers/ui/sites.rs`、`containers.rs`；必要时把 `new_form`/`spec_from_form` 的**校验语义**下沉到 `skiff_app::commands` 的构造函数 | 现有 `ui_site_writes_*`、`ui_docker_writes_*` 用例；逐字段比对错误文案 |
| 0.3 | 统一 `/ui/v1` 写入守卫：新增 `ui::guard::write(user, action)`（CSRF + `authz::authorize` 固定顺序），替换手写组合 | `handlers/ui/**`、`handlers/mod.rs`（新增 helper） | 状态码/错误码不变；补一条「所有写 handler 都调用 guard」的静态测试（枚举路由） |
| 0.4 | 插件版本目录 helper 下沉到 `skiff_plugin::install`（`plugin_version_dir` / `plugin_ui_dir`） | `crates/plugin/src/install.rs`、`handlers/plugins.rs`、`supervisor.rs`、`official.rs` | 路径字符串不变；`plugin_schema_*` 与 `plugin_ui_*` 用例 |
| 0.5 | `/ui/v1` 文件写操作改走 `commands::file` | `handlers/ui/files.rs` | `ui_files_mkdir_rename_remove_roundtrip`；`fs_api_error` 映射保持 |
| 0.6 | `/api/v1` 重启改走 `commands::docker::container_action` | `handlers/api.rs` | 审计动作名仍为 `Verb::Restart.audit_action()`；`main_flow` 现有 API 用例 |
| 0.7 | 菜单排序复用 `skiff_app::menu` 的 helper | `handlers/ui/menu.rs`、`crates/app/src/menu.rs`（暴露 `pub fn ordered_items`） | 菜单 JSON 顺序不变；`ui_menu_*` 用例 |
| 0.8 | 压缩跳过列表单一常量；补 pack 存在性断言 | `crates/web/build.rs`、`assets.rs`、`spa.rs`（测试） | 新增测试先红后绿；不改运行时行为 |

### 批次 1（需要先定契约）

| # | 目标 | 需要定的契约 |
| --- | --- | --- |
| 1.1 | DTO 冻结机制（golden JSON / key-set 快照 / 可选 JSON Schema） | 冻结粒度（字段集合 vs 完整 shape）、新增字段是否必须升 `api_version` |
| 1.2 | 抽 `app` 单一读取入口：站点列表查询、运行环境详情聚合、settings 账户安全命令 | `skiff_app` 是否接受「读模型」职责；跨 crate API 命名 |
| 1.3 | RPC 错误结构化 | 需要 `skiff-plugin`/`skiff-wire` 提供结构化错误（code + status），属于协议面变更 |
| 1.4 | 插件 UI 静态资源是否加登录门 | 影响外部可访问性；需确认插件 SDK/文档是否依赖匿名加载 |

### 批次 2（收尾）

| # | 目标 | 说明 |
| --- | --- | --- |
| 2.1 | 错误码细分：schema 校验失败改 400/409 并给稳定码 | 与商店安装侧统一 |
| 2.2 | 旧 SSR 实现退役时间表 | 决定「双实现」是长期共存还是逐页删除；删除前必须确认无排序耦合 |
| 2.3 | 审计来源补全（TOTP、公钥操作记录真实 ip） | 审计内容变化需在 TEST_MATRIX 补断言 |

---

## 5. 现在就能动手 vs 需要决策

### 5.1 现在就能动手（不改变任何对外契约）

1. 拆 `ui_api.rs` → `handlers/ui/*` + `dto.rs`（批次 0.1）。
2. 去掉 JSON→表单→SSR parser 中间层（0.2）。
3. 统一 `/ui/v1` 写守卫与校验顺序（0.3）。
4. 插件版本目录 helper 下沉（0.4）。
5. `/ui/v1` 文件写操作改走 `commands::file`（0.5）。
6. `/api/v1` 重启走命令层（0.6）。
7. 菜单排序复用单一 helper（0.7）。
8. 压缩列表单一常量 + pack 存在性测试（0.8）。

### 5.2 需要先定契约、得等决策

1. **DTO 冻结机制**（1.1）：是否引入 golden/JSON Schema，决定后才有编译期或测试期约束。
2. **`app` 读模型入口**（1.2）：跨 crate 职责调整，需确认「读模型放 app 还是新建 query 模块」。
3. **RPC 错误结构化**（1.3）：改 `skiff-wire`/SDK 错误形状，属于协议演进。
4. **插件 UI 登录门**（1.4）：影响插件匿名加载能力，属于安全契约变更。
5. **错误码语义调整**（2.1）：`shape.invalid` vs 新的包完整性码。
6. **旧 SSR 退役**（2.2）：决定双实现的生命周期。

---

## 6. 附录：`/ui/v1` 资源分布（用于拆分工作量估算）

| 资源 | handler 数 | 代表函数 |
| --- | --- | --- |
| auth / session / menu | 5 | `login_config`、`login_submit`、`session`、`menu`、`logout` |
| overview | 1 | `overview` |
| sites | 12 | `sites`、`site_detail`、`site_create`、`site_power`、`site_ssl`、`site_batch`、`site_logs` 等 |
| runtimes | 19 | `runtimes`、`runtime_detail`、`runtime_create`、`runtime_action`、`runtime_config_save`、`runtime_extensions_*` 等 |
| containers | 6 | `containers`、`container_detail`、`container_logs`、`container_action`、`container_create`、`container_limits_update` |
| images / networks / volumes | 9 | `images`、`image_pull`、`image_remove`、`networks`、`network_create`… |
| compose | 5 | `compose`、`compose_validate`、`compose_up`、`compose_down`、`compose_project_action` |
| tasks | 5 | `tasks`、`task_detail`、`task_toggle`、`task_run`、`task_delete` |
| jobs | 4 | `jobs`、`job_detail`、`job_cancel`、`job_retry` |
| audit | 1 | `audit` |
| plugins | 5 | `plugins`、`plugin_info`、`plugin_toggle`、`plugin_delete`、`plugin_confirm` |
| store | 3 | `store`、`store_install`、`store_upgrade_download` |
| settings | 11 | `settings`、`settings_panel_update`、`settings_menu*`、`settings_docker*`、`settings_password_update`、`settings_totp_*`、`settings_key_*` |
| web (nginx) | 4 | `web`、`web_logs`、`web_config_save`、`web_params_save` |
| files | 5 | `files`、`files_read`、`files_mkdir`、`files_rename`、`files_remove` |
| **合计** | **95** | 与 `lib.rs` 的 88 条路由对应（部分路由 GET/POST 共用一个 handler） |

---

## 7. 未验证项

1. **插件 UI 静态资源匿名可读是否为设计意图**：代码无认证提取器（[plugins.rs](file:///c:/dev/skiff/crates/web/src/handlers/plugins.rs#L843-L890)），但文档未找到对应条款，未验证。
2. **budding pack 漏文件时的前端行为**：`budding-boot.js` 有 `try/catch`（[budding-boot.js](file:///c:/dev/skiff/crates/web/static/budding/budding-boot.js#L1053-L1055)），实际是静默降级还是控制台报错，未实测。
3. **`KERNEL_API` 版本门覆盖面**：确认仅 [tasks.rs](file:///c:/dev/skiff/crates/app/src/tasks.rs#L97) 与 [commands/store.rs](file:///c:/dev/skiff/crates/app/src/commands/store.rs#L287) 使用 `api_compatible`；直接 `.wbpkg` 上传安装（[plugins.rs](file:///c:/dev/skiff/crates/web/src/handlers/plugins.rs#L547) → [install.rs](file:///c:/dev/skiff/crates/plugin/src/install.rs#L155)）**没有** API 版本校验。是否另有启动期兜底，未验证。
4. **两套站点列表是否存在排序差异**：SSR 依赖 `list_sites` 返回顺序，JSON 显式按 `(created_at, id)` 做 keyset；两者当前应一致，但未写测试锁定。
