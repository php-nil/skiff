# 落地差异与附录（§11–13 + 附录 A/B）

> 上级索引：[MENU_CONTRACT.md](../MENU_CONTRACT.md)。P1–P3 已全部落地；逐轮落地记录、与计划的差异和测试证据已删（过程见 `docs/changes/`），此处只留现行差异与注册表。

## 11–13. 现行差异（摘要）

- **L3 标签栏只对旧 SSR 插件工作台有效**（`.workbench-tabs` + 真实深链）；2026-11-13 原生化后新壳以页内按钮行承担 L3 入口（见入口状态行）。
- **菜单设置只列 L1/L2**：L3 由声明决定，不单独隐藏/排序；拖拽排序与管理员全局布局策略仍未做。
- **固定页槽位**只实现 `overview.*` 与 `sites.header` 通用渲染，当前官方插件只声明 `overview.health`。
- **内核「工具 → 文件管理」**：回收站并为 L3 `core/files/trash`（二级为「文件管理 / 配额管理」），旧入口 `/{p}/files/trash` 与深链保留；回收站/配额页现为 budding 壳页。

## 附录 A：内置一级与默认 L2 注册表

| L1 id | 标签 | 类型 | 默认 L2 |
|---|---|---|---|
| `overview` | 概览 | 直达 | —（页面内槽位） |
| `sites` | 网站 | 分组 | 内核固定：网站列表 / 运行环境 / Web 服务（不依赖插件） |
| `docker` | Docker | 分组 | 概览 / 容器 / 镜像 / 网络 / 卷 / 编排 / 设置（概览、编排、设置 P3，`route_available=false` 前不渲染） |
| `plugins` | 插件 | 分组 | 已安装 / 发现 / 可更新 |
| `tools` | 工具 | 分组 | 插件声明；无提供者显示空态 |
| `settings` | 设置 | 分组 | 安全设置 / 菜单设置 / 界面 / 审计日志 |

内置图标集（v1）：`dot`、`overview`、`globe`、`docker`、`plug`、`tools`、`settings`、`file`、`clock`、`activity`、`shield`、`container`、`image`、`network`、`disk`、`archive`、`list`、`user`、`lock`。

## 附录 B：非目标

- 本文件不实现任何代码、不改模板/样式/后端/测试。
- 不定义 Docker Compose 领域能力；编排页只在 P3 且后端具备后出现。
- 不改变插件安装/授权/验签流程；菜单声明校验只是 manifest 校验的新增部分。
- 不允许插件自定义一级、任意 HTML/CSS 导航、外部图标或外链资源。
