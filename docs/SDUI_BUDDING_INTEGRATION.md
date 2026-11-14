# budding 接入规则（SDUI 前端渲染层）

> 状态：**P0 已定案（2026-11-12），未实施**。本文定义「budding 作为前端渲染层」的接入边界、契约与缺口，不切换现有页面、不改业务代码。
> 上游事实来源：budding 仓库只读盘点（2026-11-12）；内核现状来源：`crates/web`、`crates/plugin` 与本仓库现有契约文档。
> 相关文档：[SDUI_HANDBOOK.md](SDUI_HANDBOOK.md)（框架无关的适配手册）、[URL_CONTRACT.md](URL_CONTRACT.md)、[UI_CONTRACT.md](UI_CONTRACT.md)、[MENU_CONTRACT.md](MENU_CONTRACT.md)、[PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md)。
> 分工：本文只维护 budding 相关的具体事实、契约形状与缺口；框架无关的分层与迁移阶段只在 SDUI_HANDBOOK 维护，本文引用不复述。

---

## 0. 一页结论

| 维度 | 结论 |
|---|---|
| 定位 | budding 只做**表现运行时**；Skiff 后端只出 **JSON 数据与动作结果**，不出布局、不出 HTML 片段、不出跳转 |
| 定案状态 | P0 七条（D1–D7）与 D8–D13 已于 2026-11-12 定案（§8.1）：产物直接入库、不引入 Node、JSON 走 `/ui/v1`、登录页也用 budding、要求上游随发行包提供 admin Pack、按 MIT、文件管理页用 budding 重做、一次性切换；文案 key+中文兜底、时间 RFC3339 UTC、审计/作业/文件 keyset 分页、静态产物版本化、CSP 保持严格 |
| 仓库关系 | **禁止修改 budding 仓库**；产物由上游发版打包后复制进本仓库；缺能力只提建议，由项目负责人决策 |
| 接入前提 | 产物入库 + 单二进制嵌入（`RustEmbed`）；**本项目不引入 Node 工具链**；测试机无公网，部署期不得依赖 npm 或 CDN |
| 最大外部阻塞 | 上游 admin Pack（`admin` / `nav-menu` / `page-tabs`）与文件管理所需能力（§6.1）均未随发行包提供（D4、D6） |
| 最大硬缺口 | 终端 UI 已完成（复用既有 exec 票据 + WS，`terminal` Pack 入库，§5.19）；WS 真连与容器内交互待 Linux 测试机验收；`input-text` 仍未透传 `autocomplete` |
| 最大分发风险 | 上游无 LICENSE 文件；「按 MIT」目前只是我方口径，需上游补 LICENSE 或书面确认（D5、§7.1） |
| 回滚 | 一次性切换，无运行时开关；回滚只能靠 git 版本回退并重新部署（D7） |

---

## 1. 定位与边界

### 1.1 三层分工

| 层 | 归属 | 负责 | 不负责 |
|---|---|---|---|
| 表现运行时 | budding（外部产物） | 布局、组件、交互状态、主题、表单控件、路由 | 权限判定、业务状态、审计、数据校验的最终裁决 |
| 传输适配 | 内核 `crates/web` | HTTP 形状、会话/CSRF、状态码、轮询/流式通道、上传下载 | 解释业务语义、拼装页面布局 |
| 资源与动作契约 | 内核 | 读 DTO、动作元数据、错误码、字段码、菜单声明 | 组件名、props、栅格、颜色、文案排版 |
| 领域服务 | 内核 `skiff-app` 及下层 | 不变量、状态迁移、事务、审计事实、作业 | 感知 HTTP、表单、页面 |

### 1.2 硬性禁止项

1. **不改 budding 仓库**：不提交 PR 到上游、不在本地 fork 后长期维护补丁；缺失能力按 §6 提建议。
2. **内核不出页面描述**：JSON 里不出现 `type`/`props`/布局/宽度/颜色一类字段；页面结构随前端产物固定。
3. **内核不返回重定向表达业务结果**：动作成功/失败用状态码 + 信封表达，不用 302/303 携带结果。
4. **前端权限不承担拦截**：`permission` 只用于显隐与路由占位；所有写操作仍由内核 `skiff_model::Permission` + `actions` 注册表判定。
5. **不引入第二套协议/权限/路径判定**：沿用 `skiff-wire`、`Permission`、`skiff_model::path`、`Docker::execute` 的单一来源。
6. **不引入 Node 工具链**：产物只接收上游打包结果，本项目不执行前端构建（D1）。

### 1.3 缺能力的处理流程

1. 先判断能否用现有组件 + 自定义动作绕过（本文 §6「我方适配」列）。
2. 不能绕过的，写成「需上游改动」建议：期望能力、Skiff 侧用途、最小接口形状、不可用时的降级。
3. 由项目负责人决策是否等待框架或改用替代方案；D7 定案后不存在单页「暂不迁移」的豁免。
4. 决策结果回写本文 §8，不在代码里留隐式分支。

### 1.4 迁移范围与执行顺序

D7 定案为**一次性切换**：所有内核页面同批交付，不存在「先上一半」的运行时状态。下表只是开发顺序与风险分级，不是分批上线计划。

| 范围 | 开发顺序 | 说明 |
|---|---|---|
| 审计（只读） | 1 | 无写动作，先验信封、权限过滤、分页 |
| 作业中心（只读 + 取消/重试） | 2 | 验证轮询、危险动作、长任务进度 |
| 站点 / 运行环境 / Docker | 3 | 表单字段多，先冻结字段码与错误回填 |
| 设置 / 菜单 | 4 | 依赖菜单壳与 admin Pack（D4） |
| 文件管理 | 5 | 依赖 §6.1 的框架阻塞项全部解除（D6） |
| 插件功能页 | 6 | 随包 schema（`ui/schema/<view>.json`）原生渲染；旧 SSR/iframe 仅作兼容保留，不再作为新入口（§4.4） |

---

## 2. 产物与接入形态

### 2.1 budding 产物事实

| 项 | 事实 |
|---|---|
| 技术栈 | Vue 3.5 + Vuetify 4 + TypeScript；pnpm workspace + Vite 8 |
| 构建 | `pnpm build:package`；要求 Node ≥ 24 |
| 产物 | `budding-<ver>.min.js`（IIFE）+ `budding-<ver>.min.css`，**无内容 hash** |
| 稳定别名 | `budding.min.js` / `budding.css`，**由 `assets.rs` 路由映射，不复制第二份字节** |
| 体积 | 入库 1.6.0 实测：JS 1,243,322 B（gzip 393,123）；CSS 652,578 B（gzip 78,049） |
| 消费方式 | `<script src>` + 挂载点 + `BuddingSDK.embed(target, schema, opts)`；加载后不自动渲染 |
| CSS 加载 | 运行时按 `document.currentScript` 推导目录并插入 `<link>`，**同目录即可** |
| 路由 | 默认 hash；history 模式需 `routerBase` |
| 句柄 | 仅 `{ manifest, unmount }`，**无实例方法**；切路由/切语言/切主题需自定义动作或重新 `embed` |
| 来源 | 上游发版打包后的 JS/CSS 直接复制进本仓库；**本项目不执行打包命令，也不引入 Node 工具链**（D1） |

### 2.2 放置与嵌入

建议目录：`crates/web/static/budding/`，文件只放版本化名；稳定别名由 `assets.rs` 映射到版本化文件（不复制字节）。版本号唯一来源是 `assets.rs::BUDDING_VERSION`，壳页引用与别名映射共用。

- 产物由上游 `pnpm build:package` 生成后直接复制；**本项目不执行该命令，CI 不新增 Node job**（D1）。

- `RustEmbed` 在编译期把目录打进二进制，对外路径 `/{p}/static/budding/<file>`。
- `crates/web/build.rs` 会为每个文件生成 `.gz` 变体，`assets.rs` 按 `Accept-Encoding` 协商；新增产物自动获得 gzip 能力，**不需要改部署脚本**。
- 现有测试 `every_static_asset_has_gzip_variant` 会覆盖新文件，构建期自动校验。
- 体积影响（1.6.0 实测）：budding 三件（JS/CSS/启动脚本）raw 1,899,900 B / gzip 472,906 B；全仓库静态资源 raw 2,517,780 B / gzip 625,915 B。与设计目标「单二进制 ≤ 14MB」的张力见 §7。

### 2.3 壳页形态

D13 已定案（2026-11-12）：采用 **history 路由 + 白名单前缀回落**，地址栏保持 `/{p}/sites/xxx` 形态，不使用 hash。由此固定路由口径：

- 页面仍走 `/{p}/...`：`GET /{p}` 与各资源页返回同一个 budding 壳，登录态由前端调用 `/ui/v1/session` 判定，401 时渲染登录视图。
- 数据走 `/ui/v1/...`（D2），位于入口前缀之外，与 `/api/v1` 并列。
- 插件功能页 `/{p}/plugins/{id}/{view}` 由壳页加载随包 schema（`GET /{p}/plugins/{id}/schema/{view}`）原生渲染；旧 SSR/iframe 工作台只接受显式 `?legacy=1` 的兼容请求（不做 `Sec-Fetch-Dest` 判断：反代可能剥离该头）。功能页与 RPC 同权限（`PluginManage`），viewer 不再能打开页面后处处 403。

两种承载方式：

- **壳 A（过渡）**：内核保留极薄壳页，askama 只输出挂载点与 script/link 标签，不含业务内容；深链回落由该路由处理。
- **壳 B（纯静态）**：壳页作为静态文件放 `static/budding/index.html`；仍需一个内核路由承接前端深链，否则直接访问深链会 404。

共同约束：

- 壳页受会话保护（`/{p}/login` 除外）；未登录的页面请求仍按《URL 契约》跳登录，`/ui/v1/*` 则返回 401 JSON（§5.2）。
- 壳页不得内联脚本（保持全局 CSP 的 `script-src 'self'`）。
- `/{p}/login` 的壳不含任何用户数据；登录所需的 `csrf` 与配置由公开引导端点下发（§5.1）。
- 页面 schema 是前端产物的一部分，写在 JS 内；内核只提供数据与动作（§3）。

### 2.3.1 后台外壳（admin）与菜单

壳页挂载的是一个 `admin` Schema，而不是单页卡片：

```text
{ type: 'admin', props: { title, brandTitle }, content: { actions: [...], body: [{ type: 'outlet' }] } }
```

- **菜单来源唯一**：`GET /ui/v1/menu` 直接投影 `skiff_app::menu::build` 的菜单树（L1/L2 归属、顺序、角色与能力门、用户隐藏偏好与 SSR 导航同源），前端只做「稳定键 → 已迁移路由」的映射，不另造第二套菜单语义。
- **路由表**：菜单树在前端组装成 budding `routes`；分组节点用入口前缀下的保留段（`/{p}/__menu/<key>`）承载层级，子项使用绝对路径 `/{p}/...`。插件菜单项按 `/ui/v1/menu` 下发的 `plugin_id` / `views` 生成 `/plugins/<id>/<view>` 路由；没有 schema 的旧插件仍回落到内核深链。
- **路由基址**：路由 path 已含 `/{p}` 前缀，因此**不设置 `routerBase`**；旧页面 schema 里的 `prefix + '/sites/...'` 绝对跳转可直接复用，不会出现双前缀。
- **按地址栏定位**：每个路由的 `schema` 写成 `skiff:<kind>`，由启动脚本的 `platform.loadJson` 拦截，按当前路径段取数并复用既有页面构造器；插件视图写成 `skiff:plugin-view`，由壳页加载 schema 并完成 `{{data.*}}` / `{{query.*}}` 绑定；未适配页面返回统一占位。
- **顶栏**：当前用户名/角色 + 退出登录（复用 `POST /ui/v1/session/logout` 与同一 CSRF 头）；固定页插件槽位（`[[mount]]`）的 schema 渲染待槽位 API 落地，当前仍不渲染。
- **图标**：1.8.0 起 budding 官方包自带 `Budding Icons` 子集字体，壳页 `<link>` 引入宿主入库的版本化图标 CSS/woff2（§2.4）；1.8.0-r3 子集含 17 个业务菜单语义字形，菜单按语义名映射到 mdi 名。`folder` / `disk` 分别用 `mdi-file-document` / `mdi-archive-outline` 就近兜底；`image` / `hard-drive` 子集缺字形，映射表显式置 `null`（菜单项不渲染图标，不留悬空占位），已登记上游 T19。

### 2.4 缓存与版本（D11 定案）

**定案**：产物按 `budding-<ver>.min.js` / `budding-<ver>.css` 版本化文件名入库，壳页引用再加 `?v=<skiff 版本>`。

| 请求 | 响应头 |
|---|---|
| 版本化文件名（可带 `?v=`） | `Cache-Control: public, max-age=31536000, immutable` |
| 稳定别名 `budding.min.js` / `budding.css` | `Cache-Control: no-cache` |

配套规则：

- 升级时**删除旧产物**，避免二进制随版本累积膨胀；`build.rs` 会为新文件名生成 `.gz` 变体。
- 现有 `.min.` 的 `max-age=86400` 策略已按文件名与查询串区分（`assets.rs`，2026-11-12 落地）：版本化文件名或带非空 `?v=` 的请求返回 `immutable`，稳定别名 `no-cache`，其余维持原值。
- **CSS 必须用版本化真实文件名**：budding 按 `document.currentScript` 推导目录加载 CSS，若其实现丢弃查询串，仅靠脚本上的 `?v=` 会让 CSS 继续命中旧缓存。该行为**待实测**。
- 稳定别名只作兼容入口，壳页不得引用它。
- 独立 Pack（`budding-code-<ver>.min.js` / `budding-terminal-<ver>.min.js`）同样按版本化文件名入库，启动脚本带 `?v=<ver>` 引用，命中 immutable；升级时与主包一起替换。
- **图标资产随包版本化**：`budding-icons-1.8.0-r3.min.css`（4,319 字节，仅把 `@font-face` 的字体名改成版本化文件名）与 `budding-icons-1.8.0-r3.woff2`（3,704 字节）取自官方包，由壳页 `<link>` 引入，带 `?v=1.8.0-r3` 时返回 `immutable`。`Budding Icons` 子集共 75 个字形（CSS `::before` 75 条），已包含侧栏 17 个业务语义图标，宿主不再另存完整 @mdi/font。woff2 本身已压缩，`build.rs` 跳过它的 `.gz` 生成（再压只会增加字节）；体积按原始字节计，不存在「gzip 后很小」的收益。

### 2.5 CSP 影响（D12 定案：保持现状，不预防性放宽）

沿用全局 CSP（`middleware.rs`），保持严格：

```text
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data:; connect-src 'self'; frame-ancestors 'none';
base-uri 'none'; form-action 'self'; object-src 'none'
```

只在浏览器实测确认缺失时按最小集补，且必须写明触发页面与原因：

| 实测现象 | 最小补充 |
|---|---|
| 字体被拦（Vuetify 字体以 data URI 内联） | `font-src 'self' data:` |
| 确认使用 Worker | `worker-src 'self' blob:` |
| 确认使用 WASM | `script-src 'self' 'wasm-unsafe-eval'` |

约束：

- 不因「框架可能用到」预先放开任何指令。
- CSS 由运行时插同源 `<link>`，`style-src 'self'` 可满足；Vuetify 内联样式依赖现有 `'unsafe-inline'`。
- `connect-src 'self'` 已覆盖同源 JSON 与轮询；`ws(s)://同源` 的实际浏览器行为**待实测**。
- 插件 iframe 的独立 CSP（`PLUGIN_UI_CSP`）与沙箱属性**不得改动**；父页放宽不会传递到子页，也不得为此修改子页策略。
- 回归测试（2026-11-12）：`middleware::tests::global_csp_directives_are_not_loosened` 逐条冻结全局指令集，`handlers::plugins::tests::plugin_ui_csp_is_frozen` 与 `plugin_iframe_sandbox_is_frozen` 冻结插件侧 CSP 与沙箱属性。

### 2.6 离线与部署前提

- 部署机、测试机**不得**执行 `pnpm install` 或访问 CDN；产物必须随仓库/二进制分发。
- **本项目不引入 Node 工具链**：产物由上游打包，本仓库只接收结果；CI 不新增 Node job（D1）。
- budding 的 MCP 属上游仓库的开发期工具，需要完整上游仓库与 `pnpm install + gen:catalog`；本项目不依赖它，也不在 CI 中调用。

---

## 3. JSON 契约

### 3.1 命名空间与路径

**D2 定案**：新开命名空间，路径 `/ui/v1`，位于入口前缀之外，与 `/api/v1` 并列。

| 约束 | 口径 |
|---|---|
| 形状 | `/ui/v1/{资源}`；动作在末段，如 `/ui/v1/sites/{domain}/config/save` |
| 方法 | 只用 `GET`（读）与 `POST`（写/动作），**禁用 PUT/PATCH/DELETE** |
| 鉴权 | 复用会话 Cookie + CSRF；`AuthUser` 对 `/ui/v1/*` 必须返回 401 JSON，不得再 303 到登录页 |
| 入口豁免 | `entry_guard` 需把 `/ui/v1/` 与 `/api/v1/` 并列放行；`/ui/*` 的其它形态仍 404 |
| 版本 | 版本在路径（`v1`），载荷内不再重复 `apiVersion` 作为路由依据；`apiVersion` 仅作兼容提示 |
| 响应 | 统一 JSON 信封（§3.2）；不返回 HTML、重定向或页面片段 |
| 资源命名 | 沿用资源契约的复数、小写、中划线；不使用 `api` 中缀 |

需要同步到《URL 契约》的条款草案见 §4.5。

### 3.2 信封结构

统一信封，与现有 `crates/web/src/error.rs::ErrorEnvelope` 尽量同形，只允许**新增字段**：

```json
{
  "data": {},
  "actions": [],
  "notices": [],
  "error": { "code": "shape.field", "message": "域名不合法", "field": "domain", "retryable": false }
}
```

| 字段 | 语义 | 约束 |
|---|---|---|
| `data` | 资源或列表 | 列表为 `{ "items": [], "next_cursor": null, "total": null }` |
| `actions` | 当前主体可执行的动作 | 只给可见动作；写动作必须带 `method` 与 `path` |
| `notices` | 本次响应附带的提示 | 不持久化；替代 flash |
| `error` | 失败载荷 | 仅在失败时出现；`code` 为稳定机器码，`field` 仅字段类错误 |

约定：

- `error.status` 不重复放进载荷，由 HTTP 状态表达；现有 HTML 侧 `ErrorEnvelope` 的 `status` 字段保留给 `/api/v1`，新命名空间可不带。
- 不允许出现 `redirect`、`html`、`layout`、`component` 字段。
- budding 的错误对象自带 `path`（schema 路径），与 Skiff 的 `field`（业务字段码）是两回事，不得混用。
- **文案双字段（D8）**：状态、错误码、提示、空态原因一律出 `{code,label}` 或 `{code,message}`；前端有字典用 `code`，否则回落到中文 `label`/`message`。
- 页面标题、按钮、字段标签、帮助文字属结构文案，写在前端 schema，**不进 DTO**。
- Vuetify 内部英文（分页、表格空文案、日期控件）只能由上游 `createVuetify()` 注入 locale 解决；标注为**上游依赖，不阻塞内部推进**。

### 3.3 字段码

- 字段码沿用领域错误携带的结构化字段，例如站点表单的 `SiteField`：`domain` / `aliases` / `root_dir` / `runtime_id` / `proxy_target`。
- 规则：**字段码 = 前端表单节点 id（或 `props.name`）**；内核与页面 schema 各自维护一张映射表，发布前做一致性测试。
- 未登记字段码回退为顶部提示，不猜测控件。
- 字段码表冻结后只增不改；改名视为破坏性变更。

### 3.4 时间（D9 定案）

- 所有 `*_at` 输出**单一 RFC3339 UTC 字符串**，例如 `2026-11-12T06:30:00Z`。
- 时长仍出秒数，不转成字符串。
- **不出双字段**（不并行输出 epoch 与字符串）。
- 数据库 epoch 秒不变，只在 `/ui/v1` 出口转换。
- 前端用 `Intl.DateTimeFormat` 按浏览器时区本地化；审计、任务、作业、站点证书等只读列表一律同一规则，内核不再拼展示字符串。
- 现存散落的 `%Y-%m-%d %H:%M` 只保留在旧页，随一次性切换删除。

### 3.5 分页与游标（D10 定案）

| 列表 | 排序 / 游标 | 说明 |
|---|---|---|
| 审计 | `(ts, id)` keyset | 现有 `idx_audit_ts` 需补 `id` 成复合索引；`ts` 为秒级，必须用 `id` 破平 |
| 作业 | `(created_at, id)` keyset；状态筛选走 `(status, created_at, id)` | 现有 `idx_jobs_created` / `idx_jobs_status` 需补 `id` |
| 文件 | `(is_dir, name)` | 文件系统列表，不走 SQLite；排序键必须进游标 |
| 任务 / 站点 / 插件 / 运行环境 | 全量，上限 500 | 量小，暂不引入游标 |

通用规则：

- `limit` 默认 50、上限 200；返回 `next_cursor`；`total` 默认不返回。
- 游标必须携带**排序键与筛选指纹**；筛选或排序变化即视为新查询，旧游标失效。
- 排序字段白名单，禁止透传任意列。
- 非法游标返回 400；禁止 `offset` 与 `cursor` 混用。

### 3.6 动作元数据

动作 id 沿用 `crates/app/src/actions.rs` 的注册表，格式 `<resource>.<verb>`，同时作为审计 `action` 的来源。

| 元数据 | 用途 |
|---|---|
| `id` | 全局唯一、稳定 |
| `permission` | 所需 `skiff_model::Permission`；前端只用于显隐 |
| `dangerous` | 前端必须走 `confirm` 动作或等价确认 |
| `async` | 返回作业引用，需轮询进度 |
| `idempotent` | 允许重复提交的语义 |
| `method` / `path` | 传输层调用方式，指向 `/ui/v1/...`；仍受 URL 契约约束 |
| `label` | 动作显示文案（中文兜底，D8） |
| `label_key` | 前端字典键（D8）；缺失时回落到 `label` |
| `input` | 参数 schema（类型、必填、范围、枚举） |

动作**永不返回页面、重定向或提示文案**；成功结果放 `data`，提示放 `notices`。

### 3.7 版本轴

budding 的 JSON **没有 schema 版本字段**，版本轴是 `catalogVersion` + `apiVersion`，字段只增不改。

Skiff 侧对应约束：

- 资源字段名冻结，只增不改；删除或改名必须升 API 版本。
- `apiVersion` 与 `/ui/v1` 的版本号保持一致，由内核在会话端点下发，前端只做兼容判断。
- 不依赖 budding 的 `catalogVersion` 做业务分支；它只表示前端组件目录版本。

---

## 4. 与现有契约的关系

### 4.1 URL 契约（继续有效）

| 条款 | 状态 | 说明 |
|---|---|---|
| 安全入口前缀 `/{p}/...` | 继续有效 | 壳页、静态资源、JSON 都必须带前缀；根路径旁路继续 404 |
| 禁用 PUT/PATCH/DELETE | 继续有效 | 动作一律 POST + 末段动词 |
| 动词白名单 | 继续有效 | 新增动作先登记 |
| GET 只读、POST 写入 | 继续有效 | 轮询与下载用 GET |
| 旧路径 301/308/303 | 部分调整 | 切换后不保留旧页；`301`/`308` 仅用于入口前缀迁移，`303` 不再用于 JSON 动作 |
| 查询参数只读、写参数进体 | 继续有效 | 分页新增 `limit` / `cursor`，先登记再实现 |
| 页面与页面内接口同构 | 部分调整 | 页面在 `/{p}/...`，JSON 统一在 `/ui/v1/...`（D2）；两者的资源归属靠路径映射表对齐，不再要求同前缀 |

### 4.2 UI 契约（部分继续有效）

| 条款 | 状态 | 补条款 |
|---|---|---|
| 每页一个主操作 | 继续有效 | 由页面 schema 固定，动作清单只提供可用性 |
| 危险操作二次确认 | 继续有效 | 内核只给 `dangerous`；前端用 `confirm` 动作或等价弹窗，文案归前端 |
| 空态说明原因与下一步 | 继续有效 | 内核需在 `data` 里给「为什么空」的结构化原因（如 `empty_reason`），不能只给空数组 |
| flash 不进 URL | 继续有效 | 替换为 `notices`；内核 `FlashKind` 需扩展为 `success`/`warning`/`error`/`info` 四级 |
| 工具栏/表格/窄屏 | 继续有效 | 由前端组件承担，内核不感知 |

### 4.3 菜单契约（继续有效）

- L1 固定 6 项（`overview`/`sites`/`docker`/`plugins`/`tools`/`settings`）不变；L2/L3 的声明、稳定键、角色/gate、偏好持久化仍归内核。
- L3 标签栏**仍由内核决定数据**（菜单声明 + 当前视图），只是渲染从 askama 换到 budding。
- budding 默认包**不含** `admin` 组（`admin` / `nav-menu` / `page-tabs`），`chart` 也不在默认包；清单与上游文档存在漂移（§7.1）。菜单壳由上游 admin Pack 提供（§6.3 R-1）。
- 菜单偏好（顺序/隐藏/固定/布局）继续存内核，不迁到 `localStorage` 作为唯一来源。

### 4.4 插件 UI 契约（随包 Schema 原生渲染）

- 插件功能页以**随签名包分发的静态 schema**（`ui/schema/<view>.json`）交付：manifest 的 `[[menu.items]]` / 单视图 `[[menu]]` / `[[mount]]` 用 `schema` 字段声明，`api = "1.3"` 起生效；安装期由 `skiff-pkgfmt` 完成静态校验。
- 校验边界（宿主侧自担，不依赖上游）：节点类型白名单、禁止外链资源与外部 URL、禁止 `api`/`url`/`html` 等绕过字段、动作白名单（`skiffPluginRpc` / `skiffPluginView` / 表单 `submit`）、节点数/深度/字节数上限。schema 本身没有网络能力。
- 数据与写操作：壳页按 schema 的 `data.<name>.method` 调 `POST /{p}/plugins/{id}/rpc`；写操作只能经 `skiffPluginRpc`，沿用 CSRF 头、权限矩阵与逐次 `plugin.rpc` 审计。schema 不允许直接写 URL。RPC 成功返回 `{ result }`，失败返回统一 JSON 错误信封（`{ error: { code, message } }`），壳页直接展示 `error.message`；旧 iframe 桥先按其 JSON 解析、再回退 HTML 文本，行为不变。
- 绑定语法：`{{data.<path>}}` 取已取回的 RPC 数据，`{{query.<key>}}` 取地址栏参数，`when` 支持 `{{data.x}}` / `!{{data.x}}`；budding 自己的 `${row.field}` 行内插值保持原样。
- 权限口径：`GET /{p}/plugins/{id}/{view}`、`GET /{p}/plugins/{id}/schema/{view}` 与 RPC 一致，均要求 `PluginManage`；旧 SSR 工作台（`/{p}/m/plugin/...`）同权限。**权限不足返回 403，不伪装成 404**（不隐藏资源存在性），因此 viewer 从旧用例的 404 变为 403，这是口径修正而非路由消失。
- 旧 SSR/iframe 不删除：`/plugins/{id}/ui/{path}`、工作台模板与 `?legacy=1` 分支继续可用，但面板内不再有入口。
- 一次性 nonce、`skiff.rpc` / `skiff.theme` / `skiff.height` / `skiff.open`、`PLUGIN_UI_CSP` 只作用于旧 SSR 页，保持不变。
- 槽位（`[[mount]]`）已支持 `schema` 声明与安装期校验；新壳仍没有通用槽位渲染 API。`overview.health` 由内核改为「取数 + 原生渲染」：`GET /ui/v1/overview` 返回启用插件的 `health_plugin`，壳页对其调 `snapshot` RPC 后用卡片渲染 CPU / 内存 / 负载 / 运行时长，失败降级为说明性 `alert`；不是 iframe，也不是通用槽位渲染器。
- 组件缺失时不自造渲染：例如文件管理需要的树/编辑器组件仍缺。站点文件插件（`com.skiff.files`）只原生化「站点目录浏览 + 文本编辑/保存」，全盘管理、回收站、配额、上传仍只在内核「工具 → 文件管理」，不做第二套文件管理器。
- 已原生化的只读批次：guard（containers / events）、monitor（overview / history / process / network）。monitor 的进程与网络页把当前快照一次取回（RPC `all: true`，进程仍受 512 PID、连接仍受 5000 条上限），排序 / 表头筛选 / 分页交给 budding 表格本地完成；展示文案由插件派生，schema 只做绑定。两页都没有写入口。
- 已原生化的写批次：firewall（rules / fail2ban）。表单内按钮的 `skiffPluginRpc` 会合并表单值（空值不发送），仍走 `POST /plugins/{id}/rpc`，CSRF、`PluginManage`、逐条审计不变；行内删除 / 解封用 `${row}` 插值。`skiffPluginView.query` 只接受标量，由壳页拼到同一插件页的 query，用于「查看某 jail 的封禁」这类只读下钻；schema 仍不能写 URL 或直接 `fetch`。数值字段由插件归一化（字符串 → 整数），内核参数校验不放宽。
- ftp（accounts / runtime）同样原生：账号新建 / 重置密码改为页内表单（schema 没有弹窗与二次确认，页面已明确提示）；`status` / `sites` / `accounts` / `protection` 的展示文案与下拉选项由插件派生。防火墙插件缺失时 `protection` 返回原因、`protection.banned` 返回空列表，FTP 页面不因可选依赖而整页失败。
- cron（list）原生：列表 + 新建/编辑共用一个 schema，`?id=` 走 `detail` 预填；`params_json` 文本域由插件解析为对象；执行/启停/删除/创建/更新仍走统一 RPC。budding 的 `when` 不随表单值重算，原生表单只收 5 段 cron，预设编辑器保留在旧界面 `?legacy=1`；内置与其它插件任务只读，编辑/删除由后端拒绝。该联动能力已登记为上游 T17。
- files（main）原生：站点级文件浏览 + 文本编辑。`list` 把当前目录拆成 `dirs` / `files` 两表（`rowActions` 不能按行隐藏，避免对目录显示「编辑」）；文件正文经 `read_text` 返回 `text_safe`（把 `${` 转义为 `${'${'}`，避免 budding 模板求值改写正文，见上游 T18），保存仍提交原文并走 `write` RPC。超过 256 KiB 或非 UTF-8 的文件只读展示并提示改用内核文件管理；语法高亮、上传、新建、重命名、删除、回收站与配额不在插件页复制。默认入口是原生页，旧完整工作台只保留显式 `?legacy=1`，菜单与列表不再链接。

### 4.5 需要补的条款（新增）

1. JSON 命名空间与版本位置：`/ui/v1` 写入 URL 契约 §2 例外表与 §9 路由清单。
2. 动作成功不再 303，改为状态码 + `notices`；旧页兼容期仅在切换前存在，D7 定案后不保留双栈。
3. `/ui/v1/*` 的前缀豁免、401 语义与鉴权口径（与 `/api/v1` 并列）。
4. 字段码表与表单节点 id 的映射规则。
5. 列表 `limit`/`cursor` 参数登记。
6. 空态原因字段（`empty_reason`）的语义与枚举。
7. 前端深链回落的 404 边界（D13 已定案，见 §4.5.1）。
8. 静态产物版本化与缓存策略（D11）。

#### 4.5.1 已同步到 `URL_CONTRACT.md` 的条款（2026-11-12）

以下条款已写入 [URL_CONTRACT.md](URL_CONTRACT.md) §2、§2.1、§6、§9；本节只保留结论摘要，不再作为待办。

- **§2 例外表**：`/ui/v1/...` 无入口前缀，会话 Cookie + CSRF，未认证 401，仅 `handlers::ui`（按资源拆分），只返 JSON。
- **§2 列表**：`/ui/v1/` 与 `/api/v1/` 并列；`/ui/` 下其它形态一律 404，不得作为静态资源或页面旁路。
- **§2.1（新增）SPA 壳页回落白名单**：仅 `GET` + `Accept: text/html` + 路径命中已登记页面前缀时返回壳页；白名单之外仍 404；`POST` 等写请求永不回落；白名单变更必须同步契约与测试。
- **§6 方法与副作用**：`/ui/v1/*` 写操作一律 POST + CSRF；`GET /ui/v1/bootstrap` 与 `POST /ui/v1/session` 为未认证端点（登录用签名 CSRF token + 来源判定）；`session/logout`、`session/password`、`session/totp`、`session/totp/enable` 为认证类例外，与现有页面路由一一对应。
- **§9 路由清单**：登记 `/ui/v1` 与壳页回落两类。

D13 结论：地址栏保持 `/{p}/sites/xxx` 形态，history 路由 + 白名单前缀回落，不使用 hash；白名单内容以 `URL_CONTRACT.md` §2.1 为唯一来源。

---

## 5. 登录、权限、审计、CSRF 的桥接

### 5.1 登录页与 CSRF（D3 定案：登录页也只用 budding）

**已实施（2026-11-12，第三批）**

- `GET /{p}/login` 返回 budding 登录壳（`id="skiff-login"` 挂载点），未认证直接 200，不经过 SPA 回落的匿名 303 分支；已登录访问仍 303 到 `/{p}`。壳只引用版本化 `budding-<ver>.min.js`、`budding-<ver>.min.css` 与 `budding-login.js`，不含内联脚本。
- 页面启动后调用 `GET /ui/v1/auth/login`（未认证、禁止缓存）：返回 `{ data: { csrf_token, entry_prefix, api_version } }`，并下发 `skiff_login` Cookie 作为兼容信号。
- 提交走 `POST /ui/v1/auth/login`（未认证）：JSON 体 `{ username, password, otp, csrf, next }`，CSRF 放 JSON 体 `csrf` 字段（与表单路径同一个签名 token，前端不额外造头）。
- CSRF：沿用现有签名 token（`skiff_identity::signed_token`，TTL 600 秒，密钥取持久化 `security.csrf_secret`）；**签名 token 仍是唯一权威**，Cookie 只在浏览器完全没有来源头时作兼容。失败沿用现有 `code` 字段：`auth.csrf_invalid`（伪造/过期，文案区分「已过期」与「校验失败」）、`auth.unauthenticated`（用户名/口令或动态码错误）。
- 来源判定：仍在内核 handler 读 `Sec-Fetch-Site` / `Origin` / `Referer`；跨站返回 403 与现有文案「跨站登录请求被拒绝」，budding 不参与判定。
- 限流：沿用 `skiff_app::auth::login` 的失败计数与并发闸门；超限返回 503（现有 `dependency.unavailable`）与「登录失败次数过多，请 N 秒后重试」。
- TOTP：现有实现是**单步表单**，没有第二步分支；`otp` 字段原样保留，错误文案仍为「用户名、口令或动态码不正确」，不用新建错误码。
- 失败呈现：服务端 JSON `error.message` 由启动脚本写入 `sessionStorage` 并重载登录页，页面内 budding `alert` 展示后即清除；错误文案不写 URL（避免可构造的伪造提示），用户名保留、口令清空——与旧页 POST 后重渲染的观感一致。
- 会话 Cookie：登录成功响应带 `{ data: { redirect_to, username, role } }` 与 `Set-Cookie`；Cookie 形态完全复用 `skiff_identity::session_cookie`（`HttpOnly; SameSite=Lax`，`Secure` 由可信代理解析后的 scheme 决定）。
- 跳转：`redirect_to` 由内核按与表单登录同一函数计算（首登强制改密 → `/{p}/settings?force=1`；站内 `next` 深链原样；否则 `/{p}`），前端只做 `location.assign`。
- 登出：`POST /ui/v1/session/logout`（会话 + CSRF 头），由内核清 Cookie。

**仍待实施 / 与早期设计稿的差异**

- 早期设计稿的 `GET /ui/v1/bootstrap`、`POST /ui/v1/session`、`x-csrf-token` 头与 `auth.csrf_expired` / `auth.cross_site` / `auth.rate_limited` / `auth.totp_*` 细分码**未采用**：当前实现复用既有 `auth.csrf_invalid` / `auth.unauthenticated` / `dependency.unavailable` 与现有中文文案，避免在登录链路上新造第二套错误码。后续如确需机器可判别 TOTP 缺省/错误，再单独提案。
- 登录成功响应暂不返回 `permissions/acl`（当前 `/{p}/login` 成功后由服务端页面加载，壳页会话接口 `GET /ui/v1/session` 提供角色与 CSRF）；接入第一张真实业务页时再按 §5.3 补 ACL 快照。
- **口令输入（已原生）**：budding 1.8.0 重打包起 `input-text` 透传原生 `type`（含 `password`）；登录页与设置页直接声明 `type: 'password'`，首帧即掩码，不再有渲染后改 DOM 的临时桥接。`autocomplete` 尚未被上游透传（schema 已声明，DOM 属性为空），待补。
- 旧 askama 登录模板与 `POST /{p}/login` 表单 handler 暂保留：新页通过测试前不删，且它仍是「前端产物加载失败」时唯一可用的兜底提交路径（无 JS 客户端可直达）。

### 5.2 401 / 403

- `/ui/v1/*` 必须被 `AuthUser` 视为机器接口：未登录返回 401 JSON，**不得再 303 到登录页**；`entry_guard` 需与 `/api/v1` 并列豁免。
- 401：`code = unauthenticated`；前端 `platform.request` 适配器统一跳转 `/{p}/login?next=...`。
- 403：`code = forbidden`，可选 `required_permission`；前端展示 `outlet.content.forbidden` 或页面内提示。
- **budding 没有 401 全局拦截**，且注入 `platform.request` 后 `apiAllowlist` 失效；适配器必须自行实现「仅允许同源相对路径 + `/ui/v1` 前缀」的白名单，否则等于放开任意地址。
- 403 的现有语义可复用：路由 `permission` 不满足时菜单剔除 + 出口 403 占位且不拉 schema。
- 会话空闲超时（`verify_session` 返回空）按 401 处理；数据库不可用（`Unavailable`）按 503 处理，二者不得塌成同一个登录页。

### 5.3 ACL 快照与重新 embed

budding 的 `acl` 是快照，权限变化必须重新 `embed`。交接路径：登录成功响应带 `acl`，前端保存并在每次 `embed` 时传入；后续以 `/ui/v1/session` 的返回为准。

| 时机 | 动作 |
|---|---|
| 登录成功 / 登出 | 重建或销毁实例；登出后清空本地 acl |
| 收到 403 且 `required_permission` 与本地 acl 不一致 | 重新拉会话；仍 403 则展示无权限 |
| 会话空闲超时或指纹失效 | 跳登录，清空 acl |
| 菜单或权限配置被管理员修改 | 下次导航时刷新会话；当前页提示「刷新以应用」 |
| 入口前缀变更 | 内核 302 到新前缀；前端按新前缀重建并重新拉会话 |
| 前端组件目录升级（产物替换） | 整体重新 `embed`，不做局部热替换 |

前端 `permission` 只决定显隐；**任何写动作都必须由内核再次判定**，越权动作内核必须拒绝并留审计。

### 5.4 审计

- 业务审计保持在命令层（`crates/app/src/commands/`）；Web 层只负责授权入口、错误映射与响应。
- 拒绝路径审计留在传输层：CSRF 失败、未认证、无权限、入口前缀 404、panic。
- budding 的 `platform.report` 是**错误遥测**，`platform.request` 是可注入的请求钩子；两者**都不得作为业务审计来源**，最多作为补充日志。

### 5.5 表单字段错误回填

- budding 提交失败只 `report` + toast + 跑 `onError` 动作链，**不会自动回填字段错误**。
- 约束：内核失败返回 400 + `error.field`；前端用自定义动作把错误写入 `ctx.extras.form.errors[fieldCode]`。
- 字段码必须与节点 id 一致（§3.3），否则错误落不到控件。

### 5.6 长任务、流式与终端

| 能力 | 现状 | budding 接入方式 |
|---|---|---|
| 作业进度 | 作业中心 `progress` + SSE 日志 | 用现有轮询端点 + `source.autoRefresh`；SSE 暂不接 |
| 运行环境安装进度 | `GET /runtimes/jobs` 轮询 | 直接映射为轮询 |
| 容器日志 | 原 SSE 流 | 已改为尾部快照 + 刷新（`GET /ui/v1/containers/{id}/logs`，§5.20）；SSE 仅旧 SSR 详情页跟随 |
| 终端 | 票据 + WS | 已落地：`terminal` Pack 内联 + 既有 exec 票据/WS（§5.19）；WS 真连待 Linux 测试机验收 |
| 文件上传 | 分片 `offset` 协议 | budding `file` 只支持单次 FormData POST；见 §6 |

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

- `GET /{p}/sites`、`/{p}/sites/{domain}`、`/{p}/sites/new`、`/{p}/runtimes`、`/{p}/runtimes/{id}` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/sites/list[/{domain}]`、`GET /{p}/m/core/sites/env[/{id}]`。站点写操作已原生化，运行环境的安装/配置/扩展仍待后续批次。
- 列表复用 `list_sites` / `list_runtimes` / `runtime_views` 与页面 `site_view` / `runtime_row` 展示模型，**没有新增 SQL**；`q` 关键字过滤与旧列表页同源（域名 / 目录 / 接入方式 / 别名）。
- `GET /ui/v1/runtimes` 与旧运行环境页同权限：`require_admin`，viewer 返回 403（`auth.forbidden`）。
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

- `GET /{p}/containers`、`/{p}/containers/{id}` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/docker/containers[/{id}]`（菜单深链新增详情段）。
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

- `GET /{p}/images`、`/{p}/networks`、`/{p}/volumes` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/docker/{images|networks|volumes}`。
- 复用 `list_images` / `list_networks` / `list_volumes` 与旧页面的 `ResourceRow` / `NetworkRowView` 展示模型，不新增 SQL。
- 写操作已原生化：`POST /ui/v1/images/pull`、`POST /ui/v1/images/{id}/remove`、`POST /ui/v1/networks`、`POST /ui/v1/networks/{id}/remove`、`POST /ui/v1/volumes`、`POST /ui/v1/volumes/{name}/remove`；逐项复用 `ImagePull` / `ImageRemove` / `NetworkCreate` / `NetworkRemove` / `VolumeCreate` / `VolumeRemove` 策略与 `docker_cmd` 审计。
- 网络名/卷名的字符集判定抽到 `resources::validate_name`，旧表单与 `/ui/v1` 共用一份。

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

- `GET /{p}/compose` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/docker/compose`。
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

### 5.12 任务与作业（第五批，只读）

- `GET /{p}/tasks`、`/{p}/tasks/{id}`、`/{p}/jobs`、`/{p}/jobs/{id}` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/tasks[/{id}]`、`/{p}/m/core/jobs[/{id}]`。
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

- `GET /{p}/audit` 交付 budding 壳页，页面为原生只读审计列表；旧 SSR 保留在 `GET /{p}/m/core/settings/audit`（最近 200 条）作无 JS 兜底。
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

- `GET /{p}/settings` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/settings/security`，但不再是产品入口。
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

- `GET /{p}/settings/menu`、`GET /{p}/settings/docker` 交付 budding 壳页；旧 SSR 分别保留在 `GET /{p}/m/core/settings/menu`、`GET /{p}/m/core/docker/settings`（菜单深链，无 JS 兜底）。
- 菜单导航的 href 改指向规范路径 `/{p}/settings/menu`、`/{p}/settings/docker`，不再以 `/m/core/...` 作为产品入口；`/{p}/docker/settings` 的 308 兼容不变。
- `GET /ui/v1/settings/menu` → `{layout, width, max_width, groups[{key,label,items[{key,label,href,hidden,pinned,pinnable,plugin,icon}]}]}`；读对登录用户开放（偏好按账号隔离）。页面用「布局按钮 + 内容宽度表单 + 分组表格」，行内「↑ / ↓ / 显隐 / 固定」，底部「恢复默认」（确认后 `action=reset`）。
- `GET /ui/v1/settings/docker` → `{registry_mirror, image_prefix, can_admin}`；读对登录用户开放，`can_admin=false` 时页面只读提示。写操作仅 Admin，失败按 JSON 信封返回（不 PRG），成功刷新页面。

**运行环境详情（第二十三批）**

- `/runtimes` 列表新增「安装运行环境」表单（类型来自 `GET /ui/v1/runtimes/catalog`，版本可留空）与「镜像参数」表单（`GET|POST /ui/v1/runtimes/images`）。
- `/runtimes/{id}` 改为真实原生页（原占位与旧版链接删除）：`GET /ui/v1/runtimes/{id}` 一次返回运行环境行、PHP 参数、Node 配置、端口、扩展清单、配置文件列表、当前选中的配置文件内容（`?file=key`）、绑定站点与 `config_mounts_stale`。
- 写操作：生命周期按钮（start/stop/restart/rebuild/sync/delete）走 `POST /ui/v1/runtimes/{id}/action/{action}`，有绑定站点时前端先确认；`POST .../probe` 只回显实测版本；PHP 参数、Node 配置、端口、配置文件、PECL、克隆、解绑各有独立端点，全部复用 `runtime_cmd` 与既有动作权限。
- 扩展表行内「安装 / 卸载」调用 `POST /ui/v1/runtimes/{id}/extensions/{key}/{op}`；批量端点 `POST .../extensions/batch` 已登记，当前页面按单项操作。
- 详情聚合仍是 Admin-only（与旧页 `require_admin` 一致）；viewer 直接请求 403，菜单不可见。

**Web 服务页（第二十四批）**

- `GET /{p}/sites/web` 改交付 budding 壳页（`require_admin`，viewer 403）；旧 SSR 保留在 `GET /{p}/m/core/sites/web`，菜单 href 已改指规范路径。
- `GET /ui/v1/web` → `{status, install, config{main_path,main_content,include_path,include_content,sites[],params}, modules, can_admin}`，与旧页同一数据源（`nginx_status`、`site_install_progress`、`nginx_config_view`、`nginx_modules`）。
- `GET /ui/v1/web/logs?load=&kind=&lines=&level=&status=&q=&offset=`：未 `load` 时不触碰文件，返回读取范围、状态码分布、截断标记与行数据；前端用「加载日志」把筛选条件带回 URL（`?logs=1&...`）再读取。
- 写操作：`POST /ui/v1/web/config`（主配置，`nginx -t` + reload）与 `POST /ui/v1/web/params`（性能参数）；SSR 表单与 JSON 共用 `nginx::apply_main` / `apply_params`，审计事件 `nginx.config.save` / `nginx.params.save` 不变。失败返回 400 并保留错误文本，不伪装成功。

### 5.15 商店（第八批）

- `GET /{p}/store` 交付 budding 壳页；旧 SSR（目录状态、四类条目、升级包下载、表单确认页）保留在 `GET /{p}/m/core/plugins/store`，仅作无 JS 兜底，产品页面不再链接它。
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

- `GET /{p}/plugins`、`GET /{p}/plugins/{id}` 交付 budding 壳页；旧 SSR 列表保留在 `/{p}/m/core/plugins/{installed|discover|updates}`，但不再从新页面链出。
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
- 旧 SSR（浏览 / 回收站 / 配额）保留在 `GET /{p}/m/core/files[/trash|/quotas]`，但不再是入口。
- 回收站与配额已原生化（第二十一批）：`GET /{p}/files/trash`、`/{p}/files/quotas` 交付壳页，入口门禁仍是 `FILE_TRASH_READ` / `FILE_QUOTA_READ`；数据与写操作**直接复用**既有 `GET /files/trash/list`、`POST /files/trash/{restore,purge,clear}`、`GET /files/quotas/list`、`POST /files/quotas/{set,cancel,scan}`（`x-csrf-token`、审计、`commands::file` 不变），未新增 `/ui/v1` 写入口。
- 回收站表（路径 / 大小 / 删除时间 / 剩余天数）行内「恢复 / 彻底删除」，顶部「清空回收站 / 刷新」；恢复冲突（目标已存在）二次确认 `overwrite`。配额页顶部「设置配额」表单（路径 + 容量/文件数上限，至少一项），配额表（路径 / 已用 / 上限 / 统计）行内「重新统计 / 取消配额」。`GET /ui/v1/files` 只新增 `can_admin`（只增不改），`can_admin` 时才显示两个入口。

### 5.18 代码查看器与日志查看器（第十三批）

- 代码查看器：文件管理里点文本文件进入 `GET /{p}/files/view?path=`（budding 隐藏路由），数据来自 `GET /ui/v1/files/read?path=`（`FILE_READ`，仅 Admin）。响应 `{path,text,size,truncated,max_edit_bytes}`；渲染用 `code` 组件（1.8.0 起渲染器类型名为 `code`，旧 `code-viewer` 已不再注册；文本字段为 `modelValue`，旧 `code` 字段组件不读取；只读、行号、复制、固定高 560），语言按扩展名映射；疑似二进制（含 NUL）只提示不渲染。纯前端高亮，不引 WASM。
- 代码 Pack：入库 `budding-code-1.8.0-r3.min.js`（84,813 B，取自 `dist/packs/code.mjs`，自包含 ESM），启动脚本在 `embed` 前用 `BuddingSDK.registerPack('code', …?v=<ver>)` 注册；渲染器类型名同为 `code`，文本字段为 `modelValue`。未注册或加载失败时 `code` 组件退化为纯文本。CSP 仍是 `script-src 'self'`，动态 import 同源可用。
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
- Pack：入库 `budding-terminal-1.8.0-r3.min.js`（392,211 B，取自 `dist/packs/terminal.mjs`，自带 xterm 与样式注入，无裸依赖），`BuddingSDK.registerPack('terminal', …?v=<ver>)`。
- **验证边界**：本机（Windows、无 Docker）只验证了路由、壳页与取票失败时的错误页；WS 真连与容器内 `/bin/sh` 交互必须在 Linux 测试机验收，未验前不得记为已完成。

### 5.20 日志入口收口（第十六批）

- 容器日志：`GET /{p}/containers/{id}/logs` 从旧 SSR SSE 页改为 budding 日志页（尾部快照 + 刷新，不做实时流）；SSE 流只保留在 `GET /{p}/containers/{id}/logs/stream`，供旧 SSR 详情页 `data-src` 跟随。新增 `GET /ui/v1/containers/{id}/logs?tail=`（策略同旧 SSE：`Verb::ContainerLogs` → `DockerRead`；`tail` 默认 500、上限 2000），容器详情新增「查看日志」入口，旧版链接收窄为「资源限制编辑」。
- 运行环境日志：新增 `GET /{p}/runtimes/{id}/logs` budding 日志页与 `GET /ui/v1/runtimes/{id}/logs?tail=`（Admin，复用 `AppState::runtime_logs`）；旧详情抽屉的快照接口移到 `GET /{p}/runtimes/{id}/logs.json`（`logs_url` 已同步），让 `/logs` 可以交壳页；运行环境列表行可点进详情占位页，占位页提供「查看运行日志」与「旧版配置 / 安装 / 扩展」两个明确入口。
- 前端统一用 `code` 组件（`language: 'plaintext'`、固定高度、行号、可复制），刷新走整页重载；不新增 SSE/WS 契约，也不在前端造分页或流式缓冲。
- `?legacy=1` 现状：仅插件 iframe 工作台与站点日志旧 SSR 两处保留（前者是插件嵌入兼容，后者是无 JS 兜底）；本轮未新增 legacy 分支。
- 验证：`main_flow` 覆盖「页面交壳 + JSON 未认证 401 / 无 Docker 503 / viewer 403 / 实例不存在 400」；无头浏览器实测三个页面（容器日志错误页、运行环境日志错误页、运行环境详情占位页）均无 iframe、无脚本错误。

---

## 6. 能力缺口与建议

判定列：**我方适配** = 只用现有能力即可；**需上游改动** = 必须框架侧补，本仓库不得修改 budding；**需负责人决策** = 有替代路径，但取舍明显。
阻塞等级：**B1** = 不补则该页无法交付；**B2** = 不补只能降级交付且需负责人接受；**B3** = 体验问题，可排期。

### 6.1 文件管理页阻塞项（D6：只用 budding 重做）

D6 定案后，原先「可绕」的缺口升级为阻塞项。以下全部属于框架侧，统一标注：**需上游决策，本仓库不得修改 budding**。

| # | 缺口 | 文件管理页为什么需要 | budding 当前能力边界（实测事实） | 建议上游补什么 | 阻塞等级 |
|---|---|---|---|---|---|
| F1 | 分片上传与续传 | 大文件、弱网、上传中断后不重传；内核现有上传是 `offset` 分片协议 | `file` 组件本身仍是单次 FormData POST；**1.8.0 起宿主可实现 `PlatformAdapter.upload`**，由宿主切片后逐片调用既有分片接口（第十一批已落地，续传待人机交互另定） | 组件内建 `chunkSize` / 断点续传（可选，非阻塞） | 已绕 |
| F2 | 上传进度与取消 | 大文件需要可见进度并可取消，否则用户只能等待超时 | 1.8.0 上传增强：`uploadProgressTarget` 写回 `{status,loaded,total,percent,attempt}`，`cancelUpload` / `retryUpload` 指令；宿主 `PlatformAdapter.upload` 按字节上报 | 无（已具备） | 已解决 |
| F3 | 目录树 | 文件管理的主导航是目录树 + 懒加载 + 面包屑联动 | 无独立 `tree` / `tree-select`；basic 16 / layout 11 / data 8 中无树组件 | 产品口径改为**平铺 + 面包屑**，负责人明确禁止树形（第十一批） | 已降级关闭 |
| F4 | 日志 / 代码查看器 | 文本编辑需要行号与语法基础；站点、Nginx、作业日志需要大文件查看 | **已解决**：1.8.0 提供 `code` 组件（旧名 `code-viewer`，行号、折行、复制、固定高度）+ `@budding/code` Pack（1.8.0-r3 起，含 `scrollToLine` 与语言/主题清单）；代码查看器与站点/作业日志查看器已落地，纯前端高亮不引 WASM | 编辑模式（当前只读） | 已关闭（只读口径） |
| F5 | 只读详情（descriptions） | 文件属性、权限、大小、时间等只读信息面板 | 无 descriptions，需用 basic 组件逐个拼 | `descriptions` 组件 | B3 |

F3 的降级说明：可退化为「列表 + 面包屑」导航，但会丢失目录全貌，是否接受由负责人确认；F4 的降级说明：小文件可用基础排版，万行级日志不可用。

### 6.2 其他缺口

| # | 缺口 | Skiff 用途 | 我方适配（可绕） | 需上游补 | 判定 |
|---|---|---|---|---|---|
| G4 | 无 SSE / WS | 作业日志、容器日志、终端 | 用 `progress` + `source.autoRefresh` 轮询 | SSE / WS 通道 | 需上游改动（如需页内实时日志） |
| G6 | 无通知中心 | 概览待办、审计提醒 | `alert` + `toast` + 菜单 `badge` | 通知中心 | 我方适配 |
| G7 | 组件默认文案为 `.vue` 内中文字面量；Vuetify `createVuetify()` 无参 → 内部英文 | 全站中文 | 逐节点覆盖 `props`；内核词典 `registerMessages()` 只覆盖 15 个键 | 完整 i18n 注入 + Vuetify locale | 需负责人决策（D8） |
| G8 | 主题不切 Vuetify 暗色、不设 `--v-theme-primary` | 现有深色主题、品牌色 | 我方 CSS 覆盖层；或 budding 页暂不做暗色 | 主题适配 Vuetify token | 需负责人决策（D15） |
| G9 | 默认包不含 `admin` / `nav-menu` / `page-tabs` | 三级菜单壳、页签栏 | 应急：basic + layout 自建（仅为临时验证，不作为方案） | 发行包随附 admin Pack | **外部阻塞（D4）** |
| G10 | 默认包不含 `chart` | 概览暂无图表 | 暂不需要 | 随包提供 chart | 暂不处理（D16） |
| G11 | 句柄无实例方法 | 切路由/切语言/切主题 | 重新 `embed`（成本待实测）；或用自定义动作驱动 | 实例方法 | 我方适配 + 需实测 |
| G12 | 无 401 全局拦截；注入 `platform.request` 后 `apiAllowlist` 失效 | 会话过期处理 | 适配器内自建同源白名单 + 401 跳转 | 内置 401 钩子 | 我方适配 |
| G13 | 表单字段错误不自动回填 | 站点/运行环境表单 | 自定义动作写 `ctx.extras.form.errors` | 内置 `error.field` 回填 | 我方适配 |
| G14 | JSON 无 schema 版本字段 | 契约演进 | 用 `apiVersion` + 路径版本；字段只增不改 | schema 版本字段 | 我方适配 |
| G15 | 产物无内容 hash | 升级缓存 | 版本化文件名 + `?v=` | 产物 hash 命名 | 我方适配（D11） |
| G16 | `input-text` 不支持 `type` / `autocomplete` | 登录口令框必须掩码 | **type 已解决**：1.8.0 重打包起 `input-text` 透传 `type`（含 `password`），登录页与设置页已用原生声明，DOM 桥接与 MutationObserver 全部删除；`autocomplete` 仍被忽略（DOM 属性为空） | `input-text` 透传 `autocomplete` | type 已关闭；autocomplete 待上游 |

### 6.3 对外需求条目（提交给上游，不由本仓库实现）

> 以下条目是外部依赖；本仓库只跟踪状态，**不修改 budding**。

#### R-1 随发行包提供 admin Pack（D4）

| 项 | 内容 |
|---|---|
| 组件 | `admin`、`nav-menu`、`page-tabs` |
| 用途 | 三级菜单壳（L1 六项 + L2 分组）、工作台标签栏（L3）、当前项高亮与 badge |
| 验收 | 随默认发行包或官方 Pack 交付；组件名与 props 稳定；不依赖外部资源；CSP `script-src 'self'` 下可用；与同一版本号发布 |
| chart | **本次不要求**：Skiff 当前无图表页；若上游同包交付不反对，但不作为验收条件 |
| 状态 | **外部阻塞**：未交付前菜单壳不能进入正式实现 |
| 应急 | 用 basic + layout 自建临时壳，仅用于验证数据契约，不进入交付；Pack 到位后删除 |

#### R-2 文件管理所需能力（D6）

| 项 | 内容 |
|---|---|
| 分片上传 + 续传 | 支持 chunk/offset、并发、失败重试、断点续传 |
| 上传进度 + 取消 | 进度事件与取消入口 |
| 目录树 | 懒加载、面包屑联动、多选 |
| 日志 / 代码查看器 | 行号、搜索、虚拟滚动；大文件可用 |
| descriptions | 只读详情面板 |
| 状态 | B1/B2 阻塞项；不补齐则文件管理页无法按 D6 交付 |
| 约束 | **需上游决策，本仓库不得修改 budding** |

---

## 7. 未决项与风险

### 7.1 分发与法律风险（最高优先级）

| 风险 | 事实 | 影响 | 缓解 |
|---|---|---|---|
| 无 LICENSE 文件（D5 定案按 MIT） | 仓库无 LICENSE，全部 `private: true` | 「按 MIT」目前只是我方口径，缺少上游书面依据；对外分发存在合规风险 | 要求上游补 LICENSE 或出具书面确认；未补齐前只作内部预览，不对外承诺 MIT |
| 明示内部开发 | README 声明契约可能随时调整、勿用于生产 | 用户侧稳定性与支持承诺 | 锁定版本 + 产物入库 + 升级需回归；对用户明示「预览」 |
| 破坏性变更无废弃期 | 上游存在未给废弃期的 breaking change | 升级即破坏 | 只接受经过我方验收的版本；固定版本号，不跟踪主干 |
| 无 CHANGELOG | 无法评估变更影响 | 升级评审困难 | 每版产物附我方验收记录；以 diff + 测试为准 |

### 7.2 定案带来的新增风险

| 风险 | 事实 | 影响 | 缓解 |
|---|---|---|---|
| admin Pack 交付时间不可控 | D4 要求上游随发行包提供 `admin` / `nav-menu` / `page-tabs`；当前发行包不含，文档与产物存在漂移 | 菜单壳与页签栏无法进入正式实现，阻塞一次性切换 | 作为外部需求单跟踪；应急自建壳只用于验证，不进入交付 |
| MIT 目前只是我方口径 | budding 仓库无 LICENSE 文件，D5 定案「按 MIT」缺少上游书面依据 | 对外分发与再分发的合法性不足 | 要求上游补 LICENSE；同时核对 Vuetify 等依赖的许可证 |
| 一次性切换无回滚缓冲 | D7 定案不留双栈、无运行时开关 | 切换后出现问题时无法在运行时切回旧 UI | 切换前打 tag / 记录 commit 与构建号；回滚 = git 版本回退 + 重新部署 + 全量回归；数据库变更需单独评估兼容性 |

### 7.3 工程风险

| 风险 | 事实 | 缓解 |
|---|---|---|
| CI 无 Node | 现有 12 个 job 均无 `setup-node`（口径：`.github/workflows/ci.yml` 的 job 定义，2026-11-12） | D1 定案后不需要 Node，产物入库即可 |
| 构建闸门不覆盖 Node | `Global\SkiffBuild` 只管 cargo | D1 后不引入 Node；未来若引入，必须接入统一入口并重新评估内存准入 |
| 文档漂移 | docs 与 dist 清单称有 admin，1.6.0 bundle 内 0 命中 | 以产物实测为准；每次升级做组件清单核对 |
| 体积 | JS+CSS 原始约 1.83 MiB，gzip 约 0.45 MiB | 纳入二进制预算实测（§2.2） |
| 双栈维护 | 开发期模板与 JSON 会短期并存 | 一次性切换（D7）：发布前删除旧模板与旧路由，不保留运行时开关 |

### 7.4 待实测清单

1. 预编译 IIFE 在 `script-src 'self'` 下是否触发 `eval`/`new Function`。
2. `document.currentScript` 推导目录在 `/{p}/static/budding/` 下是否正确加载 CSS（含 `?v=` 查询串）。
3. history 模式 `routerBase` 与 `/{p}/` 前缀、深链回落的组合行为。
4. `platform.request` 适配器注入后，同源白名单是否可靠拦截跨源。
5. 重新 `embed` 的实际成本（内存、路由状态、滚动位置）。
6. 大日志（万行级）在无专用查看器时的渲染性能。
7. 暗色下 Vuetify 组件的对比度与品牌色表现。
8. 轮询替代 SSE 时的最小间隔与后端压力。
9. 二进制体积与空闲 RSS 的实际增量。

---

## 8. 决策记录

### 8.1 已定案（2026-11-12）

> 原选项与利弊只在表后备查一句，不再展开。

| ID | 定案结论 | 硬约束 | 生效后果 |
|---|---|---|---|
| D1 | budding 发版打包后的 JS 直接放进本项目使用；**本项目不引入 Node 工具链** | 产物入库；不执行 `pnpm build:package`；CI 不新增 Node job；升级靠替换产物文件 | 离线可构建、CI 零改动；产物与上游源码可能漂移，升级需人工核对并回归 |
| D2 | JSON 新开命名空间，路径 `/ui/v1` | 位于入口前缀之外，与 `/api/v1` 并列；`entry_guard` 豁免；`AuthUser` 对该前缀返回 401 而非 303 | 所有 JSON 端点在此命名空间下；URL 契约需补例外条款（§4.5.1） |
| D3 | 登录页也只用 budding | 保留签名 CSRF token、来源判定、限流、TOTP、`HttpOnly; SameSite=Lax` 与 `Secure` 判定；新增公开引导端点 | 不再有 SSR 登录页；`/{p}/login` 返回壳；失败用稳定错误码 |
| D4 | 要求上游随发行包提供 admin Pack | 需要 `admin` / `nav-menu` / `page-tabs`；外部阻塞，不由本仓库解决 | 未交付前菜单壳与页签栏无法进入正式实现；应急自建壳不进入交付 |
| D5 | 按 MIT | 需上游补 LICENSE 或书面确认；需核对 Vuetify 等依赖许可证 | 未补齐前只作内部预览，不对外承诺 MIT |
| D6 | 文件管理页也只用 budding 重做 | 分片上传/进度、目录树、日志查看器、descriptions 由上游补齐（§6.1） | 文件页交付取决于上游；不允许用「保留旧页」兜底 |
| D7 | 一次性切换，不留双栈 | 所有内核页同批交付；无运行时开关；旧 askama 页面统一删除 | 回滚只能 git 回退 + 重新部署；切换前必须完成 §8.3 验收清单 |
| D8 | 文案 key + 中文兜底，范围收窄到状态、错误码、提示、空态原因；结构文案归前端 schema | 后端出 `{code,label}` / `{code,message}`；DTO 不承载页面标题、按钮、字段标签、帮助文字；Vuetify locale 依赖上游 | 中英混排只剩 Vuetify 内部控件；标注上游依赖，不阻塞内部推进 |
| D9 | 所有 `*_at` 单一 RFC3339 UTC；时长出秒数；不出双字段 | 数据库 epoch 秒不变，只在 `/ui/v1` 出口转换；前端用 `Intl.DateTimeFormat` 本地化 | 旧页的 `%Y-%m-%d %H:%M` 随一次性切换删除 |
| D10 | 审计 `(ts,id)`、作业 `(created_at,id)`、文件 `(is_dir,name)` keyset；任务/站点/插件/运行环境全量上限 500 | `limit` 默认 50、上限 200；游标携带排序键与筛选指纹；非法游标 400；需补复合索引 | 列表改为「加载更多」；索引需迁移 |
| D11 | 产物按 `budding-<ver>.min.js` / `budding-<ver>.css` 版本化入库，引用加 `?v=`；版本化请求 immutable，稳定别名 no-cache | 升级删除旧产物；CSS 必须用版本化真实文件名；`.gz` 由现有 `build.rs` 生成 | 需改 `assets.rs` 缓存策略；CSS 查询串行为待实测 |
| D12 | CSP 保持现状，不预防性放宽 | 只在实测缺失时按最小集补 `font-src 'self' data:` / `worker-src 'self' blob:` / `'wasm-unsafe-eval'`；插件 iframe CSP 与沙箱属性不得改动 | 需要一次逐页控制台巡检；不放宽可能暴露上游缺陷 |
| D13 | history 路由 + 白名单前缀回落，地址栏保持 `/{p}/sites/xxx`，不用 hash | 仅 `GET` + `Accept: text/html` + 命中 URL 契约 §2.1 白名单时返回壳页；白名单外仍 404；写请求永不回落；白名单变更同步契约与测试 | 前端未匹配路由自行渲染 404；白名单需随页面增删维护 |

备查：D1 原选项为产物入库/新 crate/构建期 Node 生成；D2 为新命名空间/复用 `/api/v1`/同构内容协商；D3 为保留 SSR/重建/双轨；D4 为 basic 自建/自建 Pack/上游提供；D5 为书面授权/私库/暂不分发；D6 为保留旧页/重做/分阶段；D7 为逐页开关/一次性/长期双栈。

### 8.2 仍待定

P1（切换前）已清空：D13 已于 2026-11-12 定案，见 §8.1。

#### P2：体验与长期

| ID | 决策点 | 选项 | 利 | 弊 |
|---|---|---|---|---|
| D14 | 插件页 | A 继续 iframe；B 插件改出 SDUI JSON；C 双轨 | A 与现契约一致、零风险；B 体验统一；C 渐进 | A 技术栈不统一；B 需改插件生态与沙箱模型；C 长期负担 |
| D15 | 主题与品牌色 | A 我方 CSS 覆盖；B 要求框架适配 Vuetify token；C budding 页不做暗色 | A 立即可用；B 根治；C 最省事 | A 与上游样式耦合、升级易坏；B 依赖上游；C 功能回退 |
| D16 | chart 组件 | A 暂不需要；B 自建 chart Pack | A 零成本；B 为后续图表页铺路 | A 后续图表页受限；B 维护成本 |

### 8.3 一次性切换验收清单（D7）

切换前每一项都必须有判定证据；数量口径均标注来源，不沿用旧文档的估计值。

| # | 对齐项 | 判定方式 |
|---|---|---|
| 1 | 路由 → JSON 端点映射 | 口径：`crates/web/src/lib.rs` 现有 **167 条** `.route(` 注册（2026-11-12 计数）。逐条登记去向：GET 页面 → 壳或 JSON 读端点，POST 动作 → `/ui/v1` 动作或删除；不允许存在未登记路由 |
| 2 | 必须保留的非 JSON 通道 | GET 文件下载（`Content-Disposition`）、插件 iframe HTML 与 `ui/*` 静态、插件 RPC、旧 SSR 容器日志 SSE（budding 页走尾快照）、终端票据 + WS。每条有契约测试，且 CSP/沙箱策略未放宽 |
| 3 | 权限与审计覆盖 | 动作注册表矩阵逐项测试；每个写动作产生审计；CSRF 失败、401、403、限流四类拒绝路径均有审计或兜底日志 |
| 4 | 错误与空态 | 每个资源具备稳定错误码、字段码与 `empty_reason`；列表空态能说明「为什么空 + 下一步」 |
| 5 | 菜单与页签 | L1 固定 6 项、L2/L3 来自内核 JSON；admin Pack 已到位；菜单偏好与固定槽位行为一致 |
| 6 | 静态资源与缓存 | 产物引用带版本；升级前后实测浏览器不加载旧 bundle；`.gz` 变体存在 |
| 7 | CSP | 响应头断言：无 `unsafe-eval`；`script-src 'self'`；`connect-src 'self'` 覆盖轮询；控制台无 CSP 违规 |
| 8 | 登录链路 | 登录、登出、口令修改、TOTP 开启/校验、限流、跨站拒绝逐用例回归；401/403/503 三者可区分 |
| 9 | 体积与运行开销 | 实测二进制增量与空闲 RSS，记录数值；不预设阈值，超过设计预算再决策 |

### 8.4 定案落地位置

| 定案 | 生效位置 | 落地时机 |
|---|---|---|
| D8 文案双字段 | `/ui/v1` 的 DTO、`actions`、`notices`、`error`；页面结构文案在前端 schema | 新建 JSON 端点时生效；旧页不动 |
| D9 RFC3339 | `crates/app/src/time_fmt.rs::epoch_to_rfc3339_utc`；`/ui/v1` 所有 `*_at` 出口 | 工具已落地（2026-11-12，含 0/负数/闰秒/跨年/超范围单测）；JSON 出口在新建端点时接入，旧页不动 |
| D10 分页契约 | `crates/app/src/cursor.rs`（HMAC 签名 + 筛选指纹）；`/ui/v1` 列表参数、400 错误码 | 游标编解码与篡改单测已落地（2026-11-12）；列表端点接入时生效，现有 handler 未改写 |
| D10 复合索引 | `store` 的 schema 迁移（审计、作业） | 已落地（2026-11-12，schema v19）：`(ts DESC, id DESC)`、`(created_at DESC, id DESC)`、`(status, created_at DESC, id DESC)`；迁移测试 `v18_database_upgrades_keyset_indexes_and_keeps_rows` |
| D11 缓存头 | `assets.rs` 缓存策略 + 壳页版本化引用 | 已落地（2026-11-12）：版本化文件名或非空 `?v=` → `immutable`，稳定别名 → `no-cache`；回归 `cache_control_follows_versioning` |
| D12 CSP 回归 | 响应头断言与逐页控制台巡检 | 回归测试已加（2026-11-12，见 §2.5）；逐页控制台巡检待切换前 |
| D13 深链 | `URL_CONTRACT.md` §2.1 白名单 + `Router::fallback` 回落（`crates/web/src/handlers/spa.rs`） | 契约已同步（2026-11-12）；壳页回落、`/ui/v1/session` 与 401 语义已落地（2026-11-12）；前端未匹配路由渲染 404 |

补充：D8 中「消除 Vuetify 内部英文」属上游 `createVuetify()` 注入 locale，本仓库不落地、不阻塞。

---

## 9. 变更记录

| 日期 | 变更 |
|---|---|
| 2026-11-12 | 初稿：budding 只读盘点结论落地为接入规则；覆盖定位边界、产物与离线分发、JSON 信封与动作元数据、既有契约沿用与补条款、登录/权限/审计/CSRF 桥接、能力缺口、风险与待决策项。未实施切换。 |
| 2026-11-12 | P0 七条定案：产物直接入库且不引入 Node、JSON 走 `/ui/v1`、登录页也用 budding、要求上游随包提供 admin Pack、按 MIT、文件管理页用 budding 重做、一次性切换。同步更新 §0/§1.4/§2/§3.1/§4.5/§5/§6/§7/§8，新增 URL 契约补条款草案、登录桥接设计、文件管理阻塞项、admin Pack 对外需求与一次性切换验收清单。未改业务代码。 |
| 2026-11-12 | D8–D12 定案：文案 key+中文兜底且结构文案归前端 schema、时间统一 RFC3339 UTC、审计/作业/文件 keyset 分页与小列表上限 500、静态产物版本化且稳定别名 no-cache、CSP 保持严格并按实测最小补充。同步更新 §0/§2.4/§2.5/§3.2/§3.4/§3.5/§3.6/§8.1/§8.2，新增 §8.4 定案落地位置。D13 仍待定。未改业务代码。 |
| 2026-11-12 | D9–D12 中「不随旧页删除作废」的部分落地：新增 `crates/app/src/time_fmt.rs`（epoch → RFC3339 UTC）与 `crates/app/src/cursor.rs`（不透明签名游标）；schema v19 补审计/作业 keyset 索引；`assets.rs` 区分版本化产物与稳定别名缓存；补全局 CSP、插件 CSP 与 iframe 沙箱回归测试。现有页面与列表查询未改。 |
| 2026-11-12 | D13 定案：history 路由 + 白名单前缀回落，地址栏保持 `/{p}/sites/xxx`，不用 hash。同步更新 §2.3、§4.5.1（转为「已同步」）、§8.1、§8.2、§8.4，并把 §2 例外、SPA 回落白名单、§6 方法例外与 §9 路由清单真正写入 `URL_CONTRACT.md`；同时修正 `SITE_RUNTIME_CONTRACT.md` §9.1 的过期 schema 版本。未改业务代码。 |
| 2026-11-12 | 后端地基落地（第一批）：新增 `/ui/v1` 命名空间（`handlers::ui_api`，`entry_guard` 与 `/api/v1` 并列豁免，`/ui/` 其它形态 404），`UiAuthUser` 使未认证统一 401 JSON；`GET /ui/v1/session` 下发用户/角色/CSRF/入口前缀/能力开关，`POST /ui/v1/session/logout` 复用 `x-csrf-token` 校验；`handlers::spa::fallback` 按 §2.1 白名单做壳页回落（仅 GET + `Accept: text/html`，写请求永不回落；`method_not_allowed_fallback` 覆盖 POST-only 路径上的 GET 深链），壳页只含挂载点、占位提示与 `budding.min.js` 引用。未复制前端产物，未改 `assets.rs` / `build.rs`。 |
| 2026-11-12 | 产物入库与壳页启动（第二批）：`budding-1.6.0.min.js` / `budding-1.6.0.min.css` 入库到 `crates/web/static/budding/`（字节与上游 embed 一致），稳定别名改为 `assets.rs` 映射而非第二份文件；新增同源 `budding-boot.js`（普通 script，无内联）：先取 `/ui/v1/session`，成功 `embed()` 极小 schema（page + 「该功能正在适配中」卡片），失败渲染错误；壳页引用版本化 JS `?v=`，CSS 以 `data-budding-css` 显式传参，`routerMode=history`、`routerBase=/{p}`，不传 packs、不引 loader。真实浏览器验证：登录后深链渲染卡片、`/ui/v1/session` 200、控制台无报错。 |
| 2026-11-12 | 登录页 budding 化（第三批）：`GET /{p}/login` 改为 budding 登录壳（未认证 200，不落入匿名 303；已登录仍回 `/{p}`）；新增 `GET /ui/v1/auth/login`（公开读，签发与表单同一签名 CSRF + `skiff_login` 兼容 Cookie、`no-store`）与 `POST /ui/v1/auth/login`（JSON `{username,password,otp,csrf,next}`，复用 `AppState::login`、`session_cookie` 与登录 CSRF/来源判定）；失败文案沿用现有中文并按既有 `code` 返回，错误经 `sessionStorage` 回显后清除；新增同源 `budding-login.js`。budding 1.6.0 缺口令输入类型，脚本渲染后将口令框 DOM `type` 置为 `password` 作为临时桥接（已标注待上游组件）。旧登录模板与 `POST /{p}/login` 表单路径保留为无 JS 兜底。补 7 条 `main_flow` 用例（壳页未认证 200、配置 200、成功/深链回跳、错口令/空提交、CSRF/跨站、TOTP 分支）。 |
| 2026-11-12 | 概览页 budding 化（第四批，只读）：`GET /{p}` 改交付壳页，启动脚本在入口根取 `GET /ui/v1/overview` 渲染四张计数卡片（容器/网站/插件/运行环境）与 Docker 状态提示；图表位用 `alert` 明确占位（D14–D16 未定），不引图表库、不做写操作/实时刷新。新增 `dashboard::load_overview` 供 SSR 与 JSON 共用同一批查询（无新增聚合 SQL）；`overview` DTO 字段冻结（`collected_at` RFC3339 UTC、`docker.status {code,label}`）。SSR 概览暂留 `GET /{p}/m/overview`，插件概览槽位仍有落点，待迁入 budding 后删除。同步 URL 契约 §9、SDUI §5.7 与上游 T6（`input-text` type/autocomplete）。 |
| 2026-11-13 | 第十五批插件功能页原生化（平台 + guard）：`pkgfmt` 新增 schema 静态校验与 `api = "1.3"` 声明；`GET /ui/v1/menu` 插件项补 `plugin_id` / `default_view` / `views`；壳页生成 `/plugins/<id>/<view>` 路由、加载 schema 并完成 `{{data.*}}` / `{{query.*}}` 绑定，新增 `skiffPluginRpc` / `skiffPluginView` 动作与页内视图按钮行；新增 `GET /{p}/plugins/{id}/schema/{view}`，功能页与 RPC 权限统一为 `PluginManage`；guard containers/events 改用 schema，旧 HTML/SSR 保留。 |
| 2026-11-13 | 后台外壳接入：壳页改为 `admin` Schema + `outlet`，新增 `GET /ui/v1/menu`（投影 `skiff_app::menu::build`，DTO 只含 key/label/icon/href/plugin/empty），前端按菜单键映射已迁移路由并保留 `/{p}/...` history 形态（path 带前缀、不设 `routerBase`）；新增 `skiff:<kind>` 路由 Schema 经 `platform.loadJson` 按地址栏取数，详情页与商店安装确认改走路由。MDI 图标字体（CSS 323,052 B + woff2 403,216 B）版本化入库并由壳页 `<link>` 引入，`build.rs` 跳过已压缩格式的 `.gz`。插件功能页不再作为入口：菜单丢弃 `plugin/*`，任何直接导航 `/plugins/{id}/{view}` 显示「插件功能页正在切换为原生渲染」占位，旧 SSR 工作台只接受显式 `?legacy=1`（§4.4、backlog T7）。 |
| 2026-11-13 | budding 升级 1.8.0：入库 `budding-1.8.0.min.js`（1,260,779 B）/`.min.css`（657,142 B），删除 1.6.0 双份；官方包自带 `Budding Icons` 子集（CSS 3,533 B + woff2 2,456 B），移除宿主自存的完整 @mdi/font（CSS 323,052 B + woff2 403,216 B），壳页改引 `budding-icons-1.8.0.*`。子集不含自定义菜单图标，侧栏不再声明图标，避免缺字形回退成 `mdi-xxx` 文本。新包含 code-view/highlight、virtual-list、增强上传与 terminal Pack，作为后续剩余页面切换的基础。 |
| 2026-11-13 | 第十一批设置页补齐（口令 / TOTP / 公钥原生化）：`/ui/v1` 新增 `POST /settings/password`（argon2 校验 + 会话轮换 + 新 CSRF + 删除首启口令文件）、`POST /settings/totp/setup`、`POST /settings/totp/enable`（一次性备份码）、`POST /settings/keys`、`POST /settings/keys/{id}`（仅 Admin，审计 `settings.key.import` / `settings.key.remove`）；`GET /ui/v1/settings` 的 `account` 补 `totp_pending_secret` / `totp_pending_uri`（未启用暂存态）；前端用 `className: 'skiff-password'` + `MutationObserver` 把口令输入框改为 `type=password`。错误文案沿用旧 SSR。设置页不再依赖旧 SSR，旧页仅作无 JS 兜底。 |
| 2026-11-13 | 第十二批文件管理原生化：`/ui/v1` 新增 `GET /files?path=`（平铺列表 + 父路径 + 根入口 + `upload_chunk_bytes`）、`POST /files/mkdir|rename|remove`（CSRF，删除进回收站）；前端 `virtual-list` 渲染目录、面包屑导航、条目按钮；上传由宿主 `PlatformAdapter.upload` 按 256 KiB 切片复用 `/{p}/files/upload`，并补 `platform.request`（上游仅在提供 `request` 时保留 `upload`）。**不做目录树**，§6.1 F3 按降级口径关闭。终端仍待 WS 契约。补两条 `main_flow` 用例，无头浏览器实测上传与新建目录成功。 |
| 2026-11-13 | budding 重打包基线 `1.8.0-r2`：上游 manifest 仍为 1.8.0（builtAt 2026-11-13T04:13:45Z）但字节已变（JS 1,261,442 B / CSS 657,324 B，仓内 gzip 398,697 / 78,970；上游 manifest 记 400,024 / 79,966，压缩级别不同），本地用 `-r2` 后缀避免同 URL 命中旧 `immutable` 缓存；图标同步为 `budding-icons-1.8.0-r2.min.css`（3,626 B）+ `.woff2`（2,612 B）。`input-text` 起支持原生 `type`：登录页与设置页改为 `type: 'password'`，删除 `maskPassword` / `watchPasswordMask` / `skiff-password` 与 MutationObserver，首帧即掩码（无头浏览器实测 `type=password`）；`autocomplete` 声明已写入 schema，但当前构建仍未透传到 DOM（输入框 `autocomplete` 属性为空），待上游补字段。G16 的 `type` 部分关闭。 |
| 2026-11-13 | 第十三批代码/日志查看器：新增 `GET /ui/v1/files/read?path=`（`FILE_READ`）、`GET /ui/v1/sites/{domain}/logs?part=&tail=&q=`（复用 `AppState::site_logs`）；新增路由 `/{p}/files/view`、`/{p}/sites/{domain}/logs`、`/{p}/jobs/{id}/logs`。渲染统一用 `code-viewer`（只读、行号、定高 560），高亮走入库的 `budding-highlight-1.8.0-r2.min.js`（`BuddingSDK.registerPack`，54,270 B），不引 WASM；站点日志页旧 SSR 仅接受 `?legacy=1`，作业日志按 `after` 游标最多 10 页、不建 SSE。§6.1 F4 关闭（只读口径）。 |
| 2026-11-13 | 第十四批容器终端：新增 `GET /{p}/containers/{id}/terminal`（budding 隐藏路由，容器详情加「打开终端」）。复用既有 `POST …/exec/ticket`（Admin + CSRF + `Verb::Exec`，票据绑定会话与容器、30 秒、一次即废）+ `GET …/exec?ticket=` WS（Origin 严格校验，start/fail/end 写 `Verb::Exec` 审计），无内核改动、不新建 WS 契约。前端用 `terminal` 组件内联，`resizeFormat: 'none'`、`reconnect: false`；Pack 入库 `budding-terminal-1.8.0-r2.min.js`（390,680 B）。**WS 真连待 Linux 测试机验收**。 |
| 2026-11-13 | 第十五批审计页原生化：`GET /{p}/audit` 不再占位。`store` 新增 `audit_page(keyword, before(ts,id), limit)`（`instr(lower(...))` 命中 actor/ip/action/target/detail，排序走既有 `idx_audit_ts(ts DESC, id DESC)`，未改表/加索引）；`/ui/v1/audit?q=&limit=&cursor=` 用 `AuditScope` 签名游标（绑定筛选指纹，换关键字/篡改/旧版本 400），权限保持 `AuditRead`；前端表单 + `table` + 「下一页」。补 store 3 条单测与 `main_flow` 权限/过滤/游标用例，无头浏览器实测页面、过滤与游标翻页。 |
| 2026-11-13 | 第十六批日志入口收口：容器日志改 budding 页（尾部快照），SSE 移至 `/{p}/containers/{id}/logs/stream` 仅旧详情跟随；运行环境日志新增 budding 页（旧抽屉 JSON 移至 `/logs.json`）；两端各加 `/ui/v1` 快照接口（`ContainerLogs`/`DockerRead` 与 Admin），详情占位页提供原生日志入口；`?legacy=1` 未新增。补 2 条 `main_flow` 用例 + 无头浏览器实测。 |
| 2026-11-13 | 第十七批 Docker 写操作原生化：`GET /{p}/containers/new` 改交付壳页原生表单；`/ui/v1` 新增容器新建 `POST /containers`、资源限制 `POST /containers/{id}/limits`、镜像 `POST /images/pull` 与 `/images/{id}/remove`、网络 `POST /networks` 与 `/networks/{id}/remove`、卷 `POST /volumes` 与 `/volumes/{name}/remove`。参数解析复用旧表单函数（`spec_from_form` / `limits_from_form` / `validate_name`），执行仍走 `docker_cmd` 与既有权限/CSRF/审计；容器详情补资源限制表单，三个资源页补创建表单与逐项删除。补 `main_flow` 用例（CSRF / viewer 403 / 无 Docker 失败 / 非法名称 400）。**真实 Docker 引擎上的创建与限制热更新仍需 Linux 测试机验收**。 |
| 2026-11-13 | 第十八批站点写操作原生化：`GET /{p}/sites/new` 改交付壳页原生表单；`/ui/v1` 新增 `POST /sites`（建站）、`/sites/batch`（批量启停）、`/sites/{domain}/{action}`（启停）、`/default`、`/domains`、`/config`、`/ssl`、`/backup`、`/delete`。全部复用 `SITE_*` 动作、`x-csrf-token`、审计与 `commands::site`；详情新增 `config` / `backup` / `can_manage`，列表新增 `can_admin`。站点列表补新建与批量启停，详情补站点操作、配置编辑、域名增删、证书与备份卡片。补 `main_flow` 用例（CSRF / viewer 403 / 确认串与空批量 400 / 详情按角色显隐）。**建站、配置保存与证书动作需 OpenResty/真实运行环境，测试机验收后再算完全闭环**。 |
| 2026-11-13 | 第二十批任务与作业写操作原生化：`/ui/v1` 新增 `POST /tasks/{id}/toggle|run|delete` 与 `POST /jobs/{id}/cancel|retry`，复用 `TASK_*` / `JOB_*` 动作、CSRF 头、审计与既有命令层；任务列表行内与详情补启停/运行/删除，作业详情补取消/重试；作业列表新增 `can_admin`（只增不改）。补 `main_flow` 用例（CSRF / viewer 403 / 无调度器 503 / 作业取消与不可重试 400）。**任务运行与作业取消的真实执行仍需测试机验收**。 |
| 2026-11-13 | 插件功能页原生化（第六批 files）+ 概览健康卡片：`com.skiff.files` 升 `api = "1.3"`，`/{p}/plugins/com.skiff.files/main` 默认入口改由壳页加载 `main.json`（已授权站点目录的平铺浏览 + 文本编辑/保存，保存走 `write` RPC）；旧完整工作台只保留显式 `?legacy=1`，菜单与列表不再链出。全盘管理、回收站、配额、上传仍只在内核「工具 → 文件管理」，不复制第二套语义。`GET /ui/v1/overview` 新增 `health_plugin`（已启用且声明 `overview.health` 槽位的插件 id），壳页对其调 `snapshot` 后用卡片渲染 CPU/内存/负载/运行时长，失败降级为 info `alert`；不新增槽位渲染器、不改上游。文件正文经 `text_safe`（`${` → `${'${'}`）规避模板求值，登记上游 T18。 |
| 2026-11-13 | 测试机四问题修复：`KERNEL_API` 升 `1.3`，任务门与商店安装共用 `skiff_pkgfmt::manifest::api_compatible`（修复 1.3 插件的内置任务被误判 `api_mismatch`）；概览新增 `health_reason`，`health_plugin` 为空或取数失败均渲染「系统状态」原因卡片；`POST /plugins/{id}/rpc` 失败改为 JSON 错误信封，壳页展示可读 `error.message`；`GET /ui/v1/menu` 插件项 `href` 改指原生 `/{p}/plugins/{id}/{view}`。 |
| 2026-11-13 | budding 重打包基线 `1.8.0-r3`：上游 manifest 仍为 1.8.0（builtAt 2026-11-13T12:42:39Z）但字节已变（JS 1,259,883 B / CSS 653,947 B，上游 manifest gzip 399,921 / 79,233），本地用 `-r3` 后缀区分；图标同步为 `budding-icons-1.8.0-r3.min.css`（4,319 B）+ `.woff2`（3,704 B，75 字形）。高亮包升级为 `@budding/code`：入库 `budding-code-1.8.0-r3.min.js`（84,813 B），启动脚本改注册 `code`；terminal 包更新为 392,211 B。上游本版闭环 T6/T8/T9/T10/T14/T15/T16，宿主侧删除对应绕过：侧栏补全 17 个语义图标映射；站点/运行环境/容器/镜像/网络/卷/任务/作业列表改 `source` 游标分页（删「翻页待接入」提示与 `limit=50` 首屏截断）；镜像/网络/卷删除改为行内 `rowActions`（网络按 `can_remove` 显隐）；容器日志去掉「旧版详情页」指向。 |
