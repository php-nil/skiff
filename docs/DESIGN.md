# 设计方案：Skiff（轻舟）· Rust 极简 Linux 服务器管理面板

> 版本：**v1.0（MVP 实施稿）** ｜ 产品名：**Skiff（轻舟）** ｜ 交付形态：**宿主机 systemd 直装（主）**；1Panel 商店 Docker 包为可选受限形态
> 本方案 = **MVP 方案**，只覆盖三核心：**Web 管理服务 / Docker 管理 / 插件体系**。文件管理、防火墙、网站管理、主机监控等宿主能力**内核不内置，由插件层在宿主能力面上实现**（§1.4、§3.3）。
> 状态：**MVP 代码已落地**（M0–M4 主体代码面完成，M4 的 landlock/cgroups/nftables 与 net 收口尚未实现：`cargo clippy --workspace --all-targets -- -D warnings` 干净、`cargo test --workspace` **215 项（Windows 本机 212 通过 + 3 项既存文件锁红；Linux CI 待复跑）**、插件 workspace 29 项全绿、Windows 开发机 HTTP 冒烟通过）。结构为 **14 个 crate + `xtask`**（`skiff-app` 承载 `AppState`；能力面实现在 `crates/plugin`，判定规则唯一来源在 `skiff-model`）；五轮架构审查的 P0/P1/P2 与后续优化（[ARCH_REVIEW.md](ARCH_REVIEW.md) §9、§10.4、§11、§12、§13）已闭环，**第六轮全量复审（§14）为只审不修**：分层已稳，剩余问题集中在「授权→执行→留痕」缺少唯一入口（判权拒绝 11/13 不留痕、装/卸包各写两条审计），行动清单见该节 14.7。上机前复查修掉 9 个只在 Linux 暴露的问题（[DEPLOY_TEST.md](DEPLOY_TEST.md) §9.4）；GitHub CI 与发布流水线已配置（[CI.md](CI.md)），CI 已首绿并完成 Release 流水线验证。**Linux 专属能力中，setuid 降权 / Unix socket 已在 Linux CI 跑过单测；docker.sock 与 systemd 安装尚未真机验证；landlock / cgroups / nftables 尚未实现**；全部资源数字尚未实测，逐项见 [DEPLOY_TEST.md](DEPLOY_TEST.md) §8。目标读者：决策者（哥）。
> **文档结构**：本文件是索引与一页结论；正文按章节拆在 `docs/sections/` 下（13 个文件覆盖 §1–§12，§4 分两册）。**文件名前缀 = 章号**，章节号 `§x.y` 全局唯一、拆分不重编号，跨文件引用直接按编号跳转（见下方文档地图）。

---

## 0. 一页结论

| 维度 | 决策 | 理由 |
|---|---|---|
| **MVP 范围** | **三核心：Web 管理服务 + Docker 管理 + 插件体系** | 能登录、能管容器、能装插件且崩不了内核即 MVP；主机能力在插件层（§1.4） |
| 语言/运行时 | Rust 2021 + tokio（**显式** `worker_threads=2`） | 无 GC，单二进制；tokio 默认值是 CPU 数，不配就不是 2 |
| HTTP | `axum` + `tower` | 生态稳、体积可控 |
| 前端 | `askama` SSR + `htmx` + 懒加载 ESM（xterm.js） | 免 Node 构建链，首屏 < 100KB gzip |
| 存储 | `rusqlite`(bundled) + TOML | 单文件、可外部审计 |
| 扩展 | **进程外插件（宿主机原生）+ Unix Socket JSON-RPC**，ed25519 签名、权限逐条确认、UI 沙箱 iframe、降权；landlock/cgroups 属设计目标（未实现） | 崩溃隔离，越权收敛到 Broker；插件体系 = 开放能力面（§4） |
| 部署 | **主形态：systemd 直装宿主机**；可选：1Panel 商店 Docker 包（受限，能力面收缩为 docker/identity） | 插件必须落宿主才撑得起第二核心；容器内 fork 不出宿主进程 |
| 基础镜像 | **alpine:3.23**（仅可选 Docker 形态；3.20 已 EOL，禁用） | 排障（docker exec 进面板）需要 shell；跟随受支持分支（3.23 支持到 2027-11） |
| 资源目标 | 空闲 RSS ≤ 25MB，CPU < 0.3%，二进制 ≤ 14MB | **设计目标，待 M5 实测回填**（口径见 §7） |

**主结论**：MVP = 三核心跑通——**能登录、能管容器、能装插件且崩不了内核**。产品定位 = **极简服务器面板**：内核只做 Web 管理服务 / Docker 管理 / 插件体系，文件管理、防火墙、网站管理等宿主能力以插件形式开放（能力面 docker/identity/fs/net）。插件落宿主机（全家 native）——插件体系因此是真核心而非 docker 附庸；Docker 包降级为可选受限形态（§5.4）。

---

## 文档地图

文件编号 = 章节号（`04-` 前缀为 §4 的两册，按文件名排序即 `04-plugin-package` → `04-plugin-publish`）。

| 章节 | 文件 | 内容要点 |
|---|---|---|
| §1 需求评估 | [sections/01-requirements.md](sections/01-requirements.md) | 显式需求 → 验收项、隐含需求、三处需求冲突、分层边界与明确不做 |
| §2 技术选型 | [sections/02-tech-stack.md](sections/02-tech-stack.md) | 后端依赖表（含 R1 例外标注）、前端、扩展机制三方案对比 |
| §3 总体架构 | [sections/03-architecture.md](sections/03-architecture.md) | 分层图、重启容器请求路径、内核四项原子能力 |
| §4.1–4.3 扩展体系（一） | [sections/04-plugin-package.md](sections/04-plugin-package.md) | `.wbpkg` 包格式与签名编解码、manifest 字段、生命周期与资源兜底、信任模型 |
| §4.4–4.5 扩展体系（二） | [sections/04-plugin-publish.md](sections/04-plugin-publish.md) | 第三方发布交付物与 CLI、用户侧安装 / 升级 / 回滚 / 卸载 |
| §5 部署 | [sections/05-deploy.md](sections/05-deploy.md) | systemd 直装、数据与升级、与 1Panel/宝塔共存、1Panel Docker 包、路径与卷校验 |
| §6 安全 | [sections/06-security.md](sections/06-security.md) | 认证、传输与反代、暴露面与 RBAC、Docker Socket 三层控制、Compose 子集、插件安全 |
| §7 资源预算 | [sections/07-budget.md](sections/07-budget.md) | 资源目标表、手法、登录峰值口径（与空闲 RSS 目标的关系） |
| §8 工程结构 | [sections/08-engineering.md](sections/08-engineering.md) | crates 布局、核心 ↔ 代码对照、`xtask` 两类固化校验（另有独立 `.wbpkg` 校验子命令） |
| §9 里程碑 | [sections/09-milestones.md](sections/09-milestones.md) | M0–M5 交付物与通过标准、MVP 边界、9.1 架构核实门（原型挂里程碑） |
| §10 风险 | [sections/10-risks.md](sections/10-risks.md) | 21 条风险（带 架构/契约/实现/运营 层级）、10.1 架构支柱级 6 条原型、10.2 契约级 5 条 |
| §11 已拍板决策 | [sections/11-decisions.md](sections/11-decisions.md) | 10 条默认值（可推翻），不留阻塞实施的开放问题 |
| §12 待实测清单 | [sections/12-pending-tests.md](sections/12-pending-tests.md) | 17 条实测项 + 12.1 架构层缺口 5 条，逐项挂里程碑 |
| 部署与测试（非章节） | [DEPLOY_TEST.md](DEPLOY_TEST.md) | 两种部署形态的装机步骤、配置矩阵、7 组验收用例（A–G）、未实现清单、实测记录与排查表 |
| 持续集成（非章节） | [CI.md](CI.md) | 三个工作流的门槛口径、工具链锁定与 MSRV 依据、发布产物流向、实测结果与已知风险 |
| 架构审查（非章节） | [ARCH_REVIEW.md](ARCH_REVIEW.md) | 模块划分 / 职责边界 / 依赖关系 / 复用性四维审查；重复实现清单、已致缺陷、目标结构与 P0–P2 行动清单 |
| 官方插件集（非章节） | [PLUGIN_CATALOG.md](PLUGIN_CATALOG.md) | 竞品对照、官方插件清单与首批 5 个（guard/monitor/nginx/files/cron，已落地）、`plugins/` 第二 workspace、发布包携带 `official/` 与开箱即用交付线 |

### 按角色选读

| 你的目的 | 读这些 |
|---|---|
| 只做决策（约 15 分钟） | 本文 §0 → [§10 风险](sections/10-risks.md) → [§11 决策](sections/11-decisions.md) |
| 评审方案合理性 | [§1 需求](sections/01-requirements.md) → [§3 架构](sections/03-architecture.md) → [§4（一）](sections/04-plugin-package.md) → [§4（二）](sections/04-plugin-publish.md) → [§5 部署](sections/05-deploy.md) → [§6 安全](sections/06-security.md) |
| 动手写代码 | [§8 工程结构](sections/08-engineering.md) → [§9.1 架构核实门](sections/09-milestones.md) → [§3.3 原子能力](sections/03-architecture.md) → [§6.4 三层控制](sections/06-security.md) → [§4.2 生命周期](sections/04-plugin-package.md) → [§9 里程碑](sections/09-milestones.md) |
| 只关心上架 / 发布 | [§5.4 Docker 包](sections/05-deploy.md) → [§5.5 路径校验](sections/05-deploy.md) → [§4.4 发布](sections/04-plugin-publish.md) → [§9 M5](sections/09-milestones.md) |

---

## 文档约定

- **编号规则**：`§x.y` 为全局唯一编号，拆分到多文件后**不重编号**；文件名前缀号 = 章号，引用一律按编号，文件位置见上表。本文索引部分不占正文编号。
- **表格写法**：单元格内不出现裸 `|`（会被渲染器拆列）；取值枚举写成「A 或 B」或顿号分隔，必需的 `|` 用法放到表格外的段落里。
- **表格位置**：表格与代码块一律放在顶层，不缩进进列表项（部分渲染器不解析列表内的表格）。
- **代码 span 不跨行**：一行内闭合，跨行写法在多数渲染器里会退化成普通文本。
- **数字纪律**：未实测的数字与能力声明一律标注「设计目标 / 待实测」，M5 统一回填（§7、§12）。
- **单一事实来源**：同一约束只在一处下结论，其余处引用编号；发现冲突以编号靠前、且标注「拍板」的条目为准。

---

## 变更记录

| 日期 | 变更 |
|---|---|
| 2026-10-30 | 按章节拆分为 `docs/sections/` 下 13 个文件（覆盖 §1–§12，§4 分两册），主文档只保留一页结论与索引。修正渲染错乱：§4.1 manifest 表格内用「反斜杠转义竖线」写枚举值导致拆列（改为分组表 + 文字枚举）、§5.2 表格与代码块缩进在列表项内、§6 三层控制的列表缩进与跨行代码 span、全文中文引号统一为「“ ”」。 |
| 2026-10-30 | 修正命名错乱：章节文件编号整体对齐章号（原 `05-plugin-publish.md` 装的是 §4，现为 `04-plugin-publish.md`，其后各章同步前移一位）；主文档索引章节不再使用数字编号，避免与正文 §1/§2/§3 撞车。正文事实未改动。 |
| 2026-10-30 | §10 增加层级标注（架构 10 / 契约 5 / 实现 3 / 运营 3）与 **10.1 架构支柱级待核实**（6 条：landlock 地基、独立 UID 与授权目录冲突、路径校验 TOCTOU、grant 绑定语义、迁移不可回滚边界、nftables 收口）+ **10.2 契约级待核实**（5 条）；§12 增加 **12.1 架构层缺口**（最低内核矩阵、运行时授权语义、RLIMIT_AS 双轨、cgroup 进程树清理、插件 UI 的 opaque origin 行为）。 |
| 2026-10-30 | **MVP 代码落地**：`crates/{core,store,identity,docker,plugin,pkgfmt,sdk,web}` + `web-assets/` + `xtask`；`cargo check --workspace` 零警告、`cargo test --workspace` 88 项全绿、Windows 开发机 HTTP 冒烟通过。新增 [DEPLOY_TEST.md](DEPLOY_TEST.md)（部署步骤 + 7 组验收用例 + 未实现清单 + 实测记录）。同步修正：`Db` 自锁死（`plugin()`/`verify_api_token()`）、受保护对象未拦 stop/restart、`/{prefix}/` 尾斜杠 404、非 unix 上 Docker 假连接。**状态行改为「代码已落地」，但 Linux 专属能力与全部资源数字仍未实测**（§12 不变）。 |
| 2026-10-30 | §9 增加 **9.1 架构核实门**：把 10.1 的 6 条原型按里程碑挂成硬门槛（M1 三条、M2 两条、M4 一条），列「通过门槛 / 不通过的处理」；M1 通过标准显式加入「A 组 3 条原型通过」。修正 10.1 导语「6 条全在 M1」与实际里程碑分布不符的表述。 |
| 2026-10-30 | **补齐发布链路验证**：`cargo build --release -p skiff` 通过（fat LTO，1m40s），Windows PE 产物 6.47 MB；`deploy/pack-release.sh` 首次实跑通（tar.gz + `.sha256` + `.sig` 三件套、重复打包摘要一致、正/错公钥与篡改包验签结论正确）。再修 2 处缺陷：`skiff-plugin keygen --help` 会真的写出私钥、`pack-release.sh` 包名写成不存在的 `skiff-core`。同时更正 DEPLOY_TEST §1 的内核说法：路径校验用 `canonicalize` + `openat(O_NOFOLLOW)`，**不依赖 `openat2`**。仍然未实测：Linux 发行包体积、RSS/CPU、全部 Linux 专属能力（§12 不变）。 |
| 2026-10-30 | **架构审查**：新增 [ARCH_REVIEW.md](ARCH_REVIEW.md)。结论：分层无环、格式层（`pkgfmt`）是复用样板，但存在四处边界错位——① 跨进程协议在 `plugin/ipc.rs` 与 `sdk` 各写一遍（已漂移：SDK 调 `/api/v1/capability/config`，内核无此路由）；② `AppState`/`WebConfig` 住在 `web`，`core::kernel` 反向依赖表现层；③ TOFU 判定与 `capabilities(form)` 各两份；④ `Verb`→Docker 分发三份、`parse_verb` 手写逆函数。另指出 `docker` crate 里只有 `client.rs` 碰 bollard，其余可独立成 `skiff-policy`。给出 8 项重复实现、4 个已致缺陷与 P0–P2 收敛路径。**本文只审查不修码**。 |
| 2026-10-31 | **上机就绪复查（准备真机部署）**：为 `x86_64-unknown-linux-gnu` 装上 std 后对纯 Rust crate 做类型检查，抓出 3 个只在 unix 存在的编译错误（`Supervisor::socket_path` 缺失、`skiff-policy` 未声明 `libc`、SDK 缺 `Read/Write` 导入）；修掉 6 个运行级缺陷：数据根与 `plugins/` 由 0700 改 **0711**（否则降权插件走不到自己的 `data/`）、`kernel.sock` 启动时改属组 `skiff-plugins` 且插件降权时带上附加组（`setgroups` 先于 `setgid/setuid`）、`clean_run_dirs` 重建后归还属主、`fs.write` 落盘后归还属主、插件 UID 撞车装前拦截、`pack_dir` 只收 `bin/ ui/ migrations/`（此前会把**签名私钥**与 `target/` 一起打进包：28.9MB → 461KB）。发布包补 `skiff-plugin`；`skiff-plugin new` 支持 `--sdk-path`（SDK 未发布到 crates.io）。如实修正 TOCTOU 表述：`open_dir_nofollow` 已实现但**未接入**判定链。`DEPLOY_TEST.md` 新增 §0 上机闸门、§1.1 权限口径、§2.9 插件测试物、§2.10 权限链自检。测试 104 → 105。 |
| 2026-10-31 | **接入 GitHub CI 与发布流水线**：`ci.yml`（fmt / clippy `-D warnings` / test / msrv / xtask verify / 插件包真签真验 / shellcheck + LF / 服务冒烟 / musl 静态产物）、`release.yml`（amd64 与 arm64 musl 静态包 + 摘要 + ed25519 签名 → GitHub Release）、`audit.yml`（周更 RustSec）、`dependabot.yml`；`.gitattributes` 把行尾锁成 LF。为了让门槛真能过，先修掉全仓 clippy 告警并 `cargo fmt --all`（`is_admin` 取 `&self`、`matches!`、两处 `type_complexity` 起别名）。**更正 MSRV：1.85 → 1.89**（实测 `rusqlite 0.41.1` 要求 1.89、`libsqlite3-sys 0.39`/`sha2 0.11`/`icu_*` 要 1.88，1.85 连依赖解析都过不去；1.89 下 `--all-targets` 通过）。顺手修掉 `deploy/Dockerfile` 两处**必崩**：`-p skiff-core` 包名不存在（实际包名 `skiff`）、漏 `COPY xtask` 导致 workspace 成员缺失；以及 `skiff-plugin new` 用整条路径 sanitize 生成 crate 名（`new /tmp/x/demo` → `-tmp-x-demo`，`cargo build` 报 invalid package name，改成取末段 + 首字符兜底，带回归测试）。新增 [CI.md](CI.md)。工作流**尚未在 GitHub 上跑过**，首次运行为首次实跑（已知风险见 CI.md §5）。 |
| 2026-10-31 | **收敛 P0 + P1（ARCH_REVIEW §7）**：补 `/api/v1/capability/config` 路由（此前 SDK 读配置必 404）并给配置项一个真实写入口 `skiff plugin config`；审计动作名收口到 `Verb::audit_action()`；`Location` 改 percent-encode；错误页改用 `error.html` 模板。结构上新增三个 crate：`skiff-wire`（UDS 协议帧 + 路由常量，两端共用）、`skiff-model`（`Role`/`Grant`/`Form`/`Permission`，零依赖）、`skiff-policy`（三层策略 + 路径校验，零 Docker 零 SQLite）、`plugin::trust`（TOFU 三段式从 CLI 与 Web 各一份收成一份）；`Docker::execute` 单点分发替代三处 `match`，`Verb::parse` 取代 `kernel.rs` 手写逆函数，`Capability::available_in(form)` 取代两份形态→能力映射。`cargo test --workspace` 由 88 项增至 104 项全绿，零警告；`cargo test -p skiff-policy` 20 项不依赖 Docker/SQLite。仍未实测：Linux 专属能力与资源数字（§12 不变）；SDK↔内核契约集成测试为 unix-only，本机（Windows）仅编译未运行。 |
| 2026-10-31 | **第二轮架构复审 + 全量执行（ARCH_REVIEW §10）**：复审确认上轮 9 项属实（`Cidr` 下沉属静默缩水），新发现 28 条死依赖与 2 项「文档承诺未兑现」——`CatchPanic` 安全网与 gzip 预压缩都只在文档里存在。按 §10.3 清单全闭环：`web::router()` 挂 `CatchPanicLayer`（panic → 500 + `web.panic` 审计）、`crates/web/build.rs` 生成 `.gz` 并按 `Accept-Encoding` 协商、新增叶子 crate `skiff-compose`（xtask 不再编 bollard/rusqlite）、`Cidr` 下沉 `model` 且非法 CIDR 拒启、28 条死依赖删除、`db.rs` 按聚合拆 8 子模块、`resolve_container` 收口、`web/tests/main_flow.rs` 6 项主干集成测试、`manifest::Permission` 改名 `ManifestPerm`、`web-assets/` 移入 `crates/web/{static,templates}`。测试 107 → 118 全绿，fmt/clippy `-D warnings` 干净。**遗留仅 `core → web::AppState` 反向依赖**。 |
| 2026-10-31 | **文档与注释一致性校正**：清掉随改动腐化的数字（测试 107 → 118、crate 11 → 12、源码 15,475 行 / 65 文件）与不存在的文件引用（`docs/` 曾列 `PLUGIN_API.md` / `SECURITY.md` / `REVERSE_PROXY.md`）；修正两处「注释照抄设计图」——`crates/web/src/lib.rs` 与 §3.1 原写「可信代理 → 限流 → 安全入口 → 会话 → CSRF → RBAC → 审计」七层链，实际是四层 axum 中间件（`client_info`/`load_session`/`entry_guard`/`CatchPanic`），CSRF、RBAC、审计在 handler 内，限流只在登录路径；`templates.rs` 的 `web-assets/` 旧路径改为 `crates/web/templates/`。变更记录按日期正序重排。 |
| 2026-10-31 | **工作流按开发阶段收编（不发新版）**：`release.yml` 摘掉 `v*` 标签触发、新增 `publish` 开关（默认 false：只构建 + 自检 + 传 workflow 产物，不建 GitHub Release）；`ci.yml` 的 musl `build` 改为仅 `workflow_dispatch`，纯文档改动（`docs/**`、`**.md` 等）不触发 CI；删掉 `apt-get install build-essential/pkg-config/shellcheck`（`ubuntu-24.04` 镜像已预装，shellcheck 加 `command -v` 存在性断言，宁可红也不静默跳过）；`pkg-chain` 由 release 改 debug 构建（验签名语义不需 opt）；同工具链 job 共用 rust-cache `shared-key: ci`；全 job 补 `timeout-minutes`。仍无 `continue-on-error`。同步更新 [CI.md](CI.md) 与 [DEPLOY_TEST.md](DEPLOY_TEST.md) 的发布表述。 |
| 2026-11-01 | §11 新增决策 11（crate 拆分粒度）：复核 13 crate 拆分后**拍板维持现状、暂不动**，重审触发条件见该行；详 `docs/changes/2026-11-01.md`。 |
| 2026-11-01 | 新增非章节文档 [PLUGIN_CATALOG.md](PLUGIN_CATALOG.md)（官方插件集分析建议稿）：竞品对照、官方插件清单、首批 3 个（monitor/nginx/files）、`plugins/` 第二 workspace 目录方案与开箱即用交付线；**未拍板、未建目录、不改代码**。 |
| 2026-11-01 | **发布物去签名（§11 决策 12）**：发布物只验 detached `SHA-256` 摘要，不做签名。移除 `RELEASE_SIGNING_KEY_HEX` secret 流程、`.sig` 产物、`skiff-plugin sign-file/verify-file` 子命令与 `pkgfmt::sign_bytes`（`.wbpkg` 签名/TOFU 不动，决策 2 不变）；`pack-release.sh` 去掉 `--key`，CI 的 `pkg-chain` 首步改为 keygen 落盘/公钥输出冒烟（含「`--help` 不落盘」回归）。安装口径由「先验摘要与签名」改为「先验摘要」。理由与对照见 §11 决策 12；详 `docs/changes/2026-11-01.md`。 |
| 2026-11-01 | **官方插件集落地（首批 4 个）+ 发布包携带**：新增顶层 `plugins/`（**第二 cargo workspace**，根 `Cargo.toml` `exclude`）；`com.skiff.guard`（容器守护，**零内核改动**，纯既有 `docker`/`fs`/`identity` 能力组合）、`com.skiff.monitor`、`com.skiff.nginx`（不用 `containers.exec`，写授权目录 + 重启容器）、`com.skiff.files`。内核侧只补 `fs` 能力面（`crates/core/src/{capability,fsops}.rs`：`path:` 授权根 + 256KiB 分页读写）与 cron tick 调度（`crates/plugin/src/cron.rs`）；插件侧新增插件详情页与授权根展示。发布物：`deploy/pack-release.sh --plugin-key` 调 `plugins/pack.sh` 把 4 个已签名 `.wbpkg` + `public-key.txt` 塞进 `official/`（缺私钥且未 `--skip-plugins` 拒绝出包），`install.sh` 复制到 `/var/lib/skiff/official`（不自动安装）。CI 新增 `plugins` job 与 `pkg-chain` 官方插件真打包冒烟；`release.yml` 增 `PLUGIN_SIGNING_KEY` 注入与「产物恰好 4 个包 + 逐个验签」自检。详见 `docs/changes/2026-11-01.md` 与 [`plugins/README.md`](../plugins/README.md)。 |
| 2026-11-01 | **架构审查第五轮结构收敛「全部执行」**（ARCH_REVIEW §13）：角色→权限矩阵统一到 `skiff_model::Permission`（唯一一份，删除 `policy` 内联判定与 `web` 手写 `require_admin`）；路径授权代数统一到 `skiff_model::path`（11 个零 I/O 原语，policy / fsgrant / 插件侧快筛共用）；插件能力面（`fsops`/`fsgrant`/`capability`/`interop`）从 `core` 迁回 `crates/plugin`，宿主上下文经 `CapabilityHost` trait 注入（依赖倒置，解开 `plugin → app` 环）；值类型（`ContainerSpec`/`Verb`/`Actor` 等）下沉零 I/O 的 `skiff_model`，`policy` 只留判定；`AppState` 收口 + 登录用例统一到 `skiff_app::AppState::login`（API 侧限流现在也写审计、错误文案与 UI 对齐）；`core/kernel.rs` 拆为 `core/src/kernel/` 子模块并修正 axum `Router::layer`「后加的先跑」导致的协议版本闸顺序倒置。§8 工程结构（目录树 + 依赖表）与 AGENTS.md 单一来源红线同步。根 212 项测试通过、plugins 29 项全绿。 |
| 2026-11-02 | **架构审查第六轮（全量复审，只审不修）**（ARCH_REVIEW §14）：实测基线 14 crate + `xtask`、21,536 行 / 83 个 `.rs`、**215 项测试（Windows 本机 212 通过 + 3 项既存文件锁红，Linux CI 待复跑）**、内部依赖死声明 0 条。四维评级 模块划分 A− / 职责边界 B / 依赖关系 A− / 复用性 B−。核心结论：分层与判定层已稳，**剩余缺陷同源于「授权 → 执行 → 留痕」没有唯一入口**——13 个 `policy.check` 点仅 2 个留越权痕迹，`restart` 在 `containers::action` 与 `api::restart_container` 各一份；装/卸包在域函数与调用方各写一条审计（每次操作 2 条）；CLI `plugin enable/disable` 不停进程、不写审计；插件 UI 静态文件无认证且被 `is_exempt` 豁免入口前缀。另列 10 项重复实现与 P0×2 / P1×3 / P2×4 行动清单。**本轮未改代码**。 |
| 2026-11-02 | **文档与注释一致性校正（不改行为）**：修正 `xtask check` 口径（实际只跑 1Panel + Compose，`verify-pkg` 为独立子命令）、架构审查轮次、`common` 非插件定位、Web 中间件层数、landlock/cgroups/nftables 的「未实现」标注、官方插件 4 → 5、全量测试 215 项（212 通过 + 3 项 Windows 限制）与逐包用例表，并删除不存在的 `PLUGIN_API.md` 引用。详 `docs/changes/2026-11-02.md`。 |
