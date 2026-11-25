# 与现有契约的关系（§4）

> 上级索引：[SDUI_BUDDING_INTEGRATION.md](../SDUI_BUDDING_INTEGRATION.md) ｜ 拆自 `docs/sdui/SDUI_BUDDING_INTEGRATION.md` 原 §4（2026-11-16 整理，正文未改）。

---

## 4. 与现有契约的关系

### 4.1 URL 契约（继续有效）

| 条款 | 状态 | 说明 |
|---|---|---|
| 安全入口前缀 `/{p}/...` | 继续有效 | 壳页、静态资源、JSON 都必须带前缀；根路径旁路继续 404 |
| 禁用 PUT/PATCH/DELETE | 继续有效 | 动作一律 POST + 末段动词 |
| 动词白名单 | 继续有效 | 新增动作先登记 |
| GET 只读、POST 写入 | 继续有效 | 轮询与下载用 GET |
| 旧路径 301/308/303 | 部分调整 | 切换后不保留旧页；`301`/`308` 仅用于入口前缀迁移，`303` 不再用于 JSON 动作 |
| 查询参数只读、写参数进体 | 继续有效 | 分页新增 `limit` / `cursor`，先登记再实现 |
| 页面与页面内接口同构 | 部分调整 | 页面在 `/{p}/...`，JSON 统一在 `/ui/v1/...`（D2）；两者的资源归属靠路径映射表对齐，不再要求同前缀 |

### 4.2 UI 契约（部分继续有效）

| 条款 | 状态 | 补条款 |
|---|---|---|
| 每页一个主操作 | 继续有效 | 由页面 schema 固定，动作清单只提供可用性 |
| 危险操作二次确认 | 继续有效 | 内核只给 `dangerous`；前端用 `confirm` 动作或等价弹窗，文案归前端 |
| 空态说明原因与下一步 | 继续有效 | 内核需在 `data` 里给「为什么空」的结构化原因（如 `empty_reason`），不能只给空数组 |
| flash 不进 URL | 继续有效 | 替换为 `notices`；内核 `FlashKind` 需扩展为 `success`/`warning`/`error`/`info` 四级 |
| 工具栏/表格/窄屏 | 继续有效 | 由前端组件承担，内核不感知 |

### 4.3 菜单契约（继续有效）

- L1 固定 7 项（`overview`/`sites`/`docker`/`plugins`/`tasks`/`tools`/`settings`）不变；L2/L3 的声明、稳定键、角色/gate、偏好持久化仍归内核。
- L3 标签栏**仍由内核决定数据**（菜单声明 + 当前视图），只是渲染从 askama 换到 budding。
- budding 默认包**不含** `admin` 组（`admin` / `nav-menu` / `page-tabs`），`chart` 也不在默认包；清单与上游文档存在漂移（§7.1）。菜单壳由上游 admin Pack 提供（§6.3 R-1）。
- 菜单偏好（顺序/隐藏/固定/布局）继续存内核，不迁到 `localStorage` 作为唯一来源。

### 4.4 插件 UI 契约（随包 Schema 原生渲染）

- 插件功能页以**随签名包分发的静态 schema**（`ui/schema/<view>.json`）交付：manifest 的 `[[menu.items]]` / 单视图 `[[menu]]` / `[[mount]]` 用 `schema` 字段声明，`api = "1.3"` 起生效；安装期由 `skiff-pkgfmt` 完成静态校验。
- 校验边界（宿主侧自担，不依赖上游）：节点类型白名单、禁止外链资源与外部 URL、禁止 `api`/`url`/`html` 等绕过字段、动作白名单（`skiffPluginRpc` / `skiffPluginView` / 表单 `submit`）、节点数/深度/字节数上限。schema 本身没有网络能力。安装期静态校验之外，`GET /{p}/plugins/{id}/schema/{view}` 运行期仍会重新解析；失败按 **400 `shape.plugin_schema`** 返回（输入包内容不成立），不再归因成内核 500。
- 数据与写操作：壳页按 schema 的 `data.<name>.method` 调 `POST /{p}/plugins/{id}/rpc`；写操作只能经 `skiffPluginRpc`，沿用 CSRF 头、权限矩阵与逐次 `plugin.rpc` 审计。schema 不允许直接写 URL。RPC 成功返回 `{ result }`，失败返回统一 JSON 错误信封（`{ error: { code, message } }`），壳页直接展示 `error.message`；旧 iframe 桥先按其 JSON 解析、再回退 HTML 文本，行为不变。
- 绑定语法：`{{data.<path>}}` 取已取回的 RPC 数据，`{{query.<key>}}` 取地址栏参数，`when` 支持 `{{data.x}}` / `!{{data.x}}`；budding 自己的 `${row.field}` 行内插值保持原样。
- 权限口径：`GET /{p}/plugins/{id}/{view}`、`GET /{p}/plugins/{id}/schema/{view}` 与 RPC 一致，均要求 `PluginManage`。**权限不足返回 403，不伪装成 404**（不隐藏资源存在性），因此 viewer 从旧用例的 404 变为 403，这是口径修正而非路由消失。
- iframe 工作台只保留显式 `?legacy=1` 一条通路（`/{p}/plugins/{id}/{view}?legacy=1`）与包内静态托管 `GET /{p}/plugins/{id}/ui/{path}`；页面级 SSR 已全部删除，面板内无任何入口指向它。
- 一次性 nonce、`skiff.rpc` / `skiff.theme` / `skiff.height` / `skiff.open`、`PLUGIN_UI_CSP` 只作用于该 iframe 工作台，与壳页 schema 渲染互不影响。
- 槽位（`[[mount]]`）已支持 `schema` 声明与安装期校验；新壳仍没有通用槽位渲染 API。`overview.health` 由内核改为「取数 + 原生渲染」：`GET /ui/v1/overview` 返回启用插件的 `health_plugin`，壳页对其调 `snapshot` RPC 后用卡片渲染 CPU / 内存 / 负载 / 运行时长，失败降级为说明性 `alert`；不是 iframe，也不是通用槽位渲染器。
- 组件缺失时不自造渲染：例如文件管理需要的树/编辑器组件仍缺。因此文件管理不做 schema 页——全盘管理、回收站、配额、上传仍只在内核「工具 → 文件管理」（`core/files`），不引入第二套文件管理器。
- 已原生化的只读批次：guard（containers / events）、monitor（overview / history / process / network）。monitor 的进程与网络页把当前快照一次取回（RPC `all: true`，进程仍受 512 PID、连接仍受 5000 条上限），排序 / 表头筛选 / 分页交给 budding 表格本地完成；展示文案由插件派生，schema 只做绑定。两页都没有写入口。
- 已原生化的写批次：firewall（rules / fail2ban）。表单内按钮的 `skiffPluginRpc` 会合并表单值（空值不发送），仍走 `POST /plugins/{id}/rpc`，CSRF、`PluginManage`、逐条审计不变；行内删除 / 解封用 `${row}` 插值。`skiffPluginView.query` 只接受标量，由壳页拼到同一插件页的 query，用于「查看某 jail 的封禁」这类只读下钻；schema 仍不能写 URL 或直接 `fetch`。数值字段由插件归一化（字符串 → 整数），内核参数校验不放宽。
- ftp（accounts / runtime）同样原生：账号新建 / 重置密码改为页内表单（schema 没有弹窗与二次确认，页面已明确提示）；`status` / `sites` / `accounts` / `protection` 的展示文案与下拉选项由插件派生。防火墙插件缺失时 `protection` 返回原因、`protection.banned` 返回空列表，FTP 页面不因可选依赖而整页失败。
- cron（list）原生：列表 + 新建/编辑共用一个 schema，`?id=` 走 `detail` 预填；`params_json` 文本域由插件解析为对象；执行/启停/删除/创建/更新仍走统一 RPC。budding 已支持 `visibleOn` / `disabledOn`，但插件 schema 白名单与动作白名单不允许 RPC 回填表单值，原生表单仍只收 5 段 cron，预设编辑器保留在旧界面 `?legacy=1`；内置与其它插件任务只读，编辑/删除由后端拒绝。
- files（main）原生：站点级文件浏览 + 文本编辑。`list` 把当前目录拆成 `dirs` / `files` 两表（插件 schema 白名单暂不放行按行 `visibleOn`，避免对目录显示「编辑」）；文件正文经 `read_text` 返回 `text_literal`（按 budding `\${` 规则转义，避免模板求值改写正文），textarea 用原生 `readonly`，保存仍提交原文并走 `write` RPC。超过 256 KiB 或非 UTF-8 的文件只读展示并提示改用内核文件管理；语法高亮、上传、新建、重命名、删除、回收站与配额不在插件页复制。默认入口是原生页，旧完整工作台只保留显式 `?legacy=1`，菜单与列表不再链接。

### 4.5 需要补的条款（新增）

1. JSON 命名空间与版本位置：`/ui/v1` 写入 URL 契约 §2 例外表与 §9 路由清单。
2. 动作成功不再 303，改为状态码 + `notices`；旧页兼容期仅在切换前存在，D7 定案后不保留双栈。
3. `/ui/v1/*` 的前缀豁免、401 语义与鉴权口径（与 `/api/v1` 并列）。
4. 字段码表与表单节点 id 的映射规则。
5. 列表 `limit`/`cursor` 参数登记。
6. 空态原因字段（`empty_reason`）的语义与枚举。
7. 前端深链回落的 404 边界（D13 已定案，见 §4.5.1）。
8. 静态产物版本化与缓存策略（D11）。

#### 4.5.1 已同步到 `URL_CONTRACT.md` 的条款（2026-11-12）

以下条款已写入 [URL_CONTRACT.md](../../contract/URL_CONTRACT.md) §2、§2.1、§6、§9；本节只保留结论摘要，不再作为待办。

- **§2 例外表**：`/ui/v1/...` 无入口前缀，会话 Cookie + CSRF，未认证 401，仅 `handlers::ui`（按资源拆分），只返 JSON。
- **§2 列表**：`/ui/v1/` 与 `/api/v1/` 并列；`/ui/` 下其它形态一律 404，不得作为静态资源或页面旁路。
- **§2.1（新增）SPA 壳页回落白名单**：仅 `GET` + `Accept: text/html` + 路径命中已登记页面前缀时返回壳页；白名单之外仍 404；`POST` 等写请求永不回落；白名单变更必须同步契约与测试。
- **§6 方法与副作用**：`/ui/v1/*` 写操作一律 POST + CSRF；`GET /ui/v1/bootstrap` 与 `POST /ui/v1/session` 为未认证端点（登录用签名 CSRF token + 来源判定）；`session/logout`、`settings/password`、`settings/totp/setup`、`settings/totp/enable` 为登录态自服务入口（仍校验 `x-csrf-token`），页面侧不再有对应的 HTML 表单路由。
- **§9 路由清单**：登记 `/ui/v1` 与壳页回落两类。

D13 结论：地址栏保持 `/{p}/sites/xxx` 形态，history 路由 + 白名单前缀回落，不使用 hash；白名单内容以 `URL_CONTRACT.md` §2.1 为唯一来源。

---
