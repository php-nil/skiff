# budding 上游能力核对与任务清单（1.8.0）

> **核对日期**：2026-11-13
> **对应 budding 版本**：**1.8.0**（`dist/package/budding-1.8.0.manifest.json`，`builtAt 2026-11-13T01:25Z`；`dist/docs/catalog.json` 为 1.8.0 / 57 条目；`dist/embed` 四个文件与 `dist/packs` 六个 Pack 变体）
> **上游答复**：[BUDDING_UPSTREAM_BACKLOG-UP.md](BUDDING_UPSTREAM_BACKLOG-UP.md)（逐条核对对象，保留备查）
> **用途**：直接转给 budding 团队执行的缺口清单。本文只描述「新版本下具体还差什么」，已解决项不再重复提需求。
> **读者**：budding 团队及其 AI 执行者；每条未闭环任务都是一段可直接粘贴的提示词。
> **标注约定**：**产物实测** = 在 1.8.0 产物/catalog 上检索或读取得到；**读文档确认** = 来自 1.8.0 随包手册；**推断** = 依据现有信息推断，执行前需核实。

---

## 0. 一页说明

### 0.1 本轮结论

| 分类 | 数量 | 条目 |
|---|---|---|
| 已闭环（或方案已采纳并落地主体） | 8 | P1、P2、T0、T1、T2（阶段 1）、T3、T4、T5 |
| 未闭环（重写） | 2 | T6、T7 |
| 未闭环（本轮新增） | 8 | T8、T9、T10、T11、T12、T13、T14、T15 |
| **合计未闭环** | **10** | 见 §0.3 |

关键变化：**1.8.0 补齐了后台壳（`admin` / `nav-menu` / `page-tabs` / `outlet`）、表单族（`select` / `textarea` / `checkbox` / 日期时间 / 数字）、表格服务端分页排序与多选、`virtual-list` 程序化滚动、`code-viewer` + 纯 JS 高亮、`terminal` 按需 Pack、图标字体随包**。原清单里「缺组件」类需求大部分已消失，剩余问题集中在**列表的数据分页语义、滚动生命周期、上传大文件、终端通道契约、schema 安全策略**，以及一个始终未做的表单属性透传。

### 0.2 能力核对表（我们需要的 → 1.8.0 实际）

| 我们需要的能力 | 1.8.0 现状（证据） | 结论 |
|---|---|---|
| 严格 CSP 下用 WebSocket | P1 文档 + 13 探针 × 6 场景矩阵 + 门禁 | ✅ 已闭环 |
| 渲染器资源 MIME / 缓存 / 同源 | P2 文档 + 示例 + 门禁 | ✅ 已闭环 |
| 虚拟列表程序化滚动 | `virtual-list` 6 指令：`scrollTo` / `scrollToTop` / `scrollToIndex` / `getScrollTop` / `reload` / `setData`（产物实测） | ✅ 定位够用；❌ 无滚动事件（T9） |
| 文件浏览（面包屑 + 平铺 + 虚拟） | `breadcrumbs` + `table`（`tree` / `virtual` / `selectable` / `filters` / 列 `sortable` / `perPage` / `rowActions` / `expand` / `exportable` / `quickEdit`） | ✅ 主体够用；❌ 游标分页（T10）、选择策略（T11） |
| 上传进度 / 取消 / 重试 | `file` 40 字段：`uploadProgressTarget` / `maxRetries` / `retryDelayMs` + `cancelUpload` / `retryUpload` | ✅ 单文件整体够用；❌ 分片/续传/多文件（T12） |
| 日志查看（虚拟滚动 + 跟随） | `virtual-list` + `input-text` + `copy` 动作可组合；无行号 gutter / 命中计数 | ⚠️ 组合可用；跟随即时性依赖 T9 |
| 代码只读查看 + 行号 + 高亮 | `code-viewer` 17 字段 + `@budding/highlight`（Prism 默认；产物 `wasm` / `WebAssembly` 命中 0） | ✅ 主体够用；❌ 分段与定位（T13） |
| 终端内联（非 iframe） | `terminal` Pack：13 字段 / 5 触点 / 3 指令；`resizeFormat: json`；`registerTerminalTransport` | ✅ 渲染够用；❌ 通道契约（T14） |
| 登录口令框 | `input-text` 33 字段，**无 `type`、无 `autocomplete`**（catalog + 产物核对） | ❌ 未解决（T6） |
| 下拉 / 多行文本 / 复选 / 弹窗 | `select`（单选）、`textarea`、`checkbox`（布尔）、`dialog` / `drawer` / `confirm` 动作 | ✅ 基本够用；❌ 多选下拉与复选组（T15） |
| 表格分页 / 排序 / 多选 | `paramNames.page/perPage/orderBy/orderDir/keywords`、`selectable` 多选列、列级 `sortable` | ✅ 页式够用；❌ 游标式（T10） |
| 插件页以 Schema 交付 | 路由 `schema: "/pages/x.json"` + `platform.loadJson`；`service.schemaApi` 可运行时取 schema；`custom` + `registerAction` | ✅ 加载够用；❌ 动作白名单 / 禁外链（T7） |
| 图标字体随包 | `budding-icons.css` 3.5KB + `.woff2` 2.4KB，59 个 mdi 子集，`manifest.icons.optional = true` | ✅ 可用；❌ 版本化与缓存约定（T8） |
| 后台壳与页签 | `admin` 22 字段 / 7 区域、`nav-menu`、`page-tabs`、`outlet`（catalog 实测） | ✅ 已闭环（原 D4 消失） |

### 0.3 未闭环任务总览

| 编号 | 任务 | 产出形态 | 依赖 | Skiff 用途 | 规模 |
|---|---|---|---|---|---|
| T6 | `input-text` 透传 `type` / `autocomplete` | 框架能力补齐 | 无 | 登录口令、FTP 改密、表单邮箱/数字 | S |
| T7 | 插件 Schema 的运行时安全策略（动作白名单 / 禁外链 / 静态校验） | 框架能力补齐 + 文档 | 无 | 插件功能页原生渲染的信任边界 | M |
| T8 | 图标字体的版本化文件名、缓存与子集清单 | 构建/文档 | 无 | 离线升级不踩旧缓存 | S |
| T9 | `virtual-list` 滚动生命周期触点（`scroll` / `reachBottom` / `scrollToKey`） | 框架能力补齐 | 无 | 日志跟随、无限加载、按 key 定位 | S–M |
| T10 | `table` 游标分页与追加加载 | 框架能力补齐 | 无 | 文件列表、作业列表（`next_cursor`） | M |
| T11 | `table` 选择策略、打开语义、行级下载与图标列 | 框架能力补齐 | 无 | 文件多选、双击进入、下载 | S–M |
| T12 | 上传阶段 2：分片 / 续传 / 暂停 / 多文件进度 | 框架能力补齐 + 宿主适配 | T2 阶段 1 | 大文件、备份包、多文件队列 | M–L |
| T13 | `code-viewer` 分段加载、`scrollToLine`、语言/主题清单 | 框架能力补齐 + 发布 | T4 | 大文件查看、跳到指定行、nginx/ini 配置 | S–M |
| T14 | 终端 WS 通道契约（票据、关闭码、心跳、审计钩子） | 文档 + 小幅代码 | T5 | Docker 容器终端 | S–M |
| T15 | `select` 多选与 `checkbox` 组 | 框架能力补齐 | 无 | 多状态筛选、批量卸载选项 | S |

### 0.4 统一约束（所有任务适用）

1. **只增不改**：现有 57 个渲染器、`embed(target, schema, opts)` 返回值、动作与指令语义保持可用；字段只增不改。
2. **必须补测试**：单测沿用上游测试栈；涉及滚动、WebSocket、AbortController 时用可注入 mock。
3. **离线可用**：运行时不依赖 CDN、外网字体或远程图标；不引入 `eval` / `new Function`（`budding.js` 产物实测 0 命中），保持 `script-src 'self'`。
4. **无沙箱假设**：渲染器是应用树内普通 Vue 组件，能力经 props / 动作 / `runtime` 暴露，不依赖 iframe 隔离。
5. **数字纪律**：未实测的性能数字标注「待实测」，不得写成既定事实。
6. **命名**：新增组件/渲染器遵循 `vendor:type`；官方新增沿用现有内置组，不新造分组语义。

---

## 1. 已解决：落地版本与 Skiff 侧替代口径

> 以下条目**不再提需求**，仅保留结论，便于双方对齐。

| 条目 | 落地版本 | 我们侧对应口径 | 遗留 |
|---|---|---|---|
| P1 `connect-src` ws/wss | 1.7 起，1.8.0 文档 + 矩阵 + 门禁 | CSP 保持 `default-src 'self'`；终端走同源 `wss://`，跨源显式列源，不引入通配 | 无 |
| P2 资源 MIME / 缓存 / 同源 | 1.7 起，1.8.0 文档 + 门禁 | 渲染器资源一律同源相对路径；字体 `.woff2` 以 `font/woff2` 下发；不使用 worker / wasm | 图标文件名缓存见 T8 |
| T0 虚拟滚动定位 | 1.8.0 `virtual-list` | 一律用 `invoke` 调 `scrollToIndex` / `scrollToTop` / `scrollTo`，不再靠重新挂载回顶 | 滚动事件见 T9 |
| T1 文件浏览（table + breadcrumbs，不新建 type） | 1.8.0 主体交付 | 采用 `table` + `breadcrumbs`：`tree` / `virtual` / `selectable` / `filters` / 列 `sortable` / `rowActions` 直接用；目录状态走数据域 `dir` | 游标分页 T10、选择策略与下载 T11 |
| T2 上传阶段 1 | 1.8.0 | 单文件整体上传：`uploadProgressTarget` + `cancelUpload` / `retryUpload`；宿主实现 `PlatformAdapter.upload` 以拿到真实进度 | 分片/续传/多文件见 T12 |
| T3 日志查看（组合，不新建组件） | 1.8.0 能力齐备 | 先用 `virtual-list` + `input-text` + 过滤数据 + `copy` 动作；10 万行不承诺，行号/命中计数由页面自拼 | 跟随即时性与「上滚暂停」见 T9 |
| T4 代码查看（`code-viewer` + `@budding/highlight`） | 1.8.0 | 只读用 `code-viewer`；高亮由同源 highlight Pack 提供；禁用 WASM 方案（产物无 `WebAssembly`） | 分段/定位/语言见 T13 |
| T5 终端（`@budding/terminal` Pack） | 1.8.0 | 采用 `terminal` + 宿主 transport；`resizeFormat: "json"`；重连用 `reconnect` / `reconnectDelay` / `reconnectLimit` | 通道契约见 T14 |
| P3 Pack 分发产物 | 1.8.0 `dist/packs`（上游答复新增，非原清单条目） | 六个产物（echarts / highlight / terminal 各自自包含 + external）；宿主统一用 external 变体避免重复 Vue | highlight 未进 `manifest.packages`，并入 T13 |

**已删除的过时描述**：原清单中「无 select / textarea / checkbox / dialog / page-tabs / nav-menu」「必须自建 `skiff:file-browser`」「表格不支持多选」「高亮必须自研」等结论在 1.8.0 均不成立，已从本文件移除；历史决策留在上游答复文件与 git 历史中。

---

## 2. 未闭环条目（可直接粘贴的提示词）

### T6 `input-text` 透传 `type` 与 `autocomplete`

- **产出形态**：官方表单能力补齐
- **依赖**：无
- **现状（产物实测）**：1.8.0 的 `input-text` 共 33 个字段，**没有 `type`，也没有 `autocomplete`**；渲染出的原生 `input` 固定为文本类型。`time` 组件内部使用原生 `type=time`，说明链路允许，只是 `input-text` 未开放。
- **缺口表现**：Skiff 登录页只能用启动脚本在渲染后把 DOM 改成 `password`（`crates/web/static/budding/budding-login.js` 的 `maskPassword` / `watchPasswordMask`），口令在首帧存在明文窗口；浏览器密码管理器无法识别登录表单；无法声明 `autocomplete="off"` 阻止浏览器错误回填。FTP 改密、SMTP 口令等页面同样受影响。

```text
[任务]
让 budding 的 input-text 渲染器支持 type 与 autocomplete，并透传到原生 input 元素。

[背景事实]
1. 1.8.0 的 input-text 有 33 个字段，没有 type 与 autocomplete；原生 input 固定为文本类型。
2. 同版本 time 组件已使用原生 type=time，透传链路存在，只是 input-text 未开放。
3. Skiff 登录页当前用运行后脚本改写 DOM type=password，属于临时桥接，必须在上游落地后删除。

[要产出]
1. input-text 新增字段：
   - type: 'text' | 'password' | 'email' | 'number' | 'tel' | 'url'，默认 'text'；
   - autocomplete: string，可选，原样透传到原生 input 的 autocomplete（含 'off' / 'new-password' / 'current-password'）。
2. type 与 autocomplete 必须绑定到原生 input 元素，而不是只放在外层容器。
3. 类型变化时保持值与校验状态不变；password 时浏览器表现为掩码输入。
4. 文档补字段表与一个登录表单示例。

[验收标准]
- 渲染后的 DOM input 上可断言 type 与 autocomplete 与 schema 一致。
- 四种以上 type 各有单测；不传 type 时与 1.8.0 行为完全一致。
- 口令框不依赖任何后置脚本改写 DOM 即可掩码。
- 现有表单值收集、校验与提交链路不回归。

[明确不做]
- 不做密码强度、显示/隐藏切换、文件/日期/时间等其它类型。
- 不改表单校验与提交实现。

[约束]
- 字段只增不改；离线可用；不引入 eval / new Function。
```

---

### T7 插件 Schema 的运行时安全策略（动作白名单 / 禁外链 / 静态校验）

- **产出形态**：框架能力补齐 + 文档
- **依赖**：无（Skiff 侧包格式另行定义，不阻塞本任务）
- **现状（读文档 + 产物实测）**：
  - **随包 / 远程 schema 加载已具备**：路由记录可写 `"schema": "/pages/x.json"`，经 `platform.loadJson(url, { kind })` 加载；`service` 组件另有 `schemaApi`（返回 Schema 的接口）与内联 `schema`。这一半需求可以划掉。
  - **动作可控性不足**：内核动作（`ajax` / `navigate` / `copy` 等）对任何 schema 永远可用；只有 `custom` 要求宿主 `registerAction`，未注册才报错。
  - **无外链约束**：`ajax` 的 `url`、`navigate` 的 `url`、`source`/`schemaApi` 都可指向任意外部地址，内核不做宿主白名单。
  - **无静态校验与限额**：没有公开的 `validateSchema`；节点数、深度、字节数无上限；`usedTypes` 在内部编译结果里，未作为公开 API 暴露。
  - 未知 `type` 现在会让类型加载失败并进入整页错误态，无法只让单节点降级。

- **缺口表现**：第三方/插件提供的 schema 与内核页面 schema 拥有同等能力，无法在渲染前拒绝「读其它接口」「跳外链」「超大 schema」；插件页一旦改成原生渲染，iframe 原有的同源隔离消失，需要这一层来兜底。

```text
[任务]
为「非第一方 Schema」（插件、第三方、远程下发）提供运行时安全策略与静态校验，且不影响现有第一方页面。

[背景事实]
1. 1.8.0 支持路由 schema 走 platform.loadJson、service.schemaApi 运行时取 Schema。
2. 内核动作对任何 schema 恒可用；只有 custom 动作要求宿主注册。
3. ajax / navigate / source / schemaApi 的 URL 目前不受宿主白名单约束。
4. 编译阶段内部已有 usedTypes，但未公开；未知 type 会导致整页错误态。

[要产出]
1. 实例级配置 schemaPolicy（或 embed 的 options.schemaPolicy）：
   - allowedActions?: string[]：仅放行列出的动作类型（如 ['invoke','setValue','custom:skiff:*']），未列出的动作在执行前被拒绝并上报；
   - allowedHosts?: string[]：ajax / navigate / source / schemaApi 的 URL 仅允许同源或列出的 host，其余拒绝；
   - maxNodes / maxDepth / maxBytes：超限直接拒绝，不进入渲染；
   - onViolation?: 'error' | 'strip'：整页失败或剥离违规节点后继续。
2. 公开一个纯函数 validateSchema(schema, policy) => { ok, violations: [{ path, code, message }] }，可在加载前调用。
3. 编译结果公开 usedTypes（只读数组），供宿主在渲染前判断需要的 Pack。
4. 未知 type 的降级规范：单节点渲染占位 + 上报 phase/type/path，不整页崩。

[验收标准]
- 含外链 ajax / navigate 的 schema 在 allowedHosts 下被拒绝或剥离，可断言 violations。
- 超出 maxNodes / maxDepth / maxBytes 的 schema 被拒绝且不挂载。
- 未列入 allowedActions 的动作不产生任何请求或跳转。
- 未配置 schemaPolicy 时，现有页面行为逐字节不变（含错误态文案）。
- validateSchema 对同一 schema 与运行时判定结论一致（用测试向量对齐）。

[明确不做]
- 不做插件签名、信任链、包验签（属宿主职责）。
- 不改变第一方内置组的默认行为。
- 不引入 eval / new Function。

[约束]
- 只增不改；必须补单测；离线可用。
```

---

### T8 图标字体的版本化文件名、缓存与子集清单

- **产出形态**：构建产物 + 文档
- **依赖**：无
- **现状（产物实测）**：
  - `budding-icons.css` 3529 B、`budding-icons.woff2` 2456 B，共 **59** 个 mdi 图标；CSS 用相对路径 `url("./budding-icons.woff2")`，`font-display: block`。
  - `manifest.icons = { style, font, optional: true, fontFamily }`，`files[]` 内给了 sha256；但**两个图标文件名都不带版本**，而 JS/CSS 是 `budding-1.8.0.min.{js,css}` + 稳定别名 `budding.min.js` / `budding.css`。
  - 文档只写了「必须与 budding.js 同批升级」，没有给缓存头建议。
- **缺口表现**：站点对静态资源设长缓存时，升级 1.8.0 → 1.8.1 会出现「JS 已更新、图标字体还是旧的」，缺字形（空白图标）且难以排查；子集清单不可机读，宿主无法在构建期校验「我用到的图标是否都在子集里」。

```text
[任务]
把随包图标字体改成可长期缓存、可机读、可校验的交付形态。

[背景事实]
1. 1.8.0 产出 budding-icons.css / budding-icons.woff2，59 个 mdi 子集，文件名不带版本。
2. JS/CSS 已是「版本化文件名 + 稳定别名」；图标没有跟上。
3. manifest.icons 只有 style/font/optional/fontFamily 与 files[] 的 sha256。

[要产出]
1. 产物改为版本化文件名 + 稳定别名：
   - budding-icons-<version>.css / .woff2（内容寻址或版本号），并保留 budding-icons.css / .woff2 作为别名；
   - CSS 内 url 指向同版本 woff2，不依赖查询串。
2. manifest.icons 增 version、files[{ file, bytes, sha256 }]、integrity（可选）；别名关系写入文档。
3. 产出 budding-icons.json：{ version, fontFamily, icons: ["alert", ...] }，与 CSS 中的 ::before 规则一一对应。
4. 文档给出缓存建议：版本化文件 immutable（一年），别名 no-cache；字体 MIME 必须 font/woff2。
5. 构建门禁：图标清单与 CSS 规则数量不一致、或缺字形时构建失败。

[验收标准]
- 升级版本后，仅靠版本化文件名即可绕过旧缓存；别名仍可用。
- budding-icons.json 的 icons 数量与 CSS 规则数一致（当前 59）。
- manifest 内 sha256 与文件实际内容一致。
- 文档明确「引官方子集 / 换完整 @mdi/font / 不引」三种用法的缓存差异。

[明确不做]
- 不把完整 @mdi/font 打进默认产物。
- 不改 Vuetify 的 mdi 渲染方式。

[约束]
- 只增不改；离线可用；兼容现有 `<link rel="stylesheet" href="/budding-icons.css">` 写法。
```

---

### T9 `virtual-list` 滚动生命周期触点（`scroll` / `reachBottom` / `scrollToKey`）

- **产出形态**：框架能力补齐
- **依赖**：无（T3 组合方案、日志跟随、无限加载的上游）
- **现状（产物实测）**：
  - 定位 API **够用**：`scrollTo({ top, behavior })`、`scrollToTop()`、`scrollToIndex({ index, position })`、`getScrollTop()`、`reload`、`setData`。
  - **事件只有 `init` / `refresh`**：没有 `scroll` 触点，也没有「到达底部」触点；组件不暴露滚动容器，宿主无法在不侵入 DOM 的情况下判断用户是否上滚。
  - 定位只按 index / 偏移，没有「按业务 key 定位」。
- **缺口表现**：日志页无法实现「用户上滚 → 暂停跟随 + 显示跳到最新」；无限加载无法在到底时自动取下一页（只能靠分页器或按钮）；作业详情要滚到某条记录时只能先自己算 index。

```text
[任务]
为 virtual-list 补齐滚动生命周期触点与按 key 定位，定位指令保持现状不变。

[背景事实]
1. 1.8.0 的 virtual-list 有 6 个指令：scrollTo / scrollToTop / scrollToIndex / getScrollTop / reload / setData。
2. 触点只有 init / refresh；组件不暴露滚动容器与 offsets。
3. 条目有 itemKey 字段，但定位指令只接受 index 或像素偏移。

[要产出]
1. 新增触点：
   - scroll({ top, atTop, atBottom, direction })：节流不少于 100ms，程序化滚动与用户滚动都要触发；
   - reachBottom({ reason: 'user' | 'programmatic' })：每次「从非底部进入底部」只触发一次，避免连续触发。
2. 新增指令 scrollToKey({ key, position? })，position 语义同 scrollToIndex 的 start/center/end；key 不存在时不抛异常，返回 false。
3. 可选字段 stickToBottom（boolean）：为 true 且当前在底部时，items 增长后自动跟随；用户上滚后自动解除，回到顶部按钮可恢复。若实现成本高，可只交付触点，由宿主组合。
4. 文档补触点载荷、触发时机与节流口径。

[验收标准]
- mock 滚动事件下，scroll 按节流触发；reachBottom 从非底部到只触发一次。
- 程序化 scrollTo 触发的 reachBottom 带 reason=programmatic，宿主可忽略。
- scrollToKey 命中、未命中、重复 key 三种情况均有单测。
- 组件卸载后滚动监听与定时器数量归零。
- 现有 6 个指令行为与签名不变。

[明确不做]
- 不改虚拟化算法、不暴露 offsets / DOM。
- 不做双向无限加载、拖拽排序、动态行高重算策略调整。

[约束]
- 只增不改；必须补单测；离线可用。
```

---

### T10 `table` 游标分页与追加加载

- **产出形态**：框架能力补齐
- **依赖**：无（与 T9 的 reachBottom 可组合）
- **现状（产物实测）**：`table` 的服务端模式只有**页式**语义：`paramNames.page` / `perPage` / `orderBy` / `orderDir` / `keywords`，配合 `totalPath`、底部分页器。**没有** `cursorPath`、`nextCursorPath`、`loadMore`、派发追加（`concatData` 只在 `source` 上存在，不是表格语义）。
- **缺口表现**：Skiff 的文件列表、作业列表、审计列表返回 `{ items, next_cursor }`，没有 total 与页码；现在只能「取第一页 + 隐藏分页」，无法继续往后取。改成页码需要内核伪造 offset 语义，属于为了 UI 组件改协议，不接受。

```text
[任务]
为 table 增加游标分页模式，支持首屏加载 + 追加下一页，且不改变现有页式模式。

[背景事实]
1. table 现有服务端参数只有 page / perPage / orderBy / orderDir / keywords，并以 totalPath 驱动分页器。
2. 宿主侧存在大量 { items, next_cursor } 形态接口，没有 total 与页码。
3. 表格已支持 virtual 与 height，追加模式需要与虚拟滚动共存。

[要产出]
1. 新增字段 paginationMode: 'page' | 'cursor'，默认 'page'（现有行为不变）。
2. cursor 模式字段：
   - cursorParam: string（默认 cursor），首屏为空时不发送该参数；
   - cursorPath: string（默认 next_cursor），从响应取下一页游标；
   - loadMoreText: string；
   - 指令 loadMore()：无游标或正在加载时不重复请求；
   - 追加语义：新行追加到现有 items，重复 rowKey 去重。
3. cursor 模式下隐藏页码与总数，底部显示「加载更多」；可与分页器（pagination 组件）互斥。
4. 排序 / 筛选变化时清空游标并回到首屏；错误态保留重试入口。
5. 文档写清 cursor 模式与 page 模式的参数契约差异。

[验收标准]
- mock 接口断言：首屏不带 cursor；第二页带上一页 next_cursor；next_cursor 为空后按钮消失。
- 重复 rowKey 不重复插入；排序变化后游标重置。
- cursor 模式 + virtual 同时开启时只渲染可视窗口（渲染数量远小于总数，具体数字待实测）。
- page 模式回归：现有表格的请求参数、分页器、总数展示完全不变。

[明确不做]
- 不改 page 模式默认行为与参数名。
- 不做双向游标、不做离线缓存、不做跨页选择（属 T11）。

[约束]
- 只增不改；必须补单测；离线可用。
```

---

### T11 `table` 选择策略、打开语义、行级下载与图标列

- **产出形态**：框架能力补齐
- **依赖**：可与 T10 并行
- **现状（产物实测）**：`selectable` 只表示「显示多选列」，`selectionChange` 已存在；**没有** `selectStrategy`（page / all）、**没有** shift 区间选择、**没有** `openOn`（单击/双击）、**没有**列图标映射；动作清单里**没有 `download`**（19 个内核/UI 动作中无此项）。
- **缺口表现**：文件列表翻页后选择被清空，无法「选中若干再批量删除」；单击行就进入目录，移动端误触；文件名前无法显示文件夹/文件图标；没有原生下载动作，只能 `navigate` 到下载地址，无法带 token 或 POST。

```text
[任务]
补齐 table 的选择策略、打开语义、行级下载动作与列图标，保持现有默认行为。

[背景事实]
1. selectable 目前只控制多选列显示；selectionChange 只回传当前页选择。
2. 没有 selectStrategy / rangeSelect / openOn / iconMap 字段。
3. 动作清单（1.8.0）没有 download 动作。

[要产出]
1. selectStrategy: 'page' | 'all'，默认 'page'（现状）：
   - 'all' 时选择跨页保留，modelValue 存 rowKey 列表，翻页后回显；
   - 提供 clearSelection 指令。
2. rangeSelect: boolean，默认 false：开启后支持 shift 连选当前页区间。
3. openOn: 'single' | 'double'，默认 'single'：控制 rowClick 触发的点击次数；双击时单击不再触发。
4. 新增动作 download：
   - { type: 'download', props: { url, filename?, method?, data?, headers? } }；
   - 走宿主 platform（可带鉴权头），不得用 location 直接跳转；失败触发 toast 或 error 上报。
5. 列图标：columns[].icon（string 或 { map, key }），按行值映射；缺省不显示。
6. 文档补四项的字段表与示例。

[验收标准]
- all 策略下翻页后已选项仍高亮，selectionChange 回传跨页集合；clearSelection 清空。
- shift 连选区间正确，反向选择与取消可断言。
- openOn=double 时单击不触发 rowClick，双击只触发一次。
- download 动作带 headers 发出请求，文件名与响应一致；4xx 有可见错误。
- 默认配置（page / false / single / 无 icon）下现有表格行为不变。

[明确不做]
- 不做拖拽选择、不做服务端选择持久化、不做批量操作容器（crud 已有）。
- 不改变 selectionChange 现有载荷字段，只增不改。

[约束]
- 只增不改；必须补单测；离线可用。
```

---

### T12 上传阶段 2：分片 / 续传 / 暂停 / 多文件进度

- **产出形态**：框架能力补齐 + 宿主适配接口
- **依赖**：T2 阶段 1（已交付）
- **现状（产物实测）**：`file` 40 字段，阶段 1 已具备：`uploadApi` / `uploadField` / `uploadData` / `uploadTarget` / `uploadProgressTarget`（写 `{status, loaded, total, percent, attempt}`）/ `maxRetries` / `retryDelayMs`，指令 `cancelUpload` / `retryUpload`，宿主 `PlatformAdapter.upload(req)`。**没有**分片、续传、暂停/恢复、多文件独立进度、并发控制；`multiple` 存在但进度是单一对象。
- **缺口表现**：几百 MB 的镜像/备份包单请求易超时且失败只能整文件重传；断网后无法从已上传偏移继续；多文件时只有一个总百分比，无法展示每个文件的状态；没有暂停。

```text
[任务]
在阶段 1 基础上补齐分片上传、断点续传、暂停/恢复与多文件进度，保持单请求模式为默认。

[背景事实]
1. 阶段 1 已有整体进度、取消、整文件重试与 PlatformAdapter.upload。
2. Skiff 服务端使用 offset 分片协议：创建会话 → 按 offset 上传分片 → 完成 → 可中止。
3. 多文件时 uploadProgressTarget 目前只写单一对象，无法表达每个文件。

[要产出]
1. 新增字段：
   - uploadMode: 'single' | 'chunked'，默认 'single'；
   - chunkSize: number（chunked 必填，下限与默认值写入文档，待实测）；
   - maxConcurrent: number，默认 1；
   - resumeTokenPath: string，用于把会话/已收偏移写回数据域并在重挂载后恢复。
2. 新增宿主适配（全部可选，缺省回退到 uploadApi）：
   - uploadSession(file) => { uploadId, chunkSize?, received? }
   - uploadChunk({ uploadId, offset, chunk, signal }) => { received }
   - uploadComplete({ uploadId })
   - uploadAbort({ uploadId })
3. 行为：
   - 分片顺序发送，offset 单调递增；received 与本地偏移不一致时以服务端为准；
   - 进度按文件写入 { files: [{ name, size, loaded, total, percent, status, attempt }], percent }；
   - 新增指令 pauseUpload / resumeUpload；暂停后不再发新分片，恢复从 received 继续；
   - 5xx / 408 / 429 按 maxRetries 重试当前分片，4xx（除 408/429）不重试并终止；
   - 取消或失败调用 uploadAbort（若提供）。
4. 状态机：idle → preparing → uploading → paused | retrying → success | error | cancelled。

[验收标准]
- mock 适配器断言分片数、offset 连续性、总分片数、received 回填后的续传起点。
- 暂停后无新请求；恢复从服务端 received 继续；取消调用一次 abort。
- 多文件进度各自独立且总和与整体一致；4xx 不重试。
- uploadMode=single 时请求形态、字段与 1.8.0 完全一致。

[明确不做]
- 不做秒传、加密、目录上传、拖拽排序队列。
- 不改 uploadApi / uploadField / uploadData 的既有语义。

[约束]
- 只增不改；必须补单测；离线可用。
```

---

### T13 `code-viewer` 分段加载、`scrollToLine`、语言与主题清单

- **产出形态**：框架能力补齐 + 发布补齐
- **依赖**：T4（已交付）
- **现状（产物实测 + 读文档）**：
  - 主体已具备：`code-viewer` 17 字段、5 指令（`setValue` / `getValue` / `focus` / `scrollToTop` / `getScrollTop`）、3 触点（`init` / `change` / `copy`）；`tokens` 支持服务端下发；只读/编辑双模式；`@budding/highlight` 默认 Prism，产物 `wasm` / `WebAssembly` 命中 **0**（纯 JS）。
  - 语言：产物字符串可检出约 12 类（markup/html/xml、css、clike、javascript/typescript、json、bash/shell、python、ruby、yaml、sql、markdown）；**无 nginx、toml、ini、dockerfile、diff、go、rust、java**。
  - 主题：字段只有 `auto` / `dark` / `light` 与「已注册主题名」；产物内未检出 github / monokai / dracula 等具名主题。
  - **无 `scrollToLine`**；**无分段 `source`**（只有整串 `modelValue`）；`@budding/highlight` 未出现在 `manifest.packages`（`dist/packs` 有产物，但清单只列 kernel / ui-vuetify / echarts / terminal）。
- **缺口表现**：打开 10 万行日志或大配置文件时必须整串塞进 `modelValue`，首屏慢且占内存；无法「跳到报错行」；nginx.conf / toml / ini 只能按纯文本显示；离线宿主无法从 manifest 拿到 highlight Pack 的地址与 integrity。

```text
[任务]
补齐 code-viewer 的分段加载与按行定位，明确语言/主题清单与注册方式，并把 highlight Pack 纳入发布清单。

[背景事实]
1. code-viewer 目前只有整串 modelValue，没有分段 source；指令里没有 scrollToLine。
2. highlight Pack 为纯 JS（Prism 默认），产物无 wasm；语言覆盖约 12 类，缺 nginx/toml/ini/dockerfile/diff。
3. dist/packs 已有 highlight 产物，但 manifest.packages 未列出。

[要产出]
1. 字段 source?: (args: { start_line, end_line, signal }) => Promise<{ lines, total_lines }>，与 modelValue 二选一；滚动到分段边界时加载相邻段且不重复。
2. 指令 scrollToLine({ line, position? })，内部复用 virtual-list 的 scrollToIndex；行号越界钳制。
3. 语言与主题：
   - 文档列出当前内置语言 id 与别名；说明 registerLanguage 的扩展方式（宿主可自带 Prism grammar）；
   - 至少补 nginx、toml、ini、dockerfile、diff 五类常用语法（纯 JS，不得引入 WASM）；
   - 文档列出内置主题名，并说明 registerTheme / adapter 的注册入口。
4. 发布：@budding/highlight 进入 manifest.packages 与 Pack 发布清单，提供 url、types、integrity；保证同源离线加载。
5. 大文件口径：给出「多少行开始建议分段」的实测数字或明确标注待实测。

[验收标准]
- 分段夹具：滚动跨边界时只请求相邻段，行号连续、无重复、无空洞；总行数与 total_lines 一致。
- scrollToLine 命中可视区，越界钳制且不抛异常。
- 五类新语法各有一条高亮断言；产物仍无 `WebAssembly` / `.wasm`。
- manifest 可解析出 highlight 的 url 与 integrity，缺失时加载失败有明确报错。
- 不配置 source 时整串模式行为与 1.8.0 一致。

[明确不做]
- 不做 LSP、diff、折叠、minimap、协同编辑。
- 不引入 WASM 或远程高亮服务。

[约束]
- 只增不改；必须补单测；离线可用；不引入 eval / new Function。
```

---

### T14 终端 WS 通道契约（票据、关闭码、心跳、审计钩子）

- **产出形态**：文档约定 + 小幅代码
- **依赖**：T5（已交付）
- **现状（产物实测 + 读文档）**：
  - 渲染侧已具备：`wsUrl`（仅接受 `ws://` / `wss://`，模板求值）、`height`、`fontSize`、`theme`、`readOnly`、`reconnect` / `reconnectDelay`（200–30000ms）/ `reconnectLimit`（0–20）、`resizeFormat`（`json` 时发 `{ type: 'resize', cols, rows }`）、`ariaLabel`、`emptyText`。
  - 触点：`connected` / `disconnected({ code, reason, willReconnect })` / `reconnecting({ attempt, delay })` / `exit({ code, reason })` / `error({ message })`；指令：`reconnect` / `disconnect` / `focus`。
  - 传输可注入：`registerTerminalTransport(factory)`，默认 `createWebSocketTransport`（`send` / `close` / `onOpen` / `onMessage` / `onClose` / `onError`，`binaryType=arraybuffer`）。
  - **未约定**：浏览器 WS 无法带 `Authorization` 头时票据怎么放；鉴权失败/权限不足时用哪个关闭码、是否停止重连；心跳/空闲超时；`readOnly` 是否在传输层强制；「一次终端会话」的审计边界。
- **缺口表现**：不约定关闭码，鉴权失败会被当成网络抖动无限重连，形成 401 风暴并刷满审计；没心跳时中间设备静默断链只能等 TCP 超时；`readOnly` 只禁本地输入，服务端仍可能收到输入（需宿主自行防）。

```text
[任务]
定义终端渲染器与宿主 WebSocket 通道之间的契约，并补上关闭码策略与心跳能力。
只约定通道，不实现 SSH、不实现业务鉴权。

[背景事实]
1. terminal Pack 已有重连、resize（json 格式）与可注入 transport。
2. 浏览器 WebSocket 不能设置自定义请求头；票据只能走 URL query、子协议或 Cookie。
3. disconnected 触点是唯一能拿到关闭码的位置，但当前没有「哪些码不重连」的约定。

[要产出]
1. 文档章节「终端通道契约」，至少覆盖：
   - 票据传递：推荐 `wss://host/path?ticket=<一次性票据>`；备选 `Sec-WebSocket-Protocol: skiff.v1, ticket.<t>`；明确票据必须一次性、短 TTL、用完即失效，且不得出现在日志；
   - 关闭码语义：4401（未认证/票据无效）与 4403（无权限）不得自动重连，直接进入 exit 并触发 error；1000 正常关闭不重连；1006/1011 等异常码按 reconnectLimit 退避重连；
   - 心跳：新增 heartbeat: { intervalMs, payload? } 或允许 transport 注入 ping；空闲超过阈值时主动断开并走重连；
   - 审计钩子：新增 onInput（仅计数/时间，不含内容）或 input 触点，供宿主记录「会话开始/结束/输入次数」，并说明不承诺内容审计；
   - readOnly 的边界：渲染器只禁止本地输入，服务端权限由宿主强制，文档必须写明；
   - resize 消息的上下限（cols/rows 合理范围）与发送时机（连接成功后一次 + 尺寸变化）。
2. 代码：按上述关闭码实现「不重连」判定；心跳定时器在卸载/断开时清理。
3. 示例：最小票据流程（宿主 source 取票据 → wsUrl 模板 → connected → 断开码处理）。

[验收标准]
- mock WS 关闭码 4401 / 4403 时不重连且触发 error；1006 时按 reconnectLimit 退避。
- 心跳定时器在 disconnect 与卸载后归零；不会在已关闭的 socket 上 send。
- 卸载后 socket.close 被调用、监听器归零。
- 文档明确写出票据、关闭码、审计三项的双方职责边界。

[明确不做]
- 不做 SSH 客户端、多路复用、sz/rz、多标签、录制回放。
- 不引入第三方 WebSocket 库。

[约束]
- 只增不改；必须补单测；离线可用；不引入 eval / new Function。
```

---

### T15 `select` 多选与 `checkbox` 组

- **产出形态**：框架能力补齐
- **依赖**：无
- **现状（产物实测）**：`select` 36 个字段，只有单选（`items` / `itemTitle` / `itemValue` / `itemsPath` / `clearable`），**没有 `multiple`**；`autocomplete` 有 `multiple` + `chips`，是目前唯一的多值近似；`checkbox` 16 个字段，只表达布尔，**没有 items 组形态**。
- **缺口表现**：Cron 的「状态」筛选、防火墙的协议/端口集合、FTP 卸载时的多个选项，只能用 autocomplete 伪装成多选（可搜索、交互不同），或用 `each` + checkbox 手拼（值收集要自己写），表单校验与提交语义不统一。

```text
[任务]
让 select 支持多选，并为 checkbox 提供编组形态，默认行为保持不变。

[背景事实]
1. select 目前只有单选；autocomplete 已有 multiple + chips，但交互语义不同（可搜索）。
2. checkbox 只表达布尔，没有 items 组；多值只能用 each 手拼。
3. Skiff 的多状态筛选、批量卸载选项需要标准多值表单字段。

[要产出]
1. select 新增字段：
   - multiple: boolean，默认 false；
   - chips: boolean，多选时以标签展示；
   - max: number，最多可选数量（可选）。
   modelValue 为数组；提交、校验（required 至少一项）与清空语义与单值一致。
2. checkbox 组（二选一实现）：
   - checkbox 增 items: Option[]，提供 items 时 modelValue 为数组；或
   - 新增 checkbox-group 类型，items + itemTitle/itemValue + 方向。
   两种方案都需在文档写明值与表单提交语义。
3. 文档补多选示例（多状态筛选 + 批量选项）。

[验收标准]
- 多选值按数组序列化并进入表单提交；required 在多选下表示至少一项。
- 清空、禁用、默认值、回填四种场景有单测。
- 不配置 multiple / items 时，select 与 checkbox 行为与 1.8.0 完全一致。
- 文档明确「多选下拉」与 `autocomplete multiple` 的选型差异。

[明确不做]
- 不做树形下拉、分组下拉、虚拟化下拉。
- 不改 autocomplete 现有行为。

[约束]
- 只增不改；必须补单测；离线可用。
```

---

## 3. 依赖与建议顺序

1. **T6 先做**：改动面最小（表单属性透传），收益立竿见影（登录/口令类页面不再依赖 DOM 桥接），且无依赖。
2. **T15、T8 可并行**：都是小改动，不阻塞其它任务；T8 只动构建与文档。
3. **T10 与 T9 是列表能力的地基**：T10 解决「数据怎么继续取」，T9 解决「何时取」；两者完成后，文件浏览与日志页才具备完整交互。
4. **T11 依赖 T10 的选择语义**：跨页选择与游标分页要一起验证，避免「追加后选择错位」。
5. **T12 与 T13 互不依赖**：上传与代码查看各自独立；T12 需要先与宿主确认分片协议字段命名。
6. **T14 只约定通道，不阻塞渲染**：可在 T5 已交付的渲染器上直接对接；关闭码与心跳属于增量。
7. **T7 建议最后做但必须做**：它是插件页原生渲染的安全兜底；在 Skiff 侧插件包格式定案前不动手，接口按本文草案先行评审。
8. **顺序不是发布顺序**：每条独立可回滚、独立补测试；不得把 T9 的改动混进 T10。

### Skiff 侧当前的临时做法（上游落地后需删除）

| 条目 | 临时做法 | 代价 |
|---|---|---|
| T6 | 启动脚本在渲染后改 DOM `type=password`（`budding-login.js` 的 `maskPassword` / `watchPasswordMask`） | 首帧明文窗口；组件重建 input 需 MutationObserver 兜底 |
| T10 | 只取首屏 + 隐藏分页，提示「翻页待接入」 | 文件/作业列表无法浏览第二页 |
| T9 | 跟随靠整表重挂载；无「上滚暂停」 | 滚动位置跳变，日志不可用 |
| T12 | 单请求上传 + 整体进度 | 大文件超时只能整文件重传 |
| T14 | 未建终端入口（内核暂无 `/terminal` 路由） | 终端功能待通道契约定案后另立 |
| T7 | 插件功能页保留旧 SSR，新壳只给占位 | 插件功能在新前端不可达 |

---

## 4. 事实与推测边界

### 4.1 产物实测（1.8.0）

- `catalog.json`：57 个组件、19 个动作；`virtual-list` 17 字段 / 6 指令 / 2 触点；`table` 31 字段（`paramNames.*`、`selectable`、`tree`、`virtual`、`filters`）；`file` 40 字段；`input-text` 33 字段且无 `type` / `autocomplete`；`select` 无 `multiple`；`checkbox` 无 `items`。
- `dist/embed/budding.js`：`cursorPath` / `scrollToLine` / `new Function` / `WebAssembly` 均 0 命中；`autocomplete` 命中仅来自 Vuetify 内部；`loadMore` 命中来自 Vuetify infinite-scroll 文案。
- `dist/packs/highlight.mjs`：`prism` 26 次、`wasm` 0 次；语言标识可检出约 12 类。
- `dist/packs/terminal.external.mjs`：导出 `registerTerminalTransport` / `createWebSocketTransport` / `resizePayload` 等；关闭码只透传，不含策略。
- `dist/embed/budding-icons.css` + `.woff2`：59 条 `::before` 规则，相对路径引用，无版本号。
- `dist/package/budding-1.8.0.manifest.json`：`packages` 只含 kernel / ui-vuetify / echarts / terminal；`icons.optional = true`。

### 4.2 读文档确认

- `platform.loadJson(url, { kind })` 是 schema 与路由表的统一加载通道；路由记录可写 `"schema": "/pages/x.json"`。
- `service.schemaApi` 为「返回 Schema 的接口」，是运行时 schema 的公开入口。
- `custom` 动作未注册会报错；内核动作不可被同名覆盖。
- `schemaPolicy`、`validateSchema`、`usedTypes` 公开 API **不存在**（文档与产物均未出现）。
- 图标子集数量、缓存与「同批升级」要求来自随包手册「零构建接入 → 图标字体」。

### 4.3 推断（需上游确认）

- highlight 未进 `manifest.packages` 是否为疏漏（`dist/packs` 已有产物）；本清单按「需要发布补齐」处理。
- 具名主题（github / monokai 等）可能通过 `registerTheme` 由宿主注入，产物内未检出；需上游给准确清单。
- 未知 `type` 是否在所有路径都导致整页错误态，未逐场景验证；按「需要单节点降级」提需求。
- 各任务规模（S/M/L）为按字段数、触点与测试面的经验估算，无试点数据。

---

## 5. 变更记录

| 日期 | 变更 |
|---|---|
| 2026-11-12 | 首版：P1–P2 + T0–T7，基于 budding 1.6.0 产物盘点。 |
| 2026-11-13 | 对照 budding 1.8.0 与上游答复重写：P1/P2/T0/T1/T2 阶段 1/T3/T4/T5 标为已闭环并删除过时描述；T6、T7 按新版重写；新增 T8–T15（图标版本化、滚动触点、游标分页、选择与下载、上传阶段 2、代码分段、终端通道契约、多选表单）。未闭环 10 条。 |
