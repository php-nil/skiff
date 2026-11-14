# budding 上游缺口清单

> 核对：2026-11-14，产物 **1.8.0-r4**（仓内 `crates/web/static/budding/` 的 `budding-1.8.0-r4.min.js`、`budding-code-1.8.0-r4.min.js`、`budding-icons-1.8.0-r4.woff2`）。上游答复见 [BUDDING_UPSTREAM_BACKLOG-UP.md](BUDDING_UPSTREAM_BACKLOG-UP.md)。

## 删除说明（编号留空不复用）

- **T17**：上游 1.8.0-r4 已支持 `visibleOn` / `hiddenOn` / `disabledOn`（随表单值与行作用域求值，含诊断）。宿主插件的接入待办见下方 **K1**。
- **T18**：上游已支持 `\${` 字面量转义与 `textarea.readonly`；明确不做 textarea code 模式（用 `code` 组件）。files 插件改为 `text_literal` 绑定 + 原生 `readonly`。
- **T19**：图标子集 78 字形已含 `image-outline` / `harddisk` / `folder-outline`，宿主改用原生字形。
- **T11**：原生页没有表格多选/批量操作需求；文件页用 `virtual-list` + 单行操作，站点/容器/任务/作业列表的 `rowClick` 只做单条跳转。
- **T12**：`file` 组件已在原生文件页使用；宿主 `PlatformAdapter.upload` 按 256 KiB 分片顺序上传，当前 `multiple: false`，暂停/续传/多文件独立进度暂不需要。
- **T13**：`code` 组件仍在 7 处使用（vhost 配置、文件查看、站点/任务/运行环境日志，容器详情与容器日志页各一处）；文件查看受 256 KiB 上限约束，日志均为尾部窗口读取，单次内容规模可控；nginx / toml / dockerfile 走通用高亮回退，可读性够用。

## 后续项（宿主待补契约，非上游缺口）

### K1 插件 schema 接入 budding 条件能力

- **上游已具备**：1.8.0-r4 支持节点 `visibleOn` / `hiddenOn` 与组件 `disabledOn`，随作用域响应式重算；表格 `rowActions` / `expand` / `columns[].cell` 的行作用域含 `{ ...record, record, item, index }`。
- **当前缺口（宿主侧）**：`skiff-pkgfmt` 的节点白名单仍只允许 `when`（`crates/pkgfmt/src/schema.rs`）；动作白名单也不允许 RPC 返回后回填表单值。
- **受影响面（仅官方插件页两处）**：cron 预设编辑器（预设 / 自定义 5 段表达式切换）、files 的目录 / 文件分行（行内 `visibleOn`）。内核自己的页面不受限，已在用新能力。
- **补齐需要动**：`skiff-pkgfmt` 节点白名单 + 官方插件 schema + 安装期校验测试；属于内核契约变更，不是上游继续支持的问题。
