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
| 最大硬缺口 | 文件管理页重做所需的分片上传与续传、上传进度、目录树、日志/代码查看器、只读详情均缺（§6.1） |
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
| 插件功能页 | 不迁移 | 维持 iframe 契约（§4.4），不属于本次切换范围 |

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
- 插件功能页仍走 `/{p}/plugins/{id}/{view}`，iframe 契约不变（§4.4）。

两种承载方式：

- **壳 A（过渡）**：内核保留极薄壳页，askama 只输出挂载点与 script/link 标签，不含业务内容；深链回落由该路由处理。
- **壳 B（纯静态）**：壳页作为静态文件放 `static/budding/index.html`；仍需一个内核路由承接前端深链，否则直接访问深链会 404。

共同约束：

- 壳页受会话保护（`/{p}/login` 除外）；未登录的页面请求仍按《URL 契约》跳登录，`/ui/v1/*` 则返回 401 JSON（§5.2）。
- 壳页不得内联脚本（保持全局 CSP 的 `script-src 'self'`）。
- `/{p}/login` 的壳不含任何用户数据；登录所需的 `csrf` 与配置由公开引导端点下发（§5.1）。
- 页面 schema 是前端产物的一部分，写在 JS 内；内核只提供数据与动作（§3）。

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

### 4.4 插件 UI 契约（继续有效，不改）

- 插件功能页继续 `iframe`，沙箱属性保持 `allow-scripts allow-modals allow-forms`，**不加** `allow-same-origin`。
- 一次性 nonce、`skiff.rpc` / `skiff.theme` / `skiff.height` / `skiff.open`、RPC 逐次审计、`PLUGIN_UI_CSP` 全部不变。
- 内核壳页从 askama 换成 budding 后，iframe 仍是内核 DOM 的一部分，postMessage 桥接代码需要在新壳里等价实现（**待实测**）。
- 插件自身页面不进入 budding schema；是否让插件改出 SDUI JSON 见 **D14**。

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

- **§2 例外表**：`/ui/v1/...` 无入口前缀，会话 Cookie + CSRF，未认证 401，仅 `handlers::ui_api`，只返 JSON。
- **§2 列表**：`/ui/v1/` 与 `/api/v1/` 并列；`/ui/` 下其它形态一律 404，不得作为静态资源或页面旁路。
- **§2.1（新增）SPA 壳页回落白名单**：仅 `GET` + `Accept: text/html` + 路径命中已登记页面前缀时返回壳页；白名单之外仍 404；`POST` 等写请求永不回落；白名单变更必须同步契约与测试。
- **§6 方法与副作用**：`/ui/v1/*` 写操作一律 POST + CSRF；`GET /ui/v1/bootstrap` 与 `POST /ui/v1/session` 为未认证端点（登录用签名 CSRF token + 来源判定）；`session/logout`、`session/password`、`session/totp`、`session/totp/enable` 为认证类例外，与现有页面路由一一对应。
- **§9 路由清单**：登记 `/ui/v1` 与壳页回落两类。

D13 结论：地址栏保持 `/{p}/sites/xxx` 形态，history 路由 + 白名单前缀回落，不使用 hash；白名单内容以 `URL_CONTRACT.md` §2.1 为唯一来源。

---

## 5. 登录、权限、审计、CSRF 的桥接

### 5.1 登录页与 CSRF（D3 定案：登录页也只用 budding）

**已实施（2026-11-12，第三批）**

- `GET /{p}/login` 返回 budding 登录壳（`id="skiff-login"` 挂载点），未认证直接 200，不经过 SPA 回落的匿名 303 分支；已登录访问仍 303 到 `/{p}`。壳只引用版本化 `budding-1.6.0.min.js`、`budding-1.6.0.min.css` 与 `budding-login.js`，不含内联脚本。
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
- **上游组件缺口（临时桥接）**：budding 1.6.0 的 `input-text` 未暴露 `type`，口令框默认会是明文输入。`budding-login.js` 在渲染后把口令框 DOM `type` 改为 `password`（并用 MutationObserver 兜底重渲染），待上游提供 `type` 或独立口令组件后删除该桥接；这属于演示层补齐，不涉及认证逻辑。
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
| 容器日志 | SSE | 轮询降级或等待上游 SSE；D7 后不能用旧页兜底 |
| 终端 | 票据 + WS | **未决**：budding 无 WS；需独立 xterm 页（iframe）或等待上游通道能力，不能用旧 UI 页面兜底 |
| 文件上传 | 分片 `offset` 协议 | budding `file` 只支持单次 FormData POST；见 §6 |

### 5.7 概览页（第四批，只读）

**页面与数据**

- `GET /{p}` 交付 budding 壳（未认证 303 到登录页，已登录渲染概览）；SSR 概览仍保留在 `GET /{p}/m/overview` 菜单深链，直到插件概览槽位迁入 budding。
- 壳启动脚本先取 `GET /ui/v1/session`，路径为入口根时再取 `GET /ui/v1/overview`；其余白名单深链仍渲染「该功能正在适配中」占位。
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

错误码沿用既有信封：未认证 `auth.unauthenticated`（401）、数据库不可用 `dependency.unavailable`（503）。本批没有列表，因此不涉及 `limit`/`cursor`（D10）。

**渲染映射**

- `docker.status.code != available` → 顶部 warning `alert`（标题用 `label`，正文用 `reason`）。
- 四张计数卡片：容器 / 网站 / 插件 / 运行环境；文案由前端 schema 生成，数字来自接口。
- 「趋势图表」卡片内固定 `alert` 占位，禁止在占位里暗示已选定图表库。

---

### 5.8 站点与运行环境（第一批，只读）

**页面与数据**

- `GET /{p}/sites`、`/{p}/sites/{domain}`、`/{p}/runtimes`、`/{p}/runtimes/{id}` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/sites/list[/{domain}]`、`GET /{p}/m/core/sites/env[/{id}]`，写操作仍走旧表单。
- 列表复用 `list_sites` / `list_runtimes` / `runtime_views` 与页面 `site_view` / `runtime_row` 展示模型，**没有新增 SQL**；`q` 关键字过滤与旧列表页同源（域名 / 目录 / 接入方式 / 别名）。
- `GET /ui/v1/runtimes` 与旧运行环境页同权限：`require_admin`，viewer 返回 403（`auth.forbidden`）。
- 本批只读：创建/启停/删除/配置/扩展等写操作未迁移，页面用链接指回旧 SSR 落点。

**字段表（冻结，只增不改）**

`GET /ui/v1/sites?limit=50&cursor=&q=` → `{ "data": { "items": [...], "total": n, "next_cursor": "..."|null } }`：

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

`GET /ui/v1/sites/{domain}` → `{ "data": ... }`：上面全部列表字段 + 下列字段：

| 字段 | 类型 | 说明 |
|---|---|---|
| `ssl` | object | `enabled` / `force_https` / `cert_exists` / `cert_path` / `key_path` / `subject` / `issuer` / `expires` |
| `runtime` | object \| null | 绑定运行环境快照：`id` / `name` / `kind{code,label}` / `version` / `state{code,label}` / `site_count` |
| `runtime_down` | bool | 绑定环境未运行或 socket 缺失 |
| `runtime_alert` | string | 不可用时的中文原因，正常为空串 |

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

错误码沿用既有信封：未认证 `auth.unauthenticated`（401）、viewer 读运行环境 `auth.forbidden`（403）、站点不存在 `not_found`（404）、数据库不可用 `dependency.unavailable`（503）。

**渲染映射**

- 站点列表：`table`（本地 `items`、`pagination:false`），列 = 域名 / 类型 / 接入 / 状态 / HTTPS / 运行环境 / 创建时间；行点击 `navigate` 到 `/{p}/sites/{domain}`。
- 站点详情：运行环境不可用时顶部 warning `alert`；「站点信息」「证书」两张 `card`；末尾链接到旧版详情处理配置/域名/SSL/目录。
- 运行环境列表：`table`，列 = 类型 / 实例 / 版本 / 说明 / 状态 / 站点数 / 接入；安装与配置仍链回旧版页面。
- **D10 翻页限制**：budding `table` 的远程分页发送 `page/perPage`，与内核 keyset 游标不一致；本批只取首页 50 条并在 `next_cursor` 非空时显示提示。翻页组件接入前不做假装分页，记录为待办。

### 5.9 容器（第二批）

**页面与数据**

- `GET /{p}/containers`、`/{p}/containers/{id}` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/docker/containers[/{id}]`（菜单深链新增详情段）。
- 列表复用 `Docker::list_containers` 与 `handlers::container_view`，`filter=error` 与旧页面同一判定（`container_abnormal`），不新增 SQL。
- 详情复用 `resolve_container` + `inspect_container`；读权限仍是 `policy.check(ContainerInspect)`，不新增权限规则。
- 启停/重启/删除：`POST /ui/v1/containers/{id}/{action}`，CSRF 走 `x-csrf-token` 头；策略判定、deny 审计与 `docker_cmd::container_action` 与旧 htmx 路径完全同源。
- 资源限制编辑、日志（T3）、终端（T5）仍留在旧 SSR 详情页，页面明确提示并给出回退链接。

**字段表（冻结，只增不改）**

`GET /ui/v1/containers?limit=50&cursor=&filter=error` → `{ "data": { items, total, next_cursor, filter, docker_available, docker_reason, can_operate, can_delete } }`：

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

**渲染映射**

- 列表：`table`（本地 `items`、`pagination:false`），列 = 名称 / ID / 镜像 / 状态 / 状态描述 / Compose 项目；行点击 `navigate` 到 `/{p}/containers/{id}`。`filter=error` 由 URL 查询参数传入接口。
- 详情：按块拆分「基本信息 / 端口 / 挂载 / 网络 / 资源限制 / 标签」六张 `card`；`can_operate` 时按状态给启动或重启+停止按钮，`can_delete` 时给删除按钮（删除前 `window.confirm`）；动作成功后刷新当前页，删除后回列表。
- **D10 翻页限制**同 5.8：本批只取首页 50 条，`next_cursor` 非空时提示，翻页组件待接入。

### 5.10 镜像 / 网络 / 卷（第三批，只读）

- `GET /{p}/images`、`/{p}/networks`、`/{p}/volumes` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/docker/{images|networks|volumes}`。
- 复用 `list_images` / `list_networks` / `list_volumes` 与旧页面的 `ResourceRow` / `NetworkRowView` 展示模型，不新增 SQL。
- 本批只读：拉取/删除镜像、创建/删除网络与卷仍走旧版页面；壳页给出回退链接与说明，待写操作批次迁移。

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

- 三个页面共用 `table`（本地 `items`、`pagination:false`）：镜像列 = 镜像/大小/提示；网络列 = 名称/驱动/范围/子网/网关/容器/内部外部；卷列 = 名称/驱动与挂载点/提示。
- Docker 不可用时顶部 warning `alert`；`next_cursor` 非空时提示翻页待接入；底部固定提示「写操作仍走旧版页面」并给 `/m/core/docker/*` 链接。

### 5.11 编排 Compose（第四批）

- `GET /{p}/compose` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/docker/compose`，编辑/校验/部署仍走该落点。
- 列表复用 `compose::status_of`（容器标签归组，不持久化 compose 源文件），不新增 SQL；项目按名称排序。
- 项目级启停/重启与 down 已提供 JSON 动作，复用现有 `compose_project_action` / `compose_down` 与逐容器权限判定、审计。

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
| `can_admin` | bool | 是否具备 `DockerLifecycle`（编辑/部署入口由旧页判定） |

`POST /ui/v1/compose/{project}/{action}`（`start` / `stop` / `restart`）→ `{ "data": { project, action, done, failed } }`；`failed` 含策略拒绝与命令失败。

`POST /ui/v1/compose/down`，body `{ "project": "...", "remove_volumes": false }` → `{ "data": { project, remove_volumes, report } }`，`report` 为既有命令返回的逐容器结果。

错误：未认证 `auth.unauthenticated`（401）、CSRF `auth.csrf_invalid`（403）、无编排权限 `auth.forbidden`（403）、未知动作 `shape.invalid`（400）、Docker 不可达 `dependency.unavailable`（503）。

**渲染映射**

- 项目表格：项目 / 归属 / 服务数 / 运行中，行内按钮启动、停止、重启、清理（清理前确认，卷默认保留）。
- 编辑、校验与部署仍链回旧版 `/m/core/docker/compose`；服务日志暂由旧页展示（日志查看器待 T3）。

### 5.12 任务与作业（第五批，只读）

- `GET /{p}/tasks`、`/{p}/tasks/{id}`、`/{p}/jobs`、`/{p}/jobs/{id}` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/tasks[/{id}]`、`/{p}/m/core/jobs[/{id}]`。
- 列表/详情复用 `TaskService::list_views` / `detail_view` 与 `Jobs::list` / `tail` / `detail_of`，不新增 SQL。
- 启停、立即运行、删除、取消/重试与 SSE 实时日志仍在旧版页面；日志查看器待 T3。

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

`GET /ui/v1/jobs?limit=50&cursor=` → `{ "data": { items, total, next_cursor } }`

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

错误：未认证 401、任务/作业不存在 404、任务服务不可用 503。

**渲染映射**

- 任务列表：表格（任务/方式/目标/调度/下次运行/状态/最近结果），行点击进详情；启停/运行/删除链回旧版。
- 任务详情：信息、参数、最近执行表格三块；旧版详情链接保留写操作。
- 作业列表：表格（作业/标题/类型/目标/状态/耗时/提交人/创建时间），行点击进详情。
- 作业详情：元数据、参数、结果、日志四条 `card`；日志按行 `text` 平铺（无代码查看器，T3 待落地），实时流/取消/重试链回旧版。

### 5.13 审计（第六批，阻塞为占位）

- **阻塞原因**：后端只有 `Store::recent_audit(limit)`（最近 N 条），没有关键字过滤与 keyset 游标查询；按「不写新聚合 SQL」的红线，本批不自行补查询。
- `GET /{p}/audit` 交付 budding 占位壳页（「该功能正在适配中」+ 说明），旧 SSR 保留在 `GET /{p}/m/core/settings/audit`（最近 200 条，`AuditRead` 权限不变：viewer 直接访问 403，菜单不可见）。
- **待办（解除阻塞所需）**：审计表增加 `keyword + before(ts,id)` 查询（或等价索引查询），返回 `{items, next_cursor}`；落地后再补 `GET /ui/v1/audit` 与 buds 表格翻页，字段冻结为 `ts`（RFC3339 UTC，D9）/ `actor` / `ip` / `action` / `target` / `result` / `detail`。
- 本批无新增 `/ui/v1` 接口与新增测试用例（仅 `audit_canonical_path_serves_placeholder_shell` 覆盖落点与权限）。

### 5.14 设置（第七批）

- `GET /{p}/settings` 交付 budding 壳页；旧 SSR 保留在 `GET /{p}/m/core/settings/security`（口令、TOTP、信任公钥增删与完整面板表单仍在该页）。
- 写操作只迁移「后台端口 / 安全入口 / TLS」：`POST /ui/v1/settings/panel` 复用 `settings::apply_panel_update`（同一校验、落盘、审计），CSRF 走 `x-csrf-token`，Admin 权限沿用 `Permission::Admin`。
- 口令 / TOTP 表单不在本批迁移范围：强改密期间中间件放行 `/{p}/m/core/settings/*`，改密失败仍 PRG 回旧页并消费一次 flash。
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
| `keys[]` | array | `{key_id, fingerprint_short, label}`；仅管理员返回，viewer 为空 |
| `can_admin` | bool | 是否可编辑面板配置 |
| `flash.notice` / `flash.error` | string | 本次读取消费的会话提示（刷新不重放） |

`POST /ui/v1/settings/panel`，body `{listen_addr, listen_port, entry_prefix, tls_mode, tls_cert, tls_key}`（字符串）→ `{ "data": { "message": "..." } }`；校验失败 400 `shape.invalid`，无权限 403 `auth.forbidden`，CSRF 失败 403 `auth.csrf_invalid`。

### 5.15 商店（第八批）

- `GET /{p}/store` 交付 budding 壳页；旧 SSR（目录状态、四类条目、升级包下载、表单确认页）保留在 `GET /{p}/m/core/plugins/store`。
- `GET /ui/v1/store?refresh=1` 与旧页面共用 `store::load_view`：同一目录快照、同一可安装版本判定与升级提示。
- 安装分两步，均复用旧链路与审计：
  1. `POST /ui/v1/store/plugins/{id}/install` → 调用 `store::prepare`（下载、摘要/大小、验签、版本与降级校验），返回权限确认数据；审计动作与旧表单一致（`store.install`）。
  2. `POST /ui/v1/plugins/confirm` → 调用 `plugins::verify_for_confirm` + `commit_install`（同一信任判定、授权落库、暂存包清理），与旧确认页同一条收尾路径。
- 进度：下载验签是单次同步请求，前端只给「正在下载并验签」占位提示，不引入 SSE/轮询。
- 升级包下载不在本批迁移：仍走旧页 `GET /{p}/store/upgrades/{version}/download`（GET 带副作用，不适合 JSON 动作）。

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

- `GET /{p}/plugins`、`GET /{p}/plugins/{id}` 交付 budding 壳页；旧 SSR 列表保留在 `/{p}/m/core/plugins/{installed|discover|updates}`，信息页新增深链 `/{p}/m/core/plugins/info/{id}`。
- iframe 槽位按现状：`/{p}/plugins/{id}/{view}`、`/{p}/plugins/{id}/ui/{path}` 不动，信息页的 `entry_href` 直接指向内核既有视图路由，不自定形态。
- 启停 / 卸载分别复用 `plugins::apply_toggle`、`plugins::apply_remove`（停进程、审计、任务回收、菜单偏好清理语义不变）。
- 上传安装包（发现页）与 RPC 桥仍走旧链路，不在本批迁移。

**字段表（冻结，只增不改）**

`GET /ui/v1/plugins` → `{ "data": { plugins[], enabled_count, disabled_count, can_admin, allow_unsigned, reduced_allowed } }`

`plugins[]`：`{id, name, version, api, status:{code,label}, enabled, key_id, signed, reduced, running, requires, permissions, grants, ui_path, takedown_note}`

`GET /ui/v1/plugins/{id}` → `{ "data": { plugin, grants[{kind,value}], interop_provides[], interop_consumes[], menus[{label,href,icon,children[{label,href,icon}]}], entry_href, can_admin, allow_unsigned, reduced_allowed } }`

`POST /ui/v1/plugins/{id}/toggle` → `{ "data": { id, enabled } }`

`POST /ui/v1/plugins/{id}/delete` body `{delete_data?}` → `{ "data": { id, redirect_to } }`

错误码：插件不存在 404 `lookup.not_found`；无 `PluginManage` 403 `auth.forbidden`；CSRF 失败 403 `auth.csrf_invalid`。

---

## 6. 能力缺口与建议

判定列：**我方适配** = 只用现有能力即可；**需上游改动** = 必须框架侧补，本仓库不得修改 budding；**需负责人决策** = 有替代路径，但取舍明显。
阻塞等级：**B1** = 不补则该页无法交付；**B2** = 不补只能降级交付且需负责人接受；**B3** = 体验问题，可排期。

### 6.1 文件管理页阻塞项（D6：只用 budding 重做）

D6 定案后，原先「可绕」的缺口升级为阻塞项。以下全部属于框架侧，统一标注：**需上游决策，本仓库不得修改 budding**。

| # | 缺口 | 文件管理页为什么需要 | budding 当前能力边界（实测事实） | 建议上游补什么 | 阻塞等级 |
|---|---|---|---|---|---|
| F1 | 分片上传与续传 | 大文件、弱网、上传中断后不重传；内核现有上传是 `offset` 分片协议 | `file` 组件仅单次 FormData POST，**无分片、无续传**；动作集 19 个不含分片语义 | 上传组件支持 `chunkSize` / `offset` / 并发 / 重试 / 断点续传 | B1 |
| F2 | 上传进度与取消 | 大文件需要可见进度并可取消，否则用户只能等待超时 | 无上传进度事件；`progress` 是任务展示组件，`source.autoRefresh` 是数据轮询，二者都不是上传进度 | 上传进度/取消事件，或允许注入自定义 XHR/fetch 上传适配器 | B1 |
| F3 | 目录树 | 文件管理的主导航是目录树 + 懒加载 + 面包屑联动 | 无独立 `tree` / `tree-select`；basic 16 / layout 11 / data 8 中无树组件 | `tree` + 懒加载 + 多选（拖拽可后置） | B2 |
| F4 | 日志 / 代码查看器 | 文本编辑需要行号与语法基础；站点、Nginx、作业日志需要大文件查看 | 无代码/日志查看器；只有基础排版能力，无虚拟滚动 | 只读查看器：行号、换行、搜索、虚拟滚动；可选编辑模式 | B2 |
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
| G16 | `input-text` 不支持 `type` / `autocomplete` | 登录口令框必须掩码 | 临时桥接：`crates/web/static/budding/budding-login.js` 的 `maskPassword` / `watchPasswordMask` 在渲染后改 DOM `type` 并观察重建 | `input-text` 透传 `type`/`autocomplete`（上游 T6） | 需上游改动；桥接待 T6 落地后删除 |

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
| 2 | 必须保留的非 JSON 通道 | GET 文件下载（`Content-Disposition`）、插件 iframe HTML 与 `ui/*` 静态、插件 RPC、作业/容器日志 SSE、终端票据 + WS。每条有契约测试，且 CSP/沙箱策略未放宽 |
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
