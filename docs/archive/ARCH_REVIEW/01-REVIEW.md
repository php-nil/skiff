# 现状、依赖、复用与行动清单（§0–9）

> 上级索引：[ARCH_REVIEW.md](../ARCH_REVIEW.md) ｜ 拆自 `docs/archive/ARCH_REVIEW.md` 原 §0–9（2026-11-16 整理，正文未改）。

---

> 主文档：[DESIGN.md](../../DESIGN.md) ｜ 本文件：代码架构审查 ｜ 部署与验收：[DEPLOY_TEST.md](../../test/DEPLOY_TEST.md)

**初审基线（2026-10-30）**：`crates/` 下 8 个 crate + `xtask`，12,013 行、90 个测试标记（执行 88，全过）。
**§0–§9 是初审与首轮执行的存档**，其中的行号、行数、测试数均已随改动腐化，只作对照基线；**该轮实测基线见 §14.0**（记 14 crate；其后 `sched` / `fs` 独立为 16 crate + `xtask`，再往后结构变化见 [sections/11-decisions.md](../../sections/11-decisions.md) 决策 11）。

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
