# 桥接（二）：代码查看器、日志查看器、容器终端、日志入口（§5.18–5.20）

> 上级索引：[06-BRIDGE-PAGES-B.md](../06-BRIDGE-PAGES-B.md) ｜ 主题入口：[SDUI_BUDDING_INTEGRATION.md](../../SDUI_BUDDING_INTEGRATION.md) ｜ 正文自 2026-11-26 拆分前原文件逐字搬入，未改内容。

---

### 5.18 代码查看器与日志查看器（第十三批）

- 代码查看器：文件管理里点文本文件进入 `GET /{p}/files/view?path=`（budding 隐藏路由），数据来自 `GET /ui/v1/files/read?path=`（`FILE_READ`，仅 Admin）。响应 `{path,text,size,truncated,max_edit_bytes}`；渲染用 `code` 组件（1.8.0 起渲染器类型名为 `code`，旧 `code-viewer` 已不再注册；文本字段为 `modelValue`，旧 `code` 字段组件不读取；只读、行号、复制、固定高 560），语言按扩展名映射；疑似二进制（含 NUL）只提示不渲染。纯前端高亮，不引 WASM。
- 代码 Pack：入库 `budding-code-1.8.7.min.js`（90,331 B，取自 `dist/packs/code.mjs`，自包含 ESM；与 1.8.6 逐字节相同），启动脚本在 `embed` 前用 `BuddingSDK.registerPack('code', …?v=<ver>)` 注册；渲染器类型名同为 `code`，文本字段为 `modelValue`。未注册或加载失败时 `code` 组件退化为纯文本。CSP 仍是 `script-src 'self'`，动态 import 同源可用。
- 站点日志：`GET /{p}/sites/{domain}/logs` 交壳页（服务端已无 SSR 日志页）。数据来自 `GET /ui/v1/sites/{domain}/logs?part=access|error&tail=200&q=`，复用 `AppState::site_logs`（尾部 2MB 窗口、最新在前），权限与旧页一致（登录即可读）。页面用 `code` 组件定高 560，`tail` 可选（上限 2000）、关键字只过滤该窗口，访问/错误日志切换与刷新都是页面级按需拉取，不建 SSE。
- 作业日志：`GET /{p}/tasks/jobs/{id}/logs` 交 budding 日志页，页面骨架与正文都由 `GET /ui/v1/jobs/{id}/logs?with_schema=true` 给出（`JOB_LOGS`）：服务端按 `after` 游标最多聚合 10 页（每页 500 行），合成 `HH:MM:SS [stream] line` 文本后由 `jobs_page::job_logs_schema` 出 `code` 查看器，`truncated` 时在计数行提示「可刷新继续」。`/{p}/tasks/jobs/{id}/logs/stream` 的 SSE 尾随仍在（同权限、同审计），但页面按需加载，未接页内实时流。作业详情首屏日志仍用 `code` 组件，取消 / 重试走 `/ui/v1/jobs/{id}/cancel|retry`。
- 站点详情页新增「查看访问 / 错误日志」入口；旧 SSR 日志页不再从面板导航。

### 5.19 容器终端（第十四批）

- 路由：`GET /{p}/docker/containers/{id}/terminal`（budding 隐藏路由），入口在容器详情的「打开终端」。**不使用 iframe，也不自建 xterm**。
- 通道完全复用既有契约，本轮无内核改动：
  - `POST /{p}/containers/{id}/exec/ticket`（`AdminUser` + `x-csrf-token` + `Verb::Exec` 策略）签发单连票据，绑定「当前会话 + 容器」，30 秒有效、一次即废；
  - `GET /{p}/containers/{id}/exec?ticket=` 升级 WS，`Origin` 严格校验；
  - WS 的 start / fail / end 继续写 `Verb::Exec` 审计。
- 前端：页面先取票再渲染 `terminal` 组件；`wsUrl` 由 `location.protocol` 推出 `ws://` / `wss://`，`resizeFormat: 'none'`（内核 exec 只透传 stdin，不解析 resize JSON），`reconnect: false` + 手动「重新连接（重新取票）」。
- Pack：入库 `budding-terminal-1.8.7.min.js`（395,014 B，取自 `dist/packs/terminal.mjs`，自带 xterm 与样式注入，无裸依赖；与 1.8.6 逐字节相同），`BuddingSDK.registerPack('terminal', …?v=<ver>)`。
- **验证边界**：本机（Windows、无 Docker）只验证了路由、壳页与取票失败时的错误页；WS 真连与容器内 `/bin/sh` 交互必须在 Linux 测试机验收，未验前不得记为已完成。

### 5.20 日志入口收口（第十六批）

- 容器日志：`GET /{p}/docker/containers/{id}/logs` 从旧 SSR SSE 页改为 budding 日志页（尾部快照 + 刷新，不做实时流）；SSE 流只保留在 `GET /{p}/containers/{id}/logs/stream`，供旧 SSR 详情页 `data-src` 跟随。新增 `GET /ui/v1/containers/{id}/logs?tail=`（策略同旧 SSE：`Verb::ContainerLogs` → `DockerRead`；`tail` 默认 500、上限 2000），容器详情新增「查看日志」入口，旧版链接收窄为「资源限制编辑」。
- 运行环境日志：`GET /{p}/sites/runtimes/{id}/logs` 壳页 + `GET /ui/v1/runtimes/{id}/logs?tail=&with_schema=true`（Admin，复用 `AppState::runtime_logs`）；入口是详情页的「日志」子视图（§5.14 第二十三批已改为原生页，占位页与 `/runtimes/{id}/logs.json` 旧抽屉接口都已不存在）。
- 前端统一用 `code` 组件（`language: 'plaintext'`、固定高度、行号、可复制），刷新走整页重载；不新增 SSE/WS 契约，也不在前端造分页或流式缓冲。
- `?legacy=1` 现状：仅插件 iframe 工作台与站点日志旧 SSR 两处保留（前者是插件嵌入兼容，后者是无 JS 兜底）；本轮未新增 legacy 分支。
- 验证：`main_flow` 覆盖「页面交壳 + JSON 未认证 401 / 无 Docker 503 / viewer 403 / 实例不存在 400」；无头浏览器实测三个页面（容器日志错误页、运行环境日志错误页、运行环境详情页）均无 iframe、无脚本错误。

