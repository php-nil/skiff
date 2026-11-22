# 上游 budding 需求与建议

> 重组：2026-11-22。依据是同步上游 `1.8.5`（`a3a3c8d`）后，在测试机完成「干净首装 → 验收 → 修复 → 两次升级复验」的实践结论。
> 口径：本清单只写**上游应承担**的能力；宿主自己该补的契约见 §四。编号不复用，`U1–U5` 已关闭（见 §五）。
> 用法摘录见 [BUDDING_INTERACTION_CAPABILITIES.md](BUDDING_INTERACTION_CAPABILITIES.md)；接入边界见 [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md)。

## 一、高优先级：影响正确性或显著抬高宿主成本

### U6 页脚 `submit` 需要「只跑事件、不自动成功」的模式

- **场景**：弹窗只有一张表单，保存 / 重置放 `content.footer`（1.8.5 已支持）。宿主的保存动作是 `onEvent.submit` 里的自定义动作（带字段归一化、CSRF、错误回填），不是表单原生 `api`。
- **现状**：`submit` 会依次完成「校验 → `emit('submit')` → 请求 `api` → 成功提示」。`onEvent.submit` 是 fire-and-forget，动作里的失败被吞掉；只要表单声明了 `api`（宿主为了不弹「未配置 api」只能注入 `{ sendOn:false }`），失败也会走到「提交成功」。
- **问题**：宿主必须在「不弹成功」和「不弹未配置 api 警告」之间二选一，两个选择都会产生错误反馈。Skiff 当前用「注入 no-op api + 宿主动作自己 toast/formError」绕开，属于宿主补丁。
- **建议**：
  - 表单新增 `submitMode: "event"`（或 `api: false`）：只做本地校验并触发 `onEvent.submit`，不请求、不提示成功。
  - `onEvent.submit` 动作支持返回结构化结果 `{ ok: false, message, fieldErrors }`；`ok:false` 时保持弹窗与输入，交给 `onError` / `formErrors` 呈现。
- **验收**：同一张表单声明 `submitMode:"event"` 后，动作成功不出现「提交成功」，动作失败不出现任何成功提示；弹窗保持打开且字段值不丢。

### U7 页脚按钮需要可指定表单并读取字段值

- **场景**：插件页常见「筛选表单 + 新建表单」同层；新建表单的保存放页脚。
- **现状**：页脚 `submit` / `reset` 只解析「本层唯一表单」，0 张或多张直接报错；页脚按钮读不到字段值，只能用 `submit`。
- **问题**：多表单页只能把按钮塞回表单 `content.actions`，与页脚统一交互冲突；页脚动作无法按字段值分支（例如协议为 `all` 时禁用端口）。
- **建议**：
  - `submit` / `reset` 支持 `form: "<form id>"`（或最近表单 `form: "nearest"`）。
  - 页脚动作上下文暴露 `values`（只读快照），至少允许 `disabledOn` / `visibleOn` / `when` 依据表单值与行作用域求值。
- **验收**：同层两张表单时，页脚按钮能明确提交其中一张；页脚按钮的 `disabledOn` 能随字段值变化。

### U8 失败恢复应是一等能力：保留输入并原地展示错误

- **场景**：服务端返回 409 / 422 / 字段错误；表单需要留在原弹窗，值不能丢。
- **现状**：表单失败只给 `errorMessage`，字段错误另需宿主自行调用 `formErrors`；若宿主选择「关闭后重开」以携带服务端错误，需要自己维护弹窗注册表、克隆 `content`、合并 `initialValues`，还要处理行作用域 `data` 失效。
- **问题**：宿主重复实现且容易出回归。本轮就出现过「同名目录失败后输入丢失，只剩『请输入名称』」的问题，靠宿主修复。
- **建议**：
  - 提供 `runtime.reopenLayer(layerId, { values, errors })` 或表单级 `preserveOnError`，由运行时负责恢复字段与错误。
  - 提供声明式 `errorMap`：把 HTTP 错误信封的 `error.field/message` 映射到字段错误，无需宿主写自定义动作。
- **验收**：服务端返回字段错误时，弹窗不重建、字段值不变、错误落在对应字段；宿主不写注册表也能完成。

## 二、中优先级：契约一致性与可验证性

### U9 统一 `confirm` 的参数命名与语义

- **现状**：`confirm` 动作目录用 `color`（`info` / `warning` / `error`），`LayerInput.confirm` / 宿主导入的 `runtime.confirm` 用 `level`；1.8.5 补了 `warning` 与 `alertdialog`。
- **问题**：宿主按文档写 `level` 时被静默忽略，危险操作没有红色语义；两套命名让代码审查难以发现。Skiff 本轮把站点删除 / 恢复两处从 `level` 改为 `color`。
- **建议**：统一为 `color`（保留 `level` 别名并标记弃用窗口），在 catalog、guide、类型定义三处同时给出迁移说明。
- **验收**：同一份 `confirm` 在动作目录、`runtime.confirm`、类型定义中字段名一致；`level` 别名有明确弃用版本。

### U10 `when` 需要可组合的条件表达式

- **现状**：宿主侧校验（`skiff-pkgfmt`）与上游模板都只稳支持 `{{data.x}}` / `!{{data.x}}`；复杂条件依赖后端额外算一个布尔字段。
- **问题**：本轮 cron 需要「可编辑且不是新建」才显示编辑卡片，因无法写 `&&`，只能让插件多输出一个 `editable` 字段；条件逻辑散到后端。
- **建议**：定义并公开一个**受限表达式子集**（`&&` / `||` / `!` / `==` / `!=`，仅允许 `data.*`、`row.*`、字面量），同时提供校验器与错误定位；不允许函数调用与任意 JS。
- **验收**：`when: "{{data.readonly}} == false && {{data.detail.is_new}} == false"` 能被官方校验器接受并在运行时正确求值。

### U11 提供宿主可直接复用的 schema 安全子集与校验器

- **场景**：宿主把 budding 作为嵌入式 UI，服务端 / 插件生成的 schema 必须先过白名单。
- **现状**：Skiff 自己维护 `skiff-pkgfmt`（节点白名单、动作白名单、禁用 `api/url`、`dialog` 的 content 校验），并为本轮弹窗新增 `dialog` / `reset` / `close` 与页脚 `submit`；这些规则上游没有可复用的发布物。
- **问题**：每个宿主重复实现，规则容易落后于上游（例如 1.8.5 新增页脚提交后，宿主白名单必须同步扩展）。
- **建议**：发布 `@budding/schema-lint`（或 JSON Schema + CLI）：覆盖节点/动作/props 白名单、`when` 语法、弹窗 content 槽位；支持 `strict` 与 `host-extension` 两种模式；同时提供「校验后注入宿主默认值」的钩子（如 no-op `api`）。
- **验收**：宿主不再自写白名单即可拒绝 `api` / `url` / 未知动作；上游升级时 lint 包与运行时同版本发布。

### U12 机器可读的兼容性清单与迁移说明

- **现状**：1.8.5 移除 `mount()` 旧入口、`loop` 动作改名 `forEach`、`snackbar` 关闭参数移除；`dist/package/*.manifest.json` 只记录文件清单，不标破坏性变更。
- **问题**：宿主只能靠读源码 / diff 发现破坏点；升级窗口内很难判断哪些旧写法会在运行时报「未知动作」。
- **建议**：随包发布 `compatibility.json`：`breaking` / `deprecated` / `renamedFrom` / `removedIn`，并给出 codemod 或替换表；在控制台对旧写法给出一次性告警。
- **验收**：只读 `compatibility.json` 就能列出从 1.8.4 升 1.8.5 需要改的宿主代码；旧写法运行时给出可定位告警而不是静默失败。

### U13 面向自动化的稳定测试契约

- **现状**：浏览器验收依赖可见文本与 `aria-label`；弹窗、页脚、表单字段没有统一的 `data-bud-*` 标识；`diagnose()` 主要覆盖实例状态。
- **问题**：E2E 选择器脆弱，文本微调就失效；出错时难以从宿主侧看到图层栈与最近一次动作结果。
- **建议**：
  - 为 `dialog` / `form` / `field` / `footer` 输出稳定的 `data-bud-role` + 用户可传 `testId`。
  - `diagnose()` 增加 `layers`（标题、表单 id、字段值摘要）与 `lastAction`（类型、耗时、结果）。
- **验收**：不依赖中文文案即可定位并操作弹窗保存按钮；失败时 `diagnose()` 能给出图层栈与最近动作结果。

## 三、低优先级 / 体验建议

| 编号 | 建议 | 理由 |
| --- | --- | --- |
| U14 | 表单 `successMessage` 支持 `false` / `silent` | 宿主用自定义提交链时可显式关闭默认成功提示，不必依赖 no-op api |
| U15 | `reset` 明确重置到 `initialValues` 还是空值，并支持字段级重置 | 目前只能从实现推断；服务端错误恢复后行为不一致 |
| U16 | 弹窗 `content` 槽位在文档与示例中统一为顶层 `body` / `footer` | 示例仍混有 `props.body` 写法，照抄会整页 CompileError |
| U17 | 表单错误自动聚焦到第一个错误字段 | 长表单（容器、编排、cron）靠滚动找错成本高 |

## 四、宿主侧待补（不是上游缺口）

| 编号 | 事项 | 状态 |
| --- | --- | --- |
| K1 | `skiff-pkgfmt` 放开 `visibleOn` / `hiddenOn` / `disabledOn` 与 RPC 结果回填 | 未做；当前插件 schema 仍只允许 `when` |
| K2 | 确认框接管与 22 处 `window.confirm` / 2 处 `window.prompt` / 89 处 `window.alert` 的替换 | 未做；上游 `confirm` 已具备 |
| K3 | 在 U8 落地前，宿主弹窗失败恢复仍依赖 `dialogRegistry` + 克隆 `content` | 已在 1.8.5 轮改为复用原始 content，属临时方案 |
| K4 | 插件 schema 的 no-op `api` 注入 | 已做（`inject_form_submit_stub`），待 U6 落地后删除 |

## 五、已关闭（编号不复用）

| 编号 | 结论 |
| --- | --- |
| U1–U4 | 上游 1.9.0 已实现：`confirm` 图层 / `property` / `setErrors` / `createUi` 本地化 |
| U5 | 上游 `a707369` 已实现「浮层归属图层根」；宿主不得再注入层叠补丁 |
| 旧 A1–A5 | 随 U1–U4 消解或转入宿主侧 K2 / 主题迁移 |
| 1.8.5 新增能力 | 页脚 `submit` / `reset` 解析本层唯一表单；`confirm` 支持 `warning` / `alertdialog`；`loop` 组件替代旧 `loop` 动作 |

## 六、可直接提交上游的简版清单

1. 给 `submit` 增加 `submitMode:"event"`，并让 `onEvent.submit` 可返回 `{ ok:false, message, fieldErrors }`，失败时不出现成功提示。
2. 页脚 `submit` / `reset` 支持 `form` 指定；页脚动作可读 `values` 快照。
3. 提供 `reopenLayer(layerId, { values, errors })` 与声明式 `errorMap`，失败恢复不丢输入。
4. `confirm` 统一用 `color`，`level` 作为带弃用期的别名。
5. 发布受限条件表达式子集（`&&` / `||` / `==` 等）及校验器。
6. 发布 `@budding/schema-lint` + `compatibility.json` + codemod。
7. 为弹窗/表单/字段/页脚补稳定 `data-bud-role` 与 `testId`，扩展 `diagnose()`。
