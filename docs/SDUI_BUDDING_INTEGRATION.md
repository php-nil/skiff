# budding 接入规则（SDUI 前端渲染层）

> 状态：**规则与设计稿，未实施**。本文只定义「budding 作为前端渲染层」的接入边界、契约与缺口，不切换现有页面、不改业务代码。
> 上游事实来源：budding 仓库只读盘点（2026-11-12）；内核现状来源：`crates/web`、`crates/plugin` 与本仓库现有契约文档。
> 相关文档：[SDUI_HANDBOOK.md](SDUI_HANDBOOK.md)（框架无关的适配手册）、[URL_CONTRACT.md](URL_CONTRACT.md)、[UI_CONTRACT.md](UI_CONTRACT.md)、[MENU_CONTRACT.md](MENU_CONTRACT.md)、[PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md)。
> 分工：本文只维护 budding 相关的具体事实、契约形状与缺口；框架无关的分层与迁移阶段只在 SDUI_HANDBOOK 维护，本文引用不复述。

---

## 0. 一页结论

| 维度 | 结论 |
|---|---|
| 定位 | budding 只做**表现运行时**；Skiff 后端只出 **JSON 数据与动作结果**，不出布局、不出 HTML 片段、不出跳转 |
| 仓库关系 | **禁止修改 budding 仓库**；产物（JS/CSS）复制进本仓库；缺能力只提建议，由项目负责人决策 |
| 接入前提 | 产物入库 + 单二进制嵌入（`RustEmbed`）；测试机无公网，部署期不得依赖 npm 或 CDN |
| 最大硬缺口 | 无 tree、无日志/代码查看器、无分片上传、无 SSE/WS、无 descriptions、无字段错误回填、默认包缺 admin 壳与 chart |
| 最大分发风险 | budding 无 LICENSE、全 `private: true`、README 明示内部开发且契约可能破坏性变更；**在取得书面授权前不得对外分发产物** |
| 当前建议节奏 | 先定 §8 的 P0 决策，再选一个只读页（审计）做最小验证；文件管理、终端、插件页不进入首批 |

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

### 1.3 缺能力的处理流程

1. 先判断能否用现有组件 + 自定义动作绕过（本文 §6「我方适配」列）。
2. 不能绕过的，写成「需框架改动」建议：期望能力、Skiff 侧用途、最小接口形状、不可用时的降级。
3. 由项目负责人决策是否等待框架、改用替代方案、或该页暂不迁移。
4. 决策结果回写本文 §8，不在代码里留隐式分支。

### 1.4 首批范围

| 范围 | 是否纳入首批 | 理由 |
|---|---|---|
| 审计（只读） | 是 | 无写动作，最适合验证信封、权限过滤、分页 |
| 作业中心（只读 + 取消/重试） | 是 | 验证轮询、危险动作、长任务进度 |
| 站点 / 运行环境 / Docker | 后续 | 表单字段多，字段错误回填需先结构化 |
| 文件管理 | 暂不 | 分片上传、日志查看、终端均缺能力 |
| 插件功能页 | 暂不 | 维持 iframe 契约（§4.4） |

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

### 2.2 放置与嵌入

建议目录：`crates/web/static/budding/`，文件为版本化名 + 稳定别名。

- `RustEmbed` 在编译期把目录打进二进制，对外路径 `/{p}/static/budding/<file>`。
- `crates/web/build.rs` 会为每个文件生成 `.gz` 变体，`assets.rs` 按 `Accept-Encoding` 协商；新增产物自动获得 gzip 能力，**不需要改部署脚本**。
- 现有测试 `every_static_asset_has_gzip_variant` 会覆盖新文件，构建期自动校验。
- 体积影响（估算，待实测）：原始约 1.83 MiB + gzip 约 0.45 MiB，二进制预计增加约 2.3 MiB；与设计目标「单二进制 ≤ 14MB」存在张力（§7）。

### 2.3 壳页形态

budding 需要一个挂载点与脚本标签，形态有两种（决策 **D13**）：

- **形态 A（过渡，推荐先验证）**：内核保留一个极薄壳页 `GET /{p}/ui`（可选 `/{p}/ui/{*rest}`），askama 只输出 `<div id="app">` 与 script/link 标签，不含任何业务内容；深链回落由该路由处理。
- **形态 B（纯静态）**：壳页作为静态文件放 `static/budding/index.html`，页面与资源都走 `/{p}/static/...`；仍需一个内核路由承接前端深链，否则直接访问深链会 404。

两种形态都必须满足：

- 壳页受 `AuthUser` 保护；未登录仍 303 到 `/{p}/login?next=`（或按 D3 的登录形态处理）。
- 壳页不得内联脚本（保持全局 CSP 的 `script-src 'self'`）。
- 页面 schema 是前端产物的一部分，写在 JS 内；内核只提供数据与动作（§3）。

### 2.4 缓存与版本

现状：`assets.rs` 对文件名含 `.min.` 的资源给 `Cache-Control: public, max-age=86400`，其余 `no-cache`。

问题：budding 产物无内容 hash，稳定别名 `budding.min.js` 会被浏览器缓存 1 天，升级后可能继续加载旧 bundle。

可选做法（决策 **D11**）：

1. 只引用版本化文件名（`budding-0.1.0.min.js`）+ 每次升级改模板引用；稳定别名不对外引用。
2. 引用稳定别名并追加 `?v=<skiff 版本>`；查询串不同即不同缓存键，改动最小。
3. 改 `assets.rs` 策略：`/static/budding/` 一律 `no-cache`；代价是每次回源校验。

### 2.5 CSP 影响

沿用全局 CSP（`middleware.rs`），默认严格：

```text
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data:; connect-src 'self'; frame-ancestors 'none';
base-uri 'none'; form-action 'self'; object-src 'none'
```

- budding 是预编译 IIFE，**预计不需要** `unsafe-eval`；若实际触发，视为框架侧问题（决策 **D12**），不默认放宽。
- CSS 由运行时插同源 `<link>`，`style-src 'self'` 可满足；Vuetify 的内联样式属性依赖 `style-src` 的 `'unsafe-inline'`（现有策略已含）。
- 若运行时使用 Worker / `blob:` / WASM，需要补 `worker-src blob:` 或 `wasm-unsafe-eval`；**待实测**，不在接入前预先放开。
- `connect-src 'self'` 足以支撑同源 JSON 与轮询；SSE/WS 若接入需 `connect-src` 覆盖同源（`'self'` 已含 `ws(s)://同源`，待实测浏览器行为）。

### 2.6 离线与部署前提

- 部署机、测试机**不得**执行 `pnpm install` 或访问 CDN；产物必须随仓库/二进制分发。
- Node ≥ 24 只出现在**构建期**（若选择构建期生成）；本仓库当前 CI 无 Node，见 §7.2。
- budding 的 MCP（`pnpm mcp` / `pnpm mcp:http`）只用于开发期查询组件目录、示例、动作契约与 `validateSchema`；**不作为运行时依赖，不进入 CI 必需路径**。

---

## 3. JSON 契约

### 3.1 命名空间与路径

原则：**页面路径仍归 URL 契约**，JSON 端点与页面同构或独立版本化命名空间。命名空间选择见 **D2**。

无论选哪种，必须满足：

- 只使用 `GET`（读）与 `POST`（写/动作），**禁用 PUT/PATCH/DELETE**。
- 端点身份沿用资源命名（复数、小写、中划线）。
- 若新开 `/api/ui/...`，需同步修改 `entry_guard` 的前缀豁免（当前只豁免 `/api/v1/`），并明确它仍走同一套会话鉴权。
- 资源版本放路径或载荷内，二者只选其一（建议路径 `/api/ui/v1/...`，与 `/api/v1` 一致）。

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

### 3.3 字段码

- 字段码沿用领域错误携带的结构化字段，例如站点表单的 `SiteField`：`domain` / `aliases` / `root_dir` / `runtime_id` / `proxy_target`。
- 规则：**字段码 = 前端表单节点 id（或 `props.name`）**；内核与页面 schema 各自维护一张映射表，发布前做一致性测试。
- 未登记字段码回退为顶部提示，不猜测控件。
- 字段码表冻结后只增不改；改名视为破坏性变更。

### 3.4 时间

现状混用：页面用 `%Y-%m-%d %H:%M`（本地时区），目录元数据用 RFC3339/UTC，数据库存 epoch 秒。

建议（决策 **D9**）：JSON 一律输出 RFC3339（带时区）或 epoch 秒 + 明确单位，前端负责本地化与格式；不在内核拼展示字符串。

### 3.5 分页与游标

现状：**没有真正分页**。审计固定取 200 条，任务列表全量后内存过滤。

目标（决策 **D10**）：

- `GET` 列表接受 `limit` 与 `cursor`，返回 `next_cursor`；`total` 按需给出。
- 游标必须不透明，排序键由资源声明；禁止把 `offset` 与 `cursor` 混用。
- 筛选字段、排序字段由资源契约显式声明，禁止透传任意字段路径。

### 3.6 动作元数据

动作 id 沿用 `crates/app/src/actions.rs` 的注册表，格式 `<resource>.<verb>`，同时作为审计 `action` 的来源。

| 元数据 | 用途 |
|---|---|
| `id` | 全局唯一、稳定 |
| `permission` | 所需 `skiff_model::Permission`；前端只用于显隐 |
| `dangerous` | 前端必须走 `confirm` 动作或等价确认 |
| `async` | 返回作业引用，需轮询进度 |
| `idempotent` | 允许重复提交的语义 |
| `method` / `path` | 传输层调用方式；仍受 URL 契约约束 |
| `input` | 参数 schema（类型、必填、范围、枚举） |

动作**永不返回页面、重定向或提示文案**；成功结果放 `data`，提示放 `notices`。

### 3.7 版本轴

budding 的 JSON **没有 schema 版本字段**，版本轴是 `catalogVersion` + `apiVersion`，字段只增不改。

Skiff 侧对应约束：

- 资源字段名冻结，只增不改；删除或改名必须升 API 版本。
- `apiVersion` 与 `/api/ui/v1` 的版本号保持一致，由内核在会话端点下发，前端只做兼容判断。
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
| 旧路径 301/308/303 | 部分调整 | 303 PRG 不再用于 JSON 动作；成功改为 200 + `notices`，旧页兼容期保留 |
| 查询参数只读、写参数进体 | 继续有效 | 分页新增 `limit` / `cursor`，先登记再实现 |
| 页面与页面内接口同构 | 视 D2 | 若新开 `/api/ui`，需在契约表登记为新的机器接口命名空间 |

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
- budding 默认包**不含** `admin` 组（`admin` / `nav-menu` / `page-tabs`），`chart` 也不在默认包；色与文档存在漂移（§7.1）。菜单壳如何获得见 **D4**。
- 菜单偏好（顺序/隐藏/固定/布局）继续存内核，不迁到 `localStorage` 作为唯一来源。

### 4.4 插件 UI 契约（继续有效，不改）

- 插件功能页继续 `iframe`，沙箱属性保持 `allow-scripts allow-modals allow-forms`，**不加** `allow-same-origin`。
- 一次性 nonce、`skiff.rpc` / `skiff.theme` / `skiff.height` / `skiff.open`、RPC 逐次审计、`PLUGIN_UI_CSP` 全部不变。
- 内核壳页从 askama 换成 budding 后，iframe 仍是内核 DOM 的一部分，postMessage 桥接代码需要在新壳里等价实现（**待实测**）。
- 插件自身页面不进入 budding schema；是否让插件改出 SDUI JSON 见 **D14**。

### 4.5 需要补的条款（新增）

1. JSON 命名空间与版本位置（D2 定稿后写进 URL 契约 §2/§9）。
2. 动作成功不再 303，改为状态码 + `notices`；旧页兼容期双轨。
3. `api/ui/*` 的前缀豁免与鉴权口径（若选新命名空间）。
4. 字段码表与表单节点 id 的映射规则。
5. 列表 `limit`/`cursor` 参数登记。
6. 空态原因字段（`empty_reason`）的语义与枚举。
7. 前端深链回落的 404 边界（D13）。
8. 静态产物版本化与缓存策略（D11）。

---

## 5. 登录、权限、审计、CSRF 的桥接

### 5.1 会话与 CSRF

- 由会话端点下发（命名空间以 D2 为准）：返回 `{ user, role, permissions, acl, csrf_token, api_version }`（字段名待定稿）。
- 现有 CSRF 令牌 = `sha256(secret || session_token)`，仍由内核生成；前端把 `csrf_token` 放进 `x-csrf-token` 头。
- 登录页有三种形态（决策 **D3**）：保留现有 SSR 登录页 / 用 budding 重建登录页 / 双轨过渡。无论哪种，登录 CSRF 的签名 token 与来源判定（`Sec-Fetch-Site`/`Origin`/`Referer`）都不能弱化。
- 会话续期失败（`Unavailable`）与未登录（401）必须可区分，不能都塌成登录页。

### 5.2 401 / 403

- 401：`code = unauthenticated`；前端 `platform.request` 适配器统一处理为跳转 `/{p}/login?next=...`。
- 403：`code = forbidden`，可选 `required_permission`；前端展示 `outlet.content.forbidden` 或页面内提示。
- **budding 没有 401 全局拦截**，且注入 `platform.request` 后 `apiAllowlist` 失效；适配器必须自行实现「仅允许同源相对路径 + 内核端点前缀」的白名单，否则等于放开任意地址。
- 403 的现有语义可复用：路由 `permission` 不满足时菜单剔除 + 出口 403 占位且不拉 schema。

### 5.3 ACL 快照与重新 embed

budding 的 `acl` 是快照，角色变化必须重新 `embed`。内核侧触发时机：

| 时机 | 动作 |
|---|---|
| 登录成功 / 登出 | 重新拉会话并 `embed` |
| 收到 403 且 `required_permission` 与本地 acl 不一致 | 重新拉会话；仍 403 则展示无权限 |
| 会话续期失败 | 跳登录 |
| 菜单或权限配置被管理员修改 | 下次导航时刷新会话；当前页可提示「刷新以应用」 |
| 入口前缀变更 | 由内核 302 到新前缀，前端按新前缀重建 |

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
| 容器日志 | SSE | 暂不接；保留独立页面 |
| 终端 | 票据 + WS | 暂不接；保留独立页面或 iframe |
| 文件上传 | 分片 `offset` 协议 | budding `file` 只支持单次 FormData POST；见 §6 |

---

## 6. 能力缺口与建议

判定列：**我方适配** = 只用现有能力即可；**需框架改动** = 必须上游补；**需负责人决策** = 有替代路径，但取舍明显。

| # | 缺口 | Skiff 用途 | 我方适配（可绕） | 需框架补 | 判定 |
|---|---|---|---|---|---|
| G1 | 无独立 `tree` / `tree-select` | 文件管理目录树 | 用列表 + 面包屑替代；或文件页暂不迁移 | 树组件 + 懒加载节点 | 我方适配 + 暂不迁移 |
| G2 | 无代码 / 日志查看器 | 站点日志、Nginx 日志、作业日志、PHP 配置 | `<pre>` + 等宽样式 + 截断/轮询 | 虚拟滚动日志查看器 | 我方适配（小日志）、需框架改动（大日志） |
| G3 | `file` 仅单次 FormData POST，无进度、无分片 | 文件管理上传（现有分片 offset 协议） | 保留内核文件页；或新增单次上传上限 | 分片 + 进度 + 断点续传 | 需负责人决策（D6） |
| G4 | 无 SSE / WS | 作业日志、容器日志、终端 | 用 `progress` + `source.autoRefresh` 轮询 | SSE / WS 通道 | 需框架改动（如需在页内看实时日志） |
| G5 | 无 descriptions 只读详情 | 容器详情、运行环境详情 | 用 basic 组件拼只读栅格 | descriptions 组件 | 我方适配 |
| G6 | 无通知中心 | 概览待办、审计提醒 | `alert` + `toast` + 菜单 `badge` | 通知中心 | 我方适配 |
| G7 | 组件默认文案为 `.vue` 内中文字面量；Vuetify `createVuetify()` 无参 → 内部英文 | 全站中文 | 逐节点覆盖 `props`；内核词典 `registerMessages()` 只覆盖 15 个键 | 完整 i18n 注入 + Vuetify locale | 需负责人决策（D8） |
| G8 | 主题不切 Vuetify 暗色、不设 `--v-theme-primary` | 现有深色主题、品牌色 | 我方 CSS 覆盖层；或 budding 页暂不做暗色 | 主题适配 Vuetify token | 需负责人决策（D15） |
| G9 | 默认包不含 `admin` / `nav-menu` / `page-tabs` | 三级菜单壳、页签栏 | 用 basic + layout 自建；或自建 Admin Pack | 发行包直接提供 admin 组 | 需负责人决策（D4） |
| G10 | 默认包不含 `chart` | 概览暂无图表 | 暂不需要 | 随包提供 chart | 暂不处理（D16） |
| G11 | 句柄无实例方法 | 切路由/切语言/切主题 | 重新 `embed`（成本待实测）；或用自定义动作驱动 | 实例方法 | 我方适配 + 需实测 |
| G12 | 无 401 全局拦截；注入 `platform.request` 后 `apiAllowlist` 失效 | 会话过期处理 | 适配器内自建同源白名单 + 401 跳转 | 内置 401 钩子 | 我方适配 |
| G13 | 表单字段错误不自动回填 | 站点/运行环境表单 | 自定义动作写 `ctx.extras.form.errors` | 内置 `error.field` 回填 | 我方适配 |
| G14 | JSON 无 schema 版本字段 | 契约演进 | 用 `apiVersion` + 路径版本；字段只增不改 | schema 版本字段 | 我方适配 |
| G15 | 产物无内容 hash | 升级缓存 | 版本化文件名 + `?v=` | 产物 hash 命名 | 我方适配（D11） |

---

## 7. 未决项与风险

### 7.1 分发与法律风险（最高优先级）

| 风险 | 事实 | 影响 | 缓解 |
|---|---|---|---|
| 无 LICENSE | budding 仓库无 LICENSE，全部 `private: true` | 复制产物进本仓库并分发给用户，缺少明确授权 | 取得书面授权后再入库；未取得前不对外分发（D5） |
| 明示内部开发 | README 声明契约可能随时调整、勿用于生产 | 用户侧稳定性与支持承诺 | 锁定版本 + 产物入库 + 升级需回归；对用户明示「预览」 |
| 破坏性变更无废弃期 | 上游存在未给废弃期的 breaking change | 升级即破坏 | 只接受经过我方验收的版本；固定版本号，不跟踪主干 |
| 无 CHANGELOG | 无法评估变更影响 | 升级评审困难 | 每版产物附我方验收记录；以 diff + 测试为准 |

### 7.2 工程风险

| 风险 | 事实 | 缓解 |
|---|---|---|
| CI 无 Node | 现有 12 个 job 均无 `setup-node` | 产物入库；若构建期生成，需新增 Node job（Node ≥ 24） |
| 上游 CI 用 Node 22 而 engines ≥ 24 | 上游自身不一致 | 我方固定 Node 24，记录在接入说明 |
| 构建闸门不覆盖 Node | `Global\SkiffBuild` 只管 cargo | 前端构建统一走 `xtask`/脚本入口；不与 cargo 抢内存 |
| 文档漂移 | docs 与 dist 清单称有 admin，1.6.0 bundle 内 0 命中 | 以产物实测为准；每次升级做组件清单核对 |
| 体积 | JS+CSS 原始约 1.83 MiB，gzip 约 0.45 MiB | 纳入二进制预算实测（§2.2） |
| 双栈维护 | 迁移期模板与 JSON 并存 | 页面级开关 + 完成即删旧路径（D7） |

### 7.3 待实测清单

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

## 8. 需要负责人决策的条目

> 每条只给选项与利弊，不替负责人拍板；决策后回填对应章节。

### P0：不决策无法开工

| ID | 决策点 | 选项 | 利 | 弊 |
|---|---|---|---|---|
| D1 | 产物形态与位置 | A 产物入库 `crates/web/static/budding/`；B 新建 crate 承载产物；C 构建期由 Node 生成 | A 离线最稳、CI 零改动、自动 gzip；B 隔离清晰但多一个 crate；C 单一来源但 CI/本地都需 Node 24 | A 有 diff 噪声与产物漂移风险；B 增加编译单元；C 违反离线出包习惯、体积不可控 |
| D2 | JSON 命名空间 | A 新开 `/api/ui/v1`；B 复用 `/api/v1`；C 同构路径 + 内容协商 | A 隔离干净、不污染对外 API；B 复用现成鉴权；C 最贴合 URL 契约 | A 需改 `entry_guard` 豁免并新增鉴权接线；B 对外契约随 UI 漂移；C 实现与测试成本最高 |
| D3 | 登录页与 CSRF 桥接 | A 保留现有 SSR 登录页；B budding 重建登录页；C 双轨过渡 | A 改动最小、安全面不变；B 前端统一；C 风险最低的渐进方式 | A 保留一份 askama 模板；B 要重做签名 token 与来源判定；C 两套并存需同步修改 |
| D4 | 菜单壳与 Admin Pack | A 用 basic + layout 自建菜单壳；B 用 `build-pack.mjs` 自建 Admin Pack；C 要求上游随发行包提供 | A 不依赖缺失组件；B 更接近上游设计；C 长期最省维护 | A 交互需自研、与 L3 标签栏对齐成本；B 需维护 pack 构建、随上游变更；C 上游排期不可控 |
| D5 | 许可证与分发 | A 取得书面授权后入库分发；B 私库/内网分发；C 暂不分发，仅内部验证 | A 无法律风险；B 不需公开授权但限制用户；C 风险最低 | A 依赖上游；B 与「单二进制交付」冲突；C 无法进入产品 |
| D6 | 文件管理页 | A 保留独立前端（htmx + JS）；B budding 重做；C 分阶段（先只读浏览） | A 零风险；B 体验统一；C 折中 | A 双栈；B 缺 tree/日志/分片，需大量替代；C 仍需补 G1/G2 |
| D7 | 旧 UI 去留与切换粒度 | A 逐页开关；B 一次性切换；C 长期双栈 | A 可回滚；B 收敛快；C 过渡平滑 | A 工期长、双栈成本；B 风险集中；C 维护成本最高 |

### P1：第一页切换前

| ID | 决策点 | 选项 | 利 | 弊 |
|---|---|---|---|---|
| D8 | 文案与 i18n 归属 | A 内核出 key + 前端词典；B 内核出中文；C 逐节点覆盖 | A 可 i18n、单一来源；B 改动最小；C 不改框架 | A 工作量大、需冻结 key；B 前端无法本地化；C 覆盖点随页面增长失控 |
| D9 | 时间格式 | A RFC3339 带时区；B 后端本地化字符串；C epoch 秒 | A 前端可本地化；B 与现状接近；C 无歧义 | A 需改全部读路径；B 不可移植；C 前端需转换、可读性差 |
| D10 | 分页模型 | A 游标；B 偏移；C 维持全量 | A 适合大表；B 简单；C 零改动 | A 实现与测试成本；B 深翻页差；C 数据量上来即瓶颈 |
| D11 | 静态缓存 | A 版本化文件名 + `?v=`；B 内容哈希；C `no-cache` | A 改动小、升级即失效；B 最优缓存；C 最省心 | A 依赖人工改引用；B 需清单与模板支持；C 每次回源 |
| D12 | CSP 放宽范围 | A 保持严格；B 允许 `worker-src blob:`；C 允许 `wasm-unsafe-eval` | A 最安全；B/C 按实测最小放宽 | A 可能阻塞框架能力；B/C 扩大攻击面 |
| D13 | SPA 深链 | A 未匹配路径回落壳页；B 严格 404；C 仅白名单前缀回落 | A 体验好；B 最贴合 URL 契约；C 折中 | A 与「未知路径 404」冲突；B 刷新深链会断；C 需维护白名单 |

### P2：体验与长期

| ID | 决策点 | 选项 | 利 | 弊 |
|---|---|---|---|---|
| D14 | 插件页 | A 继续 iframe；B 插件改出 SDUI JSON；C 双轨 | A 与现契约一致、零风险；B 体验统一；C 渐进 | A 技术栈不统一；B 需改插件生态与沙箱模型；C 长期负担 |
| D15 | 主题与品牌色 | A 我方 CSS 覆盖；B 要求框架适配 Vuetify token；C budding 页不做暗色 | A 立即可用；B 根治；C 最省事 | A 与上游样式耦合、升级易坏；B 依赖上游；C 功能回退 |
| D16 | chart 组件 | A 暂不需要；B 自建 chart Pack | A 零成本；B 为后续图表页铺路 | A 后续图表页受限；B 维护成本 |

---

## 9. 变更记录

| 日期 | 变更 |
|---|---|
| 2026-11-12 | 初稿：budding 只读盘点结论落地为接入规则；覆盖定位边界、产物与离线分发、JSON 信封与动作元数据、既有契约沿用与补条款、登录/权限/审计/CSRF 桥接、能力缺口、风险与待决策项。未实施切换。 |
