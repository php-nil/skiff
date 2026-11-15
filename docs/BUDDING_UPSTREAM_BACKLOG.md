# budding 上游缺口清单

> 复核：2026-11-15。上游答复（1.9.0）确认 U1–U4 全部实现且向后兼容；仓内产物已升级到 `1.8.0-r6`（功能集 1.9.0）。当前**无未关闭的上游需求**，只保留宿主侧待办。
> 能力用法见 [BUDDING_INTERACTION_CAPABILITIES.md](BUDDING_INTERACTION_CAPABILITIES.md)；接入边界与 JSON 契约见 [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md)。

## 一、上游需求

无。U1–U4 的关闭结论与替代做法见 §四。

## 二、宿主侧待补契约（不是上游缺口）

### K1 插件 schema 接入 budding 条件能力

- **上游已具备**：`visibleOn` / `hiddenOn` / `disabledOn` 随表单值与行作用域求值；行作用域含 `{ ...record, record, item, index }`。
- **当前缺口（宿主侧）**：`skiff-pkgfmt` 的节点白名单仍只允许 `when`（`crates/pkgfmt/src/schema.rs`）；动作白名单也不允许 RPC 返回后回填表单值。
- **受影响面**：cron 预设编辑器、files 目录/文件分行等官方插件页；内核自己的页面不受限。
- **补齐需要动**：`skiff-pkgfmt` 节点白名单 + 官方插件 schema + 安装期校验测试。

### K2 确认框接管与交互替换

- **上游已具备**：`confirm` 已改为图层形态 `LayerInput.confirm`（`level` / `confirmText` / `cancelText` / `requireText`）+ 结果通道（`onClose` / `closeLayer`）；`platform.confirm` 仍优先，未注入时走内建确认层，纯内核装配才回退 `window.confirm`。
- **宿主侧待做**：确认框呈现可以由宿主接管，也可以直接用内建确认层；同时替换 22 处 `window.confirm`、2 处 `window.prompt`、89 处 `window.alert`、6 处 `confirm_text` 自定义动作参数，排进下一批 UI 改造。
- **说明**：上游能力已到位，剩余是代码那路的改造节奏，不再阻塞上游。

## 三、我们在自己实现本可由组件承担的东西

> 判定口径：能用 budding 现有组件/动作直接表达，却在宿主 JS 或 SSR 模板里手写的部分。数量为 2026-11-14 对 `crates/web/static/budding/budding-boot.js` 的静态统计。

| # | 现状 | 证据 | 应改用 | 影响 |
| --- | --- | --- | --- | --- |
| A1 | 二次确认使用浏览器原生框 | 22 处 `window.confirm`、2 处 `window.prompt`（含站点删除输入域名）、6 处 `confirm_text` 自定义动作参数 | `confirm` 图层；输入型确认用 `requireText`，不再自拼 `dialog + form` | 原生框不受主题控制，移动端与键盘体验不一致；无法复用焦点陷阱与关闭策略 |
| A2 | 结果反馈使用阻塞式弹窗 | 89 处 `window.alert` | `toast` / `alert` / 表单 `errorMessage` | 成功提示也打断操作；错误信息与页面内表单错误脱节 |
| A3 | 旧 SSR 页面手写确认/表单弹窗 | 7 个模板共 16 个 `<dialog>`：`base.html`、`files.html`（9）、`jobs.html`、`networks.html`、`resources.html`、`site_env.html`（2）、`site_new_dialog.html` | `dialog` / `drawer` 动作 + `form`；确认场景用 `confirm` 图层 | 重复实现遮罩、Esc、焦点回落、滚动锁定；与 native 页面两套交互 |
| A4 | 详情信息手工拼两列 | 文件属性、容器/站点/运行环境详情等页面用 `card` + `flex` + `text` | `property` 键值栅格（`items[].content` 支持字符串模板或嵌套 Schema） | 布局重复，窄屏与复制行为需逐页维护 |
| A5 | 主题主色手写 CSS | `budding-boot.js::applyShellTheme` 注入 `--v-theme-primary` 等变量 | `createUi({ theme })` 或运行时 `registerTheme`；8 token 桥自动同步 Vuetify 变量 | 品牌色与 Vuetify token 两套来源，升级时容易漂移 |

**没有发现重复实现的项**：表格、分页、虚拟列表、表单字段与校验、代码查看、终端均已使用 budding 组件；文件上传使用 `PlatformAdapter.upload` 分片，不再自建上传进度。

## 四、已关闭（编号不复用）

| 编号 | 上游结论 | 原需求 | 现做法 / 替代 |
| --- | --- | --- | --- |
| U1 | 1.9.0 已实现 | `confirm` 按钮文案、危险语义、输入型确认 | `LayerInput.confirm` + `level` / `confirmText` / `cancelText` / `requireText`；`onClose` / `closeLayer` 返回 `{ reason, value? }`，等待中的动作链不会悬挂 |
| U2 | 1.9.0 已实现 | 只读详情组件 | 组件名定为 `property`（`descriptions` 从未发布，废弃）；`columns` / `labelWidth` / `density` / `bordered` / `emptyText`，条目 `content` 可放字符串模板或嵌套 Schema，`copyable` 只对字符串生效 |
| U3 | 1.9.0 已实现 | 字段级错误回填 | 表单 `invoke setErrors`：`args.errors` 按字段 `name` 写入、`__form__` 落表单级；值变更或提交启动先到先清；`focus` 可聚焦首个错误；不再做字段 `error` 入参 |
| U4 | 1.9.0 已实现 | Vuetify 内部文案本地化 | `createUi(options)` 接受 `locale` / `theme` / `defaults` / `messages`，`setLocale` / `registerTheme` 运行时切换；embed 句柄增加 `instance`，`BundledEmbedOptions.uiDefaults` 可透传 |
| A5 | 随 U4 消解 | 主题主色手写 CSS | 改用 `createUi({ theme })` 或 `registerTheme`，见 §三 A5 的宿主迁移 |

T11–T13、T17–T19、R-1、R-2、G4、G8–G10、G16 已由 1.8.0 能力覆盖或按产品口径关闭；F5 重编号为 U2、G13 为 U3、G7 的 Vuetify locale 部分为 U4，均已随 1.9.0 关闭。管理后台 Pack、图表、终端、代码、上传分片/进度/取消、`input-text` 的 `type` / `autocomplete`、`visibleOn` 系列条件能力均不再跟踪。
