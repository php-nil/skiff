# Budding 上游能力任务清单（文件管理 / 上传 / 日志 / 代码 / 终端）

> **用途**：对外交付的实施方案，供下游排期或直接开工。
> **基线**：2026-11-12 只读盘点；组件数量以 `apps/docs/public/catalog.json` 为准（**1.8.0、57 条目**）。
> **本轮结论**：P1/P2、T0、T2 阶段 1、T4、T5 已落地；P3 Pack 分发实施中；剩余 T1、T3、T2 阶段 2。

## 0. 前提与术语

- **Schema**：服务端下发 JSON，按 `type` 找渲染器；**不能出现函数**。取数/上传用声明式 `source` + 宿主 `PlatformAdapter`。
- **Pack**：发布/加载单位。内置组只有 `basic` / `layout` / `form` / `data` / `admin`；`advanced` 是 `meta.category`。第一方裸名，第三方 `vendor:type` + `meta.owner`。
- **程序化控制**：节点 `id` + `exposeCommand` + `invoke`（契约 §19.4）。
- **字段命名**：有 Vuetify 同名 prop 就直接用；没有的写进 `fields/vuetify-alignment.baseline.json` 并注明理由。
- **术语**：`packages/kernel/src/core`=引擎；`manifest.kernel` 产物=内核；`apps/docs`=手册；`docs/`=文档。

## 1. 状态与方案总表

| 条目 | 状态 | 对标 Vuetify 4.3.3 | Budding 方案 | 工作量 | 依赖 |
|---|---|---|---|---|---|
| P1 | 已实现（文档 + 实测 + 门禁） | 无（浏览器 CSP） | 文档：CSP 章节 + WS 示例 | S | — |
| P2 | 已实现（文档 + 实测 + 门禁） | 无（静态资源） | 文档：路径/MIME/缓存约定 + 示例 | S | — |
| T0 | 已实现 | `VVirtualScroll` | 新 type `virtual-list`（已落地） | — | — |
| T1 | 部分实现 | `VDataTableVirtual` + `VBreadcrumbs` + `VFileInput` | 补字段/动作，不新建 type | S–M | T0（懒加载） |
| T2 | 已实现（阶段 1） | `VFileInput` + `VProgressLinear` + `VBtn` | **仅阶段 1**：整体进度/取消/重试；阶段 2 暂缓 | — | — |
| T3 | 未实现（可组合） | `VVirtualScroll` + `VHighlight` + `VTextField` | 组合示例，不新建组件 | S | T0 |
| T4 | 已实现 | `VCode` + `VHighlight`（仅搜索词） | 内核 token 契约 + `@budding/highlight` + `code-viewer`（只读/编辑） | — | — |
| T5 | 已实现（官方第一方按需 Pack） | `VSheet` + `VProgressLinear` + `VBtn` + xterm | 独立按需 Pack `@budding/terminal`（裸名 `terminal`） | M | — |
| P3 | 实施中 | 无（构建/分发） | `pnpm build:packs` → `dist/packs/`；自包含 + external 两种变体 | S–M | T4 / T5 Pack、§17.5 |

## 2. 逐条方案

### P1 / P2 CSP 与资源基线

| 项 | P1 `connect-src` ws/wss | P2 资源 MIME / 缓存 / 同源 |
|---|---|---|
| 状态 | **已实现**（`docs/components.md` CSP 段 + 实测门禁） | **已实现**（同源路径、MIME 表、缓存、字体/worker/wasm 约定） |
| 依据 | [components.md](components.md#csp-建议)、[scripts/verify-csp.mjs](../scripts/verify-csp.mjs)、`tests/artifacts/csp/matrix.md` | 同上；[embed.bundled.ts](../packages/kernel/src/embed.bundled.ts#L36-L49)、[architecture.md](architecture.md#L1089) |
| 方案 | 推荐策略 + 同源 `wss://` 示例 + 跨域显式列源 | 同源路径、MIME 表、缓存约定 + 示例 |
| 验收 | 13 探针 × 6 场景实测有结论；`/` 与 `/docs/` 在推荐策略下零违规 | `default-src 'self'` 零违规；MIME / 缓存断言通过 |
| 示例 | `wss://` 同源连接 + 状态文本（见 components.md） | 同源 CSS + `.woff2` + `font-src 'self'` |

### T0 `virtual-list`（已落地）

| 项 | 方案 |
|---|---|
| 状态/依据 | **已实现**（catalog 1.8.0 / 57 条目，`data` 组）；[schema-contract-components.md](schema-contract-components.md#L353-L382)、[catalog.json](../apps/docs/public/catalog.json) |
| 对标 | `VVirtualScroll`：props `items` / `itemHeight` / `itemKey` / `height` / `maxHeight` / `minHeight` / `width` / `maxWidth` / `minWidth`；默认 slot `{ item, index, itemRef }`；expose `scrollToIndex` / `calculateVisibleItems` |
| 字段 | 同名透传上述 9 个尺寸/条目字段；远程走节点 `source` + `itemsPath`（取数组路径，baseline）；`emptyText` / `loadingText` / `ariaLabel` 记 baseline；`body` 为逐条内容插槽，作用域 `{ ...item, item, index }`；另有通用 `className` / `style` |
| 触点 | `init`（挂载后一次）、`refresh`（重新取数时，默认合并） |
| 指令 | `reload(args?)`、`setData({ data })`、`scrollTo({ top, behavior? })`、`scrollToTop()`、`scrollToIndex({ index, position? })`、`getScrollTop()`（完整定义见 §10.9） |
| 调用 | `{ "type": "invoke", "props": { "component": "list", "command": "scrollToTop" } }` |
| 验收 | 以契约 §10.9 为准；`commands` 与 `exposeCommand` 一致；既有渲染器不改 |
| 示例 | `{"type":"virtual-list","id":"list","props":{"items":"${lines}","itemHeight":24,"height":480}}` |

命名说明见契约 §10.9：保留 `virtual-list`，不改 `virtual-scroll`。

### T1 文件浏览：`table` + `breadcrumbs` 实施清单（不新建 type）

| 组 | 能力 | 字段 / 动作落点 | 新增（标注） | 量 |
|---|---|---|---|---|
| 取数 | 按目录 | `source.params.path` 绑 `${dir}`；面包屑 `itemClick` → `setValue dir` + `invoke reload { path }` | `breadcrumbs.itemClick` 触发器（payload `{ title, href, index }`；Vuetify 无点击事件） | S |
| 取数 | 懒加载 / 翻页 | `page` / `perPage` / `source`；`cursorPath` + `loadMore` | `cursorPath`、`loadMore`（Vuetify 无 cursor 语义） | M |
| 选择 | 单选 / 多选 | `showSelect` + `selectStrategy`（`single` / `page` / `all`）+ `modelValue` + `returnObject` | 对齐 Vuetify | S |
| 选择 | 范围 | shift 连选，结果仍写 `modelValue` | `rangeSelect`（baseline：Vuetify 无范围选择） | M |
| 选择 | 回传 | `selectionChange` → `{ selected, rows }` | 现有触发器 | — |
| 操作 | 打开 | `rowClick` + `dialog` / `navigate`；`openOn` 控单击/双击 | `openOn`（baseline：Vuetify 无打开语义） | S |
| 操作 | 下载 | `download` 动作（`url` 或 `content` + `filename`） | 新动作（ui-vuetify 注册） | S |
| 操作 | 重命名 / 删除 | `dialog` / `confirm` + `ajax` + `invoke reload` | 无（批量用 `loop`） | S–M |
| 操作 | 上传入口 | `dialog` + `file`（`uploadApi` / `uploadTarget`）+ `invoke reload` | 无 | S |
| 操作 | 路径 / URL 同步 | `navigate`；路由设计未实施 | 动作 `syncQuery` 或宿主路由适配 | M |
| 展示 | 面包屑 / 虚拟 / 图标 | `breadcrumbs` + `table.virtual` / `height`；`columns[].renderer` + `cell` + `icon` | `iconMap`（baseline）或小 renderer | S–M |
| 状态 | 空 / 错 | `loadingSkeleton` / `emptyText` / `noDataText`；`no-data` 插槽 | `no-data` 对齐 Vuetify；`error` 插槽 | S |

**字段命名**：`showSelect` / `selectStrategy` / `modelValue` / `itemValue` / `returnObject` 对齐 `VDataTable`；现有 `selectable` / `rowKey` 保留兼容。`cursorPath` / `rangeSelect` / `openOn` / `iconMap` 进 `vuetify-alignment.baseline.json`（Vuetify 无对应语义）。目录不进字段，走数据域 `dir`。

**Schema 示例**

```json
{
  "type": "page",
  "data": { "dir": "/", "selected": [], "breadcrumbs": [{ "title": "根目录", "href": "/" }] },
  "body": [
    { "type": "breadcrumbs", "props": { "items": "${breadcrumbs}" }, "onEvent": { "itemClick": { "actions": [
      { "type": "setValue", "props": { "target": "dir", "value": "${event.href}" } },
      { "type": "invoke", "props": { "component": "files", "command": "reload", "args": { "path": "${event.href}" } } } ] } } },
    { "type": "table", "id": "files", "props": { "virtual": true, "height": 480, "perPage": 100,
      "showSelect": true, "selectStrategy": "page", "modelValue": "${selected}", "itemValue": "path",
      "returnObject": true, "cursorPath": "next_cursor", "columns": [{ "key": "name", "title": "名称" }] },
      "source": { "method": "GET", "url": "/api/files", "params": { "path": "${dir}" } },
      "onEvent": { "selectionChange": { "actions": [
        { "type": "setValue", "props": { "target": "selected", "value": "${event.selected}" } } ] } } }
  ]
}
```

**结论**：不新建 `file-browser`；上表 15 项按字段/动作补齐。依据：[schema-contract-components.md](schema-contract-components.md#L161-L208)、[catalog.json](../apps/docs/public/catalog.json)。

### T2 上传增强（仅阶段 1）

| 项 | 方案 |
|---|---|
| 状态/依据 | **阶段 1 已实施**（契约 §8.2.1、`PlatformAdapter.upload`、`uploadProgressTarget` / `maxRetries` / `retryDelayMs`、`cancelUpload` / `retryUpload`）；[FileInput.vue](../packages/ui-vuetify/src/form/FileInput.vue)、[fetch.ts](../packages/kernel/src/core/api/fetch.ts) |
| 范围 / 复用 | 整体进度、取消、失败重试；`uploadApi` / `uploadField` / `uploadData` / `uploadTarget` / `accept` / `multiple` 不变；阶段 2 暂缓 |
| 新增字段 | `uploadProgressTarget`（string，未设置）、`maxRetries`（number，`0`）、`retryDelayMs`（number，`1000`）；均 baseline：`VFileInput` 无对应能力 |
| 宿主接口 | `upload?(req)`（下）；`signal` 继承 `ApiRequest`，取消即 abort；缺省回退 `request`，进度 0 / indeterminate |
| 进度 | `file` 写 `{ status, loaded, total, percent, attempt }` 到 `uploadProgressTarget`；`progress` 绑 `${upload.percent}` |
| 取消/重试 | `invoke cancelUpload()` / `retryUpload()`；仅网络错误 / 5xx / 408 / 429 自动重试，4xx 不重试；第 n 次延迟 `retryDelayMs × n`（≤5s） |
| 验收 | 进度单调；取消后无新请求；重试次数准确；未配置新字段时行为不变 |

```ts
interface UploadRequest extends ApiRequest { body: FormData; onProgress?(p: { loaded: number; total: number }): void }
interface PlatformAdapter { upload?(req: UploadRequest): Promise<unknown> }
```

```json
{
  "type": "form",
  "data": { "upload": { "status": "idle", "percent": 0 } },
  "body": [
    { "type": "file", "id": "attachment", "name": "attachment", "props": {
      "label": "附件", "accept": ".zip", "uploadApi": { "method": "POST", "url": "/api/uploads" },
      "uploadProgressTarget": "upload", "maxRetries": 1 } },
    { "type": "progress", "props": { "modelValue": "${upload.percent}", "height": 4 },
      "visibleOn": "${upload.status === 'uploading'}" },
    { "type": "button", "props": { "text": "取消" }, "visibleOn": "${upload.status === 'uploading'}",
      "onEvent": { "click": { "actions": [{ "type": "invoke", "props": { "component": "attachment", "command": "cancelUpload" } }] } } }
  ]
}
```

**阶段 2（暂缓）**：`uploadChunked` / `chunkSize` / `uploadChunkApi` / `uploadCompleteApi` / `uploadAbortApi` / `uploadResumeApi` / `uploadSessionPath` / `uploadOffsetPath` + `uploadChunk` 仅保留设计，服务端协议由宿主定义。**不做**：拖拽排序、服务端分片/会话协议、不改 `uploadApi` 调用方式。

### T3 日志查看：组合示例（不新增 `log-viewer`）

| 项 | 方案 |
|---|---|
| 状态/依据 | 未实现，可组合；[catalog.json](../apps/docs/public/catalog.json)、[design-source-capability.md](design-source-capability.md#L10-L13) |
| 现有字段 | `virtual-list.items` / `itemHeight` / `height`；`input-text` 的 `name` / `placeholder`（值在表单作用域）；数据域 `keyword` / `filteredLines` |
| 跟随 | 数据域 `follow`；新数据调用 `scrollTo({ top: filteredLines.length * itemHeight })`；上滚检测需新增 `virtual-list.scroll` 触点（S）或组合层监听容器 |
| 过滤 | `input-text` 写 `keyword` → 自定义 filter/action 生成 `filteredLines` → 绑定 `items`；命中词用 `VHighlight`（仅搜索词） |
| 升级条件 | 需要行号 gutter、命中计数/上下跳转、环形缓冲 `maxLines`、ANSI、复制全部、10 万行分段/后端搜索中的任意 3 项，才值得升级为组件 |

**结论**：不新增 `log-viewer`；先用组合示例覆盖 1 万行、跟随/暂停、关键字过滤。

### T4 代码高亮（已实现）

| 项 | 方案 |
|---|---|
| 状态/依据 | **已实现**（catalog 1.8.0 / 57 条目）；[code-tokens.ts](../packages/kernel/src/renderer-sdk/code-tokens.ts#L70-L90)、[highlight/src/index.ts](../packages/highlight/src/index.ts)、[CodeViewer.vue](../packages/ui-vuetify/src/data/CodeViewer.vue) |
| 分层 | 内核 token 契约 `{ line, start, end, type }[]` + 纯文本回退；`@budding/highlight` 提供 Prism / Shiki 与 Monaco / CodeMirror adapter；`code-viewer` 消费 tokens 或本地分词 |
| 查看 / 编辑 | `code-viewer` 的 `readonly` 控制两种模式；字段 `tokens` / `adapter` / `theme` / `language` / `lineNumbers` / `wrap` / `copyable` |
| 适配 | Prism 默认（无 WASM）；Shiki 可选；Monaco / CodeMirror adapter 在 Pack 内，编辑器本体由宿主提供 |
| 验收 | 未装 Pack 纯文本；装 Pack 高亮；只读 / 编辑均可用；`VHighlight` 仅搜索词 |

### T5 终端渲染器

| 项 | 方案 |
|---|---|
| 状态/依据 | **已实现**（catalog 1.8.0 / 57 条目；契约 §22、组件契约 §11.6）；[terminal/src/index.ts](../packages/terminal/src/index.ts)、[catalog.json](../apps/docs/public/catalog.json) |
| 对标 | `VSheet` / `VCard` 容器 + `VProgressLinear` / `VBtn` 状态 + xterm（非 Vuetify） |
| Budding | 独立按需 Pack `@budding/terminal`（裸名 `terminal`，`meta.owner = '@budding/terminal'`，首版 experimental）；`@xterm/xterm` / `@xterm/addon-fit` 为可选 peer，动态 import，未安装时明确降级；第三方仍可用 `vendor:type` 扩展 |
| 字段/动作 | `wsUrl` / `height` / `fontSize` / `theme` / `readOnly` / `reconnect` / `reconnectDelay` / `reconnectLimit` / `resizeFormat` / `ariaLabel` / `emptyText`（baseline）；triggers `connected` / `disconnected` / `reconnecting` / `exit` / `error`；commands `reconnect` / `disconnect` / `focus`；票据走页面 `source` → `terminalUrl`，鉴权头 / 代理由 `registerTerminalTransport` 注入 |
| 验收 | mock WS / ResizeObserver；尺寸变化发正确 cols/rows；无 iframe / worker / wasm；卸载后监听与定时器归零；未安装 xterm 时渲染降级提示并上报 error |
| 示例 | `{"type":"terminal","props":{"wsUrl":"${terminalUrl}","fontSize":14,"resizeFormat":"json","reconnectLimit":5}}` |

### P3 按需 Pack 独立分发产物（实施中）

| 项 | 方案 |
|---|---|
| 状态/依据 | **实施中**；[package.json](../package.json)（`build:packs`）、[embed.packs.ts](../packages/kernel/src/embed.packs.ts)、[build-packs.mjs](../scripts/build-packs.mjs)、[embed-groups.ts](../scripts/lib/embed-groups.ts) |
| 产物 | `dist/packs/<pack>.mjs` + `<pack>.external.mjs`；单文件 ESM，SFC 样式内联 |
| 共享作用域 | 两种变体都不内联 `vue` 与 `@budding/kernel/renderer-sdk`，避免重复实例 |
| 用途差异 | 自包含版内联 Pack 自身依赖（echarts / xterm / prism），拿来即用；external 版把 Pack 依赖也外置，由宿主提供，体积小 |
| 注册 | `BuddingSDK.registerPack()`；打包版与装载器版同语义；`embed()` 加载后注册 renderers 并调用 Pack 的 `setup(runtime)`；契约拟新增 §17.5 |
| 已完成 | `embed.packs.ts`；`embed.ts` / `embed.bundled.ts` 注册入口；`build-embed.mjs` 不再硬编码分组，默认由 [embed-groups.ts](../scripts/lib/embed-groups.ts) 的 `parseEmbedGroups()` 决定（`vite.bundle.config.ts` 消费） |
| 未完成 | `verify:packs` 浏览器验收；契约 §17.5 与 `docs/pack-artifacts.md`；全量门禁 |
| 验收 | 注册后可用；无重复 Vue / 内核实例；自包含可独立跑；external 缺依赖时明确报错；体积与记录一致 |

| Pack | 自包含 | gzip | external | gzip |
|---|---|---|---|---|
| `echarts` | 817.6 KB | 233.5 KB | 13.6 KB | 5.5 KB |
| `terminal` | 384.1 KB | 81.3 KB | 18.0 KB | 6.6 KB |
| `highlight` | 53.2 KB | 18.9 KB | 8.1 KB | 3.0 KB |

## 3. D1–D8 决策清单

| 编号 | 问题 | 结论与建议 |
|---|---|---|
| D1 | `virtual-list` 命名 | **已定**：保留 `virtual-list`，不改 `virtual-scroll`（语义命名惯例、能力边界是列表、改名破坏性变更）；契约 §10.9 已加命名说明 |
| D2 | 是否新建文件/日志/代码组件 | **已定**：不新建文件/日志容器；T1 补字段/动作，T3 组合；T4 走 `@budding/highlight` + `code-viewer`；`virtual-list` 已在内置 `data` 组 |
| D3 | T2 范围 | **阶段 1 已实施**（进度/取消/重试）；阶段 2 分片/续传暂缓，待宿主协议 |
| D4 | T5 归属 | **已定（本轮调整）**：改为官方第一方独立按需 Pack `@budding/terminal`（方案 B：xterm peer，不内置）；下游仍可用 `vendor:type` 扩展 |
| D5 | P1/P2 时机 | **已完成（方案 C）**：实测矩阵 + 文档 + `pnpm verify:csp` + CI 门禁 |
| D6 | catalog 口径 | 已更新为 **1.8.0 / 57 条目**（含 `virtual-list`、`code-viewer`、`terminal`）；后续由实现侧同步 |
| D7 | 宿主函数注入 | 取数走 `createBudding({ platform })` 的 `PlatformAdapter`；动作副作用走 `instanceCapabilities` / `customActions`；都不写进 Schema |
| D8 | 高亮 Pack 结论 | **已定**：包名 `@budding/highlight`；Prism 内置为默认适配；不新增 `highlight` 类型，`code-viewer` 进公开 catalog |

## 4. 边界与替代方案

| 边界 | 替代实现与取舍（一行） |
|---|---|
| Vuetify 组件只包装 | `virtual-list` 包一层，内部复用 `VVirtualScroll`；代价：多一层组件 |
| Schema 只传数据 | 声明式 `source` + `PlatformAdapter`；票据先写入数据域；代价：宿主需实现适配器 |
| T5 官方 Pack | `@budding/terminal`：xterm 可选 peer，首版 experimental；第三方仍可 `vendor:type`；代价：官方维护 adapter / WS 生命周期 |
| T4 高亮 | 内核 token 契约 + `@budding/highlight` + `code-viewer`；代价：多一个按需包 |
| 文件浏览用能力补齐 | 按 T1 表补字段/动作，展示复用 `table` + `breadcrumbs`；代价：大目录逐层进入 |
| 日志用组合实现 | `virtual-list` + 过滤 + 跟随；代价：行号/命中计数/复制全部需自写 |

## 5. 执行顺序

**已完成**：P1/P2（方案 C）、T0、T2 阶段 1、T4、T5；**P3 实施中**。

| 顺序 | 剩余任务 | 说明 |
|---|---|---|
| 1 | T1 文件浏览 | 契约 §8.2 / §10 + 单测；懒加载依赖 T0 |
| 2 | T3 日志组合 | playground 1 万行 demo；验证是否需升级组件 |
| 3 | T2 阶段 2 | 仅宿主协议明确后启动 |
| 4 | P3 收尾 | `verify:packs` 浏览器验收 → 契约 §17.5 + `docs/pack-artifacts.md` → 全量门禁 |

## 6. 缺口清单

| 项 | 缺什么 / 阻塞谁 | 下一步 |
|---|---|---|
| T1 | 契约 §8.2 / §10 未改、无测试 | 先落字段与单测 |
| T3 | 无可运行示例 | playground 1 万行 demo |
| T2 阶段 2 / D3 | 宿主协议未定（暂缓） | 阶段 1 后再评估 |
| T1 路由同步 | URL / 查询参数与路径同步未定 | 先定 `syncQuery` 或宿主路由适配 |
| P3 | `verify:packs` 浏览器验收、契约 §17.5、`docs/pack-artifacts.md`、全量门禁未完成 | 按执行顺序收尾 |

D1–D8 均已定或已落地；剩余未定项仅为 T1 路由同步，其余为实施缺口。

## 7. 依据索引

- [catalog.json](../apps/docs/public/catalog.json)：1.8.0、57 条目（含 `virtual-list`、`code-viewer`、`terminal`）；[components.md](components.md)（CSP）、[architecture.md](architecture.md)（安全/缓存/无沙箱）。
- [schema-contract.md](schema-contract.md) §19.4（`invoke`）、§15（实例选项）；[schema-contract-components.md](schema-contract-components.md) §10（table）；[design-source-capability.md](design-source-capability.md)（`source` 只接受 API 配置）。
- [node-behavior.ts](../packages/kernel/src/runtime/node-behavior.ts)、[types.ts](../packages/kernel/src/renderer-sdk/types.ts)（`commands`）、[tokens.ts](../packages/kernel/src/renderer-sdk/tokens.ts)（`PlatformAdapter`）、[sandbox.ts](../packages/kernel/src/core/expression/sandbox.ts)。
- [FileInput.vue](../packages/ui-vuetify/src/form/FileInput.vue) / [form/index.ts](../packages/ui-vuetify/src/form/index.ts)：上传现状；[TableBlock.vue](../packages/ui-vuetify/src/data/TableBlock.vue)：表格虚拟/选择/空态现状。
- [vuetify-alignment.baseline.json](../packages/ui-vuetify/src/fields/vuetify-alignment.baseline.json) / [vuetify-alignment.test.ts](../packages/ui-vuetify/src/fields/vuetify-alignment.test.ts)：字段对齐门禁。
- [echarts/index.ts](../packages/echarts/src/index.ts)：独立 Pack 先例；[highlight/src/index.ts](../packages/highlight/src/index.ts)、[terminal/src/index.ts](../packages/terminal/src/index.ts)：第一方按需 Pack。
- [build-packs.mjs](../scripts/build-packs.mjs)、[embed.packs.ts](../packages/kernel/src/embed.packs.ts)、[embed-groups.ts](../scripts/lib/embed-groups.ts)：Pack 分发产物与分组解析。
- [code-tokens.ts](../packages/kernel/src/renderer-sdk/code-tokens.ts)、[CodeViewer.vue](../packages/ui-vuetify/src/data/CodeViewer.vue)：token 契约与只读/编辑查看器。
- [VVirtualScroll.d.ts](../node_modules/.pnpm/vuetify@4.3.3_typescript@5.9.3_vue@3.5.44_typescript@5.9.3_/node_modules/vuetify/lib/components/VVirtualScroll/VVirtualScroll.d.ts)、[VDataTable.d.ts](../node_modules/.pnpm/vuetify@4.3.3_typescript@5.9.3_vue@3.5.44_typescript@5.9.3_/node_modules/vuetify/lib/components/VDataTable/VDataTable.d.ts)：props / slot / expose。
- [Breadcrumbs.vue](../packages/ui-vuetify/src/basic/Breadcrumbs.vue)：items 仅支持 `title` / `href`，当前无点击触发器。

## 附录：决策落地记录

| 条目 | 决策 | 落地 |
|---|---|---|
| P1/P2 | 方案 C：实测 + 文档 + 门禁 | scripts/verify-csp.mjs、pnpm verify:csp 接 CI；矩阵 	ests/artifacts/csp/matrix.md |
| T4 | 内核 token 契约 + @budding/highlight + code-viewer（只读/编辑，含 Monaco/CM adapter） | packages/highlight、packages/ui-vuetify/src/data/CodeViewer.vue；catalog 1.8.0 / 57 |
| T5 | 方案 B：官方第一方 Pack，xterm 可选 peer | packages/terminal、@budding/terminal；首版 experimental |

历史决策材料已按结果收敛，复盘可查 git 历史。
