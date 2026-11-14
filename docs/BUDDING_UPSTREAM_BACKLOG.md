# budding 上游缺口清单

> 核对：2026-11-14，通过 budding 官方 MCP 实拉 `catalogVersion=1.8.0`（57 组件 / 20 动作 / 300 示例 / 44 指南）；仓内产物版本以 `crates/web/src/assets.rs::BUDDING_VERSION` 为准。
> 本文只保留**仍需上游支持**与**宿主侧待补契约**。已具备能力的用法见 [BUDDING_INTERACTION_CAPABILITIES.md](BUDDING_INTERACTION_CAPABILITIES.md)；接入边界与 JSON 契约见 [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md)。
> 需求状态：U1–U4 已确定为上游需求，未支持前用临时方案，不阻塞。编号沿用历史 `T` / `R` / `G` / `F` 体系，已关闭编号留空不复用。

## 一、上游需求（U1–U4）

| 编号 | 需求 | 当前替代方案 | 影响范围 | 阻塞 |
| --- | --- | --- | --- | --- |
| U1 | `confirm` 增加按钮文案（确认/取消）、危险语义（`error` 色）、输入型确认（要求输入指定文本） | 普通确认仍用 `confirm` 动作（需先做 K2）；输入型确认用 `dialog` + `form` + 服务端校验 | 站点删除（输入域名）、容器/任务/作业/回收站/插件卸载等高风险操作；当前 22 处 `window.confirm`、2 处 `window.prompt` | B2 |
| U2 | 新增 `descriptions` 只读详情组件（标签/值、跨列、可复制、响应式） | `card` + `flex` + `text` 手工拼两列 | 文件属性、容器/站点/运行环境详情、插件信息、作业详情 | B3 |
| U3 | 表单字段级错误入口（字段 `error` 入参，或 `setFieldErrors` 指令） | 宿主自定义动作读取 `error.field` 后写 `ctx.extras.form.errors[name]`；页面至少保留顶部错误 + `submitErrorFocus` | 站点 / 运行环境 / 登录 / 插件安装表单 | B2 |
| U4 | Vuetify 内置文案本地化：`createUi()` 接受 locale，或 `embed` 允许传 UI 配置 | 保留现有壳主题 CSS，接受 Vuetify 内部英文；不逐项覆盖节点文案 | 日期/时间、分页、空态、数据表内置标签等宿主 UI 文案；阻塞全站中文承诺 | B3 |

### U1 确认框

- **用途**：高风险操作需要明确的危险按钮与二次确认；站点删除还要求输入域名，避免误删。
- **MCP 现状**：`confirm` 参数只有 `title` / `content`；`platform.confirm` 签名同为 `{ title?, content? }`；上游没有内置样式化确认框，宿主未注入时退化为 `window.confirm`。
- **当前替代**：普通确认用 `confirm` 动作（取消抛 `ActionCancelled`，中止后续、不跑 `onError`、静默）；输入型确认用 `dialog` + `form`，把写动作放在弹窗按钮里，服务端校验输入值。
- **代价**：`dialog` 不阻塞动作链，替代方案会把一条动作链拆成「打开弹窗 + 弹窗内提交」，每个高风险操作重复一份表单。
- **最小建议**：`confirmText` / `cancelText` / `level`，或允许 `requireText`；或提供可 await 返回值的 `dialog`。

### U2 只读详情

- **用途**：文件属性、容器/站点/运行环境详情、插件信息、作业详情需要统一的「标签 / 值」展示与复制。
- **MCP 现状**：57 个组件中无 `descriptions` / `description-list`；`search` 只命中 `list` 的中文别名「列表」。
- **当前替代**：`card` + `flex` + `text` 手工拼两列；窄屏换行、复制按钮与标签宽度由各页自行维护。
- **最小建议**：`items[{ label, value, span, copyable }]`、`columns`、响应式断点。

### U3 字段级错误回填

- **用途**：服务端已有 `error.field`（字段码）与页面字段映射表，需要把错误直接落到对应控件。
- **MCP 现状**：字段支持 `required` / `rules` / `validateApi`，但没有 `error` / `errorMessages` 入参，也没有 `setFieldErrors` 指令；表单只有表单级错误与 `submitErrorFocus`。
- **当前替代**：宿主自定义动作读取 `error.field` 后写 `ctx.extras.form.errors[name]`，再触发一次校验；属于内部字段操作，不是稳定契约。
- **最小建议**：字段级 `error` 入参，或 `setFieldErrors` 指令（`{ field: message }`）。

### U4 Vuetify 内部文案本地化

- **用途**：宿主 UI 需要全中文；`embed` 的 `locale` 目前只覆盖 budding 自身文案（`t()` / `$t` / `data-bud-locale`），日期、分页、空态等由 Vuetify 渲染的文案仍是英文。
- **源码现状**：宿主 UI 入口 `packages/ui-vuetify/src/host.ts::createUi()` 仍是 `createVuetify()` 无参，`embed.ts` 调用 `module.createUi?.()` 也不传选项，Vuetify 实例沿用默认英文 locale。
- **当前替代**：保留现有壳主题 CSS，接受 Vuetify 内部英文；不逐项覆盖节点文案（维护成本高、容易漏）。
- **影响范围**：日期/时间选择、分页、空态、数据表内置标签等所有 Vuetify 自带文案；直接决定能否承诺全站中文。
- **最小建议**：`createUi()` 接受 locale 并传给 `createVuetify({ locale: { locale: 'zhHans' } })`，或 `embed` 允许传入 UI 配置/选项；具体版本取决于上游排期，升级后按 `host.ts` 复测。

## 二、宿主侧待补契约（不是上游缺口）

### K1 插件 schema 接入 budding 条件能力

- **上游已具备**：`visibleOn` / `hiddenOn` / `disabledOn` 随表单值与行作用域求值；行作用域含 `{ ...record, record, item, index }`。
- **当前缺口（宿主侧）**：`skiff-pkgfmt` 的节点白名单仍只允许 `when`（`crates/pkgfmt/src/schema.rs`）；动作白名单也不允许 RPC 返回后回填表单值。
- **受影响面**：cron 预设编辑器、files 目录/文件分行等官方插件页；内核自己的页面不受限。
- **补齐需要动**：`skiff-pkgfmt` 节点白名单 + 官方插件 schema + 安装期校验测试。

### K2 `platform.confirm` 接管与交互替换

- 当前 `embed` 的 `platform` 只注入 `loadJson` / `request` / `upload`（`crates/web/static/budding/budding-boot.js`）；未注入 `confirm`，`confirm` 动作会退化为 `window.confirm`。
- 已定：确认框呈现由宿主接管，和 `window.confirm` / `alert` / `prompt` 的替换一起排进下一批 UI 改造，由代码那路执行。
- 代码侧需要同时处理：`platform.confirm` 的呈现、22 处 `window.confirm`、2 处 `window.prompt`、89 处 `window.alert`、6 处 `confirm_text` 自定义动作参数。

## 三、我们在自己实现本可由组件承担的东西

> 判定口径：能用 budding 现有组件/动作直接表达，却在宿主 JS 或 SSR 模板里手写的部分。数量为 2026-11-14 对 `crates/web/static/budding/budding-boot.js` 的静态统计。

| # | 现状 | 证据 | 应改用 | 影响 |
| --- | --- | --- | --- | --- |
| A1 | 二次确认使用浏览器原生框 | 22 处 `window.confirm`、2 处 `window.prompt`（含站点删除输入域名）、6 处 `confirm_text` 自定义动作参数 | `confirm` 动作（普通确认）；`dialog` + `form`（输入型确认） | 原生框不受主题控制，移动端与键盘体验不一致；无法复用焦点陷阱与关闭策略 |
| A2 | 结果反馈使用阻塞式弹窗 | 89 处 `window.alert` | `toast` / `alert` / 表单 `errorMessage` | 成功提示也打断操作；错误信息与页面内表单错误脱节 |
| A3 | 旧 SSR 页面手写确认/表单弹窗 | 7 个模板共 16 个 `<dialog>`：`base.html`、`files.html`（9）、`jobs.html`、`networks.html`、`resources.html`、`site_env.html`（2）、`site_new_dialog.html` | `dialog` / `drawer` 动作 + `form` | 重复实现遮罩、Esc、焦点回落、滚动锁定；与 native 页面两套交互 |
| A4 | 详情信息手工拼两列 | 文件属性、容器/站点/运行环境详情等页面用 `card` + `flex` + `text` | 等 U2；短期保持 | 布局重复，窄屏与复制行为需逐页维护 |
| A5 | 主题主色手写 CSS | `budding-boot.js::applyShellTheme` 注入 `--v-theme-primary` 等变量 | 保留（已定）；Vuetify locale 另见 U4 | 品牌色与 Vuetify token 两套来源，升级时容易漂移 |

**没有发现重复实现的项**：表格、分页、虚拟列表、表单字段与校验、代码查看、终端均已使用 budding 组件；文件上传使用 `PlatformAdapter.upload` 分片，不再自建上传进度。

## 四、已关闭（编号不复用）

T11–T13、T17–T19、R-1、R-2、G4、G8–G10、G16 已由 1.8.0 能力覆盖或按产品口径关闭；其中 F5 重编号为 U2，G13 重编号为 U3，G7 的 budding 文案部分已覆盖、Vuetify 内部 locale 重编号为 U4。管理后台 Pack、图表、终端、代码、上传分片/进度/取消、`input-text` 的 `type` / `autocomplete`、`visibleOn` 系列条件能力均不再跟踪。
