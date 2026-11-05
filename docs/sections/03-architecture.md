# 3. 总体架构

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§3 ｜ 上一章：[§2 技术选型](02-tech-stack.md) ｜ 下一章：[§4 扩展体系（一）](04-plugin-package.md)

## 3.1 分层

```
浏览器 (askama + htmx + 懒加载 ESM)
        │ HTTPS / WS / SSE
skiff-core (单进程 · systemd · 宿主机)
  中间件(axum layer，由内到外): 请求超时(非流式路由) → 可信代理解析 → 会话 → 安全入口 → CatchPanic
  handler 内: CSRF → RBAC(policy) → 审计；登录限流(同 IP 5 次/5 分钟)
  应用上下文: AppState/WebConfig（db · docker · supervisor · policy · 限流表 · exec 票据）
                                              ← 装配层（crates/app）
  api::v1 │ ui(SSR) │ static(embed)             ← 核心① Web 管理服务（crates/web）
  Capability Broker (鉴权/审计/配额；能力面 docker/identity/fs/net) ← 服务端在 core/src/kernel/，能力面实现在 crates/plugin
  Plugin Supervisor (签名/拉起/退避/熔断/回收/降权/沙箱) ← 核心③ 插件体系（crates/plugin）
  docker (bollard/三层策略/路径校验/Compose/exec/日志) ← 核心② Docker 管理（crates/docker）
        │ Unix Socket /run/skiff/ + JSON-RPC 2.0
插件进程 (宿主机原生, 语言受支持矩阵约束, 按需启动, 空闲 300s 回收)  ← 核心③ 第三方侧（crates/sdk 出包）
```

## 3.2 请求路径（重启容器）

`POST /api/v1/docker/containers/{id}/restart` → 中间件（会话/安全入口）→ handler 内 `policy.check` + 审计 → `Docker::execute` → `bollard` → Engine API → `202` → htmx 局部替换状态徽章。

## 3.3 内核原子能力（MVP）

| 能力 | 内容 |
|---|---|
| `docker` | 容器/镜像/网络/卷/Compose/日志流/exec 终端（白名单见 §6） |
| `identity` | 用户/会话/TOTP/API Token/RBAC |
| `fs` | 路径白名单内文件读/写/列表/建目录/删除/重命名（白名单 = 插件自身目录 + 用户授权目录，Broker 逐次校验；无独立 `stat` 动词） |
| `net` | 主机防火墙端口规则（放行/拒绝；nftables 适配，探测 firewalld/ufw；按 UID owner-match 收口插件出网）——**尚未实现**：`net.apply` 一律返回 501，属 M4 门槛 |

> **内核之外一律插件**（唯一例外是已内核化的「网站」）。文件管理器/防火墙管理/主机监控等是**插件层产物**（宿主能力面 `docker`/`identity`/`fs` 已开放，`net` 待 M4，均按 `requires` 点单），内核不内置。网站（站点列表 / 运行环境 / 设置）已于 2026-11-04 从 `com.skiff.nginx` 插件迁入内核，内核直接编排 nginx + PHP-FPM 容器。官方首批 4 个插件已落地：`guard` 走 `docker`，`files` / `monitor` 走 `fs`，`cron` 走 `interop`（见 [PLUGIN_CATALOG.md](../PLUGIN_CATALOG.md)）。

## 3.4 Web UI 导航结构与信息架构（最终版）

顶栏只列**真实存在页面路由**，没有的能力不出现（Compose 只有领域 crate、没有 Web 页面，不挂；没有「官方源」页面，就不造这个二级）。导航结构的唯一来源是 `crates/web` 的 `handlers::nav()`，模板只负责渲染。

核心原则是**插件「用 / 管」分离**：日常使用走「工具」，安装、授权、启停、更新走「插件中心」。

| 一级 | 二级 | 目标 | 说明 |
|---|---|---|---|
| 概览 | —（直接链接） | `/{p}` | 行动面板：健康块 + 按影响排序的待办 + 异常容器 + 最近审计 |
| 网站 | 网站列表 / 运行环境 / Nginx | `/{p}/sites`、`/{p}/sites/env`、`/{p}/sites/nginx` | **内核自带**（1Panel 风格的站点闭环：环境安装、建站、启停、删除、日志）；原 `com.skiff.nginx` 插件已内核化 |
| 工具 | 各「已启用且有 UI 入口」插件的 `[[ui]].label` | `/{p}/plugins/{id}` | 只列已启用插件；未启用/无 UI 不出现死入口；**0 项时显示可行动空态**；固定顺序 文件→任务→监控→守护 |
| Docker | 容器 / 镜像 / 网络 / 卷 / 设置 | `/{p}/containers`、`/{p}/images`、`/{p}/networks`、`/{p}/volumes`、`/{p}/docker/settings` | 名称保持「Docker」；设置页承载镜像加速源与镜像前缀 |
| 插件中心 | —（直接链接） | `/{p}/plugins` | 页内「发现 / 已安装 / 可更新」三个 tab；启停、更新、卸载只在插件卡片上 |
| 设置 | 安全设置 / 审计日志 | `/{p}/settings`、`/{p}/audit` | 只保留两个真实页面，不再用页内锚点冒充二级 |

配套口径：

- **概览只接真实数据**：面板运行时长、Docker 可用性、运行/总容器、网站站点数与异常站点、已启用/已安装插件、异常容器、最近审计。取不到的块（如插件「待重载」、可更新版本）不写死数字，明确显示空态/未接入说明。
- **待办按影响排序**：强制改密 / Docker 不可用 / 异常容器为危险级，停用插件 / 未开启 TOTP 为警告级；每条可深链到已筛选目标（如 `containers?filter=error`）。
- **应用入口对齐 manifest**：标签取 `[[ui]].label`，不另建映射表；未启用插件从导航消失，但仍可在插件中心卡片上启用。
- **网站工作台路径（内核自带）**：网站 → 网站列表 → 新建站点（域名/根目录/PHP 版本）→ 生成 vhost 并重载 nginx 容器 → 列表内启停/日志/删除；运行环境安装与镜像配置在「网站 → 运行环境 / 设置」。这些页面由内核实现，不依赖插件。

交互口径：

- 一级行 = **标签 + 独立箭头按钮**：箭头只负责展开/收起，不触发跳转；有自身页面的一级标签是链接（点标签跳转），无自身页面的一级标签是按钮（点标签等同展开）。展开采用**持久手风琴**：同一时间只展开一个分组，**点内容区/空白不收起**，只有切到另一个分组或按 Escape 才收起。展开状态写入 `sessionStorage`，刷新后保持；无记忆时默认展开当前页所在分组。**不存在任何 hover 展开逻辑**（无延迟计时器、无触摸设备判断）。
- 上下布局的二级是顶栏下方的**独立全宽通栏**：高度固定 44px、横向排列、左对齐，位置不随点哪一项移动，项多时横向滚动；当前项用下划线标识。侧栏布局为 248px，品牌区与用户区固定、只有中间菜单区滚动（6px 自绘滚动条，含 Firefox 与暗色配色）。
- 当前页命中二级项时，一级分组同步标记 `active`（`.nav-group.current`），顶部标签高亮；二级项加 `aria-current="page"`。
- 键盘可达：有自身页面的一级用 `<a>`，其余用 `<button>`（原生 Enter/Space 切换）；箭头用 `aria-controls` 指向二级面板，菜单关闭时带 `hidden` 不进入 Tab 顺序；展开后按 DOM 顺序（标签 → 箭头 → 二级项）流转，Escape 收起并把焦点还给箭头按钮。
- 上下布局、侧栏（left）布局与窄屏（≤860px）抽屉共用同一套行结构与交互语义；抽屉分组纵向排列，二级就近展开（不使用浮层），不出现横向溢出。
- 视觉系统在 `skiff.css` 顶部集中定义设计 token（颜色/间距/圆角/阴影/字号阶梯/控件高度），默认浅色，`html[data-theme="dark"]` 为完整暗色主题；无渐变装饰，克制阴影，`prefers-reduced-motion` 生效。
- 实现零依赖且不破坏 CSP：交互全部在 `/static/skiff.js`（外链同源脚本），无内联脚本、无 CDN、无外链字体。
