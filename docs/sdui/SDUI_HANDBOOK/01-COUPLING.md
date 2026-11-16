# 一页结论与现状耦合清单（§0–1）

> 上级索引：[SDUI_HANDBOOK.md](../SDUI_HANDBOOK.md) ｜ 拆自 `docs/sdui/SDUI_HANDBOOK.md` 原 §0–1（2026-11-16 整理，正文未改）。

---

> 状态：**设计稿；P0/P1 已落地（2026-11-10），P2 未实施**（最近核对 2026-11-13）。本文描述目标、契约与推进方式；落地范围以 §P0/P1 状态为准。
> 前置：SDUI 框架由外部提供，其组件集、事件模型与布局能力尚未确定（见 §6）。
> 关系：视觉与交互基准见 [UI_CONTRACT.md](../../contract/UI_CONTRACT.md)，插件页路由与 iframe 边界见 [PLUGIN_UI_CONTRACT.md](../../contract/PLUGIN_UI_CONTRACT.md)，菜单声明模型见 [MENU_CONTRACT.md](../../contract/MENU_CONTRACT.md)，分层红线见 [AGENTS.md](../../../AGENTS.md) §4.1 与 [03-architecture.md](../../sections/03-architecture.md) §3.1。本文不重复上述结论，只回答「内核要交出什么、保留什么、怎么迁」。

---

## 0. 一页结论

| 判断 | 结论 |
|---|---|
| 内核的长期职责 | **领域服务 + 资源描述 + 动作契约 + 权限/校验/审计 + 异步任务**；不承担布局、组件、交互状态 |
| 页面是什么 | **资源引用的组合**：页面描述由表现层拼装，内核只提供「有哪些资源、字段是什么、能做什么动作」 |
| 当前最大阻碍 | handler 把「解析 → 鉴权 → 业务 → 审计 → 渲染」写在一个函数里；视图模型携带 CSS 类名与中文文案；动作结果经重定向与内存 flash 传递 |
| 最先做的事 | 稳定错误码、统一 DTO、动作注册表、权限元数据、分页/筛选契约、作业进度模型（均不依赖 SDUI 框架） |
| 明确不做 | 为旧模板做二次抽象；在 schema 里放布局/颜色/组件；让业务层出现 page/flash/tab/modal 等词汇 |
| 迁移策略 | 新增并行端点、按页切换、最后删除模板与展示依赖；每阶段可独立回滚 |

---

## 1. 现状耦合清单

### 1.1 观测口径

以下计数为 **2026-11-12** 对 `crates/web/src`、`crates/web/templates` 与 `crates/web/static` 的静态观测（`Select-String` 正则计数，未过滤注释），用于说明耦合形态，不作为能力指标或性能数字引用。

| 项 | 数量 | 说明 |
|---|---|---|
| Web 源码 | 30 个文件、13,862 行 | 不含模板与静态资产 |
| 页面处理器 | 154 个 `pub async fn` | 四组写路径已拆为「解析 → 命令 → 响应」，读处理器仍多职责 |
| 模板 | 38 个 `.html`、33 个 askama `Template` 结构体 | 其余为基础布局与可复用片段（弹窗、卡片、提示条） |
| 表单解析 | 64 处 `parse_form`、105 个 `<form>` | 字段名即事实接口，无 schema |
| 重定向 / flash | 87 处（其中 71 处 `flash_redirect`、16 处直接 `handlers::redirect`） | 写操作结果经会话内存 flash 传给下一次渲染 |
| 审计调用 | Web 7 处、App 16 处 `.audit(...)` | 写审计已随 P1 命令层；Web 剩余为 panic、会话、任务与作业 |
| 授权判定 | 17 处 `policy.check`、12 处 `require_permission`、25 处 `user.can`、2 个中间件提取器 | 写路径已统一到 `web::authz`；其余分别用于对象级、读页与按钮显隐 |
| 查询参数 | 24 处 `Query<HashMap<String, String>>` | 每页自取，无统一分页/筛选契约 |
| 内联脚本 | 12 个 `<script>` 块（分布在 9 个模板） | 另有 5 个独立前端脚本与 xterm |
| 静态资产 | `skiff.css` 82,641B、`skiff.js` 81,800B、`htmx` 50,917B、`xterm` 289,441B、`files.js` 66,308B | 由 `rust-embed` 打包 |
| 菜单模块 | `app/menu.rs` 66,018B；`web/menu.rs` 310B 仅再导出 | 声明解析、排序、偏好写库与渲染模型仍混在一起 |

### 1.2 三类归类

| 能力面 | 现状落点 | 归类 | 应归属 |
|---|---|---|---|
| 角色权限判定、对象解析（容器/站点/任务） | 写路径统一走 `web::authz`；另有 `policy.check`（对象级）、`require_permission`（读页）、`user.can`（按钮显隐）与中间件提取器 | 业务逻辑（判定在内核） | 内核（统一到命令层） |
| 领域不变量与参数语义校验 | `limits_from_form`（传输形状）、`validate_domain`、`NginxParams::validate`、Compose 子集 | 业务逻辑 | 内核（命令层） |
| 状态迁移、幂等与并发互斥 | `Docker::execute`、`JobService`、运行环境安装锁、exec 票据 | 业务逻辑 | 内核（领域服务） |
| 审计事实 | Web 7 处、App 16 处 `.audit(...)`；写审计已随 P1 命令层 | 业务逻辑（留痕） | 内核（随命令产生） |
| 异步作业与进度 | `JobService`（提交、阶段、进度、日志、取消、重试） | 业务逻辑 + 契约 | 内核产出，传输层消费 |
| 资源读模型字段语义 | `app` 的 `RuntimeView`、`Job` 等 | 业务逻辑 | 内核（字段稳定、无展示语义） |
| 页面视图模型与格式化 | `web::templates` 的 `*View`、`human_size`、`truncate`、`"—"`/「未知」占位 | 展示逻辑 | SDUI |
| 状态到样式/文案的映射 | `state_label`、`state_class`（ok/warn/danger/muted）、`type_label` | 展示逻辑 | SDUI |
| 模板、片段与局部刷新 | askama 模板、`/containers/rows`、`hx-request` 分支 | 展示逻辑 | SDUI |
| 表单控件、字段级错误高亮 | `site_error_text` 把错误映射到字段名 | 展示逻辑 | SDUI（内核只给 `field`） |
| 重定向、PRG、flash 提示 | `flash_redirect`、`FlashKind`（位于 `skiff-app`） | 展示逻辑 | SDUI（内核改为直接返回结果） |
| 导航与菜单渲染、偏好排布 | `app/menu.rs` 的 `build`/`settings_view`/`apply_action`（`web/menu.rs` 仅再导出） | 展示逻辑（含少量授权） | 声明与授权留内核，渲染归 SDUI |
| CSS、主题、静态资产、xterm 集成 | `static/`、`base.html` 内联脚本 | 展示逻辑 | SDUI |
| 入口前缀、404、状态码语义 | `router`、`entry_guard`、`AppError` | 传输契约 | 内核 |
| 会话 Cookie、Bearer、CSRF | `middleware`、`check_csrf`、`x-csrf-token` | 传输契约 | 内核 |
| 错误载荷形状 | `AppError`（HTML 页）与 `ApiError`（`{error:{code,message}}`） | 传输契约 | 内核（需稳定机器码） |
| SSE / WebSocket / 分片上传下载 | 容器日志、终端、作业日志、`offset` 分片 | 传输契约 | 内核（SDUI 不接管） |
| 插件 UI 宿主与 RPC | `PLUGIN_UI_CONTRACT.md` 定义的 iframe 与 `rpc` | 传输契约 | 内核（短期不变） |

### 1.3 内核必须保留 / 应被 SDUI 接走

| 必须保留在内核 | 理由 | 应接走 | 理由 |
|---|---|---|---|
| 授权矩阵与对象解析 | 唯一来源在 `skiff_model::Permission` 与 `policy`；UI 只能消费结论 | 布局与组件选择 | 属于表现层能力，框架提供 |
| 领域校验与不变量 | 与存储/宿主副作用同生共死，不可由前端保证 | 表单控件与客户端校验提示 | 控件行为与视觉由框架决定 |
| 状态迁移、幂等、超时、取消 | 与 Docker/nginx/调度器副作用绑定 | 分页控件、筛选控件、排序控件 | 内核只声明查询语义 |
| 审计事实与危险动作标记 | 合规与追溯要求；`dangerous` 由内核声明 | 提示条、确认弹窗、抽屉 | 交互形态属框架 |
| 异步作业、进度与日志游标 | 事实源在 `jobs` 表与内存缓冲 | 页面文案排版与多语言呈现 | 文案可用 key 由前端决定落点 |
| 稳定错误码与错误类别 | 客户端分支依赖它，不能靠 HTTP 状态猜 | 重定向、PRG、flash 语义 | 是旧 UI 的结果传递方式 |
| 资源字段语义（类型、单位、只读、敏感、引用） | 决定数据能否被正确理解与授权 | 局部刷新片段与 htmx 分支 | 是旧渲染技术的实现细节 |
| 流式通道（SSE/WS/上传下载） | 与长连接、票据、超时层强绑定 | 主题、CSS、静态资产 | 表现层资产 |

### 1.4 耦合的根因

1. **五合一 handler**：解析表单、CSRF、授权、调用领域、写审计、拼响应都在同一函数。站点、运行时、Docker、文件四组写路径已拆为「解析 → 命令 → 响应」，读 handler 与插件/任务/作业仍是多职责一体。
2. **视图模型带表现语义**：`state_class`（`ok`/`warn`/`danger`）、`state_label`（中文）、格式化后的字符串进入视图结构；同类数据在 `app` 与 `web` 各写一遍（如运行环境的两种视图）。
3. **表单字段名即接口**：字段名散落在模板与 handler，改名等于破坏契约；没有字段类型、必填、范围的可机读描述。
4. **三套错误出口**：`AppError` 渲染 HTML 错误页，`ApiError` 输出 `{code,message}`，写操作错误又走 flash 文案；没有稳定机器码，也没有字段定位。
5. **结果经重定向传递**：写操作不返回结果本体，而是「写会话 flash + 303」，客户端拿不到结构化结果，无法做局部更新。
6. **列表重取写在写 handler 里**：如镜像删除成功后递归调用列表 handler 重新渲染整页，读与写路径缠在一起。
7. **查询参数各自为政**：`?filter=`、`?q=`、`?tab=` 由各页自行解释，审计固定取 200 条、作业固定取 limit，没有统一分页模型。
8. **导航与授权混写**：菜单既要读 manifest 声明、按角色过滤，又要排序、落库偏好、生成渲染模型；前两者属内核，后两者属表现层。逻辑已迁到 `app/menu.rs`，混写事实仍在。

### 1.5 已解耦条目（2026-11-12）

| 条目 | 原落点 | 当前事实 |
|---|---|---|
| Compose 保留项目名与归一化 | `web::handlers::compose::checked_project` | `commands::docker::prepare_project`；`compose_up`/`compose_down`/`compose_project_action` 入口强制校验，Web 无保留名常量 |
| 概览「待办」业务判定与排序 | `web::handlers::dashboard::dashboard_todos` | `skiff_app::attention::collect` 只产出 code/level/count；中文文案与深链留在 Web 的 `todo_view` |
| 站点/运行时/Docker/文件写命令 | 各写 handler 内联领域调用与审计 | P1 四组已拆到 `crates/app/src/commands/`，handler 只做解析、授权入口、错误映射与响应 |
| 站点证书就绪与运行态分类 | Web 列表自判 cert+key；`runtime_site_status`/`version_display` 在展示层做优先级 | `SiteSettings::cert_ready` 为唯一判定；`SiteRuntimeStatus`/`RuntimeVersionState` 在 app 分类，Web 只映射文案与配色 |
| 作业可取消/可重试 | Web 详情页按 `active()`/`status.terminal()` 自行计算 | `Job::can_cancel`/`can_retry` 在 app；`JobService::cancel`/`retry` 仍独立校验，Web 只读字段 |
| 内置网络不可删除 | 只在 `networks.html` 按名字隐藏按钮，后端依赖 Docker 报错 | `NetworkRow::builtin/can_remove` 判定；视图读 `can_remove`，`network_remove` 命令层先拒绝内置网络并写失败审计 |
| 任务/作业审计 | Web 的 `tasks.toggle/run/delete` 与 `job.cancel/retry` 在 handler 内联写审计 | 审计随 `commands::task` / `commands::job` 产生；`middleware.rs`/`lib.rs` 的会话与 panic 审计按传输/进程保留 |
| 站点表单错误→字段 | `web::handlers::sites::error_field` 用中文子串匹配 | `SiteError::Field(SiteField, ...)` 携带字段码；Web 只查表，未知码回顶部提示，文案与原二分逻辑不变 |
| 领域枚举中文展示 | App 的 `JobStatus::label`、`RuntimeJobState::label`、`ExtSource::label`、`CatalogState::label` | App 只留 `as_str`；中文集中在 `web::labels`，模板与 JSON 共用；`kind_label` 的产品名仍留在 App 供错误文案使用 |

---
