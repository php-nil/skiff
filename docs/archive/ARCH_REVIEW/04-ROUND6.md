# 第六轮全量复审（§14）

> 上级索引：[ARCH_REVIEW.md](../ARCH_REVIEW.md) ｜ 拆自 `docs/archive/ARCH_REVIEW.md` 原 §14（2026-11-16 整理，正文未改）。

---

## 14. 第六轮：全量复审（2026-11-02，模块划分 / 职责边界 / 依赖关系 / 复用性）

> 触发：用户要求「全面检查代码架构与组织结构」。本轮**只审查不修码**，结论全部来自当前工作区实测，
> 不引用前几轮的行号与数字。

### 14.0 基线（本轮实测）

| 项 | 数值 | 测法 |
|---|---|---|
| 结构 | **14 crate + `xtask`** | 根 `Cargo.toml` members |
| 代码量 | **21,536 行 / 83 个 `.rs`**（不含 `plugins/`）；插件 workspace 2,186 行 / 6 个 `.rs` | `find` 全量 `.rs` 后 `xargs wc -l` 求和 |
| 测试 | **215 项：212 通过 + 3 项红**（既存 Windows 文件锁限制，与 §13.3 同因；Linux CI 待复跑）；`sdk/tests/kernel_contract.rs` 在 Windows 上 0 项（unix-only） | `cargo test --workspace --locked --no-fail-fast` |
| 前端资源 | 13 个模板 / 692 行，`static/` 4 类资源 + gzip 变体 | `ls crates/web/templates` |

逐 crate 规模与测试分布：

| crate | 行数 | 文件 | 测试 | 每千行测试数 |
|---|---|---|---|---|
| `web` | 4882 | 17 | 30 | 6.1 |
| `core` | 2936 | 13 | 18 | 6.1 |
| `plugin` | 2755 | 9 | 24 | 8.7 |
| `pkgfmt` | 2153 | 5 | 24 | 11.1 |
| `sdk` | 1374 | 3 | 5 | 3.6 |
| `model` | 1244 | 7 | 28 | 22.5 |
| `store` | 1196 | 12 | 10 | 8.4 |
| `docker` | 978 | 3 | 10 | 10.2 |
| `app` | 942 | 2 | 14 | 14.9 |
| `policy` | 914 | 3 | 18 | 19.7 |
| `compose` | 681 | 1 | 6 | 8.8 |
| `identity` | 481 | 5 | 11 | 22.9 |
| `wire` | 390 | 1 | 9 | 23.1 |
| `cron` | 294 | 1 | 8 | 27.2 |
| `xtask` | 316 | 1 | 0 | 0 |

超 400 行的单文件（认知负担集中点）：`pkgfmt/package.rs` 1012、`web/tests/main_flow.rs` 983、
`pkgfmt/manifest.rs` 920、`docker/client.rs` 819、`compose/lib.rs` 681、`sdk/lib.rs` 609、
`app/lib.rs` 589、`web/handlers/plugins.rs` 571、`plugin/install.rs` 526、`policy/policy.rs` 510、
`plugin/fsops.rs` 495、`sdk/bin/skiff-plugin.rs` 496、`plugin/supervisor.rs` 477、`web/handlers/containers.rs` 465。
`store` 已无超 300 行文件（§10.4 的按聚合拆分仍然有效）。

**四维评级**：模块划分 **A−** ｜ 职责边界 **B** ｜ 依赖关系 **A−** ｜ 复用性 **B−**

### 14.1 一句话结论

**骨架已经稳了，问题全部落在「业务动作没有归口」这一处。**
五轮审查收敛掉的是分层与重复实现；本轮实测里剩下的缺陷，根因高度同源——
**「授权 → 执行 → 留痕」这套动作没有唯一入口，谁调用谁自己拼**。
于是同一个动作在 Web、JSON API、CLI、插件 UDS 四条路径上的副作用与审计条数各不相同。

三个结构面改善是本轮实测确认的（不是文档声称）：

- **依赖拓扑无环，且无死声明**：14 个 crate 的内部依赖逐条比对源码引用，声明但未使用的内部依赖 **0 条**。
- **`store` 的分层拆分保持**：`db.rs` 255 行 + 8 个聚合子模块，最大子模块 253 行。
- **判定与 I/O 的隔离仍然成立**：`model` 无 `std::fs`（只有 `std::net::IpAddr` 用于 CIDR 解析）、`compose` 与 `wire` 零 I/O、`policy` 依赖仅 `model` + `serde` + `libc`。

### 14.2 模块划分（B+ → A− 的差距）

| 判定 | 证据 | 说明 |
|---|---|---|
| `docker` 已收敛干净 | 3 个文件：`client.rs` 819（唯一碰 bollard）、`resolve.rs` 126、`lib.rs` 33（转出） | 初审的「策略混在 I/O crate」问题彻底消失 |
| `core` 已回到装配层 | `kernel/` 拆 7 个文件（最大 `capability.rs` 235 行），`commands.rs` 422 行是 CLI 编排，`config.rs` / `bootstrap.rs` / `server.rs` 各 200–380 | 第五轮 `kernel.rs` 992 行的问题不再存在 |
| `web` 仍是最宽的一层 | 4882 行里 handler 2,700 行（`plugins.rs` 571 + `containers.rs` 465 + `mod.rs` 347 + `resources.rs` 338）、模板视图 292、中间件 346、路由 196 | 宽是「表现层」的固有宽度，可接受；问题在它同时兼任业务编排（见 14.3） |
| `plugin` 职责被撑开 | 9 个文件 2,755 行：安装 526 + 能力面 HTTP 503（`capability.rs` 339 + `fsops.rs` 495 的一部分）+ 进程 477 + 互调 377 | 一个 crate 里同时住着「包管理」「沙箱进程」「HTTP 能力面」三件不相关的事，见 14.4-1 |
| `app` 仍是「上下文 + 一个用例」 | 942 行：`lib.rs` 589（AppState + 限流表 + 票据表 + interop 标记）、`auth.rs` 353（登录用例） | 用例层只建了 1/1，没有第二个用例跟进（见 14.3-1） |
| `sdk` 测试密度偏低 | 1,374 行 5 项测试（3.6/千行），其中 `bin/skiff-plugin.rs` 496 行只有 CLI 参数解析测试 | 插件作者直接用的工具链，回归面最薄 |

### 14.3 职责边界：本轮的核心发现

**1. 「授权 → 执行 → 留痕」没有归口，四条路径各拼一遍。**
`policy.check` 在生产代码里有 **13 个调用点**（`web` 12 + `core` 1），每处后面都跟着手写的
`map_err` → 执行 → `match` → `audit`。同一件事的两种写法并存：

| 路径 | 位置 | 行为 |
|---|---|---|
| UI 容器动作 | `web/handlers/containers.rs:198-240` | 判权失败写 `deny` 审计 → 执行 → ok/fail 审计 |
| JSON API 重启 | `web/handlers/api.rs:112-146` | 判权失败**不写**审计 → 执行 → ok/fail 审计 |
| 镜像/网络/卷动作 | `web/handlers/resources.rs` 6 处 | 判权失败**不写**审计 → 执行 → 结果审计 |
| 插件 UDS 调 Docker | `core/kernel/capability.rs:153-208` | 判权失败写 `deny` 审计 → 执行 → ok/fail 审计 |

结果：**13 个判定点里只有 2 个会记录越权尝试**。`audit` 表按 `result=deny` 只能筛到容器动作与插件越权，
镜像删除、网络创建、API 重启的拒绝尝试一条都不留。这不是「风格不统一」，是**可审计面缺了一大块**。

同理，`restart` 这个用例有两份几乎逐行相同的实现（`containers.rs::action` 与 `api.rs::restart_container`），
差别只在错误映射与是否写 deny 审计——正是「两份实现必然漂移」的教科书样本。

**2. 审计归属没有定层，同一次安装写两条。**
`skiff_plugin::install_package` 成功时自己写一条 `plugin.install`（`plugin/install.rs:252`，actor=`system`），
而两个调用方各自又写一条：

- Web：`web/handlers/plugins.rs:356`（actor=用户名）
- CLI：`core/commands.rs:168`（actor=`cli`）

于是**每次装包产生 2 条 `plugin.install`**，卸载同理（`install.rs:343` 一条 system + `plugins.rs:436` 一条用户名）。
第五轮 §13.2 的「CLI 装包不留审计已补齐」，补的其实是第三条路径的重复，而不是把归属定下来。
现有回归测试用 `.any(...)` 断言「存在一条」（`web/tests/main_flow.rs:452`），**结构上抓不到重复**。

**3. CLI 与 Web 的同一动作副作用不同。**
`plugin enable/disable`：Web 会先 `supervisor.stop()` 再改库并写 `plugin.toggle` 审计
（`web/handlers/plugins.rs:402-416`）；CLI 只改库两列，**不停进程、不写审计**（`core/commands.rs:201-210`）。
服务运行期间用 CLI 停用一个插件，进程继续跑、审计无痕、UI 显示已停用——三处状态互相矛盾。
`set_plugin_enabled` + `set_plugin_status` 这对调用也在两处各写一遍（两列合并写已于 2026-11-10 收口到 `Db::set_plugin_enabled_state`；CLI 仍无 `supervisor.stop()` 与审计，本条副作用差异未变）。

**4. 上传暂存是业务逻辑，却住在表现层。**
`web/handlers/plugins.rs:195-286` 自己做：读 multipart → 落 `.incoming/` → `0600` → 生成 24 字符令牌 → 二次提交时按令牌复原路径。
这套「两阶段上传」与 `pkgfmt` 的包处理、`plugin` 的安装生命周期是同一件事的三个阶段，却被拆在表现层里，
CLI 因此完全没有这个能力（`skiff plugin install` 只有单阶段）。

**5. 首登改密的白名单写在中间件里。**
`middleware.rs:179-184` 用路径字符串列表决定「必须改密时哪些页面还能访问」。新增一个页面要记得来这里加，
否则用户被莫名重定向；这条规则本质是「登录态的阶段约束」，属于 model 层的策略，不属于 HTTP 中间件。

### 14.4 依赖关系

**拓扑仍然无环，方向正确。** 逐条核对 `Cargo.toml` 声明与源码引用，**0 条死声明**（前几轮的 31 条已清理干净）。
`policy` / `pkgfmt` / `wire` 对 `store` / `docker` 的禁令全部遵守。

但第五轮的解法引入了一条新的语义越界：

| # | 依赖 | 位置 | 问题 |
|---|---|---|---|
| 1 | `plugin → axum` + `http-body-util` + `base64` | `crates/plugin/Cargo.toml`，`src/capability.rs` 339 行 | 为了解 `plugin → app` 的环，把**能力面的 HTTP 形态**也搬进了领域 crate：6 个 `fs_*` handler 直接返回 `StatusCode` / `Response`，用 `Json(...).into_response()`。后果是「插件体系」这个 crate 现在无法在不引入 web 框架的情况下被使用或单测；而 `CapabilityHost` 这层抽象本来只需要框架中立的签名（`&H, Value → (u16, Value)`）就够，axum 留在 `core::kernel` 即可 |
| 2 | `web → bollard`（间接） | `docker/lib.rs:15` `pub use bollard;`；`web/handlers/containers.rs:420-459` | 为了 exec 会话，表现层直接 match `bollard::exec::StartExecResults`，并手写 50 行 WS ↔ 容器流的泵浦循环。域逻辑漏到表现层，且这段在 Windows 上无法测 |
| 3 | `web` 内部路径别名 | `web/lib.rs:17` `pub use skiff_app as state;` | 保留是为少改 11 个 handler 的 `use`。代价是 `crate::state` 与 `skiff_app` 两条 import 路径并存，读代码时「谁是归属层」看不出来。可接受，但应视为**已完成迁移的临时物**，新代码不该再用 |

`core → web` 的反向依赖已彻底消失（`cargo tree -p skiff-app` 不含 `skiff-web`），这条是第四轮修得最干净的一处。

### 14.5 复用性：重复实现清单（按代价排序）

| # | 重复项 | 位置 | 代价 | 收敛建议 |
|---|---|---|---|---|
| 1 | 授权+执行+留痕三段式 | 13 个 `policy.check` 点（见 14.3-1） | 越权留痕 11/13 缺失，且已产生两份 restart 实现 | 在 `skiff-app` 建容器用例：`AppState::run_docker(actor, verb, obj, channel) -> Result<Value>`，内部统一「判定 → deny 审计 → 执行 → ok/fail 审计」，handler 只做渲染 |
| 2 | 装/卸包的审计 | `plugin/install.rs:252,343` + `web/handlers/plugins.rs:356,436` + `core/commands.rs:168` | 每次操作 2 条审计，actor 不一致 | 归属定在 `plugin`（域函数），调用方只带 actor 与 channel 参数；删掉外层两条 |
| 3 | 插件启停 | `web/handlers/plugins.rs:402-416` + `core/commands.rs:201-210` | CLI 停用不停进程、不写审计 | 收一个 `skiff_plugin::set_enabled(db, supervisor, id, enabled, actor, channel)` |
| 4 | 路径归属判定 | `web/handlers/plugins.rs:513-522`（canonicalize + `starts_with`）、`pkgfmt/package.rs:134 is_within` | 已是第 4、第 5 份实现；`model::path::within_root` 就在手边（且两处 crate 都依赖 `model`） | 两处改调 `skiff_model::within_root`。功能等价，但「唯一来源」的红线就是这么破的 |
| 5 | `Grant` 的 `kind:value` 字符串 | `core/commands.rs:65`、`core/kernel/capability.rs:57`、`web/handlers/api.rs:168`、`web/handlers/plugins.rs:48` | 4 处手写，改格式必漏 | `impl fmt::Display for skiff_model::Grant` |
| 6 | TTL 表回收 | `app/lib.rs:99-115`（登录限流）、`app/lib.rs:173-185`（exec 票据） | 两段逻辑同构：retain 过期 → 超限按时间淘汰到 3/4 | 泛型 `TtlMap<V>`（`retain_live` + `evict_oldest_to(3/4)`），两张表共用，各带一条边界测试 |
| 7 | 24 字符短令牌 | `web/handlers/plugins.rs:125`（nonce）、`:230-231`（上传令牌） | 同上手法写两遍 | `skiff_identity::short_token(n)` |
| 8 | 插件启停的两列写入 | `web/handlers/plugins.rs:405-408`、`core/commands.rs:206-207` | `enabled` 与 `status` 必须同步改，漏一处就不一致 | **已收口（2026-11-10）**：`Db::set_plugin_enabled_state(id, enabled)` 单条 `UPDATE` 同时写两列，状态映射收在仓储层；Web / CLI / 官方同步 / 集成测试四处调用点全部改调 |
| 9 | `PluginId` 取值 | `core/kernel/capability.rs` 5 处手写 `req.extensions().get::<cap::PluginId>()`、`core/kernel/interop.rs` 同形 | `skiff_plugin::capability::plugin_of` 已有现成函数，core 没用 | core 统一调 `cap::plugin_of(&req)` |
| 10 | kernel 能力面转发壳 | `core/kernel/capability.rs:24-41` 6 个 5 行函数 | 只做 `State<Arc<AppState>> → &*state` 转换 | 一个泛型函数 + 路由表闭包，或让 `CapabilityHost` 直接实现对 `Arc<AppState>` |

`pkgfmt` 的格式所有权、`wire` 的路由常量、`model` 的路径代数与角色矩阵这三个「样板级收口」本轮复核仍然成立：
`wire::routes::*` 两端共用、`Permission` 只有一张表、`resolve_trust` 只有一份。**新增的重复都在「动作编排」层，不在「判定/格式」层。**

### 14.6 已致 / 潜在缺陷

| # | 缺陷 | 类型 | 证据 | 影响 |
|---|---|---|---|---|
| 1 | 装包/卸包各写两条审计 | 已致 | `plugin/install.rs:252,343` 与 `web/handlers/plugins.rs:356,436`、`core/commands.rs:168` 在同一调用链上 | 审计条数翻倍、actor 不一致；按 actor 归因会得出「system 装过一次、某人又装过一次」 |
| 2 | 11/13 个判权点不留越权痕迹 | 已致 | `resources.rs` 6 处、`api.rs:98,124`、`containers.rs:76,261` 均为 `map_err` 直接转 `AppError::Forbidden`，前后无审计写入 | 无法回答「谁试过删镜像但被拒」；与 §6 的审计承诺不符 |
| 3 | CLI `plugin enable/disable` 副作用缺失 | 已致 | `core/commands.rs:201-210` 无 `supervisor.stop()`、无 `audit` | 运行中停用插件 = 进程仍在跑 + 无留痕 + UI 显示已停用 |
| 4 | 插件 UI 静态文件无认证 | 潜在 | `web/lib.rs:186` 路由未挂认证提取器，`plugin_ui`（`handlers/plugins.rs:494`）签名只有 `State` + `Path`；`middleware.rs:34-38` 的 `is_exempt` 又把 `/plugins/*/ui/*` 从安全入口前缀检查里豁免 | 不登录、不知道入口前缀也能 `GET /plugins/<id>/ui/<file>`，靠 200/404 枚举已安装插件——直接削弱 §6.3「隐蔽入口」的意图。JSON 侧 `/api/v1/plugins/ui` 是要 `AuthUser` 的，两侧口径不一致 |

缺陷 1 的根因与 14.3-2 相同；缺陷 2 与 14.3-1 相同；缺陷 3 与 14.3-3 相同——**四条里三条是同一个结构问题的投影**。

### 14.7 行动清单

| 级 | 项 | 方案 | 预期 | 潜在影响 | 成本 |
|---|---|---|---|---|---|
| P0 | 审计归属定层 | 装/卸审计只留 `plugin` 内部一处，调用方传 `actor` + `channel`；删 `web` 与 `cli` 外层两处 | 每次操作恰好 1 条，actor 可归因 | 审计历史口径变化（旧数据有重复行） | 小 |
| P0 | 判权拒绝统一留痕 | 13 个 `policy.check` 点收敛到 `AppState::run_docker`（内部写 deny 审计），或先给每个点补 deny 审计 | `result=deny` 可完整筛出越权 | 审计量上升（预期内） | 中 |
| P1 | 建容器用例层 | `skiff-app` 增 `run_docker`，`containers::action` / `api::restart_container` / `resources::*` 全部改调用 | 两份 restart 合一，新动词只改一处 | handler 签名小幅变动 | 中 |
| P1 | CLI 启停收口 | 新增 `skiff_plugin::set_enabled(db, supervisor, id, enabled, actor, channel)`；CLI 与 Web 同调 | 副作用与留痕一致 | CLI 需要能连到 supervisor（进程内 CLI 与 server 分离，需明确「CLI 改库 → server 下次扫描生效」还是直接拒绝运行中操作） | 中 |
| P1 | 插件 UI 加认证 | `plugin_ui` 挂 `AuthUser`；`is_exempt` 只豁免 `/static/` 与 `/healthz` | 与 JSON 侧口径一致，恢复隐蔽入口语义 | iframe 需带 Cookie（本来就会带） | 小 |
| P2 | `plugin` 去 axum | 能力面签名改框架中立（`&H, Value → (u16, Value)`），axum 适配回 `core::kernel` | `plugin` 可无 web 框架单测，编译面缩小 | 6 个 fs handler + 3 个能力 handler 改签名 | 中 |
| P2 | exec 泵浦下沉 | `DockerService::exec_session(container) -> (impl Sink, impl Stream)`，`web` 只做 WS 编解码 | 域逻辑回归 `docker`；可脱平台测试 | `docker` 需定义流类型 | 中 |
| P2 | 小项清理 | `Grant: Display`、`within_root` 两处替换、`TtlMap`、`short_token`、`cap::plugin_of` 复用（`set_plugin_enabled_state` 已于 2026-11-10 收口） | 重复清零 | 无行为变化 | 小–中 |
| P2 | 首登改密白名单入 model | 把「必须改密时允许的页面」变成 `model` 里的策略函数 | 新增页面不再需要改中间件 | 无 | 小 |

### 14.8 未覆盖（不要据此下结论）

- **运行时行为**：Linux 专属路径（setuid、landlock、cgroup、`kernel.sock`、docker.sock）本轮无法执行，缺陷 4 是静态读路由表与 handler 签名得出的，未做真机抓包确认。
- **并发与背压**：`exec_tickets` / `login_limiter` 的锁粒度（单 `Mutex` 包整表）未做压测；`interop_busy` 用 `HashSet` 做「单层调用」标记，多目标并发时的语义未验证。
- **性能**：未做剖析。已知的低效点仍在：`resolve_container` 每次动作都全量 `list_containers`（`handlers/mod.rs:242-256`），容器多时是 O(n) 拉取。
- **`plugins/` 第二 workspace**：只核对了它对 `skiff-model` 的复用（`plugins/common/src/lib.rs:70-88` 转出 `within` / `within_any` / `normalize_dir`），未逐 crate 审查其内部结构。
