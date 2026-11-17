# 决策记录（§8）

> 上级索引：[SDUI_BUDDING_INTEGRATION.md](../SDUI_BUDDING_INTEGRATION.md) ｜ 拆自 `docs/sdui/SDUI_BUDDING_INTEGRATION.md` 原 §8（2026-11-16 整理，正文未改）。

---

## 8. 决策记录

### 8.1 已定案（2026-11-12）

> 原选项与利弊只在表后备查一句，不再展开。

| ID | 定案结论 | 硬约束 | 生效后果 |
|---|---|---|---|
| D1 | budding 发版打包后的 JS 直接放进本项目使用；**本项目不引入 Node 工具链** | 产物入库；不执行 `pnpm build:package`；CI 不新增 Node job；升级靠替换产物文件 | 离线可构建、CI 零改动；产物与上游源码可能漂移，升级需人工核对并回归 |
| D2 | JSON 新开命名空间，路径 `/ui/v1` | 位于入口前缀之外，与 `/api/v1` 并列；`entry_guard` 豁免；`AuthUser` 对该前缀返回 401 而非 303 | 所有 JSON 端点在此命名空间下；URL 契约需补例外条款（§4.5.1） |
| D3 | 登录页也只用 budding | 保留签名 CSRF token、来源判定、限流、TOTP、`HttpOnly; SameSite=Lax` 与 `Secure` 判定；新增公开引导端点 | 不再有 SSR 登录页；`/{p}/login` 返回壳；失败用稳定错误码 |
| D4 | 要求上游随发行包提供 admin Pack | 需要 `admin` / `nav-menu` / `page-tabs`；外部阻塞，不由本仓库解决 | 未交付前菜单壳与页签栏无法进入正式实现；应急自建壳不进入交付 |
| D5 | 按 MIT | 需上游补 LICENSE 或书面确认；需核对 Vuetify 等依赖许可证 | 未补齐前只作内部预览，不对外承诺 MIT |
| D6 | 文件管理页也只用 budding 重做 | 分片上传/进度、目录树、日志查看器、只读详情由上游补齐（§6.1）；1.9.0 起细节统一用 `property` | 文件页交付取决于上游；不允许用「保留旧页」兜底 |
| D7 | 一次性切换，不留双栈 | 所有内核页同批交付；无运行时开关；旧 askama 页面统一删除 | 回滚只能 git 回退 + 重新部署；切换前必须完成 §8.3 验收清单 |
| D8 | 文案 key + 中文兜底，范围收窄到状态、错误码、提示、空态原因；结构文案归前端 schema | 后端出 `{code,label}` / `{code,message}`；DTO 不承载页面标题、按钮、字段标签、帮助文字；Vuetify locale 由 `embed` 的 `locale` / `setLocale` 控制（1.9.0） | 中英混排只剩未迁移的宿主文案 |
| D9 | 所有 `*_at` 单一 RFC3339 UTC；时长出秒数；不出双字段 | 数据库 epoch 秒不变，只在 `/ui/v1` 出口转换；前端用 `Intl.DateTimeFormat` 本地化 | 旧页的 `%Y-%m-%d %H:%M` 随一次性切换删除 |
| D10 | 审计 `(ts,id)`、作业 `(created_at,id)`、文件 `(is_dir,name)` keyset；任务/站点/插件/运行环境全量上限 500 | `limit` 默认 50、上限 200；游标携带排序键与筛选指纹；非法游标 400；需补复合索引 | 列表改为「加载更多」；索引需迁移 |
| D11 | 产物按 `budding-<ver>.min.js` / `budding-<ver>.css` 版本化入库，引用加 `?v=`；版本化请求 immutable，稳定别名 no-cache | 升级删除旧产物；CSS 必须用版本化真实文件名；`.gz` 由现有 `build.rs` 生成 | 需改 `assets.rs` 缓存策略；CSS 查询串行为待实测 |
| D12 | CSP 保持现状，不预防性放宽 | 只在实测缺失时按最小集补 `font-src 'self' data:` / `worker-src 'self' blob:` / `'wasm-unsafe-eval'`；插件 iframe CSP 与沙箱属性不得改动 | 需要一次逐页控制台巡检；不放宽可能暴露上游缺陷 |
| D13 | history 路由 + 白名单前缀回落，地址栏保持 `/{p}/sites/xxx`，不用 hash | 仅 `GET` + `Accept: text/html` + 命中 URL 契约 §2.1 白名单时返回壳页；白名单外仍 404；写请求永不回落；白名单变更同步契约与测试 | 前端未匹配路由自行渲染 404；白名单需随页面增删维护 |

备查：D1 原选项为产物入库/新 crate/构建期 Node 生成；D2 为新命名空间/复用 `/api/v1`/同构内容协商；D3 为保留 SSR/重建/双轨；D4 为 basic 自建/自建 Pack/上游提供；D5 为书面授权/私库/暂不分发；D6 为保留旧页/重做/分阶段；D7 为逐页开关/一次性/长期双栈。

### 8.1.1 追加定案（2026-11-17）：壳层 chrome 与三级菜单

| ID | 定案结论 | 硬约束 | 生效后果 |
|---|---|---|---|
| D17 | 顶栏整体隐藏（admin schema `bar: false`）；页内子视图切换由内核渲染的**横向三级菜单**承担；账号与退出登录并入「设置」页「当前账号」区块 | 面包屑槽一并隐藏（`.bud-admin__crumbs`）：L3 是**菜单**不是路径回溯，「我在哪」由左侧菜单当前项承担；菜单按「当前页有没有子视图」决定出不出——有则全量平铺、当前项一条 2px 底部强调轨，无则整条不渲染，不做下拉、不做占位；底色取内容区表面色（`--v-theme-surface`），**不跟随侧栏 chrome**；吸顶时用 `is-stuck` 补遮罩，重建带闸门防 MutationObserver 死循环；退出仍走 `POST /ui/v1/session/logout` 同一 CSRF 头 | 顶栏不复存在；核心页 L3 在 `SUBVIEW_TABS` 登记并由登记表生成隐藏路由（`sites/web` 4 个、站点详情 5 个、运行环境详情 5 个，日志均在内），插件多视图由 `/ui/v1/menu` 的 `views` 驱动、页内按钮行删除；各页自带页面标题（不再置空） |

### 8.2 仍待定

P1（切换前）已清空：D13 已于 2026-11-12 定案，见 §8.1。

#### P2：体验与长期

| ID | 决策点 | 选项 | 利 | 弊 |
|---|---|---|---|---|
| D14 | 插件页 | A 继续 iframe；B 插件改出 SDUI JSON；C 双轨 | A 与现契约一致、零风险；B 体验统一；C 渐进 | A 技术栈不统一；B 需改插件生态与沙箱模型；C 长期负担 |
| D15 | 主题与品牌色 | A 我方 CSS 覆盖；B 要求框架适配 Vuetify token；C budding 页不做暗色 | A 立即可用；B 根治；C 最省事 | A 与上游样式耦合、升级易坏；B 依赖上游；C 功能回退 |
| D16 | chart 组件 | A 暂不需要；B 自建 chart Pack | A 零成本；B 为后续图表页铺路 | A 后续图表页受限；B 维护成本 |

### 8.3 一次性切换验收清单（D7）

切换前每一项都必须有判定证据；数量口径均标注来源，不沿用旧文档的估计值。

| # | 对齐项 | 判定方式 |
|---|---|---|
| 1 | 路由 → JSON 端点映射 | 口径：`crates/web/src/lib.rs` 现有 **167 条** `.route(` 注册（2026-11-12 计数）。逐条登记去向：GET 页面 → 壳或 JSON 读端点，POST 动作 → `/ui/v1` 动作或删除；不允许存在未登记路由 |
| 2 | 必须保留的非 JSON 通道 | GET 文件下载（`Content-Disposition`）、插件 iframe HTML 与 `ui/*` 静态、插件 RPC、旧 SSR 容器日志 SSE（budding 页走尾快照）、终端票据 + WS。每条有契约测试，且 CSP/沙箱策略未放宽 |
| 3 | 权限与审计覆盖 | 动作注册表矩阵逐项测试；每个写动作产生审计；CSRF 失败、401、403、限流四类拒绝路径均有审计或兜底日志 |
| 4 | 错误与空态 | 每个资源具备稳定错误码、字段码与 `empty_reason`；列表空态能说明「为什么空 + 下一步」 |
| 5 | 菜单与页签 | L1 固定 6 项、L2/L3 来自内核 JSON；admin Pack 已到位；菜单偏好与固定槽位行为一致 |
| 6 | 静态资源与缓存 | 产物引用带版本；升级前后实测浏览器不加载旧 bundle；`.gz` 变体存在 |
| 7 | CSP | 响应头断言：无 `unsafe-eval`；`script-src 'self'`；`connect-src 'self'` 覆盖轮询；控制台无 CSP 违规 |
| 8 | 登录链路 | 登录、登出、口令修改、TOTP 开启/校验、限流、跨站拒绝逐用例回归；401/403/503 三者可区分 |
| 9 | 体积与运行开销 | 实测二进制增量与空闲 RSS，记录数值；不预设阈值，超过设计预算再决策 |

### 8.4 定案落地位置

| 定案 | 生效位置 | 落地时机 |
|---|---|---|
| D8 文案双字段 | `/ui/v1` 的 DTO、`actions`、`notices`、`error`；页面结构文案在前端 schema | 新建 JSON 端点时生效；旧页不动 |
| D9 RFC3339 | `crates/app/src/time_fmt.rs::epoch_to_rfc3339_utc`；`/ui/v1` 所有 `*_at` 出口 | 工具已落地（2026-11-12，含 0/负数/闰秒/跨年/超范围单测）；JSON 出口在新建端点时接入，旧页不动 |
| D10 分页契约 | `crates/app/src/cursor.rs`（HMAC 签名 + 筛选指纹）；`/ui/v1` 列表参数、400 错误码 | 游标编解码与篡改单测已落地（2026-11-12）；列表端点接入时生效，现有 handler 未改写 |
| D10 复合索引 | `store` 的 schema 迁移（审计、作业） | 已落地（2026-11-12，schema v19）：`(ts DESC, id DESC)`、`(created_at DESC, id DESC)`、`(status, created_at DESC, id DESC)`；迁移测试 `v18_database_upgrades_keyset_indexes_and_keeps_rows` |
| D11 缓存头 | `assets.rs` 缓存策略 + 壳页版本化引用 | 已落地（2026-11-12）：版本化文件名或非空 `?v=` → `immutable`，稳定别名 → `no-cache`；回归 `cache_control_follows_versioning` |
| D12 CSP 回归 | 响应头断言与逐页控制台巡检 | 回归测试已加（2026-11-12，见 §2.5）；逐页控制台巡检待切换前 |
| D13 深链 | `URL_CONTRACT.md` §2.1 白名单 + `Router::fallback` 回落（`crates/web/src/handlers/spa.rs`） | 契约已同步（2026-11-12）；壳页回落、`/ui/v1/session` 与 401 语义已落地（2026-11-12）；前端未匹配路由渲染 404 |
| D17 壳层 chrome | `crates/web/static/budding/budding-boot.js`（`shellSchema` / `SUBVIEW_TABS` / `matchSubviews` / `pluginSubviews` / `settingsSchema`）、`crates/web/static/skiff-shell.css`、`spa.rs` 壳页 `<link>`（`?v=2`） | 代码已落地（2026-11-17）；待出包部署到测试机后生效 |

补充：D8 中「消除 Vuetify 内部英文」在 1.9.0 已具备上游能力（`embed` 扁平 `locale` / `uiDefaults`、`setLocale`）；宿主传参与文案迁移仍需按 backlog 排期，不阻塞内部推进。

---
