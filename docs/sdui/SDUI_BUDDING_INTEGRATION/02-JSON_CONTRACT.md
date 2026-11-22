# JSON 契约（§3）

> 上级索引：[SDUI_BUDDING_INTEGRATION.md](../SDUI_BUDDING_INTEGRATION.md) ｜ 拆自 `docs/sdui/SDUI_BUDDING_INTEGRATION.md` 原 §3（2026-11-16 整理，正文未改）。

---

## 3. JSON 契约

### 3.1 命名空间与路径

**D2 定案**：新开命名空间，路径 `/ui/v1`，位于入口前缀之外，与 `/api/v1` 并列。

| 约束 | 口径 |
|---|---|
| 形状 | `/ui/v1/{资源}`；动作在末段，如 `/ui/v1/sites/{domain}/config/save` |
| 方法 | 只用 `GET`（读）与 `POST`（写/动作），**禁用 PUT/PATCH/DELETE** |
| 鉴权 | 复用会话 Cookie + CSRF；`AuthUser` 对 `/ui/v1/*` 必须返回 401 JSON，不得再 303 到登录页 |
| 入口豁免 | `entry_guard` 需把 `/ui/v1/` 与 `/api/v1/` 并列放行；`/ui/*` 的其它形态仍 404 |
| 版本 | 版本在路径（`v1`），载荷内不再重复 `apiVersion` 作为路由依据；`apiVersion` 仅作兼容提示 |
| 响应 | 统一 JSON 信封（§3.2）；不返回 HTML、重定向或页面片段 |
| 资源命名 | 沿用资源契约的复数、小写、中划线；不使用 `api` 中缀 |

需要同步到《URL 契约》的条款草案见 §4.5。

### 3.2 信封结构

统一信封，与现有 `crates/web/src/error.rs::ErrorEnvelope` 尽量同形，只允许**新增字段**：

```json
{
  "data": {},
  "actions": [],
  "notices": [],
  "error": { "code": "shape.field", "message": "域名不合法", "field": "domain", "retryable": false }
}
```

| 字段 | 语义 | 约束 |
|---|---|---|
| `data` | 资源或列表 | 列表为 `{ "items": [], "next_cursor": null, "total": null }` |
| `actions` | 当前主体可执行的动作 | 只给可见动作；写动作必须带 `method` 与 `path` |
| `notices` | 本次响应附带的提示 | 不持久化；替代 flash |
| `error` | 失败载荷 | 仅在失败时出现；`code` 为稳定机器码，`field` 仅字段类错误 |

约定：

- `error.status` 不重复放进载荷，由 HTTP 状态表达；现有 HTML 侧 `ErrorEnvelope` 的 `status` 字段保留给 `/api/v1`，新命名空间可不带。
- 不允许出现 `redirect`、`html`、`layout`、`component` 字段。
- budding 的错误对象自带 `path`（schema 路径），与 Skiff 的 `field`（业务字段码）是两回事，不得混用。
- **文案双字段（D8）**：状态、错误码、提示、空态原因一律出 `{code,label}` 或 `{code,message}`；前端有字典用 `code`，否则回落到中文 `label`/`message`。
- 页面标题、按钮、字段标签、帮助文字属结构文案，写在页面 Schema 里，**不进 DTO**。页面 Schema 只有两个产出端：静态资产 `static/budding/pages/*.json`，或 `/ui/v1` 端点的 `?with_schema=true`（见 `09-JSON_PAGES.md`）；不得回到壳脚本里拼。哪个地址走哪个产出端同样在服务端（`GET /ui/v1/page`，见 `09-JSON_PAGES.md` §9.6），壳脚本不持路径映射。
- 路由表（路径 → 标题/图标/隐藏标记/页面来源）由 `GET /ui/v1/routes` 下发，是菜单树（`skiff_app::menu::build`）的投影，不是第二份菜单。
- Vuetify 内部英文（分页、表格空文案、日期控件）只能由上游 `createVuetify()` 注入 locale 解决；标注为**上游依赖，不阻塞内部推进**。

### 3.3 字段码

- 字段码沿用领域错误携带的结构化字段，例如站点表单的 `SiteField`：`domain` / `aliases` / `root_dir` / `runtime_id` / `proxy_target`。
- 规则：**字段码 = 前端表单节点 id（或 `props.name`）**；内核与页面 schema 各自维护一张映射表，发布前做一致性测试。
- 未登记字段码回退为顶部提示，不猜测控件。
- 字段码表冻结后只增不改；改名视为破坏性变更。

### 3.4 时间（D9 定案）

- 所有 `*_at` 输出**单一 RFC3339 UTC 字符串**，例如 `2026-11-12T06:30:00Z`。
- 时长仍出秒数，不转成字符串。
- **不出双字段**（不并行输出 epoch 与字符串）。
- 数据库 epoch 秒不变，只在 `/ui/v1` 出口转换。
- 前端用 `Intl.DateTimeFormat` 按浏览器时区本地化；审计、任务、作业、站点证书等只读列表一律同一规则，内核不再拼展示字符串。
- 现存散落的 `%Y-%m-%d %H:%M` 只保留在旧页，随一次性切换删除。

### 3.5 分页与游标（D10 定案）

| 列表 | 排序 / 游标 | 说明 |
|---|---|---|
| 审计 | `(ts, id)` keyset | 现有 `idx_audit_ts` 需补 `id` 成复合索引；`ts` 为秒级，必须用 `id` 破平 |
| 作业 | `(created_at, id)` keyset；状态筛选走 `(status, created_at, id)` | 现有 `idx_jobs_created` / `idx_jobs_status` 需补 `id` |
| 文件 | `(is_dir, name)` | 文件系统列表，不走 SQLite；排序键必须进游标 |
| 任务 / 站点 / 插件 / 运行环境 | 全量，上限 500 | 量小，暂不引入游标 |

通用规则：

- `limit` 默认 50、上限 200；返回 `next_cursor`；`total` 默认不返回。
- 游标必须携带**排序键与筛选指纹**；筛选或排序变化即视为新查询，旧游标失效。
- 排序字段白名单，禁止透传任意列。
- 非法游标返回 400；禁止 `offset` 与 `cursor` 混用。

### 3.6 动作元数据

动作 id 沿用 `crates/app/src/actions.rs` 的注册表，格式 `<resource>.<verb>`，同时作为审计 `action` 的来源。

| 元数据 | 用途 |
|---|---|
| `id` | 全局唯一、稳定 |
| `permission` | 所需 `skiff_model::Permission`；前端只用于显隐 |
| `dangerous` | 前端必须走 `confirm` 动作或等价确认 |
| `async` | 返回作业引用，需轮询进度 |
| `idempotent` | 允许重复提交的语义 |
| `method` / `path` | 传输层调用方式，指向 `/ui/v1/...`；仍受 URL 契约约束 |
| `label` | 动作显示文案（中文兜底，D8） |
| `label_key` | 前端字典键（D8）；缺失时回落到 `label` |
| `input` | 参数 schema（类型、必填、范围、枚举） |

动作**永不返回页面、重定向或提示文案**；成功结果放 `data`，提示放 `notices`。

### 3.7 版本轴

budding 的 JSON **没有 schema 版本字段**，版本轴是 `catalogVersion` + `apiVersion`，字段只增不改。

Skiff 侧对应约束：

- 资源字段名冻结，只增不改；删除或改名必须升 API 版本。
- `apiVersion` 与 `/ui/v1` 的版本号保持一致，由内核在会话端点下发，前端只做兼容判断。
- 不依赖 budding 的 `catalogVersion` 做业务分支；它只表示前端组件目录版本。

#### 3.7.1 冻结机制（key-set 快照）

- **落地位置**：`crates/web/tests/main_flow.rs` 的 `ui_v1_envelope_and_bootstrap_key_sets_are_frozen` 与 `ui_v1_key_response_shapes_are_frozen`。用例对关键响应断言**精确字段集**（顺序无关），不比对具体值；错误信封、登录引导、会话、概览、菜单、站点列表/详情、运行环境、容器、镜像/网络/卷、任务、作业、审计、插件、商店、设置、compose、Web 服务、文件管理均在覆盖范围内。
- **只增不改的判定**：新增可选字段不算破坏，但必须同步更新上述用例里的冻结清单；删除字段、改名或改变字段语义属于破坏性变更，必须升 `api_version`（新路径或新版本段），并在本节记录原因与迁移口径。
- **不引入新依赖**：快照只依赖 `serde_json::Value` 的键集，不引入 insta/黄金文件框架；嵌套资源行只冻结"行对象"这一层，避免把动态变量（时间、计数、路径）写进断言。
- **失败处理**：该用例失败说明 wire 形状发生变化。先判断是"有意的字段新增"（更新清单即可）还是"非预期删除/改名"（回退改动），不得直接放宽断言或删用例。

#### 3.7.2 壳主题字段与着色路径

`settings.ui.*`（沿用既有键，不新增设置存储）在 `primary` 之外扩充：

| 键 | 语义 |
| --- | --- |
| `primary` | 主色；作用于菜单、页签、按钮与图标激活态 |
| `menu` | 菜单底色（侧栏 / 顶栏 / 页签条共用）；空值表示随页面底色自动推导 |
| `page` | 页面底色；浅色自动深字、深色自动白字 |
| `menu_custom` | `1` / `0`：是否使用自定义菜单底色 |

- **服务端裁决**：`/ui/v1/settings/ui` 只接受 `#rrggbb`；任一颜色非法返回 400 且整组不落库。主色与菜单底色的对比度低于 4.5:1 时自动混色校正（`crates/app/src/theme.rs`）。
- **下发形状**：`GET /ui/v1/session` 的 `theme` 与 `GET /ui/v1/settings/ui` 均返回 `primary` / `menu` / `page` / `menu_custom` / `menu_text` / `page_text` / `on_primary` / `adjusted`（设置页额外带 `can_admin`），字段集已纳入 §3.7.1 冻结。
- **着色**：壳页在 `<head>` 内联 `<style id="skiff-shell-theme">`（`crates/web/src/handlers/spa.rs::theme_css`），把 `primary` / `menu` / `page` 与已推导的 `menu_text` / `page_text` / `on_primary` 写成文档级变量（`:root`、`.v-theme--light/.v-theme--dark`、`.bud-admin`、`.v-application`）。颜色计算只有 `crates/app/src/theme.rs` 一处，前端不再重算亮度与黑白取色；壳脚本只负责补 `bud-admin--chrome` 类（r5 起 schema 根仍拒绝 `theme` 属性，CompileError），补类逻辑常驻 MutationObserver，防止 Vue 重渲染抹掉手工类。
- **交互**：预设方案（品牌蓝 / 翡翠绿 / 深灰 / 暖橙）点击即整体提交；自定义表单使用色块 `chip` + `#rrggbb` 文本输入，不依赖颜色选择器节点（r5 未提供）。

---
