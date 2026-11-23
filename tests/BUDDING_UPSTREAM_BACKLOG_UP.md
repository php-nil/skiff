# 上游 budding 需求与建议（处理回执）

> 基线：下游基于 `1.8.5`（`a3a3c8d`）的实践清单；本回执对应 **1.8.6**（2026-11-23 工作区）。
> 状态：✅ 已处理 ｜ 🟡 部分处理（有替代路径） ｜ ⏳ 暂缓（待方案决策） ｜ ❌ 不做（已说明理由）
> 编号不复用；`U1–U5`、`U11` 已关闭。宿主自身要补的见 §五。
> 组件字段与动作语义以契约（`docs/schema-contract*.md`）与手册为准；本文件只回答「改了什么、怎么用、还差什么」。

## 一、变更速览（给下游）

| 编号 | 状态 | 本轮变更 | 你需要做什么 |
| --- | --- | --- | --- |
| U6 | 🟡 | 表单新增 `submitMode: "event"`（只校验 + 触发 `onEvent.submit`，不请求 api、不弹成功）；结构化返回值未做 | 保存动作从「注入 no-op api」改为 `submitMode:"event"` |
| U7 | 🟡 | `submit` / `reset` 支持 `props.form: "<表单 id>"`；页脚读字段值未做 | 多表单页把页脚按钮改为点名表单 |
| U8 | 🟡 | `ApiError.payload` 携带业务信封 `data`；`onError` 链可读 `${error...}`；`setErrors` 升为一等动作；失败不关弹窗、值不丢；`errorMap` 未做 | 用 `onError` + `setErrors` 回填；删除「关闭后重开」补丁 |
| U9 | ✅ | `confirm` 全链路统一 `color`；`level` 别名**未加**（运行时从未支持） | 宿主把 `level` 改成 `color`（`info` / `warning` / `error`） |
| U10 | ✅ | 条件表达式原生支持 `&&` / `\|\|` / `!` / `==` / `!=` / `??`；校验器可定位语法错误，非 `${}` 条件给 `CONDITION_NOT_EXPRESSION` 提示 | 复杂条件直接写 `${...}`，不要再用 `{{...}}` 或后端布尔字段 |
| U11 | ✅ | 新增 `@budding/schema-lint`（`lintSchema` + `budding-lint` CLI）、内核 `diagnose({ policy })`、MCP `lintSchema` | 宿主白名单改为 policy 文件，删除自写 `skiff-pkgfmt` 规则 |
| U12 | 🟡 | 内核 `COMPAT_RENAMES`：未知动作/类型报错时提示新名（`loop` → `forEach`）；完整 `compatibility.json` 推迟 | 升级时按控制台提示改名，不再静默失败 |
| U13 | ✅ | `testId` → `data-testid`；`data-bud-role`（`dialog` / `drawer` / `confirm` / `footer` / `form` / `field`）；`inspectLayers()` 图层快照 | E2E 改用稳定锚点；删除文案选择器 |
| U14 | ✅ | `successMessage: false` 关闭成功提示（`onSuccess` / `resetOnSuccess` 照常） | 自定义提交链关闭默认提示 |
| U15 | ✅ | 契约写明 `reset()` 恢复到「重置基线」而非清空；字段级重置未做 | 删除宿主侧对 reset 行为的猜测 |
| U16 | ✅ | 文档与示例统一为动作顶层 `content.body` / `content.footer`（无 `props.body`） | 按新写法迁移旧示例 |
| U17 | ✅ | `submitErrorFocus: "focus" / "scroll"` 已具备（默认 `none`） | 长表单显式设为 `focus` |

## 二、已处理项：改了什么、怎么用

### U6 事件模式提交

场景不变：保存逻辑在 `onEvent.submit` 的自定义动作里，不需要表单原生 `api`。

```json
{
  "type": "form",
  "testId": "user-form",
  "props": { "submitMode": "event", "initialValues": { "name": "" } },
  "body": [{ "type": "input-text", "props": { "name": "name", "label": "名称" } }],
  "onEvent": { "submit": { "actions": [{ "type": "request", "props": { "api": { "method": "POST", "url": "/api/users" } } }] } },
  "content": { "actions": [{ "type": "button", "props": { "text": "保存" }, "action": { "type": "submit" } }] }
}
```

- `submitMode: "event"`：校验通过后只触发 `onEvent.submit`，**不请求 `api`、不弹成功提示、不执行 `onSuccess` / `onError`**；载体不重建、字段值保留。未知值回退 `api`。
- `submitMode: "api"`（默认）行为与 1.8.5 完全一致。
- `successMessage: false` 可单独关闭成功提示（U14）。
- 迁移：删除宿主 `inject_form_submit_stub`（K4）。

### U7 页脚点名表单

```json
{
  "type": "form",
  "id": "user-form",
  "props": { "api": { "method": "POST", "url": "/api/users" } },
  "body": [{ "type": "input-text", "props": { "name": "name" } }]
}
```

```json
{ "type": "submit", "props": { "form": "user-form" } }
```

- 目标解析三档：`props.form` 点名 → 最近祖先表单 / `crud` 查询表单 → 本层唯一表单。
- 点名不存在的 id 会报错，并提示「普通页面跨子树请用 `invoke`」。
- `reset` 同样支持 `props.form`。
- 页脚仍读不到字段值（见 §三）。

### U8 失败保留与原地回填

```json
{
  "type": "form",
  "props": { "submitMode": "event", "actionsAlign": "end" },
  "body": [{ "type": "input-text", "props": { "name": "email", "label": "邮箱" } }],
  "onEvent": {
    "submit": {
      "actions": [{ "type": "request", "props": { "api": { "method": "POST", "url": "/api/users" } } }],
      "onError": [{ "type": "setErrors", "props": { "errors": { "email": "该邮箱已被注册" }, "focus": true } }]
    }
  }
}
```

- 提交失败不会关闭弹窗，字段值原样保留；`setErrors` 按键合并，`null` 清除该键，`__form__` 为表单级错误。
- `setErrors` 与表单指令同名：**动作**走目标解析（`props.form` → 祖先 → 本层唯一），**指令**走 `invoke` 精确寻址；普通页面跨子树仍用 `invoke`。
- `ApiError.payload`：业务信封失败时携带 `data` 原样（不 clone、不裁剪，不承诺结构）；网络错误 / HTTP 非 2xx / 解析失败不填充。
- `onError` 链注入错误快照 `error`：`{ name, message, code?, payload? }`，可写 `${error.message}` / `${error.payload.fields.email}`；只在本次 `onError` 可见（嵌套覆盖外层、返回后恢复），`actions` / `then` 不可见，且会遮蔽数据域同名键（数据域写 `${data.error...}`）。
- 迁移：删除宿主的 `dialogRegistry` + 克隆 `content` 临时方案（K3）。

### U9 `confirm` 统一 `color`

- 动作、`LayerInput.confirm`、`runtime.confirm`、`platform.confirm` 一律 `color`，取值 `info` / `warning` / `error`。
- **没有 `level` 别名**：运行时从未支持，加别名会固化错误命名。宿主按文档写 `level` 会被忽略，请直接改 `color`。

### U10 可组合条件

```json
{ "when": "${data.readonly == false && data.detail.is_new == false}" }
```

- 支持 `&&` / `||` / `!` / `==` / `!=` / `>` / `<` / `??`、三元、成员访问、白名单函数。
- 校验器（`validateSchema` / `lintSchema` / MCP `validateSchema`）会以表达式语法错误定位到节点路径。
- **没有 `{{...}}` 双花括号语法**；写成 `{{...}}` 会被当成非空字符串（恒真）。`diagnoseSchema` / `validateSchema` / `budding-lint` 现对条件字段的非 `${...}` 普通字符串给 `CONDITION_NOT_EXPRESSION` hint（`visibleOn` / `hiddenOn` / `disabledOn` / `when`），不判错、不改变运行语义。

### U11 宿主侧 schema 安全子集

```ts
import { lintSchema } from '@budding/schema-lint'
import catalog from './catalog.json' with { type: 'json' }

const result = lintSchema(schema, {
  catalog,
  policy: {
    allowTypes: ['page', 'button', 'text', 'table', 'acme:widget'],
    denyActions: ['custom:exec'],
    hostActions: [{ type: 'custom:pay', params: ['orderId'] }],
    requireApiVersion: '>=1.0.0 <2'
  }
})
```

```bash
budding-lint schema.json --policy budding-lint.json --strict
```

- 规则不复制：`diagnoseSchema()` 是唯一实现，运行时 `diagnose({ policy })`、MCP `validateSchema` / `lintSchema`、本包 `lintSchema()` 同源。
- policy 字段：`allowTypes` / `denyTypes` / `allowActions` / `denyActions` / `hostActions` / `requireApiVersion`；**deny 优先于 allow**；白名单命中但 catalog 未收录按「宿主扩展」给 hint，不再报未知。
- CLI 退出码：`0` 无 error、`1` 有 error、`2` 参数 / 文件 / policy 结构错误。
- 包当前 `private`（随仓库 / tarball 分发，未发 npm）；详细语义见 `schema-lint.md`。

### U12 改名提示

- 未知动作 / 类型报错时追加提示，例如：`不支持的动作类型: loop；该动作已改名为 forEach（1.8.4）`。
- `budding:` 前缀等价（`budding:loop` 与 `loop` 同判）；旧名**不会自动转发**，仍按未知处理。
- 门禁保证改名表的目标存在于当前注册表。

### U13 稳定测试契约与图层快照

| 锚点 | 出现位置 |
| --- | --- |
| `data-bud-role="dialog"` / `"drawer"` / `"confirm"` | 图层卡片 |
| `data-bud-role="footer"` | 图层页脚 |
| `data-bud-role="form"` | 表单根 |
| `data-bud-role="field"` + `data-bud-field="<name>"` | 字段根 |
| `data-testid="<testId>"` | 任意节点（新增核心属性 `testId`，空值不输出） |

```json
{ "type": "button", "testId": "save", "props": { "text": "保存" }, "action": { "type": "submit" } }
```

```ts
budding.inspectLayers() // [{ id, kind, title?, placement?, path }]，自底向顶，普通对象
```

- `inspectLayers()` 不含 body / footer / scope / 字段值；`kind` 与卡片 `data-bud-role` 同一判定。
- 选择器示例：`[data-bud-role="dialog"] [data-testid="save"]`。

### U15 / U16 / U17

- **U15**：`reset()` 恢复到重置基线（`data` + `initialValues` 合并值；节点 `source` 结果到达后基线更新为 source 结果），同时清空 `errors` / `touched` / `submitted`，不是清空。字段级重置未提供。
- **U16**：弹窗 / 抽屉的动作内容统一写在动作顶层 `content.body` / `content.footer`；`props.body` 写法已从文档与示例中清除。
- **U17**：`submitErrorFocus: "focus"`（滚动 + 聚焦首个错误字段）/ `"scroll"`（只滚动）/ `"none"`（默认）。`setErrors` 传 `focus: true` 走同一套定位逻辑。

## 三、尚未处理（清单）

| 编号 | 事项 | 原因 / 现状 | 建议 |
| --- | --- | --- | --- |
| U6-b | `onEvent.submit` 返回 `{ ok:false, message, fieldErrors }` | `runActions` / `emit` 返回 `void`，属触点契约级改动；`parallel` / `forEach` 的结果归并规则未定 | 用 `onError` + `setErrors` 表达失败，已可闭环 |
| U7-b | 页脚动作读取表单值快照（`values`） | 注入图层作用域会造成双向耦合、整层重渲染、键冲突 | 需要联动就把按钮放回表单 `content.actions`；后续可加 `getValues` 指令按需拉取 |
| U8-c | 声明式 `errorMap` | 映射 DSL 要定数组多错误、点路径、code→文案、缺字段兜底，过早进契约会出特例 | 先让宿主用 `onError` + `${error.payload...}` + `setErrors` 映射 |
| U8-e | `reopenLayer` / `preserveOnError` | 失败本来就不关图层，值不丢是现状；重建 DOM 反而破坏「保留输入」 | 不做 |
| U9-b | `level` 弃用别名 | 运行时从未支持 `level`，加别名会固化错误命名 | 不做，直接改 `color` |
| U11-b | JSON Schema 导出 | lint 只做安全子集判定，不产出 JSON Schema | 明确不做 |
| U12-a | 完整 `compatibility.json` + codemod | 内部开发期每条破坏都要登记，事实源与 semver 政策未定 | 推迟到首个公开版本；届时以 `compat/*.json` 为唯一事实源并进 release 门禁 |
| U13-b | `lastAction`（最近动作结果） | 已在 `inspectLayers()` 排除；动作观测走宿主 `installActionEngine` 中间件 | 不做，避免内核第二套订阅与埋点开销 |
| U15-b | 字段级重置 | 价值低、出错面大（错误清理、基线更新），`setValue` 可替代 | 暂缓 |

## 四、宿主侧待补（不是上游缺口）

| 编号 | 事项 | 状态 |
| --- | --- | --- |
| K1 | `skiff-pkgfmt` 放开 `visibleOn` / `hiddenOn` / `disabledOn` 与 RPC 结果回填 | 未做（宿主） |
| K2 | 确认框接管：22 处 `window.confirm` / 2 处 `window.prompt` / 89 处 `window.alert` | 未做（上游 `confirm` 已具备） |
| K3 | 失败恢复仍用 `dialogRegistry` + 克隆 `content` | **可删除**：U8 落地后失败不关图层，`setErrors` 原地回填 |
| K4 | 插件 schema 注入 no-op `api` | **可删除**：U6 落地后改用 `submitMode:"event"` |

## 五、已关闭（编号不复用）

| 编号 | 结论 |
| --- | --- |
| U1–U4 | 1.8.x 版本线已实现：`confirm` 图层 / `property` / `setErrors` / `createUi` 本地化 |
| U5 | `a707369` 已实现「浮层归属图层根」，宿主不得再注入层叠补丁 |
| U11 | 本轮落地：`@budding/schema-lint` + CLI + MCP `lintSchema` + 内核 `diagnose({ policy })`（详见 §二） |
| 旧 A1–A5 | 随 U1–U4 消解或转入宿主侧 K2 / 主题迁移 |

## 六、升级自查（下游可直接照做）

1. 删除插件 schema 的 no-op `api` 注入（K4），保存链改为 `submitMode:"event"`。
2. 删除弹窗失败重开的 `dialogRegistry`（K3），失败改为 `onError` → `setErrors`，字段文案可用 `${error.payload...}` 直接映射。
3. `confirm` 的 `level` 全部替换为 `color`。
4. 复杂 `when` 由后端布尔字段改回 `${...}` 表达式；`{{...}}` 写法全部替换。
5. E2E 选择器改用 `data-bud-role` / `data-testid`。
6. 宿主白名单迁到 `budding-lint` policy，CI 跑 `budding-lint --strict`。
7. 长表单显式设置 `submitErrorFocus: "focus"`。
8. 若使用多表单弹窗，页脚按钮改用 `{"type":"submit","props":{"form":"<id>"}}`。
