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
| 定案状态 | P0 七条（D1–D7）与 D8–D12 已于 2026-11-12 定案（§8.1）：产物直接入库、不引入 Node、JSON 走 `/ui/v1`、登录页也用 budding、要求上游随发行包提供 admin Pack、按 MIT、文件管理页用 budding 重做、一次性切换；文案 key+中文兜底、时间 RFC3339 UTC、审计/作业/文件 keyset 分页、静态产物版本化、CSP 保持严格 |
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
| 产物 | `budding-<ver>.min.js`（IIFE）+ `budding.css`，**无内容 hash** |
| 稳定别名 | `budding.min.js` / `budding.css` + sha256 清单 |
| 体积 | JS 1,221,352 B（gzip 387,557）；CSS 652,578 B（gzip 79,035） |
| 消费方式 | `<script src>` + 挂载点 + `BuddingSDK.embed(target, schema, opts)`；加载后不自动渲染 |
| CSS 加载 | 运行时按 `document.currentScript` 推导目录并插入 `<link>`，**同目录即可** |
| 路由 | 默认 hash；history 模式需 `routerBase` |
| 句柄 | 仅 `{ manifest, unmount }`，**无实例方法**；切路由/切语言/切主题需自定义动作或重新 `embed` |
| 来源 | 上游发版打包后的 JS/CSS 直接复制进本仓库；**本项目不执行打包命令，也不引入 Node 工具链**（D1） |

### 2.2 放置与嵌入

建议目录：`crates/web/static/budding/`，文件为版本化名 + 稳定别名。

- 产物由上游 `pnpm build:package` 生成后直接复制；**本项目不执行该命令，CI 不新增 Node job**（D1）。

- `RustEmbed` 在编译期把目录打进二进制，对外路径 `/{p}/static/budding/<file>`。
- `crates/web/build.rs` 会为每个文件生成 `.gz` 变体，`assets.rs` 按 `Accept-Encoding` 协商；新增产物自动获得 gzip 能力，**不需要改部署脚本**。
- 现有测试 `every_static_asset_has_gzip_variant` 会覆盖新文件，构建期自动校验。
- 体积影响（估算，待实测）：原始约 1.83 MiB + gzip 约 0.45 MiB，二进制预计增加约 2.3 MiB；与设计目标「单二进制 ≤ 14MB」存在张力（§7）。

### 2.3 壳页形态

壳页承载方式仍属 D13（未定案）。定案后的路由口径固定为：

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
- 现有 `.min.` 一律 `max-age=86400` 的策略需要按目录或查询串区分（`assets.rs`）。
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
7. 前端深链回落的 404 边界（D13）。
8. 静态产物版本化与缓存策略（D11）。

#### 4.5.1 待同步到 `URL_CONTRACT.md` 的条款草案

> 本轮只写在这里，**不改 `URL_CONTRACT.md` 本体**；待负责人确认后由文档负责人在主文更新。

§2「前缀与例外」表新增一行：

```text
| 前端 JSON 接口 | `/ui/v1/...` | 无入口前缀；会话 Cookie + CSRF 鉴权；仅 `handlers::ui_api`；响应为 JSON 信封，不返回 HTML；未认证返回 401 |
```

§2 列表补一条：

```text
- `/ui/v1/` 与 `/api/v1/` 并列；`/ui/` 下其它形态一律 404，不得作为静态资源或页面旁路。
```

§9「现有路由清单（按资源）」新增一行：

```text
| 前端 JSON | `GET/POST /ui/v1/...`（按资源在实现前逐条登记；认证、站点、运行时、Docker、文件、任务、作业、插件、设置、审计） |
```

§6「方法与副作用」补充说明：

```text
- `/ui/v1/*` 的写操作一律 POST + CSRF；公开引导 `GET /ui/v1/bootstrap` 与登录 `POST /ui/v1/session` 为未认证端点（登录用签名 CSRF token + 来源判定）；`session/logout`、`session/password`、`session/totp`、`session/totp/enable` 为认证类例外，语义与现有 `/{p}/logout`、`/{p}/password`、`/{p}/totp`、`/{p}/totp/enable` 一一对应。
```

---

## 5. 登录、权限、审计、CSRF 的桥接

### 5.1 登录页与 CSRF（D3 定案：登录页也只用 budding）

**页面与公开引导**

- `GET /{p}/login` 返回 budding 壳，不含任何用户数据，不要求会话。
- 壳启动后调用公开引导端点 `GET /ui/v1/bootstrap`（无会话、无 CSRF）：返回 `{ entry_prefix, version, login_csrf, csrf_ttl_secs }`，并顺带下发 `skiff_login` Cookie 作为兼容信号。
- 引导端点只回登录所需的最小配置，不返回用户名、角色、插件或环境信息。

**登录提交**

- `POST /ui/v1/session`，JSON 体 `{ username, password, otp, next }`，CSRF 放 `x-csrf-token` 头。
- CSRF：沿用现有签名 token（`skiff_identity::signed_token`，TTL 600 秒，密钥取持久化 `security.csrf_secret`）；**表单签名 token 仍是唯一权威**，Cookie 只作兼容。过期返回 `auth.csrf_expired`。
- 来源判定：仍在内核传输层（handler）读 `Sec-Fetch-Site` / `Origin` / `Referer` 并与 Host、`external_url` 比对；budding 不参与该判定。`same-origin` 与 `none` 放行，`cross-site` 与 `same-site` 拒绝并返回 `auth.cross_site`。
- 限流：沿用内核 `skiff_app::auth::login` 的失败计数与并发闸门（按 IP，必要时叠加用户名）；超限返回 `auth.rate_limited` 并带 `retry_after`。现实现按 503 返回，建议改为 429，属实现期确认项。
- TOTP：仍单步提交 `otp`。口令正确但缺 OTP 时返回 `auth.totp_required`，前端切到 OTP 输入后重提；OTP 错误返回 `auth.totp_invalid`。若现有命令层无法区分「口令正确但缺 OTP」与「口令错误」，需新增错误码（内部工作量，不改变校验逻辑）。

**会话 Cookie**

- 保持 `HttpOnly; SameSite=Lax`；`Secure` 由可信代理解析后的 scheme 决定；budding 脚本接触不到会话令牌。
- 登录成功响应：`{ user, role, permissions, acl, csrf_token, api_version, next }`；前端据此 `embed` 并落到 `next`。
- 失败呈现：统一 `{ error: { code, message } }`；锁定/等待提示走 `notices`，不写 URL。
- 登出：`POST /ui/v1/session/logout`（会话 + CSRF），由内核清 Cookie。
- 口令与 TOTP 管理：`POST /ui/v1/session/password`、`/session/totp`、`/session/totp/enable`，与现有 `/{p}/password`、`/{p}/totp`、`/{p}/totp/enable` 一一对应，判定逻辑复用命令层。

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

备查：D1 原选项为产物入库/新 crate/构建期 Node 生成；D2 为新命名空间/复用 `/api/v1`/同构内容协商；D3 为保留 SSR/重建/双轨；D4 为 basic 自建/自建 Pack/上游提供；D5 为书面授权/私库/暂不分发；D6 为保留旧页/重做/分阶段；D7 为逐页开关/一次性/长期双栈。

### 8.2 仍待定

#### P1：切换前（仅剩 D13）

| ID | 决策点 | 选项 | 利 | 弊 |
|---|---|---|---|---|
| D13 | SPA 深链 | A 未匹配路径回落壳页；B 严格 404；C 仅白名单前缀回落 | A 体验好；B 最贴合 URL 契约；C 折中 | A 与「未知路径 404」冲突；B 刷新深链会断；C 需维护白名单 |

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
| D9 RFC3339 | `/ui/v1` 所有 `*_at` 出口 | 新建 JSON 端点时生效；旧页格式化随切换删除 |
| D10 分页契约 | `/ui/v1` 列表参数、游标编码、400 错误码 | 新建列表端点时生效 |
| D10 复合索引 | `store` 的 schema 迁移（审计、作业） | **本仓库现在就能做**，不依赖前端 |
| D11 缓存头 | `assets.rs` 缓存策略 + 壳页版本化引用 | **本仓库现在就能改**；产物入库后生效 |
| D12 CSP 回归 | 响应头断言与逐页控制台巡检 | **本仓库现在就能加测试**；放宽只在实测后 |
| D13 深链 | 壳页路由与 URL 契约例外 | **仍待定**，不在本次落地范围 |

补充：D8 中「消除 Vuetify 内部英文」属上游 `createVuetify()` 注入 locale，本仓库不落地、不阻塞。

---

## 9. 变更记录

| 日期 | 变更 |
|---|---|
| 2026-11-12 | 初稿：budding 只读盘点结论落地为接入规则；覆盖定位边界、产物与离线分发、JSON 信封与动作元数据、既有契约沿用与补条款、登录/权限/审计/CSRF 桥接、能力缺口、风险与待决策项。未实施切换。 |
| 2026-11-12 | P0 七条定案：产物直接入库且不引入 Node、JSON 走 `/ui/v1`、登录页也用 budding、要求上游随包提供 admin Pack、按 MIT、文件管理页用 budding 重做、一次性切换。同步更新 §0/§1.4/§2/§3.1/§4.5/§5/§6/§7/§8，新增 URL 契约补条款草案、登录桥接设计、文件管理阻塞项、admin Pack 对外需求与一次性切换验收清单。未改业务代码。 |
| 2026-11-12 | D8–D12 定案：文案 key+中文兜底且结构文案归前端 schema、时间统一 RFC3339 UTC、审计/作业/文件 keyset 分页与小列表上限 500、静态产物版本化且稳定别名 no-cache、CSP 保持严格并按实测最小补充。同步更新 §0/§2.4/§2.5/§3.2/§3.4/§3.5/§3.6/§8.1/§8.2，新增 §8.4 定案落地位置。D13 仍待定。未改业务代码。 |
