# 架构与组织审查（MVP 代码）

> 主文档：[DESIGN.md](DESIGN.md) ｜ 本文件：代码架构审查 ｜ 部署与验收：[DEPLOY_TEST.md](DEPLOY_TEST.md)

**初审基线（2026-10-30）**：`crates/` 下 8 个 crate + `xtask`，12,013 行、90 个测试标记（执行 88，全过）。
**§0–§9 是初审与首轮执行的存档**，其中的行号、行数、测试数均已随改动腐化，只作对照基线；**当前实测基线见 §14.0**（该轮记 14 crate；其后 `sched` / `fs` 独立，当前 16 crate + `xtask`，见 [sections/11-decisions.md](sections/11-decisions.md) 决策 11）。

> **2026-10-31 首轮执行快照**：P0 四项与 P1 五项已按 §7 顺序落地（见 §9；逐项复核见 §10.1）。彼时结构 11 个 crate + `xtask`、**14,755 行**（54 个 `.rs`）、**107 项测试全绿**。**第二轮执行（§10.4）又拆出 `skiff-compose` 并把测试推到 118 项**，**第五轮（§13）再把能力面迁回 `crates/plugin`、值类型下沉 `skiff-model`**，当前基线以最新一轮（§14.0）的全量实测为准。读代码以符号名为准，勿引用正文行号。

审查维度：模块划分 / 职责边界 / 依赖关系 / 复用性。所有判断都附 `文件:行号`；凡属推断而非代码事实，显式标「推断」。

## 0. 结论摘要

**分层是对的，但有四处边界错位，其中两处已经在产生真实缺陷**（不是理论风险）。

| 维度 | 评级 | 一句话 |
|---|---|---|
| 模块划分 | B | crate 粒度合理、无循环依赖；但 `web` 一个 crate 承担了「展示 + 应用上下文 + 部分业务编排」三件事 |
| 职责边界 | C+ | `docker` 里混着**不依赖 bollard 的纯策略**；`core::kernel`（服务端 Broker）反向 import 表现层的 `AppState` |
| 依赖关系 | B− | 拓扑无环、方向大体正确；`core → web` 这一条把「组合根 → 表现层」变成了「业务组件 → 表现层」 |
| 复用性 | C | 「格式」复用做得好（`pkgfmt` 五个消费者共用一份实现）；「协议」和「安全判定」各写两遍 |

**四个结构性问题**（按影响排序）：

1. **跨进程协议实现两份**：`plugin/ipc.rs` 与 `sdk/src/lib.rs` 各自实现一遍 HTTP-over-UDS 帧格式、1 MiB 上限、JSON-RPC 错误封装。两边的路由字符串没有共享定义——**已经漂移**：SDK 调 `/api/v1/capability/config`，内核根本没注册这条路由（`core/src/kernel.rs:63-69` vs `sdk/src/lib.rs:116`），`Plugin::config()` 永久 404。
2. **应用上下文住在表现层**：`AppState`/`WebConfig` 定义在 `web/state.rs:17,33`，而 `core/src/kernel.rs:14` 直接 `use skiff_web::AppState`。于是「内核 Broker」无法脱离 `web` 编译，也无法用窄接口单测。
3. **安全判定写两遍**：TOFU 三段式（probe → 查信任库 → 二次验签或拒装）在 `core/src/commands.rs:80-105` 与 `web/handlers/plugins.rs:143-183` 各有一份；`capabilities(form)` 同样两份（`commands.rs:142-153`、`handlers/plugins.rs:247-254`）。这是「装不装」的判定，两处漂移就是安全口径漂移。
4. **动词分发写三遍**：`Verb` → Docker 方法的映射在 `web/handlers/containers.rs:222-228`、`web/handlers/api.rs:178`、`core/src/kernel.rs:300-341` 三处重复，且 `Verb` 的字符串解析在 `kernel.rs:406` 又手写了一个 `parse_verb`（`policy::Verb::as_str()` 的逆函数）。附带后果：审计 action 出现两套命名（`docker.restart` vs `docker.containers.restart`）。

## 1. 现状地图

| crate | 行数 | 文件 | 代码实际承担的职责 | 依赖 |
|---|---|---|---|---|
| `store` | 1076 | 4 | SQLite 连接/迁移/备份 + 9 类仓储（用户、会话、Token、插件、grant、信任键、配置、审计、settings） | 无 skiff 依赖 |
| `identity` | 546 | 6 | argon2id、TOTP、RBAC、会话令牌、备份码 | `store` |
| `docker` | 2374 | 5 | bollard 封装（I/O）+ 三层策略（纯逻辑）+ Compose 子集解析 + 路径校验 | `store`、`identity` |
| `pkgfmt` | 1507 | 4 | `.wbpkg` 清单/签名/打包解包（**内核与 SDK 共用的唯一格式实现**） | 无 skiff 依赖 |
| `plugin` | 938 | 4 | 安装/升级/卸载、独立 UID 分配、UDS IPC 客户端、Supervisor | `store`、`pkgfmt` |
| `web` | 3105 | 16 | 路由 + 中间件 + 8 组处理器 + 模板结构 + 静态资源 + `AppState`/`WebConfig` | `store`、`identity`、`docker`、`plugin`、`pkgfmt` |
| `core`（bin `skiff`） | 1759 | 7 | 配置解析、启动装配、HTTP 服务、kernel.sock Broker、CLI 子命令 | 全部 |
| `sdk` | 708 | 2 | 插件 SDK（std 侧协议实现）+ `skiff-plugin` CLI | `pkgfmt` |
| `xtask` | — | 1 | 商店包变量对齐、Compose 字段子集、包结构与验签 | `docker`、`pkgfmt` |

超 400 行的文件（认知负担集中点）：

| 文件 | 行数 | 备注 |
|---|---|---|
| `store/src/db.rs` | 936 | 9 类仓储 + 迁移 + 测试同处一文件 |
| `pkgfmt/src/package.rs` | 857 | 扫描、两遍解包、打包、`TeeWriter`、测试 |
| `docker/src/policy.rs` | 759 | 策略 + `Form` + `ContainerSpec` + 大量测试 |
| `docker/src/client.rs` | 653 | Docker 全部操作 + `ContainerRow` 投影 |
| `docker/src/compose.rs` | 617 | 子集解析 + 拓扑环检测 + 规范化 |
| `core/src/kernel.rs` | 426 | Broker：鉴权、能力路由、Docker 分发、审计 |
| `plugin/src/supervisor.rs` | 410 | 进程生命周期 + `pre_exec` 沙箱设置 |
| `web/src/handlers/plugins.rs` | 402 | 上传暂存 + TOFU + 安装 + 启停 + RPC 代理 + 插件 UI |

## 2. 依赖关系

```text
                     ┌──────────── pkgfmt ◄──── sdk
                     │                ▲
        store ◄── identity             │
          ▲          ▲                 │
          │          │                 │
        docker ──────┘                 │
          ▲                            │
        plugin ◄───────────────────────┘
          ▲
         web ──────────► store / identity / docker / plugin / pkgfmt
          ▲
         core ─────────► 全部（组合根）
```

**无循环依赖**，方向大体正确：数据层在下、格式层独立、组合根在最上。这是这套代码最结实的地方。

但有三条「能编过、语义上不该有」的依赖：

| 依赖 | 位置 | 问题 | 处理 |
|---|---|---|---|
| `core → web::AppState` | `core/src/kernel.rs:14` | Broker 依赖表现层的应用上下文；`AppState` 里 `login_limiter`、`csrf_secret`、`login_semaphore` 对 Broker 全是噪音 | 把「应用上下文」下沉为独立 crate（见 §6），或给 Broker 定义窄 trait（`db` / `docker` / `policy` 三件） |
| `core → web::Cidr` | `core/src/bootstrap.rs:87` | CIDR 是网络基础设施类型，住在「HTML 展示」crate 里；`Config.trusted_proxies` 还是 `Vec<String>`（`config.rs:48`），配置校验阶段不验证语法，非法项只在启动时 `warn` 后被丢弃 | `Cidr` 下沉到 `identity` 或独立 `net` 工具；`Config::validate()` 里对非法 CIDR **直接拒绝启动** |
| `docker → identity` | `docker/src/policy.rs` 用 `RoleExt` | 策略要判断角色权限，依赖合理；但 `Role` 本身在 `store`，`RoleExt` 在 `identity`，一个概念劈成两半 | 可保持；若做 §6 的 `model` 下沉，两者合并到一处更干净 |

## 3. 模块划分与职责边界（逐 crate）

| crate | 判定 | 证据 | 建议 |
|---|---|---|---|
| `store` | **偏宽**：单一类型 `Db` 承载 9 类仓储 | `db.rs` 全部 `pub fn` 都挂在 `impl Db` 上 | 按聚合拆 `mod users/sessions/plugins/trust/audit`，对外仍以 `Db` 为门面（`impl Db` 分散到各 mod），文件回到 200–300 行量级 |
| `identity` | **边界清晰** | 纯函数式：口令、TOTP、令牌、RBAC | 保持。可接收 `Cidr` 下沉 |
| `docker` | **偏宽且混层**：纯策略 + 网络 I/O + 文件系统校验挤在一个 crate | 全 crate 2374 行里只有 `client.rs`（653 行）真正碰 bollard；`policy.rs`、`compose.rs`、`paths.rs` 对 bollard **零引用**（`grep -c bollard` = 0，仅一处注释提到） | 拆 `skiff-policy`（纯判定，零 I/O，可被 web、core、xtask 共用并单测）与 `skiff-docker`（bollard 封装）。**这是收益最高的拆分**：策略是安全核心，应该能脱离 Docker 环境跑测试 |
| `pkgfmt` | **样板**：格式所有权写在 `lib.rs` 抬头，5 个消费者共用 | `pkgfmt/src/lib.rs:1-5` 明确「避免打包器与校验器各写一遍」 | 保持，并把 §4 的协议也照这个模式收口 |
| `plugin` | 边界清楚（安装/进程/IPC 客户端） | `install.rs`、`supervisor.rs`、`ipc.rs` | 保持；`install.rs` 里 DB 写入与文件操作交织，可抽出「计划 → 应用」两段 |
| `web` | **偏宽**：展示 + 应用上下文 + 安全判定 + 上传暂存 | `state.rs` 定义全局上下文；`plugins.rs` 做 TOFU 判定 | 上下文下沉；TOFU/能力映射上收到 `plugin` crate（见 §4） |
| `core` | **组合根被撑大**：CLI 里混着业务逻辑（备份拷贝、安装编排） | `commands.rs:220-260` 备份含插件数据拷贝；`commands.rs:67-170` 安装含 TOFU 与环境判定 | `kernel` 换成独立 crate 或至少与 `web` 解耦；`commands.rs` 只留「解析参数 + 调用服务 + 打印结果」，业务逻辑回落到 `plugin`/`store` |
| `sdk` | 边界清楚，但**协议自持** | `lib.rs:281-313` 自己拼 HTTP 头 | 见 §4 第 1 项 |

**部署形态 `Form` 位置错**：`pub enum Form` 定义在 `docker/src/policy.rs:294`，但它是「部署形态」概念，被 `core/config`、`core/bootstrap`、`core/commands`、`web/handlers/plugins` 使用。后果是 `plugin`、`pkgfmt` 想表达「某形态支持哪些能力」就必须依赖 `docker`——于是两端各自写了一份映射（§4 第 3 项）。

## 4. 复用性：重复实现清单

| # | 重复项 | 位置 A | 位置 B | 风险 | 收敛建议 |
|---|---|---|---|---|---|
| 1 | UDS HTTP 帧 + JSON-RPC 封装 + 1 MiB 上限 | `plugin/src/ipc.rs:42-80, 142-178` | `sdk/src/lib.rs:145-313` | **已漂移**：路由字符串不一致导致 `config()` 404；帧上限两处各写常量 | 新建 `skiff-wire`：只放**纯编解码**（构造请求头、解析响应、错误封装、`MAX_FRAME`），I/O 留在各自 crate（一端 tokio、一端 std）。路由表用常量集中定义，两端共享 |
| 2 | TOFU 三段式安装判定 | `core/src/commands.rs:80-105` | `web/handlers/plugins.rs:143-183` | 分支顺序与提示文案已经不同；`allow_unsigned` 的短路时机不一致 | 上收为 `skiff_plugin::trust::resolve(path, db, allow_unsigned) -> Result<PackageMeta, TrustError>`，两边只负责把错误翻译成 UI/CLI 文案 |
| 3 | `capabilities(form)` 形态→能力映射 | `core/src/commands.rs:142-153` | `web/handlers/plugins.rs:247-254` | 新增形态或能力时必忘一处，且忘掉的那一侧会**多给或少给**能力 | 作为 `Capability::available_in(form)` 放到 `pkgfmt`（`Form` 一并下沉） |
| 4 | 容器 id/name/前缀解析 | `web/handlers/containers.rs:73,202,277,320`、`api.rs:142,170`（**6 处**） | — | 六份相同闭包，改匹配规则要改六处 | `handlers::resolve_container(&state, &id) -> Result<ContainerRow, AppError>` |
| 5 | `Verb` → Docker 方法分发 | `containers.rs:222-228` | `api.rs:178`、`kernel.rs:300-341` | 第三处有 `unreachable!()`，新增动词时一旦漏改就是 panic 或 501 | `DockerService::execute(verb, target) -> Result<Value>` 单点分发，调用方只做审计与渲染 |
| 6 | `Verb` 字符串解析 | `policy.rs::as_str()` | `kernel.rs:406 parse_verb` | 手写逆函数，加动词必忘 | `Verb::from_str`（与 `as_str` 同处，靠测试成对校验） |
| 7 | HTML 转义 | `web/src/util.rs:140` | `error.rs:80`、`middleware.rs:188` | 见 §5 第 3 条：被用到非 HTML 上下文 | 保留唯一实现，但**只在 HTML 拼接处**用；URL 参数走 percent-encode |
| 8 | 错误页渲染 | `templates.rs:225 ErrorPage`（未被使用） | `error.rs:69-84` 手拼 HTML + 自己的 code→title 表 | 两套标题文案，改一处不生效 | `AppError::into_response` 用 `ErrorPage` 模板渲染 |

正面样板（值得继续照做）：`pkgfmt` 的格式所有权、`deny_unknown_fields` 在 `ContainerSpec`/`Manifest`/Compose 三处的统一「fail-closed」约定、`PathGuard` 只暴露 `validate_*` 而不暴露裸路径操作。

## 5. 已经造成的真实缺陷（由结构问题直接导致）

| # | 缺陷 | 根因 | 影响 | 修法 |
|---|---|---|---|---|
| 1 | `Plugin::config()` 永久 404 | 协议两端各持路由字符串，无共享定义、无集成测试 | 所有依赖配置项的插件拿不到配置（`sdk/src/lib.rs:116` → `kernel.rs:63-69` 无此路由） | 补 `/api/v1/capability/config` 路由（读 `db.plugin_config`，secret 项按需返回），并把路由常量收进 `skiff-wire` |
| 2 | 审计 action 两套命名 | 三处分发各自拼字符串 | `docker.restart` 与 `docker.containers.restart` 并存，按 action 过滤审计会漏 | 统一由 `Verb::audit_action()` 生成，禁止调用方手拼 |
| 3 | `Location` 头里做 HTML 转义 | 转义助手按「字符串」而非「上下文」选 | `?a=1&b=2` 变成 `?a=1&amp;b=2`，登录后跳转的参数名被污染（`middleware.rs:184-190`） | URL 上下文用 percent-encode（`next` 已做前缀校验 `auth.rs:143`，无开放重定向） |
| 4 | 错误页双渲染路径 | 模板与手拼并存 | 改文案要改两处，`ErrorPage` 成了死代码 | 见 §4 第 8 项 |

## 6. 建议的目标结构

```text
skiff-model      角色、权限、Grant、部署形态（Form）、CIDR —— 零 I/O 的共享词汇表
skiff-store      数据层（依赖 model）
skiff-identity   口令/会话/TOTP/RBAC（依赖 store、model）
skiff-policy     三层策略判定（依赖 model、store 的 Grant 读取）  ← 从 docker 拆出
skiff-docker     bollard 封装 + Compose 解析 + 路径校验（依赖 policy）
skiff-wire       UDS 协议编解码 + 路由常量 + 帧上限（依赖 serde）
skiff-pkgfmt     包格式（依赖 model）
skiff-plugin     安装/信任判定/Supervisor/IPC 客户端（依赖 pkgfmt、wire、model）
skiff-app        装配与上下文：AppState、WebConfig、服务生命周期（依赖全部下层）
skiff-web        路由 + 处理器 + 模板（依赖 app、policy、docker、plugin）
skiff（bin）     CLI + 启动入口
```

关键取舍：

- **`skiff-policy` 独立**是本轮最高性价比的改动：策略是安全核心，拆出来后可以零 Docker 环境跑全量判定测试，也让 `xtask` 能直接校策略表。
- **`skiff-wire` 只放纯编解码**，不引入异步运行时：内核侧用 tokio、插件侧用 std，共享层保持 `&[u8] → Result<Frame>` 的纯函数形态即可。
- **`skiff-model` 不必做全**：先把 `Form`、`Grant`、`Role`、`Cidr` 四个「跨 crate 共享的词汇」搬进去，其余领域对象留在各 crate 内部。
- **`web` 不再拥有上下文**：`AppState` 移到 `skiff-app`，`web` 只消费。Broker 则拿到一个只含 `db`/`docker`/`policy` 的窄结构体。

## 7. 行动清单

### P0 修缺陷（不改结构也能做，每项独立可验）

| 项 | 验收标准 |
|---|---|
| 补 `capability/config` 路由 | SDK `Plugin::config()` 返回插件配置；secret 项取值成功；无权限时 403 |
| 审计命名统一 | 全仓只有 `Verb::audit_action()` 生成 `docker.*` 动作名；`grep '"docker\.'` 只剩一处 |
| `Location` 改 percent-encode | 带 `&` 的 `next` 登录后跳转参数完整 |
| `ErrorPage` 接上 | `error.rs` 不再手拼 HTML，标题只在一处定义 |

### P1 边界收敛（结构改动，有回归测试配套）

| 项 | 验收标准 |
|---|---|
| 抽 `skiff-wire` | 两端共用编解码与路由常量；新增集成测试：起一个假 kernel，跑通 SDK 全部能力方法 |
| `Verb::from_str` + `DockerService::execute` | `kernel.rs` 的 160 行分发塌成一次调用；新增动词只改一处 |
| TOFU 上收到 `plugin` | `commands.rs` 与 `handlers/plugins.rs` 各自只剩「调一次 + 翻文案」；同一批包在两条路径上的判定结果一致（用同一组用例断言） |
| 抽 `skiff-policy` | `policy.rs` 迁出后 `cargo test -p skiff-policy` 不需要 Docker 环境；`docker` 只剩 I/O |
| `Form`/`Grant`/`Role`/`Cidr` 下沉 `skiff-model` | `capabilities(form)` 只剩一份实现 |

### P2 可选（收益明确但不急）

- `store/src/db.rs` 按聚合拆文件（对外 API 不变）。
- `handlers/mod.rs` 增加 `resolve_container`，消掉 6 处重复。
- 引入至少一个 `tests/` 集成测试目录：覆盖「登录 → 建容器 → 插件安装」主干，作为跨 crate 契约的守门人。

## 8. 审查方法与未覆盖

方法：静态阅读全部 `src`（12,013 行）+ `Cargo.toml` 依赖图 + 针对「同名/同义实现」做全仓 grep；用 `cargo check`/`cargo test` 确认改动前的基线（0 warning、88 项通过）。

未覆盖（不要据此下结论）：

- **运行时行为**：Linux 专属路径（setuid、landlock、Unix socket、docker.sock）在本机无法执行，§5 的缺陷 1 是通过路由表静态比对确认的，未做真机联调。
- **并发与背压**：`AppState` 里 `Mutex<HashMap>` 的登录限流、SSE 与 exec 流的生命周期未做压力验证。
- **性能**：未做剖析；`list_containers` 在每次动作里被调用（`containers.rs:195`、`api.rs:163`）属已知的低效点，容器多时是 O(n) 全量拉取。
- **SQL 注入面**：抽查为全参数化（`params![...]`），未逐条核对。

## 9. P0 / P1 执行结果（2026-10-31）

按 §7 顺序执行，每步跑 `cargo check --workspace` + `cargo test --workspace` 后才进下一步。

### P0（修缺陷）

| 项 | 落地 | 验收证据 |
|---|---|---|
| 补 `capability/config` 路由 | `kernel.rs` 新增 `config_get`，读 `db.plugin_config` 并返回 JSON；secret 项对插件自身可读、审计只记条数不记值 | 路由与 SDK `Plugin::config()` 共用 `skiff_wire::routes::CAP_CONFIG` |
| 给配置一个真实写入口 | `skiff plugin config <id>`（列值，secret 打码）/ `skiff plugin config <id> <key> <value>`（写值，`secret` 由 manifest 决定，键必须在 `config_schema` 内） | `cli::tests::plugin_config_splits_list_and_set`、`ConfigField::check_value`（number/bool/url 取值校验） |
| 审计命名统一 | 新增 `Verb::audit_action()`，全仓 `grep '"docker\.'` 只剩 `audit_action` 内部一处；`resources.rs`/`api.rs`/`containers.rs`/`kernel.rs` 的手拼字面量全部替换；`fs:read` 类动作改点分 `fs.read` | `policy::tests::audit_action_is_derived_from_as_str` |
| `Location` 改 percent-encode | 新增 `util::encode_uri_component`，登录跳转 `next` 参数不再被 HTML 转义污染 | `util::tests::uri_component_encoding` |
| `ErrorPage` 接上 | `AppError::into_response` 渲染 `error.html`，标题只在模板/一处定义；前缀由 `router()` 注入 `OnceLock`（`IntoResponse` 拿不到 AppState） | 模板渲染失败时退化为纯文本，不递归 |

### P1（边界收敛）

| 项 | 落地 | 验收证据 |
|---|---|---|
| 抽 `skiff-wire` | 新 crate：`MAX_FRAME`/`MAX_HEAD`、`build_request`、`parse_request_head`、`parse_response`、`build_response_head`、`rpc_request_body`/`rpc_result`、`routes::*`；`plugin/ipc.rs` 与 `sdk` 都改成纯 I/O + 调用它 | wire 自身 6 项测试；`sdk/tests/kernel_contract.rs` 起假 kernel 跑通 config/identity/fs×3/docker/net 共 7 条能力路径并校验 Bearer 头（**unix-only，本机未执行**） |
| `Verb::from_str` + `Docker::execute` | `Verb::parse`/`Verb::all` 与 `as_str` 同文件；`Docker::execute(verb, target)` 单点分发，`kernel.rs` 的 60 行 `match` 塌成一次调用，`containers.rs`/`api.rs` 同步改调用 | `policy::tests::verb_strings_roundtrip` 遍历 `Verb::all()` 校验 `parse∘as_str = id` |
| TOFU 上收 | `plugin::trust::resolve(path, db, allow_unsigned) -> TrustDecision`；CLI 与 Web 各只剩「调一次 + 翻文案」 | `trust::tests::matrix_is_unchanged_by_caller`（8 种输入组合里 4 类边界） |
| 抽 `skiff-policy` | `policy.rs` + `paths.rs` 整块迁出，改为依赖 `skiff-model`；`docker` 只保留 I/O 并转出策略类型 | `cargo test -p skiff-policy` → 20 项通过，依赖树只有 `serde` + `skiff-model`（无 rusqlite / bollard） |
| 词汇下沉 `skiff-model` | `Role`/`Grant`/`Form`/`Permission`/`RoleExt` 移入零依赖 crate；`store`/`identity`/`policy`/`docker` 均以 `pub use` 转出，调用方无需改 | `model` 3 项测试（角色序、grant kind、form 拼写） |

### 未做（明确留下）

- **P2 三项**（`db.rs` 拆文件、`resolve_container` 去重 6 处、`tests/` 主干集成测试）未动。
- **`AppState` 下沉成 `skiff-app`**（§4 结论 2）未做：它牵动 `web`/`core` 全部 handler 签名，收益低于本次已完成的四项，留作独立一轮。当前 `core → web` 的反向依赖仍在。
- `sdk/tests/kernel_contract.rs` 是 unix-only，Windows 开发机上**只编译不执行**；真机验证要等 Linux 环境。

## 10. 第二轮复审（2026-10-31，模块划分 / 职责边界 / 依赖关系 / 复用性）

### 10.0 复审基线（2026-10-31 复审当时实测，已被 §10.4 取代）

> 下表是第二轮复审**动手前**的快照。执行完 §10.3 后结构变为 12 crate + `xtask`、15,475 行 / 65 个 `.rs`、118 项测试——以 §10.4 为准。

| 项 | 数值 | 测法 |
|---|---|---|
| 结构 | 11 crate + `xtask` | 根 `Cargo.toml` members |
| 代码量 | **14,755 行** / 54 个 `.rs` | `find crates xtask -name '*.rs' \| xargs wc -l` |
| 测试 | **107 项全绿**（+1 doc-test ignored） | `cargo test --workspace --locked` |
| 测试分布 | core 9 / docker 10 / identity 11 / model 6 / pkgfmt 19 / plugin 6 / policy 20 / sdk 4 / store 10 / web 6 / wire 6 | 逐 crate `test result` 汇总 |
| 超 400 行文件 | web 3409(16 文件) / core 2072 / pkgfmt 1741 / docker 1510 / policy 1320 / plugin 1166 / store 1116；单文件最大 `store/src/db.rs` 1023、`pkgfmt/src/package.rs` 1012 | 同上 |

### 10.1 上轮 P0/P1 落地复核（逐项重验，9/9 属实）

| 项 | 结论 | 证据 |
|---|---|---|
| `capability/config` 路由 | 成立 | `kernel.rs` 路由表挂 `routes::CAP_CONFIG` ↔ SDK `Plugin::config()` |
| 审计命名单点 | 成立 | `grep '"docker\.'` 全仓只剩 `policy.rs::audit_action` 内部一处，成对测试在 `policy.rs:743-760` |
| `Docker::execute` 单点分发 | 成立 | 唯一 `match Verb` 在 `docker/src/client.rs:494+`；`kernel.rs:438` 明示「不再自己 match」 |
| TOFU 单点 | 成立 | `trust::resolve` 唯一实现；`commands.rs:89` 与 `handlers/plugins.rs:158` 各只调一次 |
| `Capability::available_in` 单点 | 成立 | 实现仅 `pkgfmt/manifest.rs:48`，两处调用 |
| `skiff-wire` 收口 | 成立 | `plugin/ipc.rs` 与 SDK 均只做 I/O，帧/路由/上限走 wire；`kernel.rs:88-89` 注释记录过字面量漂移教训 |
| `skiff-policy` 独立 | 成立 | 20 项测试独立跑；依赖仅 `model`/`serde`/`libc`，无 rusqlite/bollard |
| `ErrorPage` 接上 | 成立 | `error.rs:74` 渲染模板 |
| 词汇下沉 `model` | **部分成立** | Role/Grant/Form/Permission/RoleExt 已入；**`Cidr` 被静默略过**，见 10.2-不足-1 |

P2 三项与 `AppState` 下沉仍未动（与 §9 一致）：`db.rs` 1023 行单文件；容器解析未收口（`list_containers` 调用点 8 处：`api.rs`×3、`containers.rs`×5，另有 `dashboard.rs`×1）；无工作区级 `tests/`；`core → web::AppState` 反向依赖仍在。

### 10.2 本轮新发现

**冗余 —— 死依赖 31 条**（口径：crate 源码内无 `dep::`/`use dep` 路径引用；逐条人工复核过误报）：

| crate | 死依赖（runtime） | 死依赖（dev） |
|---|---|---|
| `core` | thiserror、hex、sha2 | — |
| `docker` | **skiff-store**、thiserror、tokio、tracing、libc | tempfile |
| `identity` | thiserror、serde | — |
| `plugin` | serde | tempfile |
| `sdk` | serde、toml、ed25519-dalek、sha2、rand、tar、zstd、chrono、libc | — |
| `store` | serde、serde_json、chrono | — |
| `web` | tower、tower-http、bytes、tracing、thiserror、base64 | — |

两条有放大效应：

- **`docker → skiff-store` 是死边**，却是 rusqlite 进入 docker/xtask 编译链的唯一通道（`cargo tree -p xtask -i rusqlite`：`xtask → skiff-docker → skiff-store → rusqlite`）。`Bytes` 实际用的是 `axum::body::Bytes`，`DockerError`/`AppError` 均为手写 `Display`，无一依赖对应 crate。
- 工作区 `tower-http` 开着 compression-gzip/timeout/limit/catch-panic 四个 feature，**全仓零调用**（`web/src/lib.rs` 路由只挂 `axum::middleware::from_fn_*`）。

**缺失 —— 文档承诺未兑现 2 项**：

1. **CatchPanic 安全网不存在**：`core/src/main.rs:4` 写明「panic = unwind …配合 tower-http 的 CatchPanic 转 500 + 审计」，但全仓无 `CatchPanicLayer`。现状：handler panic → 请求直接断，无 500、无审计。这是虚假安全感，按「报成功前先确认真的成了」的口径列为 P0。
2. **gzip 预压缩不存在**：`crates/web/Cargo.toml:31` 注释与 `docs/sections/02-tech-stack.md:26` 均称「build.rs 做 gzip 预压缩 + embed」，`crates/web/build.rs` 不存在，`web-assets` 无 `.gz` 产物。

**不足**：

1. **P1 执行与计划不一致且未标注**：§7 P1 原文是「`Form`/`Grant`/`Role`/`Cidr` 下沉 `skiff-model`」，§9 实际搬了 Role/Grant/Form/Permission/RoleExt，`Cidr` 仍在 `web/src/util.rs:8`（`core/bootstrap.rs:88` 反向引用 `skiff_web::Cidr`），§9「未做」清单也没提它。缺口本身小，静默缩水本身是记录问题。
2. **policy「零 I/O」表述不准**：`policy/Cargo.toml` description 与 `policy.rs` 抬头写「零 I/O」，但 `paths.rs` 做 `std::fs::canonicalize` 与 `libc::openat`（函数级 `cfg(target_os)`，`libc` 依赖本身未做 target 门控）。准确说法：零数据库/零 Docker，判定可独立测试（成立）；路径校验有文件系统触碰（`open_dir_nofollow` 已实现但未接入判定链，`paths.rs:8-10` 自注）。
3. **xtask 依赖过重**：只为 `skiff_docker::compose::parse`（`xtask/src/main.rs:266`）就编译 bollard + tokio（删掉死边 store 后仍剩）。Compose 子集是纯 serde_yaml 逻辑，应落叶子 crate 或 feature 拆分。
4. **`web-assets` 破坏 crate 自包含**：`askama.toml` `dirs = ["../../web-assets/templates"]`、`assets.rs` `#[folder = "../../web-assets/static"]`。资源只有 web 消费，却住在工作区根；挪 crate 即断。
5. **词汇一词两名**：`model::rbac::Permission`（角色→动作枚举）与 `pkgfmt::manifest::Permission`（清单权限声明）同名不同义；`Form` 经 model/policy/docker 三重转出，3 条 import 路径并存。
6. **core 不是装配层**：core 2,072 行里 `kernel.rs` 532 行是完整 UDS Broker、`commands.rs` 384 行含业务编排（上轮已判，维持）。`AppState` 下沉 `skiff-app` 仍未做。
7. 小项：`web/state.rs:187` `pub type IpParse = IpAddr;` 零引用；`bootstrap.rs:88-90` 非法 CIDR 仍 warn 后丢弃（初审 §2 建议的「拒绝启动」未做）。

**健康面（维持初审判断）**：依赖拓扑无环；`model`/`wire` 叶子干净；`pkgfmt` 五消费者共用一份格式实现（`plugin::install`/`plugin::trust`/`web`/`xtask`/`sdk` bin）；store「谁存谁定义」边界清晰；`policy` 可脱离 Docker/SQLite 独立测试。

**四维评级（本轮）**：模块划分 **B+** ｜ 职责边界 **B−**（承诺未兑现 2 项 + core 撑大）｜ 依赖关系 **B−**（拓扑对，31 条死声明拖累）｜ 复用性 **B−**（格式/协议/策略收口好，工具链与资产布局欠账）。

### 10.3 行动清单（第二轮）

| 级 | 项 | 方案 | 预期 | 潜在影响 | 成本 |
|---|---|---|---|---|---|
| P0 | CatchPanic 接线或撤回承诺 | `router()` 挂 `CatchPanicLayer`（panic→500+审计），**或**删掉 `main.rs:4` 与设计文档的表述 | 错误路径行为与文档一致 | 若接线：tower-http 由死转活 | 小 |
| P1 | 清 31 条死依赖 | 逐 crate 删声明，跑全量测试回归 | 编译面/插件作者依赖面缩小；xtask 不再编 rusqlite | 仅 Cargo.lock 变化 | 小 |
| P1 | 预压缩落地或撤回表述 | 补 `build.rs`（gz/br + embed 选择），或改 `02-tech-stack.md:26` 与 Cargo 注释 | 文档=实现 | 二选一 | 小–中 |
| P1 | `Cidr` 补下沉 `model`（或明确记录留下）+ 非法 CIDR 拒启 | `Cidr` 迁 `skiff-model`；`Config::validate()` 对非法项报错退出 | P1 计划闭环；配置错误fail-fast | `core`/`web` import 路径调整 | 小 |
| P1 | Compose 解析落叶子 crate | `skiff-compose`（serde_yaml 子集）或 `skiff-docker` feature 拆分 | xtask 去 bollard/tokio | 新 crate 或 feature | 中 |
| P2 | 上轮 P2 三项 | `db.rs` 按聚合拆 mod；`resolve_container` 收口；工作区 `tests/` 主干集成测试 | 认知负担下降、契约有守门人 | 无行为变化 | 中 |
| P2 | 文案与消歧 | policy 改「零数据库/零 Docker」；`manifest::Permission` 改名；删 `IpParse`；转出链标注 canonical 路径；`web-assets` 入 crate 或注明理由 | 可读性 | 无 | 小 |

### 10.4 第二轮执行结果（2026-10-31，「全部执行」）

> 两处二选一的决策：P0 CatchPanic 选**接线**（不删承诺）；预压缩选**落地**（不改文档）。

**P0**

| 项 | 落地 | 验收证据 |
|---|---|---|
| CatchPanic 接线 | `web::router()` 最外层 `CatchPanicLayer::custom(panic_response)`：panic → `tracing::error` + 审计 `web.panic` + ErrorPage 500；`main.rs:4` 的承诺变为事实，tower-http 由死转活 | `tests::catch_panic_returns_500_and_writes_audit` |

**P1**

| 项 | 落地 | 验收证据 |
|---|---|---|
| 清死依赖 | **净删 28 条**（含 `docker→skiff-store`、sdk 9 条、web 4 条、store 3 条等）。§10.2 列的 31 条中有 3 条不作为删除项：`sdk←hex`（`skiff-plugin.rs:412` 实际使用，误删后当场恢复）、`web←tower-http`（CatchPanic 接线后转活）、`web←tracing`（panic 与错误日志在用）——死名单以扫描证据为准，不凭印象 | 终扫零命中（当前 0 条）；`cargo tree -p xtask -i rusqlite` / `-p skiff-docker -i rusqlite` 均报 `did not match any packages`（依赖树中已无 rusqlite） |
| compose 落叶子 crate | 新 crate `skiff-compose`（依赖 `policy`/`serde`/`serde_yaml`）；`skiff-docker` 转出 `pub use skiff_compose as compose`；xtask 直连 | xtask 依赖树无 bollard/rusqlite/tokio；compose 6 项测试随迁 |
| gzip 预压缩落地 | `crates/web/build.rs`（flate2）生成 `.gz` 变体并 embed；`assets.rs` 按 `Accept-Encoding` 协商、带 `Vary`、`.gz` 路径拒直取；`.gitignore` 忽略生成物 | 3 项测试：协商矩阵 / 变体齐全 / gzip 解压与原文一致 |
| `Cidr` 下沉 + 非法拒启 | `Cidr` 入 `skiff-model`（web 私有副本删除）；`Config.trusted_proxies: Vec<Cidr>`，非法项在 `load()` 直接报错退出；bootstrap 不再 warn 后丢弃 | `invalid_trusted_proxy_cidr_fails_startup`；`rejects_bad_cidr` 随迁 model |

**P2**

| 项 | 落地 | 验收证据 |
|---|---|---|
| `db.rs` 按聚合拆分 | `db.rs` 收缩为连接/迁移/备份（~250 行）+ 8 个子模块（settings/users/backup_codes/sessions/tokens/plugins/trust/audit），`impl Db` 门面 API 不变，测试随各聚合 | store 10 项原数通过 |
| `resolve_container` 收口 | 6 处重复闭包 → `handlers::resolve_container` 单点 | `grep 'find(\|c\| c.id == id'` 零命中 |
| 主干集成测试 | `crates/web/tests/main_flow.rs` 6 项：登录→会话→仪表盘、错口令+审计、未登录跳转、viewer 装插件 403、未导入公钥拒装、签名包（导入公钥→上传→确认→落库 grant+审计） | 全过；「建容器」主干属 unix/Docker 真机缺口，已在测试文件头如实标注 |
| 文案与消歧 | `manifest::Permission`→`ManifestPerm`（与 `model::Permission` 消歧）；policy/model 的「零 I/O」改为「零数据库、零 Docker（路径校验触碰文件系统）」；删 `IpParse`；identity/plugin 转出链标注 canonical；`web-assets/` 入 `crates/web/{static,templates}`；`08-engineering` 目录树与依赖方向表对齐现实（含新增 `compose`、`sdk` 行） | 全量测试 + clippy 通过 |

**收口数字（实测）**：**12 crate + xtask**（新增 `skiff-compose`）；**118 项测试全绿**（107 → 118：+CatchPanic、+CIDR 拒启、+3 预压缩、+6 集成）；`cargo fmt --all` + `cargo clippy --workspace --all-targets --all-features -- -D warnings` 干净。

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
| Linux 真机上的 SSE 不断连验证 | 需要真 Docker 与真实连接，归 [DEPLOY_TEST.md](DEPLOY_TEST.md) 上机清单 |

## 13. 第五轮：结构收敛「全部执行」（2026-11-01）

前四轮修的是「缺陷」与「局部边界」，`crates/` 的骨架没动：能力面寄居在 `core`、
角色判定写三份、路径代数写三份、值类型散落在判定 crate 里。本轮把这些一次性收口，
**P0 三项 + P1 四项 + P2 五项全部落地**。逐项明细见
[docs/changes/2026-11-01.md](changes/2026-11-01.md)。

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

## 15. async 路径上的同步 SQLite（2026-11-11 登记；同日第二轮实测与迁移）

**结论**：`skiff_store::Db` 是 `std::sync::Mutex<rusqlite::Connection>` + 同步 `rusqlite`。release 实测单条
语句 **0.01–0.25 ms（p99 ≤ 0.34 ms）**，但有三类操作会把单次持锁拉到 **数十到数百毫秒**：
`purge_audit_before`（大表 DELETE，20 万行 182 ms）、`VACUUM INTO` 备份（193 ms）、以及跨进程
`busy_timeout`（5 s）。运行时只有 **2 个 worker**（`crates/core/src/main.rs:145-147`），两个同步调用
同时被拖住就会让整个 HTTP/UDS 运行时停摆。因此按「spawn_blocking + 有界并发」迁移高频入口，见 §15.3。

### 15.1 清点清单（async 上下文中的同步调用）

| 入口 | 位置 | 频次 | 单次耗时（实测/量级） | 请求热路径 | 持有哪些锁 |
| --- | --- | --- | --- | --- | --- |
| 会话加载中间件 | `crates/web/src/middleware.rs:208-227`（`verify_session` + `user_by_id`） | 每请求 1–2 条 SELECT | 0.03 ms + 0.015 ms | 是（含静态资源） | 仅 `Db` 互斥；`last_seen` 续期按 30 s 节流 |
| Bearer 鉴权提取器 | `crates/web/src/middleware.rs:283-354`（`verify_api_token` [+UPDATE] / `verify_session` / `user_by_id`） | 每个 Bearer 请求 2–3 条 | 0.03–0.06 ms | 是（API） | 仅 `Db` 互斥 |
| 页面上下文 / 菜单 | `crates/web/src/handlers/mod.rs:73,93,107` → `crates/app/src/menu.rs:1140,1157,1468,54`（`get_user_pref` + `get_setting` + 2×`list_plugins`） | 每个 HTML 页面 3–4 条，`list_plugins` ×2 | 0.47 ms/次（10 插件 × 8 KB manifest） | 是（所有页面） | 仅 `Db` 互斥；同步函数，未持有异步锁 |
| 概览页 | `crates/web/src/handlers/dashboard.rs:40-82` | 5 条读 | 合计 ~1 ms | 是 | 仅 `Db` 互斥 |
| 站点 / 运行环境页 | `crates/web/src/handlers/sites/crud.rs:48-54,507,527,629,802`；`crates/app/src/sites/{crud,runtime}.rs`（65 + 49 处） | 1–N 条，动作路径含循环 | 0.1 ms/次；N+1 时线性增长 | 是 | 仅 `Db` 互斥；TLS 写库时另持 `nginx_write_lock` |
| 设置页 | `crates/web/src/handlers/settings.rs:26,37,203-220,301,330,403` | 4–8 条 | 0.01–0.5 ms | 是 | 仅 `Db` 互斥 |
| 插件页 / 安装 / 卸载 | `crates/web/src/handlers/plugins.rs:38-784`；`crates/app/src/menu.rs:930`（按用户逐条改写） | 页面 1–3 条；装卸 5–15 条 + 逐用户 | 单条短，但装卸是批量 | 是（低频重操作） | 仅 `Db` 互斥；包体 I/O 已在 `blocking_fs` |
| 作业轮询 | `crates/web/src/handlers/jobs.rs:187-313`；SSE 循环 `:405-430`（`get` + `logs_since` 每轮） | SSE 每轮 2 条 | 0.35 ms（tail 200） | 是（长连接轮询） | 仅 `Db` 互斥 |
| 作业执行落日志 | `crates/app/src/jobs.rs:395-427`（`append_job_logs`，事务内 2+ 语句 + COUNT/SUM） | 每行 1 次，批量 `log_lines` 1 次 | 0.20 ms/行，尾部 8 ms | 是（安装/升级作业） | 仅 `Db` 互斥 |
| 调度 tick | `crates/core/src/server.rs:124-130` → `crates/app/src/tasks.rs:292-310` → `crates/sched/src/scheduler.rs:573-618` | 1 s × 任务数（每任务 `get_task` + 可能 `put_task`/`append_run`/`append_audit`） | 0.024 ms/条；N 任务线性 | 后台定时 | **`TaskService::scheduler` 的 tokio Mutex 全程持有** |
| tick 落盘（二次写） | `crates/app/src/tasks.rs:302-310,416-433` | 每 tick 每条记录/审计再写一次（`INSERT OR REPLACE` / 普通 INSERT） | 同上 | 后台定时 | 同上（在调度锁内） |
| 会话清理 | `crates/core/src/server.rs:167`（`purge_expired_sessions`） | 每 30 s 1 条 DELETE | 未实测（随过期会话数增长） | 否 | 仅 `Db` 互斥 |
| 插件鉴权（UDS） | `crates/core/src/kernel/auth.rs:43,90-98`（`list_plugins` 全表 + 失败审计） | 每个插件能力面请求 1–2 条 | 0.47 ms（`list_plugins`） | 插件 RPC 热路径 | 仅 `Db` 互斥 |
| 能力面调用 | `crates/core/src/kernel/capability.rs:50-389`（`plugin` / `plugin_config` / 审计 2–3 条） | 每请求 2–5 条 | 0.02–0.5 ms | 插件 RPC 热路径 | 仅 `Db` 互斥 |
| FTP 能力面 | `crates/core/src/kernel/ftp.rs:114-166,195-227`（授权 + 审计 + `list_sites` + 每站点 `runtime_by_id`） | 每请求 3–N 条 | `list_sites` 0.1 ms + N×0.02 ms | 插件 RPC 热路径 | 仅 `Db` 互斥 |
| 插件互调 | `crates/core/src/kernel/interop.rs:35-273`（`plugin` ×2 + `list_plugins` + 审计） | 每请求 3–5 条 | 0.5–1 ms | 插件 RPC 热路径 | 仅 `Db` 互斥 |
| 插件生命周期 | `crates/plugin/src/supervisor.rs:537-565`（状态回写 + 审计） | 启动/停止/回收各 1–2 条 | 短 | 否 | **`running` 的 tokio Mutex 在写库时仍被持有**（`set_plugin_status`） |
| 启动自检 | `crates/core/src/bootstrap.rs:35-158`（建库、迁移、settings、首个管理员） | 一次性 10+ 条 | `open+migrate(18)` 31 ms | 否（未开始服务） | 无 |
| 调度器装载 | `crates/app/src/tasks.rs:175-183` → `crates/sched/src/scheduler.rs:201,217` | 启动 2 次全量 `list_tasks` | 0.024 ms + 解析 | 否 | 无 |
| 集群同步 | — | 本仓库无集群同步入口 | — | — | — |

`crates/app/src/admin_fs.rs`（回收站/配额/审计，23 处）与 `crates/web/src/handlers/files.rs` 的全部调用
都在 `handlers::blocking_fs`（`spawn_blocking`）里执行，**不在 worker 线程上**，不计入本清单。

### 15.2 实测数据

**release 组（结论引用口径）**：Windows 开发机，2 核 4 线程，SSD，WAL，`synchronous=NORMAL`，
`cargo run --release -p skiff-store --example db_latency`（临时测量程序，跑完即删）。数据集：200 会话 /
10 插件 × 8 KB manifest / 30 站点 / 50 作业 × 200 行日志 / 20 万审计行。单次调用：

| 操作 | avg | p99 | max |
| --- | --- | --- | --- |
| `verify_session`（30 s 内不续期） | 0.013 ms | 0.019 ms | 0.55 ms |
| `audit`（单行 INSERT） | 0.049 ms | 0.076 ms | **8.35 ms**（WAL checkpoint） |
| `list_plugins`（10 × 8 KB） | 0.222 ms | 0.336 ms | 1.54 ms |
| `list_sites(30)` | 0.054 ms | 0.075 ms | 0.18 ms |
| `append_job_logs`（1 行） | 0.124 ms | 0.233 ms | **12.5 ms**（WAL checkpoint） |
| `recent_audit(200)` | 0.240 ms | 0.334 ms | 0.33 ms |
| `purge_audit_before`（约 20 万行） | — | — | **182 ms**（首次；清空后 0.05 ms） |
| `backup_to`（`VACUUM INTO`，20 万审计行） | 192.8 ms | 193.4 ms | 193.4 ms |
| `Db::open + migrate(18)`（新库） | 20.4 ms | 21.1 ms | 21.1 ms |
| 争用：备份（92 ms）期间 4 线程查询 | 5075 次查询 | — | **单次最坏等待 92.9 ms** |

**debug 组（对照，不复用于结论）**：同机同数据集、debug 构建（SQLite 为 C 实现，差异主要在 Rust
封装层，量级偏大）。单次调用：

| 操作 | avg | p99 | max |
| --- | --- | --- | --- |
| `verify_session`（30 s 内不续期） | 0.029 ms | 0.096 ms | 0.42 ms |
| `verify_session`（触发续期 UPDATE） | 0.048 ms | 0.231 ms | 0.39 ms |
| `user_by_id` | 0.015 ms | 0.026 ms | 0.42 ms |
| `audit`（单行 INSERT） | 0.055 ms | 0.152 ms | **7.9 ms**（WAL checkpoint） |
| `recent_audit(200)` | 0.455 ms | 0.730 ms | 0.73 ms |
| `list_plugins`（10 × 8 KB） | 0.471 ms | 1.464 ms | 2.39 ms |
| `list_sites(30)` | 0.105 ms | 0.230 ms | 0.88 ms |
| `scheduled_tasks`（tick 全量） | 0.024 ms | 0.050 ms | 0.13 ms |
| `scheduled_task`（主键单条） | 0.024 ms | 0.046 ms | 0.10 ms |
| `append_job_logs`（1 行） | 0.198 ms | 0.702 ms | **8.0 ms**（WAL checkpoint） |
| `job_log_tail(200)` | 0.347 ms | 0.965 ms | 0.97 ms |
| `purge_audit_before`（约 20 万行） | — | — | **260 ms** |
| `backup_to`（`VACUUM INTO`，20 万审计行） | **291 ms** | 296 ms | 296 ms |
| `Db::open + migrate(18)`（新库） | 31 ms | 35 ms | 35 ms |
| 争用：备份（43 ms）期间 4 线程查询 | 332 次查询 | — | **单次最坏等待 35.7 ms** |

**「最坏情况会阻塞运行时线程多久」**：

1. 常规读写：release p99 ≤ 0.34 ms；WAL checkpoint 偶发 8–12 ms（`audit` / 作业日志写入的 max）。
2. 大表维护：审计裁剪（20 万行 182 ms）、`VACUUM INTO`（20 万行 193 ms）与数据量近似线性；审计保留
   90 天、作业日志保留 30 天，线上库达到百万行时这两项进入**秒级**。它们已在 `spawn_blocking` 内，
   但仍占用唯一的 `Db` 互斥：期间所有迁移/未迁移的 DB 调用排在同一把锁上。
3. 跨进程争用：`busy_timeout=5 s`。CLI 的 `backup` / `doctor` / `audit` 会以第二连接打开同一库；
   一旦与在线写入撞上，**请求线程最多被卡 5 s**，且 `std::sync::Mutex::lock()` 不能被打断/超时。
4. worker 只有 2 个：上述任一长操作 + 另一个普通请求同时命中，HTTP 与 `kernel.sock` 全部停摆；
   `/healthz` 也拿不到线程。

### 15.3 方案对比与选择

| 维度 | A. `spawn_blocking` + 有界并发 | B. 专用 DB 线程 + 请求通道 | C. 连接池 + 阻塞线程池 |
| --- | --- | --- | --- |
| 内存 | 每并发一个阻塞线程（虚拟栈 2 MB，实际 RSS 小）；上限 4 可封顶 | 1 个常驻线程 + 有界队列；内存最省且可预测 | 连接数 × 页缓存（默认 2 MB）+ 线程；最高 |
| 可观测性 | permit 等待/执行耗时需自己记（本轮加日志字段）；spawn_blocking 队列深度不可见 | 队列深度天然可观测，背压清晰 | 池指标成熟，但需引入库 |
| 取消语义 | future 丢弃后阻塞任务**继续执行到结束**（审计/事务不被截断，不留半提交）；读操作浪费一次 | 请求已入队即执行；需额外取消令牌才能中断 | 借出连接上的操作无法中断，归还延迟；最易出现「取消后仍在写」的观测盲区 |
| 与现有事务边界 | 完全不改 `Db`：每条语句/事务原样 | 闭包内仍调 `Db` 同步方法，边界不变，但需替换全局句柄 | 事务边界不变，但多连接下 `BEGIN IMMEDIATE` 的 busy 语义变化 |
| 新依赖 | 无（tokio 已在 `app/web/core`） | 无（std mpsc + tokio oneshot） | 需要池实现（新 crate 或自研） |
| 改造面 | 中：逐个 async 调用点包一层，可分批 | 大：所有 async 侧必须改走服务对象，sync 侧（CLI/bootstrap）保留直连，两套入口易漂移 | 大：替换 `Db` 构造与全部调用点 |
| 实际收益 | 直接消除「阻塞 worker」；单写者模型不要求多连接 | 与 A 相当，额外消除锁竞争等待 | 针对「连接不足」，而实测瓶颈是单写者串行，收益低 |

**选 A**。理由：唯一实际风险是「阻塞 worker」，A 正好消除它；语义零变化、零新依赖、可按入口分批
回归；B 的收益与 A 相近但要替换全局 `Db` 句柄，回归面大；C 解决的瓶颈不存在，还要新依赖。

### 15.4 本轮实施范围与边界

已迁移（`skiff_app::db_async`：`spawn_blocking` + 全局 `Semaphore(4)`；permit 随闭包移动、在闭包结束时
释放，取消不会提前归还。排队最多等 2 s，超时返回 `Busy` 并打 warn，不静默堆积；闭包内再投递会返回
`Nested` 被拒绝，避免占着 permit 等 permit 自锁）：

1. `web` 会话中间件：`load_session` 与 `AuthUser` 的 Bearer 路径（每请求必经）；
2. `core/kernel`：插件鉴权 `uid_to_plugin`、`capability::config_get` / `identity_self`、`ftp::context`
   的站点快照（N+1 整体一次投递）；
3. `web/handlers/dashboard.rs` 的 5 条读（合并为一次投递）；
4. `app/tasks.rs::tick_once` 的运行记录/审计落盘：移出 `scheduler` 锁后再投递（锁内只留调度状态机）。

未迁移（记录为后续，不在本轮）：

- `menu` / `page_ctx`：同步函数被所有页面渲染复用，改 async 需动全部 handler 签名；
- `jobs` 日志逐行写入：需把作业执行器整体改为 async 感知；
- `app/sites/**` 的同步 helper：位于动作编排深处，先按 §14 收口动作层再迁；
- `purge_audit_before` / `backup_to` 这类秒级长操作：不属于线程池问题，已按 §15.5 的维护车道单独处理
  （分批删除 + 第二连接快照），不经过 `db_async` 的 permit。

**边界**：`std::sync::MutexGuard` 不是 `Send`，不会出现「持锁跨 await」型死锁；本轮同时修正
`supervisor.rs:537-545` 在 `running` 异步锁内写库的问题（先释放再写）。所有迁入 `spawn_blocking`
的闭包都是自包含语句/事务，取消只会让结果无人接收，不会留下半提交。

### 15.5 备份争用归因与剩余风险

§15.2 里「备份 92 ms 期间 5075 次查询、最坏单次等待 92.9 ms」**不是 `db_async` 的信号量排队**，而是
**同一个 `Db` 互斥被整段持有**。判定依据：

1. 该测量程序用 `std::thread` 直接调 `Db::verify_session` / `Db::backup_to`，全程没有 tokio、没有
   `db_async`、没有 permit，等待只可能发生在 `Db.conn` 的 `std::sync::Mutex` 上；
2. `Db::backup_to`（当日位于 `crates/store/src/db.rs`，现为
   `crates/store/src/db/maintenance.rs`）先取 `self.conn` 守卫，再在守卫内执行 `VACUUM INTO`，
   守卫直到函数结束才释放——备份窗口 = 持锁窗口；
3. 因此最坏等待约等于整个备份时长，是 SQLite 单连接串行化的直接结果，`spawn_blocking` 只能保证
   等待期间**不占 worker**，不能缩短等待本身。

现状核对（**下表为归因当日状态**；维护车道已于 2026-11-12 按下文「实施结果」落地）：

| 操作 | 调用路径 | 是否走 `db_async` / 占 permit | 持锁表现 |
| --- | --- | --- | --- |
| 站点/数据库备份（`backup_to`） | `tasks.rs::db_vacuum` / `site_backup`，内置任务在独立 `spawn_blocking` 里执行 | **否**（不占 `db_async` permit） | 整个 `VACUUM INTO` 期间独占 `Db` 互斥 |
| 审计裁剪（`purge_audit_before`） | `tasks.rs::audit_prune`，同上 | **否** | 整条 DELETE 期间独占 `Db` 互斥 |
| CLI `skiff backup` | 独立进程（`core/src/commands.rs:461`） | 不适用 | 跨进程 SQLite 锁，服务侧最多等 `busy_timeout` 5 s |

结论：**给维护操作单独的低并发额度没有收益**——它限的是 `db_async` 的 permit，而这两类操作根本不
经过该池子；即使给它们单独 1 个额度，普通请求仍会在 `Db` 互斥上等满整个维护窗口。真正能缩短等待的
办法只有一条：**让维护操作不再持共享连接**。

**实施结果（2026-11-12，第 0–2 步）**：维护车道收口到 `crates/store/src/db/maintenance.rs`。

1. **可观测**：`Db::conn_lane(lane)` 在拿到锁后记录等待，≥50 ms 打 warn（`lane=short|maintenance`）；
   维护方法统一打 `lane="maintenance"` + `op` / `duration_ms` / `rows` / `bytes`；`db_async` 模块文档
   写明「覆盖范围只到 `short` 车道」，加大 permit 对维护窗口无效。
2. **审计裁剪分批**：5000 行/批、每批一个自动提交事务、批间让出 5 ms；累计删除数仍是返回值与
   `rows` 日志，调用方（`tasks.rs` 的 `audit_prune`）语义不变。
3. **备份走维护专用第二连接**：普通读写方式打开——**不要改成 `SQLITE_OPEN_READ_ONLY`**，WAL 库的
   只读连接仍要读写同目录 `-shm` / `-wal`，缺写权限会 `SQLITE_CANTOPEN`（约束写在代码注释里）；
   `busy_timeout=1500ms`（主连接 5 s 是面向带外 CLI 进程的），争用即失败上抛，不与它叠加；该连接
   只发读语句与 `VACUUM INTO`，备份后在同一连接上跑一次 `PRAGMA wal_checkpoint(PASSIVE)`。
   dest≠源、dest 完整性校验、失败清理保持原样。
4. **串行性**：维护写操作仍全部走主连接，并由进程内静态 `Mutex` 串行；备份与审计裁剪不会并行，
   第二连接只读快照，不引入第二写者。

实测（release，Windows 动态内存虚拟机 4.92 GB / 2C4T，200,000 审计行，4 个读线程并发
`recent_audit(50)`，直接运行已构建二进制以避开闸门编译争用；**Linux 测试机尚未复测**）：

| 操作 | 改造前（§15.2） | 改造后 |
| --- | --- | --- |
| 备份 | 备份期间 5075 次查询，最坏单次等待 92.9 ms（= 持锁窗口） | 快照 22.2 MB，耗时 381 / 391 / 411 ms；读者 p99 7.7–11.8 ms、最坏 13.3–23.8 ms |
| 审计裁剪 | 182 ms，最坏等待 182 ms | 维护窗口 646–676 ms；读者 p50 0.18–0.35 ms、p99 21.2–22.6 ms、最坏 39.1–39.6 ms |

读等待从「等于维护窗口」降到「单批 / 单次 checkpoint 的 I/O 抖动」量级（约 4–7x）；代价是裁剪总窗口
因批次与让出变长约 3.6x，备份后多一次 PASSIVE checkpoint。以上只代表本机这一组样本，不外推生产。

**剩余风险（更新）**：

- 备份不再占 `Db` 互斥，但 `VACUUM INTO` 仍要读全库；期间跨进程 CLI 写库会与它争 SQLite 锁，
  维护侧 1.5 s 后直接失败上抛（有意不排队）。
- `purge_audit_before` 的写仍在主连接上：单批内仍会短暂独占互斥，批量越小等待越短、总窗口越长。
- 维护操作仍由内置任务低峰调度；百万行级库的窗口继续线性放大，届时需重新评估。

**遗留（P1，本轮不做）**：rusqlite `backup` feature 的在线备份（可增量、可限定步数）与「维护背压标志」。

## 变更记录

| 日期 | 变更 |
|---|---|
| 2026-10-30 | 初版：按模块划分 / 职责边界 / 依赖关系 / 复用性四维审查，附 4 个结构性结论、8 项重复实现、4 个已致缺陷、6 步收敛路径 |
| 2026-10-31 | 追加 §9：P0 四项 + P1 五项的执行结果与验收证据；新增 3 个 crate（`wire`/`model`/`policy`），`plugin::trust` 收口 TOFU；测试 88 → 104 |
| 2026-10-31 | 第二轮复审 §10：上轮 9 项逐条重验全部属实（`Cidr` 下沉除外，属静默缩水）；实测基线 14,755 行 / 107 测试；新发现死依赖 31 条（含 `docker→skiff-store`）、承诺未兑现 2 项（CatchPanic、gzip 预压缩）；头部过期数字清理为初审快照标注 |
| 2026-10-31 | 「全部执行」§10.4：§10.3 行动清单 P0/P1/P2 全闭环——CatchPanic 接线、预压缩落地、`skiff-compose` 拆出、`Cidr` 下沉+非法拒启、28 条死依赖清理、`db.rs` 按聚合拆分、`resolve_container` 收口、6 项主干集成测试、`ManifestPerm` 改名与 `web-assets` 入 crate。测试 107 → 118，fmt/clippy `-D warnings` 干净 |
| 2026-10-31 | 第三轮 §11 代码优化：容器解析精确优先+歧义拒绝、WS Origin 缺失即拒、登录限流表有界、**插件上传 body 上限修复（>2MB 包此前无法安装）**、安全头去每请求解析、`Verb::parse` 由 `all()` 派生、tower-http 死特性收敛。测试 118 → 136 |
| 2026-10-31 | 第四轮 §12：§11.2 四项全部落地——exec 单连票据（30s/一次性/绑定会话与容器）、API Token scope 语义与强制（角色权限 ∩ scope）、按路由分组的请求超时（SSE/WS 排除，504 错误页）、`AppState` 下沉 `skiff-app`（kernel 不再依赖 `skiff-web`）。13 crate + xtask，测试 136 → 149 |
| 2026-11-01 | 第五轮 §13：结构收敛「全部执行」——角色矩阵统一到 `skiff_model::Permission`、路径代数统一到 `skiff_model::path`、能力面迁回 `crates/plugin`（`CapabilityHost` 解环）、值类型下沉 `model`、`AppState` 收口 + 登录用例统一、`core/kernel.rs` 拆为 `kernel/` 子模块并修正中间件顺序。根 212 项通过、plugins 29 项全绿 |
| 2026-11-02 | 第六轮 §14：全量复审（只审查不修码）。实测基线 14 crate + xtask、21,536 行 / 83 文件、215 项测试（212 通过 + 3 项既存 Windows 文件锁红，Linux CI 待复跑）、0 条死依赖。核心发现：**「授权 → 执行 → 留痕」没有唯一入口**，13 个判权点里 11 个不留越权痕迹，`restart` 有两份实现；装/卸包各写两条审计（域函数与调用方都写）；CLI 启停不停进程、不写审计；插件 UI 静态文件无认证且豁免入口前缀。新发现 10 项重复实现（均在动作编排层，判定/格式层仍单点），给出 P0 两项 / P1 三项 / P2 四项行动清单 |
| 2026-11-11 | 追加 §15 待办：async 路径上的同步 SQLite（`Db` 为 `std::sync::Mutex<Connection>`，涉及 web handler、菜单偏好、调度 tick、插件卸载、内核能力面；风险是阻塞 worker 而非死锁）。只登记不改码，`spawn_blocking` 迁移单独立项 |
| 2026-11-11 | §15 第二轮：完成 async 侧同步 SQLite 的清点与实测（release：常规语句 p99 ≤ 0.34 ms、WAL checkpoint 峰值 8–12 ms、审计裁剪 182 ms、`VACUUM INTO` 193 ms，另留 debug 对照；worker 仅 2 个），对比三种方案后选「`spawn_blocking` + 有界并发」，迁移会话中间件、内核能力面鉴权/快照、概览页与 tick 落盘，并修正 supervisor 在异步锁内写库 |
| 2026-11-11 | §15.5 归因备份争用：最坏等待等于备份窗口，属 `Db` 单连接互斥而非 `db_async` 信号量；备份/审计裁剪不经过 permit 池，单独限流无收益。剩余风险（维护窗口内整站 DB 排队、跨进程 busy 5 s）如实保留，维护操作改独立连接/分批删除列为 P1 遗留项 |
| 2026-11-12 | §15.5 第 0–2 步落地：维护车道收口 `store::maintenance`（`lane=short/maintenance` 日志、连接等待 ≥50 ms warn）；审计裁剪改 5000 行/批 + 批间让出；备份改维护专用第二连接（普通读写打开、`busy_timeout=1500ms`、只读快照、结束 PASSIVE checkpoint），保留 dest 完整性校验。release 实测：备份读者最坏等待 92.9 ms → 13.3–23.8 ms，裁剪 182 ms → 最坏 39.1–39.6 ms（窗口 646–676 ms）。rusqlite 在线备份与维护背压标志仍为遗留 P1 |
