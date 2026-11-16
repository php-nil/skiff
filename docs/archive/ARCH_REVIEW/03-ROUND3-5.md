# 第三至五轮：代码优化与结构收敛（§11–13）

> 上级索引：[ARCH_REVIEW.md](../ARCH_REVIEW.md) ｜ 拆自 `docs/archive/ARCH_REVIEW.md` 原 §11–13（2026-11-16 整理，正文未改）。

---

## 11. 第三轮：代码优化（2026-10-31）

> 目标：最小改动、可验证、有依据。**不做**风格性重排，只处理「行为不正确 / 声明与实现不符 / 有可测量代价」的项。
> 基线：`cargo test --workspace` 118 项全绿、`fmt` 与 `clippy -D warnings` 干净（不是「已坏」状态）。

### 11.1 逐项

| # | 问题（依据） | 改动 | 验收证据 |
|---|---|---|---|
| 1 | `handlers::resolve_container` 用单趟 `find` 同时匹配「id 全等 / 名称全等 / id 前缀」：列表顺序决定结果，精确 id 或名称可能输给靠前的前缀命中；两个容器前缀相同则静默取第一个。容器操作是破坏性的，猜错代价高于拒绝 | 拆出纯函数 `match_container`：阶段一精确 id/名称（扫完整表），阶段二 id 前缀且要求唯一，歧义报 400 | `handlers::tests` 6 项（精确优先 / 名称命中 / 唯一前缀 / 歧义拒绝 / 不存在 / 空引用） |
| 2 | `exec_ws` 的 Origin 校验写成 `if let Some(origin)`：**缺头即放行**；且无条件接受 `http://{host}`。`06-security` 写的是「Origin 与预期 origin 完全一致」 | 抽出 `handlers::ws_origin_allowed(&Option<&str>)`：缺失/空即拒；`http://` 仅 `dev_http` 下接受；`external_url` 为权威来源并容忍尾斜杠 | `handlers::tests` 5 项（缺头拒 / 同源 https 过 / 生产 http 拒而 dev 过 / 跨站拒 / external_url 命中） |
| 3 | `AppState::login_limiter` 每个新源 IP 插一条**永不回收**的记录：`LOGIN_WINDOW`/`LOGIN_BLOCK` 定义了有效期却无任何 GC，条目只增不减（可信代理下 `X-Forwarded-For` 可批量造键），构成内存 DoS 面 | 上限 `LOGIN_TRACK_MAX = 4096`；越界先 `retain` 回收过期条目，仍超限按最早失败时间淘汰到 3/4（避免每来一个新 IP 全表扫）；判定逻辑抽成 `login_gate` / `note_failure` 以便注入 `Instant` | `state::tests` 4 项（封禁与剩余秒数 / 窗口后清零 / 表有界 / 过期回收且不牺牲封禁中的 IP） |
| 4 | **真实功能缺陷**：`/plugins/install` 落在 axum 默认 2MB body 上限之内，而 `.wbpkg` 内含插件二进制必然超过。超限错误发生在 `Multipart` 解析阶段，被 `map_err` 包装成「上传解析失败」，handler 里那条 `DEFAULT_LIMITS.archive_bytes`（100MB）判断**永远走不到** | 该路由挂 `DefaultBodyLimit::max(archive_bytes + 64KB)`：框架上限抬到应用层上限之上，真正判定仍由 handler 做 | `main_flow::plugin_upload_body_limit_is_not_the_framework_default`（3MB 上传：改前「上传解析失败」，改后走到 tar 解析） |
| 5 | `entry_guard` 对**每个响应**做 3 次 `HeaderValue::parse().unwrap()` + 一次 CSP 字符串拼接解析 | 静态头改 `HeaderValue::from_static`；CSP 用 `OnceLock<HeaderValue>` 缓存一次 | `tests::security_headers_on_every_response` |
| 6 | `Verb::parse` 与 `Verb::as_str` 各写一份 26 行 match，靠注释提醒同步（策略层解析表的漂移会表现为「白名单里有、解析不了」） | `parse` 改为由 `Verb::all()` 派生（线性查找，仅内核回调解析路径，不在请求热路径） | `policy` 21 项，含新增 `all_covers_every_verb`（数量 26 + `as_str` 无重复） |
| 7 | `tower-http` 启用 `compression-gzip` / `timeout` / `limit` 三个特性，但代码只挂 `CatchPanicLayer`（gzip 是 `build.rs` 预压缩） | workspace 依赖收敛为 `features = ["catch-panic"]` | 构建通过；`cargo tree -p tower-http -e features` 仅剩基础依赖；`Cargo.lock` 无变化（说明此前这些特性未真正引入额外包，收益是声明与实现一致，不是瘦身） |
| 8 | 注释写「解析表单体（限制 256KB）」，实际是 axum 默认 2MB | 注释改为事实描述，并指向插件上传路由的显式上限 | 无代码变更 |

### 11.2 未做（明确留下）

| 项 | 原因 |
|---|---|
| API Token `scopes` 只存不判 | `06-security` 只写「API Token 带 scope」，未定义语义与强制点。补语义属功能设计，不是最小改动；按「不猜设计」留待专门一轮 |
| `core/kernel` 仍挂 `web::AppState` | 与 §10.4 遗留一致：AppState 下沉 `skiff-app` 是独立一轮的结构改动 |
| 全局请求超时 | SSE / WS 需要长连接，`TimeoutLayer` 会误杀；正确做法是按路由分别设超时，改动面大于本轮范围 |
| `exec_ws` 的「短期单连票据」 | `06-security` 承诺项，属功能缺失而非优化，需要新存储与前端配合 |

**收口数字（实测）**：**118 → 136 项测试全绿**（+18：6 容器匹配、5 Origin、4 限流、1 安全头、1 上传上限、1 动词表完整性）；`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`、`cargo test --workspace --locked` 全过。

## 12. 第四轮：把 §11.2「明确没做」的四项做完（2026-10-31）

§11 留这四项的理由是「不属于最小改动」。本轮重新审了一遍：三项是**文档已承诺、代码没兑现**的功能缺口，一项是**目标结构里写了却一直没做**的搬迁。都不是「要不要做」，而是「什么时候做」。四项全部落地。

| # | 项 | 实际问题 | 做法 | 验证 |
|---|---|---|---|---|
| 1 | exec 单连票据 | `06-security` §6.2 承诺「使用短期单连票据绑定会话和权限」，实现里只有 Origin + 会话 Cookie：任何拿到 Cookie 的上下文都能直接连终端，票据这层完全不存在。「Origin 校验」只能证明来源站点，不能证明「这次连接是用户刚点的」 | `POST /{prefix}/containers/{id}/exec/ticket`（admin + CSRF + `Exec` 动词策略）签发票据：30 秒 TTL、一次性、绑定 session id 与容器 id，内存只存 `sha256`；WS 必须带 `?ticket=` 才 upgrade；前端改为先取票再连。票据表 512 上限，越界先回收过期再按最早到期淘汰 | `app` 5 项（单次使用 / 过期 / 跨会话 / 跨容器 / 表有界）+ `main_flow::exec_requires_single_use_ticket_bound_to_session_and_container`（`authorize_exec` 全链路，含 viewer 拒绝） |
| 2 | API Token scopes | 设计只写「API Token 带 scope」，代码里 `verify_api_token` 返回的 scope 被 `_scopes` 丢掉；CLI 更把占位串 `api` 写进这一列——等于这一列纯装饰，任何 token 都是全权限 | `skiff-model::ScopeSet`：`docker:read` / `docker:write` / `plugins:read` / `*`；语义是**收窄**——token 权限 = 角色权限 ∩ scope；`docker:write` 蕴含 `docker:read`；解析严格，未知 scope 报错。`token create` 新增 `--scope`（默认 `docker:read`）。`/api/v1` 五个端点各挂所需 scope，会话认证不受影响 | `model` 6 项（空集 / 蕴含 / `*` / 未知报错 / 规范化 / 往返）+ `main_flow::api_token_scopes_are_enforced`（只有 `plugins:read` 的 token 打 docker 得 403、非法 scope 得 401 且留审计、Cookie 会话照常 200） |
| 3 | 请求超时 | 之前判断「全局 `TimeoutLayer` 会误杀 SSE/WS」——这个判断对，但结论应当是按路由分组，而不是不做。缺超时时，一个卡住的 Docker 调用会永久占住连接与信号量 | `middleware::request_timeout`（`tokio::time::timeout` + 新增 `AppError::Timeout` → 504 错误页，超时即丢弃 future 取消下游）。路由侧把 `logs`/`exec` 摘进独立的 `streaming` router，`request_timeout` 只 layer 在 `.merge(streaming)` **之前**——`Router::layer` 只作用于已加入的路由，流式端点因此天然不受影响 | `lib::tests::slow_handler_times_out_with_504`（慢 handler 得 504 且带「请求超时」文案，快请求仍 200） |
| 4 | `AppState` 下沉 | §6 目标结构里写着 `skiff-app`（装配与上下文），§10.4 与 §11 一直挂在遗留项：`core/src/kernel.rs` 直接 `use skiff_web::AppState`，插件 UDS 服务被迫反向依赖表现层，既编译更重，也无法脱离 axum 单测 | 新建 `crates/app`（`skiff-app`），`state.rs` 整体迁入；`web` 用 `pub use skiff_app as state` 保留路径别名（11 个 handler 的 `use crate::state::…` 一行未改）；`core` 的 kernel / bootstrap / server 改吃 `skiff_app::AppState` | `cargo tree -p skiff-app` 中 `skiff-web` 计数为 **0**；全量 149 项测试与 clippy `-D warnings` 通过 |

### 12.1 顺带收口

- `check_csrf_header` 原是 `handlers/plugins.rs` 的私有函数，提到 `handlers/mod.rs`：exec 票据端点复用同一实现，CSRF 判定仍只有 `check_csrf` 一处。
- `ScopeSet::to_wire` 把「write 蕴含 read」显式写出来：读库的人不必翻 `allows()` 才知道这枚 token 能不能读。

### 12.2 收口数字（实测）

**13 个 crate + `xtask`**（新增 `skiff-app`）；**149 项测试全绿**（136 → 149：+6 scope、+5 票据、+1 exec 闸门、+1 scope 集成、+1 超时）；`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`、`cargo run -p xtask -- check`、Linux 目标 `cargo check -p skiff-{model,policy,wire} --target x86_64-unknown-linux-gnu` 均通过。

### 12.3 仍未做（诚实标注）

| 项 | 理由 |
|---|---|
| 票据不限制「同一 admin 的并发终端数」 | 多终端是产品允许的用法，限额属策略选择而非缺陷；风险主体是 admin 本人 |
| scope 不细到动词 | 粗粒度是刻意的：`docker:read` 覆盖 list/inspect/logs，动词级限制由 `policy` 的角色表承担，两处都不越权 |
| token 默认 scope 是 `docker:read` | 最小权限默认；要用 `restart` 之类必须显式 `--scope docker:write`，help 与创建输出都已写明 |
| Linux 真机上的 SSE 不断连验证 | 需要真 Docker 与真实连接，归 [DEPLOY_TEST.md](../../test/DEPLOY_TEST.md) 上机清单 |

## 13. 第五轮：结构收敛「全部执行」（2026-11-01）

前四轮修的是「缺陷」与「局部边界」，`crates/` 的骨架没动：能力面寄居在 `core`、
角色判定写三份、路径代数写三份、值类型散落在判定 crate 里。本轮把这些一次性收口，
**P0 三项 + P1 四项 + P2 五项全部落地**。逐项明细见
[docs/changes/2026-11-01.md](../../changes/2026-11-01.md)。

### 13.1 四条收敛主线

| 主线 | 收敛前 | 收敛后 |
|---|---|---|
| **角色→权限矩阵** | `policy` 内联 `Action` 三分法 + `web` 手写 `is_admin() \|\| role == Operator` + `Verb::permission()` 手写，三份各改各的 | `skiff_model::Permission` + `RoleExt::can()` 唯一一份；`Verb::permission()` 由 `all()` 派生 |
| **路径授权代数** | `policy::PathGuard` / `plugin::fsgrant` / 插件侧快筛各写一遍前缀匹配 | `skiff_model::path` 11 个零 I/O 原语；三处共用（字符串级快筛 vs canonical 级判定分工写进注释） |
| **能力面归属** | `crates/core/src/kernel.rs`（992 行）混着能力面实现 + socket 监听 | 能力面搬回 `crates/plugin`（`fsops`/`fsgrant`/`capability`/`interop`），`core` 只剩装配；宿主上下文经 `CapabilityHost` trait 注入（依赖倒置，解开 `plugin → app` 环） |
| **值类型下沉** | `ContainerSpec` 在 `policy`（带 `libc`）、`compose` 为了拿它而依赖 `policy` | 值类型一律进零 I/O 的 `skiff_model`（`container.rs` / `verb.rs` / `path.rs`）；`policy` 只留判定 |

### 13.2 顺带修掉的两个真实缺陷

- **中间件顺序反了**：axum 0.8 的 `Router::layer` 是**后加的先跑**（后加 = 更外层）。
  原 `.layer(protocol_guard).layer(plugin_auth)` 的注释声称「协议版本先于鉴权判定」，
  实际执行顺序相反。已改为先挂鉴权、最后挂版本闸（版本闸真正先于鉴权），顺序用
  一次性探针实测确认。
- **CLI 装包不留审计**：Web 侧装包写 `plugin.install` 审计，CLI 侧不写——两条安装
  路径留痕口径不一。已补齐（actor=`cli`、channel=`local`）。

### 13.3 收口数字（实测）

根 workspace **212 项测试通过**，3 项红均为既存 Windows 环境限制
（`pkgfmt` staging 清理断言与 `plugin::install` 两条迁移恢复用例的 `os error 145`），
以 Linux CI 为准；插件 workspace **29 项全绿**。`cargo fmt --all` 与
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` 干净。

### 13.4 仍未做（诚实标注）

| 项 | 理由 |
|---|---|
| Linux 目标交叉编译与真机验收 | 本机无 `x86_64-linux-gnu-gcc`，`kernel` 的 unix-only socket 层（90 行）只能由 Linux CI 复核 |
| `policy::PathGuard` 的 TOCTOU 窗口 | 与 §12.3 同因：`open_dir_nofollow` 已实现但未接入判定链，对外不得声称已消除 |
| 路径原语的字符串级 / canonical 级双口径 | 有意保留：插件侧快筛与内核判定本就该分层，不合并成一套 |
