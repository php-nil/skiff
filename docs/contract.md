# 契约索引

> 契约是实现的唯一依据：动手前先对契约；实现与契约不一致时，改实现或同步修订契约，不在代码里留「另一套事实」。
> 正文在 [`contract/`](contract/)。

## 契约总则（共性）

- **契约与实现分账**：契约写「应该怎样」，代码实际行为与契约的差异只记 [`impl/`](impl/) 的 `*_IMPL_NOTES.md`，不写回契约。
- **菜单键稳定、URL 规范、菜单键 ≠ URL**：改 URL 不改菜单键，改菜单键不改 URL 契约；插件页面与 iframe 分工另见 [PLUGIN_UI_CONTRACT.md](contract/PLUGIN_UI_CONTRACT.md)。
- **内核四件套**（fs / scheduler / ftp / firewall）契约各自独立成篇；授权判定、路径代数、动词分发等唯一来源见 AGENTS.md §4.1。
- 每篇契约头部带「状态」（设计口径 / 已落地 / 契约版本）；契约自身的修订流水不入正文，见 `docs/changes/`。

## 子文档

| 子文档 | 管什么 | 什么时候读 |
| --- | --- | --- |
| [URL_CONTRACT.md](contract/URL_CONTRACT.md) | 页面、接口、动作的路径与动词，方法与副作用（分 2 册） | 改 URL / handler / 表单动作 |
| [MENU_CONTRACT.md](contract/MENU_CONTRACT.md) | 菜单归属、声明、渲染与持久化（分 5 册） | 加菜单项、调归属 |
| [PLUGIN_UI_CONTRACT.md](contract/PLUGIN_UI_CONTRACT.md) | 插件信息页/功能页、`view`、iframe 与 CSP | 改插件页形态 |
| [UI_CONTRACT.md](contract/UI_CONTRACT.md) | 标题、提示、工具栏、空态、危险确认 | 改内核页面或文案 |
| [SITE_RUNTIME_CONTRACT.md](contract/SITE_RUNTIME_CONTRACT.md) | 站点模型、建站类型、PHP/Node 生命周期（分 5 册） | 改站点 / 运行环境 |
| [KERNEL_FS_CONTRACT.md](contract/KERNEL_FS_CONTRACT.md) | fs 授权、操作面、回收站与配额（分 4 册） | 改文件管理 / 授权 |
| [KERNEL_SCHEDULER_CONTRACT.md](contract/KERNEL_SCHEDULER_CONTRACT.md) | 任务模型、调度、回执与插件唤醒（分 3 册） | 改计划任务 |
| [KERNEL_FTP_CONTRACT.md](contract/KERNEL_FTP_CONTRACT.md) | ftp 能力面、账号、uid/gid | 改 FTP |
| [KERNEL_FIREWALL_CONTRACT.md](contract/KERNEL_FIREWALL_CONTRACT.md) | firewall 能力面、作用域与审计 | 改防火墙 |
