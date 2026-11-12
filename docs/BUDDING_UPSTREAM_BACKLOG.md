# budding 上游能力任务清单（文件管理 / 上传 / 日志 / 代码 / 终端）

> 状态：**外部协作清单，不代表本仓库实施**。本文档交给 budding 团队按序实现；Skiff 侧只读消费，不修改 budding 仓库。
> 事实来源：budding 1.6.0 产物与源码只读盘点（2026-11-12）；Skiff 侧需求见 [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md) §6。
> 标注约定：**待实测** = 尚未验证的数字或行为；**推断，需核实** = 依据现有信息推断、执行前需在仓库内确认。

---

## 0. 一页说明

### 0.1 这份清单给谁、怎么用

- **读者**：budding 团队及其 AI 执行者。每条任务都是一段可直接粘贴的提示词。
- **顺序**：先做前置文档项（P1、P2），再按 T0 → T1 → T2 → T3 → T4 → T5 推进；T0 是 T1/T3/T4 的硬前置。
- **产出形态**分三类，每条任务开头注明：
  - **框架能力补齐**：修改 budding 仓库的组件/渲染器基础设施；
  - **官方 advanced 渲染器**：在 budding 内新增或扩展渲染器；
  - **仅文档约定**：只产出规范与示例，不改运行时行为。
- **名称规则**：新增渲染器/类型一律遵循 `vendor:type`，本清单统一使用 `budding:*`；注册位置沿用现有 `builtin-groups` 机制（具体文件以仓库为准，**需核实**）。

### 0.2 任务总览

| 编号 | 任务 | 产出形态 | 依赖 | Skiff 用途 |
|---|---|---|---|---|
| P1 | `connect-src` 对 ws/wss 的官方结论与终端 CSP 指引 | 仅文档约定 | 无 | 终端页在严格 CSP 下可用 |
| P2 | 渲染器内资源加载的 MIME / 缓存 / 同源约定 | 仅文档约定 | 无 | 字体、图标、xterm 资源加载不触发 CSP 违规 |
| T0 | `VVirtualScroll` 暴露 `scrollToIndex` / `scrollTo` 等实例方法 | 框架能力补齐 | 无 | 日志尾部跟随、代码跳行、文件列表回顶 |
| T1 | 官方「文件浏览器」渲染器（面包屑 + 平铺列表，禁止树形） | 官方 advanced 渲染器 | T0 | 文件管理页主视图 |
| T2 | 上传渲染器 MVP（单文件、固定分片、进度、取消、重试一次） | 官方 advanced 渲染器 | 无（可与 T1 并行） | 文件上传 |
| T3 | 日志查看器（虚拟滚动、行号、跟随、搜索、全量复制） | 官方 advanced 渲染器 | T0 | 站点/Nginx/作业日志 |
| T4 | 代码查看器（只读、行号、纯 JS 高亮、大文件分段） | 官方 advanced 渲染器 | T0 | 文本文件查看 |
| T5 | 终端渲染器（xterm 内联、ResizeObserver、WS 生命周期） | 官方 advanced 渲染器 | P1 | Docker 容器终端 |

### 0.3 统一约束（所有任务都适用）

1. **不破坏现有契约**：现有 54 个渲染器、schema 字段、`embed(target, schema, opts)` 返回值 `{ manifest?, unmount }` 必须保持可用；不得引入 breaking change，字段只增不改。
2. **必须补测试**：每个任务都要有单元测试；涉及浏览器 API（ResizeObserver、WebSocket、AbortController）时用可注入的 mock，测试环境沿用上游现有测试栈。
3. **离线可用**：运行时不依赖 CDN、外网字体或远程图标；所有资源随包分发。
4. **禁止 `eval` / `new Function`**：必须满足官方 CSP 建议 `script-src 'self'`；动态 `import()` 无法携带 nonce、SRI 不生效，不得依赖它们做安全边界（**推断，需核实**上游对动态 import 的实际策略）。
5. **无沙箱假设**：渲染器是应用树内的普通 Vue 组件，从 `__BUDDING_SHARED__['vue']` / `['vuetify/components']` 取用；不要依赖 iframe 隔离来实现功能。
6. **无实例句柄**：不要假设调用方能拿到组件实例；需要程序化控制时，通过受控 props/事件或 `runtime`（自定义动作第三参）完成。
7. **类型命名**：遵循 `vendor:type`，如 `budding:file-browser`、`budding:upload`、`budding:log-viewer`、`budding:code-viewer`、`budding:terminal`。

---

## P1 `connect-src` 对 ws/wss 的官方结论与终端 CSP 指引

- **产出形态**：仅文档约定
- **依赖**：无
- **为什么先做**：Skiff 的终端必须走 WebSocket；若官方不能明确 `connect-src 'self'` 是否覆盖 `ws://` 与 `wss://`，终端任务会在联调阶段才发现被 CSP 拦截。

### 可粘贴提示词

```text
[任务]
给出 budding 渲染器在严格 CSP 下使用 WebSocket 的官方结论，并写入文档与一个最小示例。不要修改运行时行为。

[背景事实]
1. budding 官方 CSP 建议为 script-src 'self'，不使用 eval / new Function；动态 import 无法携带 nonce，SRI 不生效。
2. budding 全仓目前没有 EventSource / WebSocket / ReadableStream 的封装；但渲染器内可以直接使用原生 API。
3. Skiff 的终端方案是「一次性票据 + 同源 WebSocket」，需要明确 connect-src 的写法。

[要产出]
1. 一份文档章节，明确以下矩阵，并给出每个结论的依据（浏览器规范或实测）：
   - connect-src 'self' 是否允许同源 ws:// 与 wss://；
   - 同源不同端口（如 ws://host:8080）是否仍算 'self'；
   - 显式写 ws://host、wss://host 时的写法与通配限制；
   - https 页面连接 ws:// 是否被浏览器拦截。
2. 一个最小可运行示例：终端渲染器只建立连接、收发文本、显示连接状态，不实现 xterm。
3. 示例页在严格 CSP 下运行，控制台零违规的记录方式（截图或日志）。

[验收标准]
- 文档矩阵覆盖 http 页面 / https 页面 × ws / wss × 同源同端口 / 同源不同端口 / 跨源，共 9 种组合中的每一种都有明确结论。
- 示例在推荐 CSP 下可连接、可收发、可关闭；被拦截的组合给出预期报错。
- 文档明确写出 Skiff 侧应使用的 connect-src 值，并标注「待实测」项。

[明确不做]
- 不实现终端 UI、重连策略、resize；这些属于 T5。
- 不修改全局 CSP 默认值。
- 不引入第三方 WebSocket 库。

[约束]
- 不引入 eval / new Function。
- 文档与示例随仓库离线可用。
- 结论中凡未实测的组合必须标注「待实测」，不得写成既定事实。
```

---

## P2 渲染器内资源加载的 MIME / 缓存 / 同源约定

- **产出形态**：仅文档约定
- **依赖**：无
- **为什么先做**：日志、代码、终端渲染器要加载自身 CSS、字体、worker 或静态资源；Skiff 的 CSP 严格，MIME 与路径约定不清会直接表现为白屏或字体缺失。

### 可粘贴提示词

```text
[任务]
定义 budding 渲染器加载自身静态资源（CSS、字体、图标、worker、wasm 若未来引入）的官方约定，并给出一个符合约定的示例。不要修改运行时默认行为。

[背景事实]
1. budding 渲染器是应用树内普通 Vue 组件，无沙箱。
2. Skiff 的全局 CSP 为：default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'；font-src、worker-src、wasm 相关指令当前未显式声明。
3. 运行时会按 document.currentScript 推导目录并插入 CSS <link>；查询串可能被丢弃（推断，需核实）。

[要产出]
1. 一份约定文档，至少覆盖：
   - 资源必须使用与渲染器同源的相对路径，禁止 CDN 与绝对外链；
   - 各类资源的推荐 MIME：.js / .css / .woff2 / .svg / .json；
   - 缓存建议：版本化文件名 + 长缓存；非版本化文件 no-cache；
   - 字体优先使用独立文件而不是 data URI；若必须 data URI，明确需要 font-src 'self' data:；
   - worker 与 wasm 的默认态度：默认不使用；确需使用时列出最小 CSP 增量（worker-src 'self' blob: / script-src 'self' 'wasm-unsafe-eval'）。
2. 一个示例渲染器，加载同源 CSS 与字体，在严格 CSP 下正常渲染。
3. 一份「常见违规 → 现象 → 最小修复」对照表。

[验收标准]
- 示例在 default-src 'self' 下加载成功，控制台零违规。
- 文档明确回答：字体用 data URI 时是否必须加 font-src；worker 与 wasm 是否默认禁止。
- 对照表至少覆盖字体、图片、worker、wasm、样式内联五类。

[明确不做]
- 不修改 Skiff 的 CSP；只给出建议值。
- 不引入外部字体服务。
- 不实现具体业务渲染器。

[约束]
- 不引入 eval / new Function。
- 文档与示例离线可用。
- 未实测的结论标注「待实测」。
```

---

## T0 `VVirtualScroll` 暴露 `scrollToIndex` / `scrollTo` 等实例方法

- **产出形态**：框架能力补齐
- **依赖**：无（T1/T3/T4 的硬前置）
- **现状事实**：`VVirtualScroll` 已支持动态行高（每项通过 `onUpdate:height` 回报，`virtual.js` 重算 offsets），但**组件未 `expose()`**，模板 ref 拿不到 `scrollToIndex`，因此无法实现「日志尾部跟随」「跳到第 N 行」「文件列表回顶」。

### 可粘贴提示词

```text
[任务]
为 budding 内置的 VVirtualScroll（Vuetify 4.3.3，组件已内联进 bundle）补充程序化滚动能力，且不破坏任何现有用法。

[背景事实]
1. 渲染器从 __BUDDING_SHARED__['vue'] / ['vuetify/components'] 取用组件，是应用树内普通 Vue 组件，无沙箱。
2. 当前 VVirtualScroll 支持动态行高：子项通过 onUpdate:height 回报高度，virtual.js 负责重算 offsets。
3. 组件没有 expose()，所以父组件通过模板 ref 无法调用任何滚动方法。
4. 现有 54 个渲染器可能已经在使用 VVirtualScroll，必须保持行为不变。

[要产出]
1. 在 VVirtualScroll 上通过 defineExpose 暴露以下方法（名称固定）：
   - scrollToIndex(index: number, opts?: { align?: 'start' | 'center' | 'end' | 'nearest'; behavior?: 'auto' | 'smooth' }): void
   - scrollTo(offset: { top?: number; left?: number; behavior?: 'auto' | 'smooth' }): void
   - scrollToTop(opts?: { behavior?: 'auto' | 'smooth' }): void
   - getScrollTop(): number
2. 行为要求：
   - index 越界时钳制到 [0, itemCount-1]，不抛异常；
   - align 为 'nearest' 时，目标已在可视区则不滚动；
   - 动态行高下以当前 offsets 计算目标位置；数据量变化后再次调用仍正确；
   - 平滑滚动沿用浏览器原生 behavior，不自定义动画。
3. 类型声明：补充到组件导出类型中，调用方在 TypeScript 下可直接使用。
4. 一个演示页：1 万条可变行高数据，按钮触发 scrollToIndex(0 / 5000 / 9999) 与 scrollToTop。

[验收标准]
- 新增单元测试覆盖：越界钳制、四种 align、动态高度、数据量增加后调用、空列表调用。
- 现有 VVirtualScroll 相关测试全部通过，既有渲染器不需要改代码。
- 演示页中三个跳转目标均能到达且无跳变异常；性能数字标注「待实测」。
- 不新增必需 props，不改变现有事件名与插槽。

[明确不做]
- 不重写虚拟滚动算法，不改变行高估算策略。
- 不暴露内部 offsets 数组或 DOM 节点。
- 不新增额外依赖。

[约束]
- 向后兼容：只增不改。
- 必须补单元测试。
- 不引入 eval / new Function；离线可用。
```

---

## T1 官方「文件浏览器」渲染器（面包屑 + 平铺列表，禁止树形）

- **产出形态**：官方 advanced 渲染器（`budding:file-browser`）
- **依赖**：T0
- **关键决定**：**明确禁止树形**。原因：budding 无独立 tree 组件；大目录下懒加载树会放大请求数量；平铺列表 + 面包屑 + 虚拟滚动在实现与性能上更可控。

### 可粘贴提示词

```text
[任务]
在 budding 中新增官方文件浏览器渲染器 budding:file-browser，用于浏览目录与进入子目录。只做浏览，不做上传/删除/重命名/拖拽。

[背景事实]
1. 可用组件：VBreadcrumbs、VSheet、VToolbar、VTextField、VDataTable、VDataTableVirtual、VEmptyState、VSkeletonLoader；VVirtualScroll 由 T0 提供 scrollToIndex/scrollToTop。
2. budding 无独立 tree / tree-select 组件；本任务明确不引入树形。
3. 大目录场景需要游标分页与虚拟滚动，不能用一次性全量渲染。

[要产出]
1. 渲染器 budding:file-browser，注册到 builtin-groups（具体文件以仓库为准，需核实）。
2. 数据契约（props）：
   - initialPath: string，默认 '/'
   - source: 由宿主提供的取数函数，签名 load({ path, cursor, sort, showHidden }) => Promise<{ items: Entry[]; next_cursor: string | null }>
   - Entry = { name: string; is_dir: boolean; size: number; mtime: string; kind?: string }
   - sort: { key: 'name' | 'size' | 'mtime'; dir: 'asc' | 'desc' }，默认 { key: 'name', dir: 'asc' }
   - showHidden: boolean，默认 false
   - selectionMode: 'none' | 'single' | 'multi'，默认 'single'
3. 布局与交互：
   - 顶部 VBreadcrumbs 展示当前路径，点击任意层级导航；
   - 主体为平铺列表（禁止树形），列：名称、大小、修改时间、类型；
   - 双击目录进入；双击文件触发 open 事件；
   - 排序切换会重置游标并重新加载；
   - 目录优先排序，同级按当前 sort 排序；
   - 空目录显示 VEmptyState；加载中显示 VSkeletonLoader；错误显示可重试状态；
   - 列表使用虚拟滚动，滚动到底部自动加载 next_cursor；
   - 切换路径或排序时清空选择。
4. 事件：navigate({ path })、open({ entry })、selectionChange({ entries })、refresh()。
5. 状态机：loading | ready | empty | error；error 状态必须保留重试入口。

[验收标准]
- 单元测试覆盖：路径切换重置游标、排序切换重置游标、空目录、错误重试、目录优先排序、选择在导航后清空。
- 1 万条目下滚动流畅且只渲染可视窗口（用 mock 高度断言渲染数量远小于总数，具体数字待实测）。
- 面包屑点击与双击进入均触发正确事件，不直接修改内部状态之外的宿主数据。
- 空态、加载态、错误态三态可分别用测试夹具复现。

[明确不做]
- 不做树形视图、不做拖拽、不做右键菜单。
- 不做上传、删除、重命名、移动、复制、下载。
- 不做多标签页或多窗格。

[约束]
- 不破坏现有渲染器与 schema 契约；新增字段只增不改。
- 必须补单元测试；离线可用；不引入 eval / new Function。
```

---

## T2 上传渲染器 MVP（单文件、固定分片、进度、取消、失败重试一次）

- **产出形态**：官方 advanced 渲染器（`budding:upload`）
- **依赖**：无（可与 T1 并行）
- **现状事实**：budding 的 `file` 渲染器（`FileInput.vue`）只有布尔 `loading`，`FormData` 单次请求，**无分片、无进度、无取消、无 `onEvent` 触点**。

### 可粘贴提示词

```text
[任务]
在 budding 中新增上传渲染器 budding:upload，提供单文件分片上传的 MVP 能力。

[背景事实]
1. 现有 FileInput.vue 的 file 渲染器只有布尔 loading，FormData 单次 POST，没有分片、进度、取消或事件钩子。
2. platform.request 只把响应读成 text 再 parse，不暴露流；因此进度必须来自上传适配器本身，而不是响应流。
3. Skiff 服务端使用 offset 分片协议，本渲染器必须允许宿主注入会话/分片/完成/中止四类适配函数。

[要产出]
1. 渲染器 budding:upload，注册到 builtin-groups（具体文件以仓库为准，需核实）。
2. Props：
   - accept?: string
   - maxSizeBytes?: number
   - chunkSize: number（可配置；默认值与取值理由写入文档，待实测）
   - session?: (file: File) => Promise<{ uploadId: string; chunkSize?: number }>
   - uploadChunk: (args: { uploadId: string; offset: number; chunk: Blob; signal: AbortSignal }) => Promise<{ received: number }>
   - complete?: (args: { uploadId: string }) => Promise<void>
   - abort?: (args: { uploadId: string }) => Promise<void>
3. 默认适配：未提供 session/uploadChunk 时，退化为单次 FormData POST，保持与现有 file 渲染器相同的兼容行为。
4. 行为：
   - 顺序发送分片，offset 单调递增；
   - 进度事件 progress({ loaded, total, percent })；
   - 取消使用 AbortController，取消后不得再发送新分片；
   - 网络错误或 5xx 自动重试一次，4xx 不重试；
   - 成功后触发 success，失败触发 error，取消触发 cancelled；
   - 取消或失败时若提供 abort 适配器，必须调用它。
5. 状态机：idle → preparing → uploading → retrying → success | error | cancelled。

[验收标准]
- 用 mock 适配器断言分片顺序、offset 连续性、总分片数。
- 进度单调不减，百分比的最终值为 100（允许浮点误差）。
- 5xx 时恰好重试一次；4xx 时不重试。
- 取消后不再调用 uploadChunk，且调用一次 abort。
- 不提供分片适配器时，单次 POST 路径与现有行为一致。

[明确不做]
- 不做跨会话续传、暂停/恢复。
- 不做并发上传（并发数固定为 1）。
- 不做目录上传、拖拽上传、多文件队列。
- 不做压缩、加密、秒传。

[约束]
- 不破坏现有 file 渲染器；新增事件只增不改。
- 必须补单元测试；离线可用；不引入 eval / new Function。
```

---

## T3 日志查看器（虚拟滚动、行号、跟随、搜索、全量复制）

- **产出形态**：官方 advanced 渲染器（`budding:log-viewer`）
- **依赖**：T0
- **现状事实**：`VCode` 只是 `createSimpleFunctional('v-code','code')`，无高亮、无行号、无滚动；`VHighlight` 只做关键词高亮。

### 可粘贴提示词

```text
[任务]
在 budding 中新增日志查看器渲染器 budding:log-viewer，用于查看大体积纯文本日志。

[背景事实]
1. VVirtualScroll 支持动态行高，但本任务使用定高行以降低复杂度；T0 已提供 scrollToIndex 与 scrollTo。
2. VCode 无行号、无滚动、无高亮能力；VHighlight 只做关键词高亮。
3. Skiff 的日志是纯文本，不含 ANSI 控制序列（终端颜色属于 T5）。

[要产出]
1. 渲染器 budding:log-viewer，注册到 builtin-groups（具体文件以仓库为准，需核实）。
2. Props：
   - lines: string[] 或 source: (args: { cursor?: string; limit: number }) => Promise<{ lines: string[]; next_cursor?: string | null }>，二选一
   - rowHeight: number（可配置，默认值写入文档，待实测）
   - follow: boolean，默认 true
   - wrap: boolean，默认 false
   - maxLines?: number（环形缓冲上限，超出丢弃最旧行）
3. 行为：
   - 定高行虚拟滚动，左侧行号 gutter；
   - follow=true 且处于底部时，新行到达自动滚到底；
   - 用户向上滚动时暂停跟随，并显示「跳到最新」按钮；点击后恢复跟随；
   - 搜索框输入关键词后，用 VHighlight 标记命中；显示命中计数与上一个/下一个跳转；
   - 「复制全部」从数据模型拼接完整文本写入剪贴板，不依赖 DOM 选区。
4. 明确记录虚拟化限制：浏览器原生「全选/复制」只能拿到可视窗口附近的 DOM 行（约 ±100px），因此必须提供基于数据模型的复制全部。

[验收标准]
- 10 万行夹具下首屏可交互（具体耗时待实测），滚动过程无卡顿异常。
- 跟随行为可测：底部时新行到达自动到底；上滚后不自动滚动；点击「跳到最新」恢复。
- 搜索命中计数与高亮数量一致；上一个/下一个能定位到命中行。
- 「复制全部」的内容包含可视窗口之外的行，且行序与原数据一致。
- 行号与内容严格对齐；wrap=false 时超长行水平滚动。

[明确不做]
- 不做 ANSI 颜色解析、不做时间戳解析、不做日志级别过滤。
- 不做文件下载、不做日志搜索后端、不做正则搜索（MVP 只做字面量）。
- 不做 wrap=true 的复杂选区复制（可后续单独立项）。

[约束]
- 不破坏现有渲染器与 schema 契约。
- 必须补单元测试；离线可用；不引入 eval / new Function。
```

---

## T4 代码查看器（只读、行号、纯 JS 高亮、大文件分段）

- **产出形态**：官方 advanced 渲染器（`budding:code-viewer`）
- **依赖**：T0
- **关键约束**：**不得依赖 WASM**，避免引入 `wasm-unsafe-eval`；高亮必须是纯 JavaScript 实现。

### 可粘贴提示词

```text
[任务]
在 budding 中新增只读代码查看器渲染器 budding:code-viewer。

[背景事实]
1. VCode 只是 createSimpleFunctional('v-code','code')，没有高亮、行号或滚动能力。
2. Skiff 的 CSP 保持严格，不接受 wasm-unsafe-eval；因此不得使用 Shiki/Oniguruma 之类的 WASM 高亮方案。
3. 大文件不能一次性全部渲染，需要按行区间分段加载。

[要产出]
1. 渲染器 budding:code-viewer，注册到 builtin-groups（具体文件以仓库为准，需核实）。
2. Props：
   - text?: string 或 source: (args: { start_line: number; end_line: number }) => Promise<{ lines: string[]; total_lines: number }>
   - language: 'text' | 'javascript' | 'typescript' | 'json' | 'shell'，默认 'text'
   - startLine: number，默认 1
   - showLineNumbers: boolean，默认 true
   - wrap: boolean，默认 false
   - segmentLines: number（分段阈值，默认值写入文档，待实测）
3. 行为：
   - 只读，不可编辑；
   - 行号与内容对齐；
   - 纯 JS 词法高亮，MVP 至少识别：关键字、字符串、注释、数字、布尔/null；
   - 高亮必须使用文本节点或安全转义，禁止把未转义内容塞进 innerHTML；
   - 大文件按 segmentLines 分段请求/渲染，滚动到边界时加载下一段；
   - 提供 scrollToLine(line) 受控方法或事件入口，内部使用 T0 的 scrollToIndex。
4. 状态机：loading | ready | error。

[验收标准]
- 夹具包含 `<script>alert(1)</script>` 与 `</code>`，渲染后必须显示为文本，不得执行或破坏结构。
- 行号计数与 total_lines 一致；滚动到分段边界能加载下一段且不重复。
- 关键词、字符串、注释、数字各有至少一条高亮断言。
- 不加载任何 .wasm 文件；产物中检索 `WebAssembly` 与 `wasm` 无命中。
- 10 万行分段夹具可滚动浏览（性能数字待实测）。

[明确不做]
- 不做编辑、保存、Diff、LSP、跳转定义。
- 不做完整语言语法树，只做词法级高亮。
- 不做代码折叠、不做 minimap。
- 不引入 WASM 或远程高亮服务。

[约束]
- 不破坏现有渲染器与 schema 契约。
- 必须补单元测试；离线可用；不引入 eval / new Function。
```

---

## T5 终端渲染器（xterm 内联、ResizeObserver、WS 生命周期）

- **产出形态**：官方 advanced 渲染器（`budding:terminal`）
- **依赖**：P1
- **关键约束**：**必须内联挂载 xterm，禁止 iframe**。

### 可粘贴提示词

```text
[任务]
在 budding 中新增终端渲染器 budding:terminal，将 xterm 直接挂载在渲染器容器内，不使用 iframe。

[背景事实]
1. budding 渲染器是应用树内普通 Vue 组件，无沙箱；因此可以直接操作 DOM 与原生 WebSocket。
2. 渲染器内可直接使用原生 API；budding 目前没有 WebSocket / ResizeObserver 封装，需自行实现并清理。
3. Skiff 的终端是「一次性票据 + 同源 WebSocket」，连接地址由宿主传入。
4. CSP 的 connect-src 结论见 P1，本任务依赖该结论。

[要产出]
1. 渲染器 budding:terminal，注册到 builtin-groups（具体文件以仓库为准，需核实）。
2. Props：
   - connect: () => WebSocket 或 wsUrl: string，二选一
   - fontSize: number，默认 14
   - theme?: Record<string, string>
   - reconnect?: { maxAttempts: number; baseDelayMs: number }
   - readOnly?: boolean，默认 false
3. 行为：
   - 在容器内创建并挂载 xterm，不使用 iframe；
   - ResizeObserver 监听容器尺寸，配合 FitAddon 计算 cols/rows；尺寸变化时通过 WS 发送 resize 消息（消息格式由 props 回调或约定给定，需在文档写明）；
   - 自动聚焦，并在点击容器时恢复焦点；
   - WS 生命周期：open → 数据双向转发 → close/error 时更新状态；
   - 重连：仅在非用户主动关闭时重连，指数退避，次数上限由 reconnect.maxAttempts 控制；
   - 卸载时：关闭 WS、断开并 disconnect ResizeObserver、dispose xterm、移除所有事件监听。
4. 事件：connected、disconnected、reconnecting({ attempt })、exit({ code })、error。
5. 状态：idle | connecting | connected | reconnecting | closed | error。

[验收标准]
- 用 mock WebSocket 与 mock ResizeObserver 断言：挂载后发起连接；尺寸变化发送正确的 cols/rows；卸载后 socket.close 被调用且监听器数量归零。
- 服务端主动断开时按退避重连，达到上限后进入 closed；用户主动关闭后不再重连。
- 容器内不存在 iframe 元素。
- 在 P1 推荐的 CSP 下可连接；不得要求放宽 script-src。

[明确不做]
- 不做 SSH 客户端、不做多路复用、不做文件传输（sz/rz）。
- 不做多标签终端、不做分屏。
- 不做终端录制与回放。
- 不引入第三方终端库（除 xterm 本身）。

[约束]
- 不破坏现有渲染器与 schema 契约。
- 必须补单元测试；离线可用；不引入 eval / new Function。
```

---

## 执行顺序与依赖说明

1. **P1、P2 先出文档**：不阻塞编码，但决定 T5 能否联调、以及字体/资源类问题是否会误判为框架缺陷。
2. **T0 必须最先完成**：T1 的滚动加载、T3 的「跳到最新」、T4 的 `scrollToLine` 都直接依赖它；T0 未完成时，上层只能靠重新挂载组件绕过，体验和状态保持都会退化。
3. **T1 与 T2 可并行**：两者互不依赖；但文件管理页最终需要二者同页协作，建议 T1 先出骨架、T2 后接入。
4. **T3、T4 在 T0 之后可并行**：二者都基于定高行虚拟滚动，可复用 T0 的滚动方法；代码高亮不要引入 WASM，否则会迫使 Skiff 放宽 CSP。
5. **T5 最后做**：依赖 P1 的 CSP 结论，且与其它任务没有代码耦合；它是唯一需要处理长连接生命周期的任务。
6. **顺序不是发布顺序**：每个任务都应独立可回滚、独立补测试；不得为了赶进度把 T0 的改动混进 T1。

### Skiff 可以先自己顶上（`skiff:*` Pack 对照）

> 仅为应急对照，不代表最终取舍；取舍由负责人决定。

| 任务 | Skiff 不等上游的临时做法 | 代价 |
|---|---|---|
| T0 | 无法可靠替代；只能通过「重新挂载列表」回到顶部/底部 | 状态丢失、滚动位置跳变；日志跟随不可用 |
| T1 | 用 `VDataTableVirtual` + `VBreadcrumbs` 自建 `skiff:file-browser` | 需自行维护游标、排序与空态；上游实现到位后需迁移 |
| T2 | 用 `platform.request` + 自定义 XHR 适配器做单文件上传 | 进度与取消要自写；与服务端 offset 协议的对接需自行测试 |
| T3 | 用 `VVirtualScroll` + 等宽文本自建 `skiff:log-viewer` | 无「跳到最新」的可靠定位（受 T0 限制）；搜索高亮需自写 |
| T4 | 用只读 `<pre>` 展示，不做高亮 | 大文件卡顿、无行号；仅能作为临时查看 |
| T5 | 独立的 xterm 页面（非 iframe 嵌入业务页） | 与主界面割裂；仍需自己处理票据、resize 与重连 |

**共同注意**：任何 `skiff:*` 临时实现都必须遵守同一套 CSP、离线与测试约束，并且在对应上游任务交付后能够平滑替换，不得把临时实现固化成第二套契约。
