# 上游 budding 需求与建议（处理台账）

> 基线：`1.8.7`（上游 `8906804`，2026-11-24）。上游回执见 [`tests/BUDDING_UPSTREAM_BACKLOG_UP.md`](../../tests/BUDDING_UPSTREAM_BACKLOG_UP.md)——**逐条答复的原文在那里**，本文只记「还要不要做、我们怎么处置」，不复制回执正文。
> 口径：§一 是上游侧事项（有回执即标状态）；§二 是宿主自己该补的（K 编号）；§三 是仍然开放、下一轮该提的。编号不复用。
> 用法摘录见 [BUDDING_INTERACTION_CAPABILITIES.md](BUDDING_INTERACTION_CAPABILITIES.md)；接入边界见 [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md)。

## 一、上游事项状态

状态：✅ 已处理 ｜ 🟡 部分处理（有替代路径）｜ ⏳ 暂缓（待方案）｜ ❌ 不做

| 编号 | 诉求 | 状态 | 本仓处置 |
| --- | --- | --- | --- |
| U6 | 表单「只跑事件、不自动成功」的提交模式 | 🟡 | `submitMode: "event"` 已用：`schema_kit` 的 `form` / `form_body` / `search_form` 与插件 schema 全部改声明；no-op `api` 不再存在。结构化返回（`{ok:false, message, fieldErrors}`）上游暂缓，失败仍由宿主动作自己写回 |
| U7 | 页脚按钮点名表单 / 读字段值 | 🟡 | `props.form: "<id>"` 可用（本仓暂无多表单同层场景，未用）；页脚读字段值未做，需按值分支的动作继续留在表单 `content.actions` |
| U8 | 失败保留输入 + 原地回填 | 🟡 | 失败不关图层、值不丢、`setErrors` 升为一等动作、`onError` 可读 `${error...}`；`errorMap` 未做。宿主：K3 已删除，字段错误回填继续用 `skiffFormErrors`（服务端错误是 HTTP 错误码 + `error.field`，不是业务信封，`payload` 不填充）|
| U9 | `confirm` 参数统一 | ✅ | 宿主侧 `hostConfirm` 的 `level` 已改 `color`（`danger` → `error`）。上游**不加** `level` 别名，新代码不要再写 |
| U10 | `when` 可组合条件 | ✅ | 条件一律 `${...}`，`&&` / `\|\|` / `!` / `==` / `??` 原生支持；`{{...}}` 不是语法（恒真），校验器给 `CONDITION_NOT_EXPRESSION` 提示。宿主插件 schema 的 `{{data.x}}` 是**壳自己的占位符替换**（`pluginResolve`），与 budding 表达式无关，不冲突 |
| U11 | 可复用的 schema 安全子集 | ✅ | 上游出了 `@budding/schema-lint`（`lintSchema` + CLI + MCP）与内核 `diagnose({ policy })`；本仓仍用 `skiff-pkgfmt`（不引 Node 工具链，D1），规则按上游语义对齐即可，暂不替换 |
| U12 | 机器可读兼容清单 | 🟡 | 只落了最小改名表 `COMPAT_RENAMES`（未知动作报错时提示新名）；完整 `compatibility.json` 推迟到上游首个公开版本 |
| U13 | 稳定测试锚点 | ✅ | `data-bud-role`（`dialog` / `drawer` / `confirm` / `footer` / `form` / `field`）、节点 `testId` → `data-testid`、`inspectLayers()` 图层快照。`lastAction` 不做（走 `installActionEngine` 中间件） |
| U14 | `successMessage: false` | ✅ | 宿主自管提交链一律 `submitMode: "event"`，成功提示由宿主动作 `toast` 决定，不再需要声明 `successMessage` |
| U15 | `reset` 语义 | ✅ | 恢复到「重置基线」（`data` + `initialValues`，`source` 到达后更新基线），不是清空；字段级重置未做 |
| U16 | 弹窗 content 槽位 | ✅ | 动作顶层 `content.body` / `content.footer`；`props.body` 写法本仓已清零 |
| U17 | 提交错误聚焦 | ✅ | `submitErrorFocus: "focus"` / `"scroll"`；`setErrors` 传 `focus: true` 走同一套定位 |

## 二、宿主侧待补（K 编号）

| 编号 | 事项 | 状态 |
| --- | --- | --- |
| K1 | `skiff-pkgfmt` 放开 `visibleOn` / `hiddenOn` / `disabledOn` 与 RPC 结果回填 | **未做**。当前插件 schema 只允许 `when: "{{data.x}}"` / `"!{{data.x}}"`（壳替换后判真假）。上游 1.8.6 已支持完整 `${}` 条件，放开时须同时把壳的 `pluginResolve` 从「字符串替换」改成「交给渲染器求值」，否则两条条件语义并存 |
| K2 | 确认框接管（`window.confirm` / `prompt` / `alert` 全量替换） | **未做**。上游 `confirm` 已具备 `color` / `requireText`；命令式 JS 走 `hostConfirm`（已映射 `runtime.confirm`），页面 Schema 走 `confirm` 动作 |
| K3 | 弹窗失败恢复用注册表 + 克隆重开 | ✅ 已删除。失败不关图层，`formError` 直接 `setErrors` 原地回填（`__form__` 或具体字段），壳里不再有 `dialogRegistry` / `clone content` |
| K4 | 插件 schema 注入 no-op `api` | ✅ 已删除。改为 `submitMode: "event"` 声明；`skiff-pkgfmt` 对「有 `onEvent.submit` 但没声明 event 模式」的表单**直接拒装**，避免提交静默空转 |

## 三、仍开放、下一轮可提

1. **`onEvent.submit` 返回结构化结果**（`{ok:false, message, fieldErrors}`）：现在失败要由宿主动作自己写表单错误，动作链层面无法表达「这步失败」。
2. **页脚动作读字段值快照**：多表单 / 按值分支的场景仍被迫把按钮放回表单 `content.actions`。
3. **声明式 `errorMap`**：把 HTTP 错误信封的字段错误映射到字段，宿主就不必维护 `skiffFormErrors`。
4. **`compatibility.json` + codemod**：升级窗口内判断「哪些旧写法会失败」仍靠读源码。

## 四、已关闭（编号不复用）

| 编号 | 结论 |
| --- | --- |
| U1–U4 | 1.8.x 版本线已实现：`confirm` 图层 / `property` / `setErrors` / `createUi` 本地化 |
| U5 | `a707369` 已实现「浮层归属图层根」；宿主不得再注入层叠补丁 |
| 旧 A1–A5 | 随 U1–U4 消解或转入 K2 / 主题迁移 |
| 1.8.5 新增能力 | 页脚 `submit` / `reset` 解析本层唯一表单；`confirm` 支持 `warning` / `alertdialog`；`loop` 组件替代旧 `loop` 动作 |
| 1.8.6 新增能力 | `submitMode: "event"`、`setErrors` 动作、`ApiError.payload`、`onError` 错误快照、`testId` / `data-bud-role`、`inspectLayers()`、`@budding/schema-lint`、改名提示 |
| 1.8.7 新增能力 | `page` 的 `content.actions` 页头主操作区（稳定锚点 `page-header` / `page-actions`）、`query-table` 替代 `crud`、表头快速筛选 / 自定义筛选、`SYNC_URL_MODES` |
