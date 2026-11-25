# 页面 JSON 资产（§9）

> 上级索引：[SDUI_BUDDING_INTEGRATION.md](../SDUI_BUDDING_INTEGRATION.md)。
> 规约对象：`crates/web/static/budding/pages/*.json` 与壳脚本 `budding-boot.js` 的分工。
> 与 §2（JSON 契约）、§5（桥接页）配套：§2 管数据面形状，本文管「页面长什么样写在哪」。

---

## 9.1 为什么拆

`budding-boot.js` 曾经承担两件事：**运行时胶水**（启动、路由、主题、平台适配、命令式动作）与**整页 Schema 的拼装**（每页几十到几百行的表结构字面量）。后者本质是数据，用 JavaScript 拼装带来三个长期成本：

| 成本 | 表现 |
| --- | --- |
| 不可 diff | 同一处交互散落在函数调用、三元表达式与共享 helper 里，改一列要读三个函数 |
| 重复造能力 | budding 自带 `source` / `confirm` / `request` / `form.api` / `invoke`，脚本里却用 `skiff*` 动作重新实现一遍「确认 → 请求 → 刷新」 |
| 脚本膨胀 | 单文件曾达 ≈ 9 千行；按页面迁完后 4.4k 行只剩运行时 |

结论：**页面布局属于表现层数据**，落在可评审的 JSON 里；脚本只留运行时。

页面 JSON 有两个合法产出端，按「布局是否需要请求上下文」择一：

| 产出端 | 适用 | 现有页面 |
| --- | --- | --- |
| 静态资产 `pages/*.json` | 布局不随主体、数据、页签变化 | 镜像 / 网络 / 卷 / 新建容器 |
| 服务端 UI 端点（`crates/web` 的 `/ui/v1`，`with_schema=true`） | 布局要按权限、页签、记录本身变化 | 其余全部页面（见 §9.6） |

**术语边界（关键）**：本仓说的「内核」特指**核心调用层**——领域服务与命令层（`skiff-app` 及下层、`policy` / `wire` / `model`），与 UI 无关。[SDUI_HANDBOOK §2.9](../SDUI_HANDBOOK/02-LAYERS.md) 禁的是**命令层感知布局**（在业务代码里写卡片标题、列宽、空态文案），不是「服务端不许产 UI JSON」。站点这类**项目内置 UI** 由 `crates/web` 的 UI 端点产出 JSON 是正当形态；真正的错是把这些布局字面量堆进 `budding-boot.js`。

## 9.2 资产布局与加载链路

```text
crates/web/static/budding/
  budding-boot.js            壳脚本（启动 / 路由 / 主题 / 平台桥 / 命令式动作）
  pages/
    docker-images.json       页面 Schema（自取数 + 声明式交互）
    docker-networks.json
    docker-volumes.json
    _file-row.json           不是页面：目录行模板，`files_page.rs` 用 include_str! 内联
```

两条链路，终点都是「壳脚本拿到 JSON 文本交给 SDK 编译」，区别只在 JSON 从哪来：

- **资产路**：菜单 → 路由表里该页的 `schema` 是资产 URL → SDK 调 `platform.loadJson(url)` 直接取 JSON。
- **服务端路**：菜单 → 路由表里该页的 `schema` 是 `skiff:page` → 壳脚本问 `GET /ui/v1/page?path=&search=`「这个地址的页面结构去哪取」→ 服务端回答端点 URL（含 `?tab=` / `?with_schema=true` 等归一口径）→ 取回 `body.schema` → `prepareServerSchema()` → SDK 编译。

**路由表在服务端**（`handlers/ui/routes.rs`，`GET /ui/v1/routes`）：菜单树是路由的投影，服务端一次算出「路径 / 标题 / 图标 / 隐藏标记 / 侧栏高亮落点 / 页面来源」，壳脚本只消费。判据是**布局是否随请求上下文变化**：不随变用资产，随变用服务端。不要把服务端路的页面搬回资产——那等于把 `can_manage`、页签、记录字段从服务端搬回脚本；也不要把路径映射搬回脚本——那份表与内核菜单是同一份事实，写两遍必然漂移，且脚本那份无法在 CI 里验证。

| 约束 | 口径 |
| --- | --- |
| 地址 | `/{入口前缀}/static/budding/pages/<name>.json`；前缀由服务端在路由表里拼好，JSON 内不写入口段 |
| 缓存 | 静态路由对非 `.min.` 资源返回 `no-cache`（ETag 协商），改 JSON 刷新即生效，**不需要**升 `SHELL_VERSION` |
| 热更 | 测试机设 `SKIFF_EXTRA_STATIC_DIR` 时磁盘同名文件覆盖入库版本，改 JSON 连重编都不用 |
| 体积 | 单文件按页拆，不合并成大文件；`.gz` 由 `build.rs` 自动生成，不需要手工维护 |

## 9.3 页面 JSON 约定

**取数**：页面根节点用 `source` 自取数，不需要脚本预先 fetch。

```json
{
  "type": "page",
  "source": { "url": "/ui/v1/images", "params": { "limit": 1 } },
  "props": { "title": "镜像", "subtitle": "${data && data.can_admin ? '…' : '…'}" }
}
```

- `/ui/v1` 的信封是 `{ "data": {...} }`，页面 `source` 把结果合进根作用域，表达式因此写 `${data.<字段>}`；列表组件的 `itemsPath` 同理写 `data.items`。
- 列表用组件自己的 `source`（`paging: 'cursor'` + `cursorPath`），页面 `source` 只取一次元数据（`can_admin` / `docker_available` / 计数）。
- 初始渲染时数据未到，表达式按**防御式**书写（`data && data.can_admin`），避免 gate 求值抛错后节点被 fail-closed 隐藏且不再回头。

**页头主操作**：列表页的「新建 / 拉取 / 创建」与详情页的启停 / 删除等主操作写 `content.actions`（budding 1.8.7），与标题同行、右对齐；不要再在正文顶部另起一行右对齐，也不要单独放一张「操作」卡。`actions` 按页面作用域求值，可直接按 `data.can_admin` 显隐：

```json
{
  "type": "page",
  "props": { "title": "镜像", "subtitle": "…" },
  "content": {
    "actions": [
      { "type": "button", "props": { "text": "拉取镜像", "color": "primary" }, "visibleOn": "${!!(data && data.can_admin)}" }
    ]
  },
  "body": []
}
```

- 服务端拼装走 `schema_kit::page_with_actions`（`actions` 为空时自动退回普通 `page`，不声明空插槽）。
- 有 `actions` 时 budding 渲染 `data-bud-role="page-header"` / `"page-actions"` 锚点；标题与副标题仍是 `.bud-page-title` / `.bud-page-subtitle`，壳层三级菜单插在整个页头之后。

**绑定与条件**：一律用 budding 原生 `${}` 模板与 `visibleOn` / `hiddenOn`，不引入宿主自造的占位符。

| 需求 | 写法 |
| --- | --- |
| 文案 / 属性插值 | `"subtitle": "${data.can_admin ? 'A' : 'B'}"` |
| 三元选择 | 表达式内直接写，支持 `?:`、`&&`、`!` |
| 节点显隐 | `"visibleOn": "${!!(data && data.can_admin)}"` |
| 行作用域 | 行操作里直接读行字段：`${id}` / `${name}` / `${can_remove}` |
| 入口前缀 / API 基址 | 壳脚本经 `embed` 的全局 `context` 注入 `prefix` / `api`，写 `${prefix}/sites/list` |

**交互**：只用内置动作，脚本里不再为「确认 + 请求 + 刷新」写专用动作。

| 场景 | 动作链 |
| --- | --- |
| 删除等不可逆操作 | `confirm` → `request` → `invoke`（目标组件 `reload`） |
| 详情页写后整页重取 | `request` → `skiffReload`（页面骨架与数据都由服务端重算） |
| 表单提交（宿主自管链） | `form.props.submitMode: "event"` + `onEvent.submit` 的宿主动作；组件不发请求、不弹成功提示 |
| 表单提交（服务端直接收） | `form.props.api` 自带提交，`props.successMessage` 给成功提示，`props.onSuccess` 放刷新（仅限资产页里「提交即入库」的简单表单） |
| 校验 | 字段 `required` / `rules`；服务端字段错误走 `setErrors`（动作或 `invoke`） |
| 失败原地回填 | 宿主动作按 budding 的目标解析规则取表单控制器：先 `ctx.extras.form`（动作在表单子树里），再 `ctx.extras['budding:layer-forms']`（**弹窗页脚的主路径**，页脚与正文是兄弟子树）；取到后 `setErrors({ __form__: 文案 })` + `focusFirstError()`，两张表单都取不到才退回 `toast`。**不关图层、不重建 DOM、不丢输入** |
| 请求失败提示 | `request.props.messages.error`；SDK 会调用 `markFailurePrompted`，节点级兜底提示不再叠加 |

`request` 的失败提示是「本操作失败」级别的保真（服务端明细只在 `form` 的 `errorMessage` 缺省值里透出）；需要更强保真时走 `onError` 动作组，或宿主动作自己把失败原因写回表单。

**CSRF**：壳脚本的平台请求适配器（`apiRequest`）对所有非 `GET/HEAD` 请求统一补 `x-csrf-token`。页面 JSON 不写 header，写操作才能保持声明式。

**弹窗标记**：服务端下发的 `dialog` 动作由壳脚本 `prepareServerSchema` 递归找出正文里的表单，给 `onEvent.submit[0].props` 打一个 `inLayer: true`（资产页的 `_file-row.json` 手写同一字段）。宿主动作用它区分「成功后关层」与「整页刷新」——不再有 dialog 注册表，也不靠运行时猜层级。

**表单控制器**：`ctx.extras.form` 只在动作位于表单子树内时存在。弹窗的提交按钮在图层 `footer`，与正文是两棵兄弟子树，宿主动作必须再查一次图层数据袋 `extras['budding:layer-forms']`（`list()` 唯一一张即用；多张按 budding 的规矩应改用 `props.form` 点名，壳层不猜）。只认 `extras.form` 会得到**静默失败**：层不关、值不丢、也没有任何错误文案。

## 9.4 壳脚本保留什么

只保留「必须命令式」的部分，其余一律不进脚本：

| 保留 | 理由 |
| --- | --- |
| 启动、会话校验、三级子菜单、外层 chrome | 属于壳层装配，不在页面 Schema 语义内（路由表由 `/ui/v1/routes` 下发，脚本不拼） |
| 平台适配器 `request` / `upload` / `loadJson` | 传输层：CSRF、信封、401 跳登录、分片上传（写请求只有一个 `postJson` 通道） |
| 宿主动作实现（`skiff*`） | A 形的「确认 → 请求 → 刷新」已全部声明化；只剩表单回填、下载、RPC 转发等逐请求拼参数或需串行状态的场景，见 §9.6 |
| 插件功能页（随包 Schema 的 `data` 声明 + RPC 转发） | 数据面是插件 RPC，不是 `/ui/v1` |
| 安装 / 作业轮询、终端票据换取 | 有状态的多步时序 |
| 外壳补 `bud-admin--chrome` 类 | Vue 重渲染会抹掉手工类，只能常驻观察器 |

**主题不在此列**：界面配色的 CSS 由服务端在壳页内联下发（`spa.rs::theme_css`，颜色取自已校正的 `skiff_app::theme::Theme`），前端不再实现亮度/对比度算法——同一套取色规则写两份必然漂移。

判据：**脚本里可以有「怎么做到」，不能有「长什么样」**。写卡片标题、列宽、空态文案、按钮顺序属后者，一律迁出；把请求体拼成 JSON、算分片大小、取 CSRF 属前者，留在脚本。

## 9.5 校验

- 结构校验：拿 budding 的 `catalog.json` 比对节点 `type`、`props` 键与动作参数，未知项即报错（本地脚本，不入 CI）。
- 用例：`crates/web/tests/main_flow.rs` 的 `budding_boot_json_pages_are_wired_and_parse` 钉住「路由 → 资产 → 根节点 `page` / `source` / 表格 `id` / 动作链顺序」；`budding_boot_status_table_honors_caller_empty_text` 与 `budding_boot_p1_confirms_and_kill_entry_present` 一类文案 / 入口断言，按页面归属读对应产出端（资产页读 JSON，服务端页读 `.rs`）。
- 浏览器验收：测试机逐页点击，写操作必须走「确认 → 请求 → 表格刷新」，控制台零报错。

## 9.6 迁移状态

**全部页面已迁完**，壳脚本不含任何整页布局构建器，也不含路由表（3.4k 行，动作体占主）。逐页归属：

| 页面 | 产出端 | 入口 |
| --- | --- | --- |
| 镜像 / 网络 / 卷 | `pages/docker-*.json` | 资产 |
| 新建容器 | `handlers/ui/containers_page.rs` | 列表弹窗 + `GET /ui/v1/pages/containers-new`（深链 `/docker/containers/new`） |
| 容器列表 / 详情 / 日志 | `handlers/ui/containers_page.rs` | `?with_schema=true` |
| compose | `handlers/ui/compose_page.rs` | `?with_schema=true` |
| 运行环境列表 / 详情 / 日志 | `handlers/ui/runtimes_page.rs` | `?limit=&with_schema=true`、`?tab=&file=&with_schema=true` |
| 新建站点 / 站点日志 | `handlers/ui/sites_extra_page.rs` | `GET /ui/v1/pages/sites-new`、`?with_schema=true` |
| 站点列表 / 详情 | `handlers/ui/sites.rs` / `site_page.rs` | `?with_schema=true`、`?tab=` |
| 概览 | `handlers/ui/overview_page.rs` | `?with_schema=true` |
| Web 服务（状态 / 配置 / 日志 / 模块） | `handlers/ui/web_page.rs` | `?view=&with_schema=true` |
| 安全 / 菜单 / 界面 / Docker 设置 | `handlers/ui/settings_page.rs` | 四个 GET 各带 `?with_schema=true` |
| 插件列表 / 插件详情 | `handlers/ui/plugins_page.rs` | `?tab=&with_schema=true` |
| 文件管理（目录 / 查看 / 回收站 / 配额） | `handlers/ui/files_page.rs` | `?with_schema=true`（`trash_schema` / `quotas_schema` 由 `handlers/ui/files.rs` 调用，挂响应顶层） |
| 商店 | `handlers/ui/store_page.rs` | `?with_schema=true` |
| 任务 / 作业 / 审计 | `handlers/ui/tasks_page.rs` / `jobs_page.rs` / `audit_page.rs` | `?with_schema=true`；作业日志走 `GET /ui/v1/jobs/{id}/logs`（服务端聚合最多 10 页） |

两条产出路由的判断标准不是「服务端产还是资产产」，而是**布局是否随请求上下文变化**：随变（权限、页签、记录状态）留服务端，不随变进资产。两者都不许回到脚本里拼。

共用构件在 `handlers/ui/schema_kit.rs`：`page` / `text_line` / `detail_lines` / `table` / `list_table` / `search_form` / `code_block` / `write_card` / `form` / `alert` 等。它只拼 JSON 形状，不读业务状态、不判权限，避免每页各写一份表格骨架。搜索跳转统一走宿主动作 `skiffSearch`，替代原先每页一个的 `skiffAuditSearch` / `skiffTasksSearch` 家族。

留在脚本的形态例外（不是布局，是动作体）：

| 例外 | 为什么不能声明化 |
| --- | --- |
| 容器终端（`skiffTerminalReconnect`） | `wsUrl` 必须由浏览器拼当前 origin + 一次性 ticket，JSON 拿不到 |
| 商店安装确认（`skiffStoreConfirm`） | 依赖 install 响应里的一次性 confirm token，需两步串行 |
| 插件功能页（`skiffPluginRpc` / `skiffPluginView`） | 布局来自插件包自带的 Schema，数据面是插件 RPC，不是 `/ui/v1` |
| 整页重取（`skiffReload`） | 只做 `location.reload()`，不含业务参数；供详情页写后刷新 |
| 表单字段回填（`skiffFormErrors`） | 可选：`onError` 明确声明才执行，把最近一次失败的 `error.field` 经 `setErrors` 贴回表单；不声明时默认提示不受影响 |

检查入口：`budding_boot_a_form_writes_are_declarative` 钉住第一批（容器 / 任务 / 作业 / 插件 / 回收站）已无命令式写动作体，且动作链顺序（确认 → 请求 → 刷新 / 跳转）留在服务端 schema。

尚未迁出的写动作（第二批 B 形 + 设置 / 文件）：

| 动作 | 归属 | 待办 |
| --- | --- | --- |
| `skiffSiteWrite` / `skiffSiteToggle` | 站点 | 弹窗表单 + 失败回弹窗 |
| `skiffComposeAction` / `skiffComposeEdit` | compose | 同上 |
| `skiffRuntime*` 系列 | 运行环境 | 同上 |
| `skiffWebConfig` / `WebEnv` / `WebParams` | Web 服务 | 同上 |
| `skiffSettingsSave` / `MenuWrite` / `UiThemeWrite` / `UiPreset` / `DockerSettingsWrite` / `PasswordChange` / `TotpSetup` / `TotpEnable` / `KeyImport` / `KeyRemove` / `QuotaWrite` | 设置 / 配额 | 同上 |
| `skiffFileWrite` 系列 | 文件管理 | 同上 |
| `skiffPluginView` / `FileOpen` / `FileDownload` | 插件 / 文件 | 先试 `download` / `navigate` 覆盖 |
| `skiffSearch` / `QueryNav` / `LogFilter` / `LogSwitch` | 导航 | 尽量转 `navigate`，转不动的保留脚本 |

前三者在路由表里各有一个 `special` 标记（`skiff:terminal` / `skiff:store-confirm` / `skiff:plugin-view`），由 `/ui/v1/page` 给出，脚本按标记分派——标记之外没有第二种「按路径判断该走哪个特例」的逻辑。

路由表与页面取数地址的唯一产出端：

| 内容 | 产出端 | 谁消费 |
| --- | --- | --- |
| 路由表（菜单投影 + 详情/子视图隐藏路由 + 插件视图清单） | `GET /ui/v1/routes` | `embed()` 的 `routes` |
| 内置三级子视图登记表（`pattern` / `blocked` / `title` / `tabs`，Rust `SUBVIEWS` 的镜像） | 同上响应的 `data.subviews` | 壳脚本 `matchSubviews`（2026-11-24 起脚本不再硬编码该表） |
| 「当前地址的结构去哪取」 | `GET /ui/v1/page?path=&search=` | `platform.loadJson('skiff:page')` |
| 图标语义名 → mdi 字形 | `routes.rs::MENU_ICONS` | 路由表的 `icon` 字段 |

`data` 的键集冻结为 `routes` / `plugins` / `subviews`（`main_flow::ui_routes_project_menu_with_hidden_details` 比对）；新增可选键必须同时更新那份冻结清单与本表，删改语义要升 `api_version`。

`page_source()` 是纯函数，12 条单测钉住「路径 → 端点」的映射与编码（`%2F` 只解码一次、query 值解码后再编码、未知路径不给来源）。加页面时改这一处即可，不会再出现「脚本里改了、服务端没改」。
| 文件分片上传 | 切分、并发、进度回填都是命令式流程；表单本身已迁到资产 / 服务端 schema |
| 安装进度 | 已改为表格 `source.autoRefresh`，脚本侧轮询代码删除 |
