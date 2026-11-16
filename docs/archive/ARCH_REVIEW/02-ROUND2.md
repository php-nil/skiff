# 第二轮复审：模块划分、职责边界与依赖（§10）

> 上级索引：[ARCH_REVIEW.md](../ARCH_REVIEW.md) ｜ 拆自 `docs/archive/ARCH_REVIEW.md` 原 §10（2026-11-16 整理，正文未改）。

---

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
