# 插件功能页去 iframe、改 Schema 原生渲染：可行性与工作量评估

> 状态：**评估稿（2026-11-13），未实施**。本文只做事实盘点、契约影响与工作量估算，不改任何代码与既有契约正文。
>
> **后续进展（2026-11-13）**：插件功能页原生化已按本文推荐路线落地（guard → monitor → firewall → ftp → cron → files），并由 `overview.health` 改为「RPC 取数 + 原生卡片」替代槽位渲染。本文中「插件菜单被丢弃」「files 保留占位」等现状描述已过期；当前口径以 [PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md) 与 [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md) 为准。
> 事实来源：`plugins/*/manifest.toml`、`plugins/*/ui/*`、`crates/web/src/handlers/plugins.rs`、`crates/web/src/lib.rs`、`crates/pkgfmt/src/manifest.rs`、`crates/web/static/budding/budding-boot.js`（逐条读代码确认，标注方式见 §6）。
> 相关文档：[PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md)、[MENU_CONTRACT.md](MENU_CONTRACT.md)、[SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md)、[URL_CONTRACT.md](URL_CONTRACT.md)。

---

## 0. 结论摘要

| 问题 | 结论 |
|---|---|
| 是否可行 | **可行**，且比预想的阻塞小：插件的数据面本来就是纯 JSON-RPC，缺的只是「页面描述」这一层 |
| 关键前提 | budding 的 schema 是**运行时数据**（`BuddingSDK.embed(mount, schema, options)`），壳已支持非本地 schema URL（`loadJson`） |
| 当前实际状态 | 新壳**已经把 `plugin/*` 菜单项整个丢弃**，插件功能页在新前端里不可达；直达 `/{p}/plugins/{id}/{view}` 落到占位页（`skiff:plugin-view`）。所以这不是体验优化，是**功能缺口** |
| 推荐形态 | **静态 schema 随包**（`ui/schema/<view>.json`，在签名包内、安装期校验），数据继续走现有 RPC，内核只做「转发 + 校验」，不组装布局 |
| 总工作量 | 平台一次性 **6–10 人日** + 5 个可迁移插件 12 个视图 **14–23 人日** ≈ **20–33 人日**（不含 files 完整重做） |
| files 文件管理页 | 现在不可迁移（树/编辑器/上传/虚拟滚动缺组件），建议**新壳保留占位**，旧 SSR 保留但明确不作为新入口 |
| 建议首批 | **guard**（2 个只读页，成本最低，先把读通路打通）；随后 monitor（4 页 + 1 槽位）；写通路在 firewall 批验证 |

---

## 1. 现状盘点

### 1.1 渲染方与路由（代码确认）

| 路由 | 渲染方 | 事实 |
|---|---|---|
| `/{p}/plugins`、`/{p}/plugins/{id}` | 内核壳（budding） | 已迁移；旧 SSR 保留在 `/{p}/m/core/plugins/*` |
| `/{p}/plugins/{id}/{view}` | 内核 SSR + iframe | 路由仍注册为 `handlers::plugins::view`，直达会出旧工作台 |
| `/{p}/m/plugin/{id}/{l2}[/{l3}]` | 内核 SSR + iframe | 菜单深链仍是旧页 |
| `/{p}/plugins/{id}/ui/{path}` | 内核托管静态资源 | `PLUGIN_UI_CSP`：`default-src 'none'`、`script-src 'self'`、`connect-src 'none'` |
| `POST /{p}/plugins/{id}/rpc` | 内核代发 | 需 `PluginManagerUser`（`Permission::PluginManage`）+ CSRF 头，逐次写 `plugin.rpc` 审计 |

插件 HTML 的沙箱属性固定为 `sandbox="allow-scripts allow-modals allow-forms"`，**不含** `allow-same-origin`：插件页拿不到面板 Cookie，也不能发同源请求。

### 1.2 数据通路（代码确认）

```text
插件 HTML(app.js) --postMessage skiff.rpc--> 父页面(SSR/壳) --POST /plugins/{id}/rpc(CSRF)--> Supervisor --UDS--> 插件进程
                                             <-- {"result": <JSON>} --
```

- 每个插件的业务方法返回的都是结构化 JSON（见 §1.3 的 RPC 列），**没有 HTML 片段**。
- 视图切换：guard/monitor/ftp/firewall 是「一个视图一个 HTML 文件」，L3 切换由父页面换 `iframe.src`；testkit 是「同一文件 + `data-view` + `skiff.menu` 消息」。
- `[[ui]].path` 只作插件中心元数据；视图 → `ui/<view>.html`；旧包 `main` 回落 `ui/index.html`。
- `[[mount]]` 槽位（monitor 的 `overview.health`）同样是 iframe，由 `mount_card.html` 包外框。

### 1.3 逐个视图事实表

「元素」列来自 HTML 结构；「读/写 RPC」列来自 `ui/app.js` 的 `rpc(...)` 调用与插件 `src/main.rs` 的方法分派。

| # | 插件 / 视图 | 资产 | 主要元素 | 读 RPC | 写 RPC | 难度 |
|---|---|---|---|---|---|---|
| 1 | guard `containers` | `containers.html` + 共用 `app.js`(155 行) | 3 张统计卡 + 1 张表（3 列）+ 刷新 | `status`、`config` | 无（重启由调度器 `reconcile` 驱动） | 低 |
| 2 | guard `events` | `events.html` + 共用 `app.js` | 1 张表（4 列）+ 刷新 | `status` | 无 | 低 |
| 3 | monitor `overview` | `overview.html` + `app.js`(358 行) | 7 张指标卡 + 刷新 | `snapshot`、`config`、`history` | 无 | 低 |
| 4 | monitor `history` | `history.html` + `app.js` | 1 张表（6 列，右对齐）+ 刷新 | `snapshot`、`history` | 无 | 低 |
| 5 | monitor `process` | `process.html` + `app.js` | 工具栏（搜索、用户过滤、每页 25/50/100/200）+ 可排序表（7 列）+ 上一页/下一页 | `processes`（含 `page/pageSize/sort/order/q/user`） | 无 | 中高 |
| 6 | monitor `network` | `network.html` + `app.js` | 工具栏（协议/状态下拉、搜索、每页条数）+ 表（6 列）+ 分页 | `network`（同分页参数） | 无 | 中高 |
| 7 | monitor `health`（槽位） | `health.html` + `health.js` | 4 个指标（CPU/内存/负载/运行） | `snapshot` | 无 | 低 |
| 8 | files `main` | `main.html`(29 KB) + `app.js`(1081 行) + `vendor/highlight.js` | 授权根快捷区 + 懒加载目录树 + 面包屑 + 上级/跳转 + 刷新/新建目录/重命名/永久删除 + 过滤 + 多选列表 + 右键菜单 + 4 个 `<dialog>` + 代码编辑器（行号/高亮/脏标记/保存）+ toast | `roots`、`list`、`read` | `mkdir`、`rename`、`remove`、`write` | **很高（当前不可行）** |
| 9 | cron `list` | `list.html` + `app.js`(1003 行) | 搜索 + 6 项筛选下拉 + 6 列表格 + 行内 4 动作（运行/启停/编辑/删除）+ 弹窗编辑器（预设、表达式校验与描述、目标选择、下次运行预览、表单回填） | `list`、`targets`、`config`、`schedule.describe`、`schedule.build` | `create`、`update`、`toggle`、`remove`、`run`、`reschedule` | 高 |
| 10 | ftp `accounts` | `accounts.html` + `app.js`(881 行) | 页头动作（安装/服务配置/新建账号）+ 服务状态行 + 搜索 + 7 列表格 + 行内动作（改密/删除/禁用）+ 2 个弹窗（改密、服务配置） | `status`、`sites`、`accounts`、`config` | `install`、`create`、`password`、`delete`、`toggle`、`config.save` | 中高 |
| 11 | ftp `runtime` | `runtime.html` + `app.js` | 状态 kv 网格 + 安装/重启/配置 + fail2ban 卡片（启用/停用/封禁列表/解封）+ 回收区（2 个复选框 + 危险按钮） | `status`、`config`、`protection`、`protection.banned` | `install`、`restart`、`uninstall`、`config.save`、`protection.enable/disable/unban` | 中高 |
| 12 | firewall `rules` | `rules.html` + `app.js`(412 行) | 后端状态卡 + 计数 + 6 列表格 + 行内删除 + 2 个弹窗表单（放行端口、自定义规则，含协议/动作下拉与校验） | `status`、`rules.list`、`config` | `rule.add`、`rule.remove`、`port.allow` | 中 |
| 13 | firewall `fail2ban` | `fail2ban.html` + `app.js` | 服务状态卡 + 动作区 + 8 字段 jail 表单（含 backend 下拉）+ jail 列表（封禁数、解封）+ 移除 | `fail2ban.status`、`fail2ban.banned` | `fail2ban.install/start/stop/uninstall`、`fail2ban.jail.ensure/remove`、`fail2ban.unban/ban` | 中高 |
| 14 | testkit（4 视图 + 槽位） | `main/check/group/single.html`、`mount.html` + `app.js`(234 行) | 架构验收页：多视图、单视图、槽位、RPC 成败、能力面、主题 | `ping`、`state`、`identity`、`fs_read`、`interop_catalog`、`interop_cron` | `fail`（故意失败） | 低（建议随平台批迁移，作为契约夹具） |

汇总（代码确认）：6 个官方插件共 **12 个功能视图 + 1 个槽位**；UI 脚本合计约 3,690 行；`rpc()` 调用点 62 处；`addEventListener` 155 处；`<dialog>`/弹窗 35 处。

### 1.4 容易误判的关键事实

1. **插件在新壳里已经不可达**（代码确认）：`budding-boot.js` 的 `menuRoute()` 对 `plugin/` 前缀直接 `return null`，注释写明「插件自带 HTML/iframe 的入口一律不进路由」；直达 `/{p}/plugins/:id/:view` 命中 `skiff:plugin-view` 占位，文案是「插件功能页正在切换为原生渲染」。
2. **新壳没有 L3 标签栏**（代码确认）：boot 中 `tabs` / `page-tabs` / `nav-menu` 命中 0；上游 admin Pack（D4）未交付。多视图插件今天只能「一个视图一条路由」，或在页内自绘链接行。
3. **新壳没有下拉/弹窗/多行文本/复选组件被使用过**（代码确认，boot 中 `dialog`/`modal`/`select`/`checkbox`/`textarea` 命中均为 0）：枚举字段用 `input-text` 承载（设置页 `tls_mode` 就是让用户手输 `auto/on/off`）。这直接影响 cron/ftp/firewall 的表单保真度。
4. **表格能力**（代码确认，2026-11-13 更新）：1.8.0 起 `table` 支持 `paging: 'cursor'` 与 `loadMore`，内核列表已改为 `source` 游标分页；服务端排序仍写在表头 `onEvent` 里靠自绘。monitor 的进程/网络页需要服务端排序，schema 化时仍要接受降级或自建。
5. **权限口径已经不自洽**（代码确认）：功能页路由只要 `AuthUser`（任意登录用户），但 RPC 要 `PluginManagerUser`；插件菜单 `roles` 默认全角色。也就是说 viewer 打开页面后所有取数都会 403 —— 今天事实上就是 admin-only，schema 化时必须显式定口径，而不是沿用现状。
6. **`config_schema` 已是现成的声明式配置**（代码确认）：`pkgfmt::ConfigField` 支持 `string/text/number/bool/password/url`，有类型取值校验（CLI 与 UI 可共用），但**没有任何 UI**（仅 `skiff plugin config` 命令）。schema 化可顺带零成本获得设置表单，属于新功能而非保真迁移。
7. **签名链可复用**：schema 文件放进包内即随 `.wbpkg` 验签，信任模型不变；运行时由插件 RPC 返回 schema 则脱离「安装期可审计」的静态事实。

---

## 2. 目标形态与新契约

### 2.1 三种可选形态

| 方案 | 做法 | 优点 | 缺点 | 结论 |
|---|---|---|---|---|
| **A 静态 schema 随包** | 包内 `ui/schema/<view>.json`；manifest 视图声明指向它；数据仍走 RPC | 随包验签、安装期可校验、可离线审阅；与现有 `ui/*` 托管模型同构；内核不组装布局 | 静态，无法按状态变形（可用数据驱动显隐弥补） | **推荐** |
| B 运行时 schema | 插件新增 `ui.schema` RPC，每次渲染返回 schema | 可随权限/状态变形；插件作者最灵活 | 每次渲染依赖插件进程；schema 不可在安装期审计；缓存与失败降级复杂 | 可作为 A 的补充（仅动态片段） |
| C 内核按插件组装 | 内核为每个插件写页面拼装代码 | 内核完全掌控体验 | 需要内核理解插件业务，违反「内核不感知插件业务」与 D8 的 DTO 边界 | 否决 |

方案 A 与现有边界一致：**页面结构由插件（作者）给出，内核只校验与转发；数据与动作仍由内核单一来源裁决。**

### 2.2 推荐契约草案（仅评估结论，不改现行契约正文）

1. **manifest（api 1.3 起）**
   - `[[menu]]` / `[[menu.items]]` 增加 `schema = "list.json"`；`[[mount]]` 增加 `schema = "health.json"`。
   - 与 `view`（映射 `ui/<view>.html`）**二选一**：同一声明同时给 `view` 与 `schema` 拒装；只给 `schema` 时不再要求 `ui/<view>.html` 存在。
   - `api <= 1.2` 的包行为完全不变（继续 HTML + iframe，仅旧 SSR 可达）。
2. **文件与校验（安装期，`pkgfmt`）**
   - 路径 `ui/schema/<name>.json`，单文件上限建议 128 KiB，UTF-8 JSON。
   - 节点类型白名单（`page`/`card`/`table`/`form`/`input-text`/`button`/`alert`/`flex`/`text`/`link` 等），未知类型拒装。
   - 禁止：外部 URL（`http(s)://`、`//`、`data:`）、`html`/`raw` 节点、未注册的自定义 action 名、`target=_blank` 之外的跳出方式。
   - 错误码沿用 `MENU_*` 命名风格，新增 `MENU_SCHEMA_INVALID` / `MENU_SCHEMA_MISSING` / `MENU_SCHEMA_CONFLICT`。
3. **数据绑定**
   - schema 内声明数据源（如 `dataSource: { rpc: "status", params: {} }`），壳在渲染前调用新动作 `skiffPluginRpc` 取数；写操作同样只能通过 `skiffPluginRpc` 发出。
   - `skiffPluginRpc` = 现有 `POST /{p}/plugins/{id}/rpc` 的壳侧封装：**继续带 CSRF 头、继续逐次审计、继续校验插件启用态**。schema 里不允许直接写 `api.url` 指向任意同源端点。
4. **路由与资源**
   - `GET /{p}/plugins/{id}/{view}` 继续是规范入口（已注册，保留）；壳内改为渲染 schema 页，不再渲染工作台 iframe。
   - 新增 `GET /{p}/plugins/{id}/schema/{view}`（或复用 `ui/schema/<view>.json` 托管 + 壳侧 `loadJson`）返回 schema；内核只做「包内文件 + 白名单」转发，不拼布局。
5. **槽位**
   - `[[mount]] schema` 时，`mount_card` 渲染 schema 而不是 iframe；失败降级文案与重试交互沿用现有卡片。
6. **L3**
   - admin Pack 到位前，用「由 `/ui/v1/menu` 的 `items` 生成的链接/按钮行」充当标签栏；到位后替换为 `page-tabs`，DOM 语义与 [MENU_CONTRACT.md](MENU_CONTRACT.md) §3.1 对齐。
7. **兼容**
   - 旧 HTML 视图不删除：旧 SSR（`/{p}/m/plugin/...`）继续可用，但新壳不再提供入口；第三方插件不受强制（是否强制见决策 D2）。

### 2.3 与既有契约的具体冲突点

| 文档 / 条款 | 现行规定 | 冲突 | 需要的处理 |
|---|---|---|---|
| [PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md) §1 路由表 | 功能页 = 内核框架 + 插件 iframe | 目标形态不再有 iframe | 改写该行，新增「schema 视图」定义 |
| 同上 §2.1 视图映射 | `{view} → ui/{view}.html`，文件缺失即 `MENU_VIEW_MISSING` | schema 视图没有 HTML 文件 | 映射规则改为「`view` 或 `schema` 二选一」，新增 `MENU_SCHEMA_*` |
| 同上 §4 沙箱与 CSP | iframe 沙箱（无同源）、`connect-src 'none'` | schema 在父 origin 渲染，具备同源能力 | 必须新增 §「schema 安全边界」：action 白名单 + 禁外链 + 安装期校验 |
| 同上 §5 旧包兼容 | `ui/index.html` 回落 | 旧包永远只能走 iframe | 明确「旧包仅旧 SSR 可达」，不进新壳 |
| 同上 §7 测试矩阵 | U4/U12/U14 均以 iframe 为前提 | 断言失效 | 新增 schema 版断言，旧断言降级为 legacy 用例 |
| [MENU_CONTRACT.md](MENU_CONTRACT.md) §1.2 表 | 「L2/L3 内容页 → 插件 iframe」 | 内容页渲染权变化 | 该行改为「插件声明 → 内核校验 → schema 渲染」 |
| 同上 §1.3、§3.1、§3.5 | 以 iframe 为结论与 DOM 示例 | 设计与 DOM 都过时 | §1.3 理由需重写为「内容页由内核渲染、结构由插件声明」 |
| 同上 §5.2 槽位 | `[[mount]]` 渲染 iframe | 槽位也要 schema | 增加 `schema` 字段与卡片渲染分支 |
| [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md) §1.4 | 「插件功能页：不迁移，维持 iframe」 | 与本决策直接相反 | 该行改为「迁移（schema）」并写明批次 |
| 同上 §4.4 | 「插件 UI 契约继续有效，不改」 | 不再成立 | 改为「iframe 仅 legacy；新契约见 PLUGIN_UI_CONTRACT」 |
| 同上 §8.2 D14 | 插件页 A/B/C 仍未定 | 本报告就是在评估 B | 需要用户拍板后回写 D14 |
| [URL_CONTRACT.md](URL_CONTRACT.md) §2、§9 | `/{p}/plugins` 说明「功能页仍走 iframe」；第九批条目写「iframe 槽位不变」 | 描述过时 | 在迁移批次里同步改为 schema 路由 |
| [UI_CONTRACT.md](UI_CONTRACT.md) / [DESIGN.md](DESIGN.md) / [sections/06-security.md](sections/06-security.md) | 以「插件内容始终在沙箱 iframe 里」为安全结论 | 安全模型变化 | 需补「schema 渲染的可信边界」结论 |
| [BUDDING_UPSTREAM_BACKLOG.md](BUDDING_UPSTREAM_BACKLOG.md) | 已登记 T1/T2/T5/T6 | 新增缺口：下拉、弹窗、表格分页/排序/多选 | 需补条目（见 §4.2） |

---

## 3. 工作量估算

单位：人日。区间下限对应「照现有元素 1:1 翻译且组件够用」，上限含联调、空态/错误态、权限与审计用例。**均为推测**，依据是 §1.3 的元素计数与脚本行数，未做试点。

### 3.1 平台一次性投入

| 工作项 | 内容 | 人日 |
|---|---|---|
| `pkgfmt` schema 声明与校验 | manifest 字段、二选一规则、节点白名单、禁外链、大小上限、错误码、单测 | 1.5–2.5 |
| 内核转发与权限口径 | schema 托管路由、启用态校验、RPC 读/写权限拆分（决策 D3）、审计口径（D4） | 1–2 |
| 新壳接入 | `plugin/` 菜单入路由、schema 加载与数据绑定、`skiffPluginRpc` 动作、错误/空态/刷新、L3 链接行 | 2.5–4 |
| 槽位 | `mount_card` schema 分支 + 失败降级 | 0.5–1 |
| 回归与安全 | CSP 断言、CSP/外链拒绝用例、e2e（两套形态并存） | 1–1.5 |
| **合计** | | **6–10** |

### 3.2 逐视图投入

| 视图 | 人日 | 主要不确定性 |
|---|---|---|
| guard `containers` | 0.5–0.8 | 无 |
| guard `events` | 0.3–0.7 | 无 |
| monitor `overview` | 0.5–1 | 指标卡排布 |
| monitor `history` | 0.5–1 | 无 |
| monitor `process` | 1–1.5 | 排序/过滤/分页是否要自建表格 |
| monitor `network` | 1–1.5 | 同上；两个枚举下拉 |
| monitor `health` 槽位 | 0.5 | 槽位高度与降级 |
| files `main` | 0.2（占位）/ 5–8（完整） | 上游组件缺口，现在不可行 |
| cron `list` | 3–5 | 表达式构建器、目标选择、弹窗编辑器 |
| ftp `accounts` | 1.5–2.5 | 密码框（T6）、弹窗、多动作列 |
| ftp `runtime` | 1.5–2.5 | fail2ban 卡片、危险区确认、复选 |
| firewall `rules` | 1–2 | 两个弹窗表单 + 枚举下拉 |
| firewall `fail2ban` | 1.5–2.5 | 8 字段表单 + 封禁列表 |
| testkit（4 视图 + 槽位） | 1–1.5 | 作为契约夹具，建议随平台批 |
| **小计（不含 files 完整重做）** | **14–23** | |
| **总计（含平台）** | **20–33** | |

### 3.3 不确定项清单

1. budding 表格是否支持服务端分页、表头排序、行多选（未验证；现有内核页全部关闭分页）。
2. 是否存在下拉（select）、弹窗、复选、多行文本组件（我方 boot 从未使用；**不能据此断定产物没有**）。
3. `input-text` 的 `type` 透传缺口（T6）会直接影响 ftp 的密码框。
4. L3 标签栏依赖 D4，若到期未交付，只能长期用链接行（与契约 §3.1 有视觉差异）。
5. schema 数据绑定的实现位置（壳侧 vs 内核侧）会改变平台工作量 ±1–2 人日。
6. 各插件作者（当前均为本仓库）对 schema 表达的接受度；cron 编辑器可能需要一次「降级换保真」的取舍。

---

## 4. 风险与替代方案

### 4.1 天生不适合 schema 化的页面

| 页面 / 能力 | 原因 | 替代方案 |
|---|---|---|
| files `main`（目录树、代码编辑器、多选、右键、上传） | 无树、无编辑器、无虚拟滚动、无上传进度；highlight.js 属于插件自带第三方代码 | **新壳保留占位**（现状已是占位）；旧 SSR 保留并标注「不再作为新入口」；等 T1/T2/T4/T5 后再评估 |
| monitor `process` / `network` | 200 条/页、可排序、多维过滤；表格能力未验证 | 先做「前 50 条 + 服务端排序/过滤 + 加载更多」，接受交互降级；或暂留占位 |
| cron 表达式编辑器 | 预设 + describe/build + 下次运行预览，是纯自定义逻辑 | 降级为「表达式输入 + describe 预览按钮」，构建器后置；或注册专用自定义 action |
| files 的语法高亮编辑器 | 需要代码查看器组件 | 只读查看器后置；本批不做 |
| 终端 / 流式日志 | 当前 6 个官方插件均无终端与 WS；内核终端另属文件管理批 | 本轮不涉及，保持既有 WS 契约 |

### 4.2 安全与信任风险（最高优先级）

1. **沙箱边界消失**：现在插件 HTML 在 opaque origin，拿不到 Cookie、`connect-src 'none'`；schema 在父 origin 渲染，插件作者可描述同源请求。必须：只允许注册过的 action、禁止 schema 内任意 URL、安装期校验 + 运行时二次校验、禁止 `html/raw` 与脚本类节点。
2. **CSRF 不能靠 schema**：壳的通用表单不会自动带 `x-csrf-token`（CSRF 头只出现在壳内硬编码动作里），因此写操作必须走 `skiffPluginRpc` 动作，而不是 schema 里的 `api.url`。
3. **审计噪声**：现在每次取数都写一条 `plugin.rpc` 审计；schema 页带轮询后会放大。需要决策 D4。
4. **权限错位被固化**：若不先定 D3，schema 化会把「页面可见但数据 403」从缺陷固化成契约。

### 4.3 生态与兼容风险

| 风险 | 说明 | 缓解 |
|---|---|---|
| 第三方 HTML 插件被「静默消失」 | 新壳已丢弃 `plugin/*` 菜单 | 至少在插件中心标注「该插件功能页仅旧版可用」，或在 D2 中决定强制迁移 |
| 双轨维护 | `ui/*.html` 与 `ui/schema/*.json` 并存 | 明确 HTML 只作 legacy、不再演进；文档冻结 HTML 契约 |
| 契约漂移 | schema 是「页面描述」，与 D8「内核不出页面描述」字面冲突 | 在契约里写清：**插件作者出结构，内核只校验与转发**，不违反 D8（D8 约束的是内核 DTO） |

---

## 5. 推荐路线与需要决策的点

### 5.1 分批路线

| 批次 | 内容 | 交付判据 |
|---|---|---|
| **P0（平台）** | manifest schema 字段 + pkgfmt 校验 + schema 托管/转发 + `skiffPluginRpc` + 壳内 `plugin/` 路由 + L3 链接行 + 槽位分支 + testkit 迁移 | testkit 4 视图 + 槽位在新壳原生渲染；旧 SSR 不回归；安全拒绝用例全绿 |
| **P1** | guard（2 个只读页） | 只读通路打通；卡片/表格/刷新/空态与旧页一致 |
| **P2** | monitor（4 页 + `overview.health` 槽位） | 表格分页/排序降级方案定稿；槽位替换成功且失败可回退 |
| **P3** | firewall（2 页，写通路） | `skiffPluginRpc` 写 + 确认 + 审计；枚举字段降级可接受 |
| **P4** | ftp（2 页） | 密码框（T6）方案确定；危险回收区二次确认 |
| **P5** | cron（1 页） | 表达式编辑器取舍定稿 |
| **files** | 保持占位 | 上游 T1/T2/T4/T5 到位后再排期 |

选择 guard 先行的理由：2 个视图、零写操作、元素最少（3 卡 2 表）、无权限分歧，最适合验证「schema 加载 + RPC 取数 + 空态/错误态 + 刷新」这一整条新通路；失败时回退成本最低。

### 5.2 需要用户决策

| ID | 决策点 | 选项 | 建议 |
|---|---|---|---|
| D1 | schema 归属 | 包内静态 / RPC 动态 / 内核硬编码 | **包内静态** |
| D2 | 第三方插件是否强制 | 1.3 可选双轨 / 立即强制 / 永久双轨 | **1.3 可选，1.4 再评估** |
| D3 | 只读权限 | 给 viewer 开只读 RPC（新增 `plugin.rpc.read` 或按菜单 roles）/ 功能页继续 admin-only | 倾向**按菜单 roles 开只读**，写仍 `PluginManage`；否则功能页对非管理员永远无意义 |
| D4 | 审计口径 | 读也逐次审计 / 读降级为 debug 日志 / 读按会话合并 | 倾向**读降级、保留写** |
| D5 | L3 标签栏 | 等 D4（admin Pack）/ 先用链接行 | **先用链接行**，Pack 到位替换 |
| D6 | files 页 | 占位 / 等上游后一次性重做 | **占位** |
| D7 | 是否顺带做 `config_schema` 表单 | 做 / 不做 | 做的话是**新增功能**，不与保真迁移混在一批 |

---

## 6. 事实与推测边界

### 6.1 读代码确认的事实

- 12 个功能视图 + 1 个槽位的清单、UI 文件、元素与 RPC 方法（`plugins/*/manifest.toml`、`plugins/*/ui/*`）。
- 数据通路与 RPC 端点、`PluginManage` 权限、CSRF 头、逐次审计（`crates/web/src/handlers/plugins.rs`、`crates/web/src/middleware.rs`）。
- 功能页路由使用 `AuthUser`，RPC 使用 `PluginManagerUser`（同上两文件）。
- 新壳丢弃 `plugin/*` 菜单、`skiff:plugin-view` 占位、`loadJson` 支持远程 schema、`embed(mount, schema, options)` 运行时注入（`crates/web/static/budding/budding-boot.js`）。
- 新壳未使用 tabs / dialog / select / checkbox / textarea；表格 `pagination: false`（同上）。
- `config_schema` 的声明字段与类型校验、无 UI（`crates/pkgfmt/src/manifest.rs`、`crates/core/src/commands.rs`）。
- iframe 沙箱属性与 `PLUGIN_UI_CSP` 被测试冻结（`crates/web/src/handlers/plugins.rs` 单测）。

### 6.2 推测（未实测）

- 人日区间（基于元素计数与脚本行数的经验估算，无试点数据）。
- budding 产物中「没有 select/dialog/多选表格」——只验证了「我方 boot 未使用」，没有逐个组件在产物中取样。
- schema 数据绑定放在壳侧 vs 内核侧的成本差异。
- 服务端排序/分页在 schema 表格下的实际可用性。

---

## 7. 变更记录

| 日期 | 变更 |
|---|---|
| 2026-11-13 | 首版：插件页去 iframe 的可行性与工作量评估；逐视图事实表、目标契约草案、契约冲突清单、工作量估算、风险与分批路线、决策点。未改代码与既有契约正文。 |
