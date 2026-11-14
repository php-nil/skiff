# budding 上游缺口清单

> 核对：2026-11-14，产物 **1.8.0-r3**（仓内 `crates/web/static/budding/` 的 `budding-1.8.0-r3.min.js`、`budding-code-1.8.0-r3.min.js`、`budding-icons-1.8.0-r3.woff2`）。

### T17 `when` / `disabled` 随表单值与行作用域求值

- **要什么**：`when` / `disabled` 能在表单值变化时重算，并能按表格行作用域求值。
- **为什么卡**：Cron 新建/编辑无法在「预设调度」与「自定义 5 段表达式」之间随选择显隐，只能收单一 `expr` 输入；任务列表无法按行隐藏内置任务/其它插件任务的「编辑/删除」，只能保留按钮再由后端拒绝。
- **期望的上游改动**：表单内随 values 重算；`rowActions` / 单元格按行求值；同一作用域支持 `disabled`；非法表达式降级为显示并只上报一次；文档写清作用域与重算边界。
- **当前临时做法**：拆成多个表单/多张表；插件派生布尔字段（`editable` / `can_enable` / `show_banned`）；不适用操作保留按钮，失败时展示内核错误。

### T18 文本字面量绑定（不解析 `${...}`）与 `textarea` 只读/代码模式

- **要什么**：文本类节点能把内容按字面量绑定（不做 `${...}` 求值），`textarea` 支持只读与代码模式。
- **为什么卡**：站点文件插件（`com.skiff.files`）编辑 Shell / JS / 模板时，正文里的 `${VAR}`、`${1}` 会被当作模板求值，显示或保存前可能被改写；`textarea` 没有只读态，只能隐藏保存按钮再由后端拒绝。
- **期望的上游改动**：新增显式字面量绑定（如 `textLiteral` / `valueLiteral` 或 `literal: true`），对 `text`、`textarea`、表格单元格与表单默认值语义一致；`textarea` 支持 `readonly` / `code`（等宽、保留空白、可选行号，高亮可选）；文档写清两种绑定的边界。
- **当前临时做法**：插件返回 `text_safe`，把 `${` 转义为 `${'${'}` 后绑定；保存仍提交原文。超过 256 KiB 的文件只读展示并提示用内核文件管理。

### T19 图标子集补齐 `image` / `hard-drive` 字形

- **要什么**：图标子集 allowlist 补 `mdi-image-outline`（或 `mdi-image`）、`mdi-harddisk`，建议顺带 `mdi-folder-outline`。
- **为什么卡**：Docker → 镜像、工具 → 配额管理两个二级菜单没有图标，与同级项不齐；无字形时宿主只能留空，不能回退成 `mdi-xxx` 文本。
- **期望的上游改动**：allowlist 增加上述字形，重新生成版本化 woff2 与 CSS，并在 manifest 记录版本。
- **当前临时做法**：`folder` / `disk` 用 `mdi-file-document` / `mdi-archive-outline` 就近兜底；`image` / `hard-drive` 映射为 `null`，不下发 `icon`。
