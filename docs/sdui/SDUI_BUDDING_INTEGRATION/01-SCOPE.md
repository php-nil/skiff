# 定位、边界、产物与接入形态（§0–2）

> 上级索引：[SDUI_BUDDING_INTEGRATION.md](../SDUI_BUDDING_INTEGRATION.md) ｜ 拆自 `docs/sdui/SDUI_BUDDING_INTEGRATION.md` 原 §0–2（2026-11-16 整理，正文未改）。

---

> 状态：**P0 已定案（2026-11-12），未实施**。本文定义「budding 作为前端渲染层」的接入边界、契约与缺口，不切换现有页面、不改业务代码。
> 上游事实来源：budding 仓库只读盘点（2026-11-12）；内核现状来源：`crates/web`、`crates/plugin` 与本仓库现有契约文档。
> 相关文档：[SDUI_HANDBOOK.md](../SDUI_HANDBOOK.md)（框架无关的适配手册）、[URL_CONTRACT.md](../../contract/URL_CONTRACT.md)、[UI_CONTRACT.md](../../contract/UI_CONTRACT.md)、[MENU_CONTRACT.md](../../contract/MENU_CONTRACT.md)、[PLUGIN_UI_CONTRACT.md](../../contract/PLUGIN_UI_CONTRACT.md)。
> 分工：本文只维护 budding 相关的具体事实、契约形状与缺口；框架无关的分层与迁移阶段只在 SDUI_HANDBOOK 维护，本文引用不复述。

---

## 0. 一页结论

| 维度 | 结论 |
|---|---|
| 定位 | budding 只做**表现运行时**；Skiff 后端只出 **JSON 数据与动作结果**，不出布局、不出 HTML 片段、不出跳转 |
| 定案状态 | P0 七条（D1–D7）与 D8–D13 已于 2026-11-12 定案（§8.1）：产物直接入库、不引入 Node、JSON 走 `/ui/v1`、登录页也用 budding、要求上游随发行包提供 admin Pack、按 MIT、文件管理页用 budding 重做、一次性切换；文案 key+中文兜底、时间 RFC3339 UTC、审计/作业/文件 keyset 分页、静态产物版本化、CSP 保持严格 |
| 仓库关系 | **禁止修改 budding 仓库**；产物由上游发版打包后复制进本仓库；缺能力只提建议，由项目负责人决策 |
| 接入前提 | 产物入库 + 单二进制嵌入（`RustEmbed`）；**本项目不引入 Node 工具链**；测试机无公网，部署期不得依赖 npm 或 CDN |
| 最大外部阻塞 | 无未关闭的上游能力项：`admin` / `nav-menu` / `page-tabs` 已随包，文件管理所需能力（§6.1）已关闭；外部事项只剩 LICENSE（D5）与终端 WS 真连验收 |
| 最大硬缺口 | 终端 UI 已完成（复用既有 exec 票据 + WS，`terminal` Pack 入库，§5.19）；WS 真连与容器内交互待 Linux 测试机验收。1.9.0 的确认框 / `property` / `setErrors` / `autocomplete` / UI 配置已可用，但宿主仍是旧写法（backlog A1–A5） |
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
| 体积 | 入库 `1.8.0-r7` 实测：JS 1,292,889 B；CSS 661,321 B（功能集 1.9.0，产物自报版本仍为 1.8.0） |
| 消费方式 | `<script src>` + 挂载点 + `BuddingSDK.embed(target, schema, opts)`；加载后不自动渲染。`opts` 支持扁平 `locale` / `theme` / `messages` / `uiDefaults`（1.9.0） |
| CSS 加载 | 运行时按 `document.currentScript` 推导目录并插入 `<link>`，**同目录即可** |
| 路由 | 默认 hash；history 模式需 `routerBase` |
| 句柄 | `{ manifest, unmount, instance }`（1.9.0 起带实例）；实例提供 `setLocale` / `registerTheme` 等运行时切换能力，不必重新 `embed` |
| 来源 | 上游发版打包后的 JS/CSS 直接复制进本仓库；**本项目不执行打包命令，也不引入 Node 工具链**（D1） |

### 2.2 放置与嵌入

建议目录：`crates/web/static/budding/`，文件只放版本化名；稳定别名由 `assets.rs` 映射到版本化文件（不复制字节）。版本号唯一来源是 `assets.rs::BUDDING_VERSION`，壳页引用与别名映射共用。

- 产物由上游 `pnpm build:package` 生成后直接复制；**本项目不执行该命令，CI 不新增 Node job**（D1）。

- `RustEmbed` 在编译期把目录打进二进制，对外路径 `/{p}/static/budding/<file>`。
- `crates/web/build.rs` 会为每个文件生成 `.gz` 变体，`assets.rs` 按 `Accept-Encoding` 协商；新增产物自动获得 gzip 能力，**不需要改部署脚本**。
- 现有测试 `every_static_asset_has_gzip_variant` 会覆盖新文件，构建期自动校验。
- 体积影响（`1.8.0-r7` 实测）：budding 三件（JS/CSS/启动脚本）raw 2,247,511 B；gzip 变体由 `build.rs` 生成（本机 `gzip -9` 实测：JS 410,042 B、CSS 79,165 B、启动脚本 67,773 B，合计 556,980 B），与设计目标「单二进制 ≤ 14MB」的张力见 §7。

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
{ type: 'admin', props: { title, brandTitle, bar: false }, content: { body: [{ type: 'outlet' }] } }
```

- **菜单来源唯一**：`GET /ui/v1/menu` 直接投影 `skiff_app::menu::build` 的菜单树（L1/L2 归属、顺序、角色与能力门、用户隐藏偏好与 SSR 导航同源），前端只做「稳定键 → 已迁移路由」的映射，不另造第二套菜单语义。
- **路由表**：菜单树在前端组装成 budding `routes`；分组节点用入口前缀下的保留段（`/{p}/__menu/<key>`）承载层级，子项使用绝对路径 `/{p}/...`。插件菜单项按 `/ui/v1/menu` 下发的 `plugin_id` / `views` 生成 `/plugins/<id>/<view>` 路由；没有 schema 的旧插件仍回落到内核深链。
- **路由基址**：路由 path 已含 `/{p}` 前缀，因此**不设置 `routerBase`**；旧页面 schema 里的 `prefix + '/sites/...'` 绝对跳转可直接复用，不会出现双前缀。
- **按地址栏定位**：每个路由的 `schema` 写成 `skiff:<kind>`，由启动脚本的 `platform.loadJson` 拦截，按当前路径段取数并复用既有页面构造器；插件视图写成 `skiff:plugin-view`，由壳页加载 schema 并完成 `{{data.*}}` / `{{query.*}}` 绑定；未适配页面返回统一占位。
- **壳层 chrome（2026-11-17 起）**：`bar: false` 关掉 budding 默认顶栏（品牌 / 页面标题 / 账号 / 退出登录），原生面包屑槽同步隐藏（`.bud-admin__crumbs`）；位置与同级切换由内核渲染的**三级路径条**承担（`.skiff-path`，节点挂 `.bud-admin__main` 首子节点，样式 `static/skiff-shell.css`）：一级（分组）› 二级（页面）› 三级（子视图），每级下拉切同级，重建闸门防 MutationObserver 死循环。账号与退出登录移入「设置」页「当前账号」区块（仍走 `POST /ui/v1/session/logout` 与同一 CSRF 头）；固定页插件槽位（`[[mount]]`）的 schema 渲染待槽位 API 落地，当前仍不渲染。
- **三级子视图登记**：核心页 L3 在 `budding-boot.js` 的 `SUBVIEWS` 登记（当前 `core/sites/web` → 状态 / 配置 / 日志 / 模块，路由 `/{p}/sites/web/{,config,logs,modules}`，隐藏路由不进侧栏、路径条 `meta.subs` 可切）；插件多视图仍由 `pluginTabs` 页内按钮行承担。
- **图标**：1.8.0 起 budding 官方包自带 `Budding Icons` 子集字体，壳页 `<link>` 引入宿主入库的版本化图标 CSS/woff2（§2.4）；`1.8.0-r7` 子集仍为 78 字形（重打包未改字形），菜单语义名直接映射到 mdi 名（含 `mdi-image-outline` / `mdi-harddisk` / `mdi-folder-outline`），不再做近似兜底或留空。

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
- **图标资产随包版本化**：`budding-icons-1.8.0-r7.min.css`（4,648 字节，仅把 `@font-face` 的字体名改成版本化文件名）与 `budding-icons-1.8.0-r7.woff2`（3,992 字节）取自官方包，由壳页 `<link>` 引入，带 `?v=1.8.0-r7` 时返回 `immutable`（woff2 也按版本化文件名命中 `immutable`，`assets.rs` 已识别 `.woff2` 后缀）。`Budding Icons` 子集共 78 个字形（CSS `::before` 78 条），含镜像 / 磁盘 / 文件夹等业务语义图标，宿主不再另存完整 @mdi/font。woff2 本身已压缩，`build.rs` 跳过它的 `.gz` 生成（再压只会增加字节）；体积按原始字节计，不存在「gzip 后很小」的收益。

### 2.4.1 浮层层级归属（`1.8.0-r7` 起的结构契约）

上游在 `docs/plan-layer-stacking.md` 定案：浮层不再靠「猜一个更大的 z-index」，而是按 **DOM 归属**表达它属于哪一层。

- 图层根节点（`.bud-layer-root`，`z-index: 2400`）同时带 `data-bud-layer-root` 属性，这个属性是**浮层的 teleport 挂载点**，属于结构契约；样式 class 可被宿主覆盖，属性不可。
- 官方 UI 底座给 `VMenu` / `VSelect` / `VAutocomplete` / `VCombobox` / `VTooltip` 注入默认 `defaults: { attach: '[data-bud-layer-root]' }`。宿主显式传入的同名键优先，其余键原样保留。
- 由此两处故障同时消失：浮层不再落在 `2400` 之下被压住，也不再被 `isolateBackground` 标记 `inert` 导致「看得见、点不动」。
- **宿主约束**：不得再为弹窗内下拉注入 z-index / `visibility` / append-to-body 之类的补丁（2026-11-16 的宿主补丁已撤销，见 [BUDDING_UPSTREAM_BACKLOG.md](../BUDDING_UPSTREAM_BACKLOG.md) §4 U5）；壳若自定义 `uiDefaults`，同名键会覆盖官方默认值，需自行保证 `attach` 仍指向图层根。

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
