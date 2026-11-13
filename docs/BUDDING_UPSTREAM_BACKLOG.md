# budding 上游缺口清单（1.8.0）

> **核对日期/版本**：2026-11-13，budding **1.8.0**（`dist/package/budding-1.8.0.manifest.json`，`builtAt 2026-11-13T12:42:39Z`；宿主人库标记 **`1.8.0-r3`**，catalog 1.8.0 / 57 条目）。
> 本文只列仍未闭环、需要 budding 团队执行的事项；T6 / T8 / T9 / T10 / T14 / T15 / T16 已在本版闭环并整条删除，T11 / T13 只保留未完成部分。

## 未闭环任务总览

| 编号 | 剩余任务 | 产出形态 | 依赖 | Skiff 用途 | 规模 |
|---|---|---|---|---|---|
| T11 | `table` 跨页选择、区间选择、打开语义、列图标 | 框架能力补齐 | 可与 T10 并行 | 文件多选跨页批量操作、双击进入、目录/文件图标 | S–M |
| T12 | 上传：分片 / 续传 / 暂停 / 多文件进度 | 框架能力补齐 + 宿主适配 | 无 | 大文件、备份包、多文件队列 | M–L |
| T13 | `code` 分段加载与常用配置语法 | 框架能力补齐 + 发布 | 分段加载需后端协议先行 | 大文件查看、nginx / toml / ini 配置高亮 | S–M |
| T17 | `when` / `disabled` 随表单值与行作用域求值 | 框架能力补齐 | 无 | 按类型联动表单项、按行隐藏不适用操作 | S–M |
| T18 | 文本字面量绑定（不解析 `${...}`）与 `textarea` 只读/代码模式 | 框架能力补齐 | 无（与 T13 独立） | 站点文件插件显示/编辑含 `${...}` 的 Shell、JS、模板文件 | S |

## 统一约束（所有任务适用）

1. **只增不改**：现有 57 个渲染器、`embed(target, schema, opts)` 返回值、动作与指令语义保持可用；字段只增不改。
2. **必须补测试**：单测沿用上游测试栈；涉及滚动、WebSocket、AbortController 时用可注入 mock。
3. **离线可用**：运行时不依赖 CDN、外网字体或远程图标；不引入 `eval` / `new Function`（`budding.js` 产物实测 0 命中），保持 `script-src 'self'`。
4. **无沙箱假设**：渲染器是应用树内普通 Vue 组件，能力经 props / 动作 / `runtime` 暴露，不依赖 iframe 隔离。
5. **数字纪律**：未实测的性能数字标注「待实测」，不得写成既定事实。
6. **命名**：新增组件/渲染器遵循 `vendor:type`；官方新增沿用现有内置组，不新造分组语义。

---

## 剩余部分（可直接粘贴的提示词）

### T11 `table` 跨页选择、区间选择、打开语义与列图标

- **产出形态**：框架能力补齐
- **现状（r3 产物实测）**：本版已交付行级 `download` 动作与 `PlatformAdapter.download?`（GET 缺省退化为 `<a download>`，POST / 自定义头 / 数据体需宿主适配）。**仍未提供**：`selectStrategy: 'all'`、`rangeSelect`、`openOn`、列 `iconMap`。`table` 字段表里没有这四项；bundle 中的 `selectStrategy` / `openOn` 命中属于 Vuetify `list` 与内部实现，`rangeSelect` / `iconMap` 为 0 命中。
- **缺口表现**：文件列表翻页后选择被清空，无法「选中若干再批量删除」；单击行就进入目录，移动端误触；文件名前无法显示文件夹/文件图标。

```text
[任务]
补齐 table 的跨页选择、区间选择、打开语义与列图标，保持现有默认行为不变。

[背景事实]
1. 1.8.0 已交付行级 download 动作，本任务不再涉及下载。
2. selectable 目前只控制多选列显示；selectionChange 只回传当前页选择。
3. 没有 selectStrategy / rangeSelect / openOn / iconMap 字段。

[要产出]
1. selectStrategy: 'page' | 'all'，默认 'page'（现状）：
   - 'all' 时选择跨页保留，modelValue 存 rowKey 列表，翻页后回显；
   - 提供 clearSelection 指令。
2. rangeSelect: boolean，默认 false：开启后支持 shift 连选当前页区间。
3. openOn: 'single' | 'double'，默认 'single'：控制 rowClick 触发的点击次数；双击时单击不再触发。
4. 列图标：columns[].icon（string 或 { map, key }），按行值映射；缺省不显示。
5. 文档补四项的字段表与示例。

[验收标准]
- all 策略下翻页后已选项仍高亮，selectionChange 回传跨页集合；clearSelection 清空。
- shift 连选区间正确，反向选择与取消可断言。
- openOn=double 时单击不触发 rowClick，双击只触发一次。
- 列图标按 rowKey 正确映射，缺省列不渲染图标。
- 默认配置（page / false / single / 无 icon）下现有表格行为不变。

[明确不做]
- 不做拖拽选择、不做服务端选择持久化、不做批量操作容器（crud 已有）。
- 不改变 selectionChange 现有载荷字段，只增不改。

[约束]
- 只增不改；必须补单测；离线可用。
```

---

### T12 上传：分片 / 续传 / 暂停 / 多文件进度

- **产出形态**：框架能力补齐 + 宿主适配接口
- **依赖**：无（1.8.0 已有单请求上传）
- **现状（r3 产物实测）**：`file` 40 字段，现有实现已具备：`uploadApi` / `uploadField` / `uploadData` / `uploadTarget` / `uploadProgressTarget`（写 `{status, loaded, total, percent, attempt}`）/ `maxRetries` / `retryDelayMs`，指令 `cancelUpload` / `retryUpload`，宿主 `PlatformAdapter.upload(req)`。**没有**分片、续传、暂停/恢复、多文件独立进度、并发控制；`multiple` 存在但进度是单一对象。
- **缺口表现**：几百 MB 的镜像/备份包单请求易超时且失败只能整文件重传；断网后无法从已上传偏移继续；多文件时只有一个总百分比，无法展示每个文件的状态；没有暂停。

```text
[任务]
在现有单请求上传基础上补齐分片上传、断点续传、暂停/恢复与多文件进度，保持单请求模式为默认。

[背景事实]
1. 现有实现已有整体进度、取消、整文件重试与 PlatformAdapter.upload。
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

### T13 `code` 分段加载与常用配置语法

- **产出形态**：框架能力补齐 + 发布
- **现状（r3 产物实测）**：本版已将高亮包升级为 `@budding/code` 并纳入 `manifest.packages`（`dist/packs/code.mjs` 84,813 B，另有 `code.none` 35,011 / `code.shiki` 36,149 / `code.external` 38,184），`scrollToLine`、`getLanguages`、`getThemes` 已可用，产物 `WebAssembly` 0 命中。**仍缺**：
  - 分段 `source`：`Code.vue` 没有 `source` / `start_line` / `total_lines`，只能整串 `modelValue`；
  - 语法覆盖：内置语言 id 约为 markup/html/xml/svg/css/clike/javascript/typescript/json/bash/shell/python/markdown/sql（含别名），`nginx` / `toml` 在产物中 0 命中，`ini` / `dockerfile` 仅有个别词法片段，没有正式 grammar；也没有 `diff` / `yaml` / `ruby`。
- **缺口表现**：打开 10 万行日志或大配置文件时必须整串塞进 `modelValue`，首屏慢且占内存；nginx.conf / toml / ini 只能按纯文本显示。

```text
[任务]
补齐 code 的分段加载，并补齐常用配置语法；跳行与语言/主题清单已在 1.8.0 交付，不在本任务范围。

[背景事实]
1. @budding/code 已提供 scrollToLine / getLanguages / getThemes，产物无 wasm。
2. 分段 source 不存在：Code.vue 只有整串 modelValue。
3. 内置语言表缺 nginx / toml / ini / dockerfile / diff / yaml。

[要产出]
1. 字段 source?: (args: { start_line, end_line, signal }) => Promise<{ lines, total_lines }>，与 modelValue 二选一；滚动到分段边界时加载相邻段且不重复。
2. 补 nginx、toml、ini、dockerfile、diff 五类常用语法（纯 JS，不得引入 WASM），并把语言 id 与别名写入文档。
3. 分段加载与 scrollToLine 组合：跨段跳行时先加载目标段再定位。
4. 大文件口径：给出「多少行开始建议分段」的实测数字或明确标注待实测。

[验收标准]
- 分段夹具：滚动跨边界时只请求相邻段，行号连续、无重复、无空洞；总行数与 total_lines 一致。
- scrollToLine 在分段模式下命中可视区，越界钳制且不抛异常。
- 五类新语法各有一条高亮断言；产物仍无 WebAssembly / .wasm。
- 不配置 source 时整串模式行为与 1.8.0 一致。

[明确不做]
- 不做 LSP、折叠、minimap、协同编辑。
- 不引入 WASM 或远程高亮服务。
- 不重复实现已交付的 scrollToLine / 语言与主题清单。

[约束]
- 只增不改；必须补单测；离线可用；不引入 eval / new Function。
```

---

### T17 `when` / `disabled` 随表单值与行作用域求值

- **产出形态**：框架能力补齐
- **依赖**：无
- **现状（r3 产物实测 + 宿主接入结论）**：`when` 只在渲染前求值一次（Skiff 壳页在加载时把 `when` 布尔化后交给渲染器）；动作 props 里的 `${row.field}` 可按行求值，但**行作用域的 `when` 不会逐行求值**。表单值只对表单内部的动作 props 可见，不能驱动 `when` 做联动显隐。
- **缺口表现**：
  - Cron 新建/编辑：按「目标类型」切换预设调度字段做不了，只能退化为单个 5 段表达式输入框；
  - FTP 服务配置：TLS 非关闭时才显示证书路径做不了，只能始终显示；
  - 任务列表：内置任务 / 其它插件任务不应显示「编辑/删除」，`rowActions` 无法按行隐藏，只能保留按钮并由后端报错；
  - 其它页面：多选筛选、批量选项等同样需要「值变化 → 显隐/禁用」。
- **Skiff 侧当前做法**：拆成多个表单/多张表；用插件派生的布尔字段（如 `can_enable` / `show_banned`）替代组合条件；不适用操作保留按钮，失败时展示内核错误。没有自造渲染器。

```text
[任务]
让 schema 的 when / disabled 在表单值与行作用域下可求值，并明确是否随值变化重算。
默认静态行为保持不变，只增不改。

[背景事实]
1. 当前 when 只在渲染前求值一次；动作 props 支持 ${row.field}，但行作用域的 when 不逐行求值。
2. 表单值对内部动作 props 可见，但不能驱动 when 显隐。
3. 宿主 schema 由静态 JSON 下发，不允许 eval / new Function。

[要产出]
1. 定义 when 的求值作用域与时机：
   - 页面加载时静态求值（现行为）；
   - 表单内：随 values 变化重算；
   - 表格 rowActions / 单元格：按行作用域求值；
   - 未提供作用域时按静态布尔处理，不报错。
2. 同作用域支持 disabled（或只读）表达式，语义与 when 一致。
3. 表达式语法沿用现有 ${...}，不得引入 eval / new Function；非法表达式降级为「显示」并上报一次错误，不得整页崩溃。
4. 隐藏节点的动作不得被触发；隐藏再显示时表单值不丢失。
5. 文档写清作用域（$values / row / query）、重算时机与性能边界（表单字段较多时的重算频率，给出实测或标注待实测）。

[验收标准]
- 表单字段变化后，依赖它的区块/字段按 when 显隐，且无整页重渲染导致的输入丢失。
- 表格行内按钮按 row 字段显隐：同一列不同行可不同。
- 非法表达式只上报一次错误，页面其余部分可用。
- 不配置动态条件时，静态 when 行为与 1.8.0 完全一致。

[明确不做]
- 不做任意 JS 表达式求值、不做跨节点副作用、不做表单校验规则引擎。
- 不改变现有动作 props 的 ${row.field} 语义。

[约束]
- 只增不改；必须补单测；离线可用；不引入 eval / new Function。
```

---

### T18 文本字面量绑定（不解析 `${...}`）与 `textarea` 只读/代码模式

- **产出形态**：框架能力补齐
- **依赖**：无（与 T13 独立，可并行）
- **现状（r3 产物实测 + 宿主接入结论）**：budding 对**所有字符串 props** 统一做 `${...}` 模板求值，没有转义语法；r3 产物中 `textLiteral` / `valueLiteral` 仍为 0 命中。宿主把文件内容放进 `text` / `textarea` 的 `defaultValue` 时，内容里的 `${...}` 会被当成表达式求值：轻则显示被替换，重则表达式报错导致节点渲染失败。`textarea` 也没有只读/代码模式，长文本只能按纯文本域显示。
- **缺口表现**：
  - 站点文件插件（`com.skiff.files`）编辑 Shell / JS / 模板文件时，正文里的 `${VAR}`、`${1}` 会被当作模板处理，保存前就可能被改写；
  - 宿主当前只能把 `${` 替换成 `${'${'}` 再交给渲染器（依赖表达式求值结果拼回原文），属于补丁而非契约；
  - `textarea` 只读态要靠隐藏保存按钮 + 后端拒绝，不能声明「只读」。
- **Skiff 侧当前做法**：插件在 RPC 返回时提供 `text_safe`（把 `${` 转义为 `${'${'}`），schema 绑定 `text_safe`；保存时提交用户编辑后的原文。超过 256 KiB 的文件只读展示并提示用内核文件管理。

```text
[任务]
为文本类节点提供「字面量绑定」能力，并提供 textarea 的只读与代码模式。
默认行为保持不变，只增不改。

[背景事实]
1. 1.8.0-r3 对所有字符串 props 统一解析 ${...}，没有转义语法；textLiteral / valueLiteral 不存在。
2. 宿主需要在 text / textarea 中展示任意文件内容（可能含 ${VAR}、${1}、反引号与模板片段）。
3. 宿主不允许 eval / new Function，schema 仍为静态 JSON。

[要产出]
1. 新增显式的字面量绑定方式（例如 `textLiteral` / `valueLiteral`，或 `literal: true` 属性），
   声明后该 prop 不做 ${...} 求值，原样渲染。
2. textarea 支持 `readonly`（或 `disabled`）与 `code` 模式：等宽字体、保留空白、可选行号；
   语法高亮可作为可选能力，未提供时不得报错。
3. 字面量绑定同样适用于表格单元格与表单默认值，语义一致。
4. 文档写清字面量与模板两种绑定的边界，给出含 ${...} 文件内容的示例。

[验收标准]
- 文本内容含 ${VAR} / ${1} / 嵌套 ${} 时原样显示，控制台无表达式错误。
- 保存后回读内容与用户输入逐字节一致（除宿主明确声明的换行归一）。
- 未声明字面量时，现有 ${...} 模板行为完全不变。

[明确不做]
- 不做富文本 / Markdown 渲染，不做文件系统访问，不做自动语法猜测。
- 不改变现有 ${row.field} 与 ${field} 的求值语义。

[约束]
- 只增不改；必须补单测；离线可用；不引入 eval / new Function。
```

---

## Skiff 侧当前的临时做法（上游落地后需删除）

| 条目 | 临时做法 | 代价 |
|---|---|---|
| T11 | 翻页后选择被清空；单击即进入；目录/文件无图标 | 批量操作只能逐页做，移动端误触 |
| T12 | 单请求上传 + 整体进度 | 大文件超时只能整文件重传 |
| T13 | 整串 `modelValue` 读取；nginx / toml / ini 按纯文本显示 | 大文件首屏慢，配置无可读高亮 |
| T17 | 拆成多表单/多表格 + 服务端派生布尔；不适用的行内操作保留按钮，由后端拒绝 | 表单更长，内置/异构任务仍显示不适用操作 |
| T18 | 插件把文件正文的 `${` 统一转义成 `${'${'}` 再绑定（`text_safe`），保存仍提交原文；只读态靠隐藏保存按钮 + 后端拒绝 | 依赖表达式求值结果拼回原文 |

## 事实与推测边界

**产物实测（1.8.0-r3）**

- `table`：游标分页（`paging` / `cursorParam` / `cursorPath` / `loadMoreText` / `loadMore`）与 `download` 动作已交付；`selectStrategy: 'all'` / `rangeSelect` / `openOn` / `iconMap` 未提供。
- `file` 40 字段：无分片 / 续传 / 暂停 / 多文件独立进度。
- `@budding/code`：`scrollToLine` / `getLanguages` / `getThemes` 可用，无分段 source；语言表缺 nginx / toml / ini / dockerfile / diff / yaml，产物无 `WebAssembly`。
- 所有字符串 props 统一走 `${...}` 模板求值，无字面量语法；`textarea` 无 `readonly` / `code` 模式。

**读文档确认**

- 路由 schema 走 `platform.loadJson`，`service.schemaApi` 可运行时取 schema。
- `custom` 动作未注册会报错；内核动作不可被同名覆盖。
- `schemaPolicy` / `validateSchema` / `usedTypes` 不存在。

**推断（需上游确认）**

- 未知 `type` 是否一律导致整页错误态，未逐场景验证。
- 规模（S/M/L）为经验估算；分段加载的收益数字均以交付实测为准。
