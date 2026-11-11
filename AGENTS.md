# 仓库总览与开发约定

> 本文档为仓库全局通用规范。子项目可维护独立规范，但不得与本规范冲突；同一信息仅在一处维护，其他位置保留相对路径引用。
> **执行约定：任何任务动手前必须先阅读本文件**，再按「四、文档索引」选读对应专项文档，然后执行。

## 一、项目概述

* **项目名称**：Skiff（轻舟）—— Rust 极简 Linux 服务器管理面板
* **核心定位**：只做三核心——Web 管理服务 / Docker 管理 / 插件体系。内核持有 fs 能力面的**管理员客户端**（「工具 → 文件管理」页面，IO 全部经 `skiff-fs`），但不把宿主文件业务塞回内核业务层；防火墙、主机监控等其余宿主功能仍由插件层在宿主能力面（docker/identity/fs/net）实现。主交付形态为宿主机 systemd 直装；1Panel 商店 Docker 包是可选受限形态（能力面收缩为 docker/identity）。

## 二、仓库结构

| 路径 | 类型 | 说明 | 文档入口 |
| --- | --- | --- | --- |
| `crates/` | 子项目 | Rust workspace 的 16 个 crate（`xtask` 在顶层单独列出），成员清单以根 `Cargo.toml` 为准；crate 分层与职责见文档入口 | `docs/sections/08-engineering.md` |
| `plugins/` | 子项目 | **第二个 workspace**（`exclude = ["plugins"]`，不受根 `--workspace` 覆盖）：共享库 `common`、官方插件 `guard` / `monitor` / `files` / `cron` / `ftp` / `firewall` 与 `pack.sh` 打包脚本；发布包 `official/` 的产出源头 | `plugins/README.md` |
| `xtask/` | 工程校验 | `cargo run -p xtask -- check` 固化两类校验（1Panel 变量、Compose 字段子集）；`verify-pkg` 为独立子命令，校验 `.wbpkg` 结构/摘要/签名 | `docs/sections/08-engineering.md` |
| `deploy/` | 部署物料 | 安装/卸载/打包脚本、systemd unit、Dockerfile、1Panel 商店包、`config.toml.example`；`skiff-server/` 为商店清单源数据与 JSON Schema | `docs/DEPLOY_TEST.md`、`docs/SKIFF_SERVER.md` |
| `docs/` | 全局文档 | 全仓库专项文档 | 见「四、文档索引」 |
| `tools/` | 开发机工具 | `linux-check.sh`（本地 Linux 目标编译级复核）、`iter.sh`（单轮迭代编排：门禁/取包/装/验收/回滚协同）、`perf/`（性能观测与报告脚本）、`skiff-server/`（商店清单校验与静态产物生成，根 workspace 成员） | `docs/ITERATION.md`、`docs/SKIFF_SERVER.md` |
| `.github/workflows/` | CI/CD | `ci.yml` / `release.yml` / `audit.yml` / `iter.yml` 四个工作流 | `docs/CI.md` |

> 顶层目录变动时，须同步更新上表；子项目新增独立规范（`crates/<name>/AGENTS.md`）须在上表登记入口。`target/`、`skiff-run/`、`.workbuddy/` 等是本地产物/工作数据，已被 gitignore，不得入库。

## 三、全局约定

### 3.1 开发与交付

* **同步更新**：代码、逻辑变动必须与测试用例、相关文档同步更新。
* **迁移兼容**：当前为测试开发阶段，没有正式环境，不需要兼容旧版本支持迁移。旧版失效文档也无需保留，一切放在当前已有功能上。
* **交付门槛**：命令与判定口径只在 [docs/CI.md](docs/CI.md) §2、§6 维护；本文件不重复命令清单。
  * 动 `plugins/` 时，根三关**不覆盖它**（第二 workspace），另跑插件 workspace 三关，命令见 `docs/CI.md` §6。
* **测试机部署留痕**：任何一次向测试机部署后，必须更新 [docs/TEST_ENV.md](docs/TEST_ENV.md) 的部署记录与验收结论，并在汇报中交代「地址 + 账号」；缺这步视为部署未完成。
* **红线**（违反即返工）：
  * 分层禁令：`policy` / `pkgfmt` / `wire` 不得依赖 `store` / `docker`；`app` 不含路由与模板；`impl` 必须与类型同 crate。
  * 单一来源：插件↔内核 UDS 协议只在 `skiff-wire`；TOFU 判定只在 `plugin::trust::resolve`；动词→Docker 分发只在 `Docker::execute`；角色→权限矩阵只在 `skiff_model::Permission`；路径授权代数只在 `skiff_model::path`（唯一有意例外：`skiff-fs` 的 I/O 感知解析器，见 `docs/KERNEL_FS_IMPL_NOTES.md` §2.2.1）；插件能力面实现只在 `crates/plugin`。
  * 数字纪律：未实测的数字与能力声明只标「设计目标 / 待实测」，不得当已验证事实引用。
  * 权限口径：数据根与 `plugins/` 保持 0711（可穿越不可列），勿改回 0700，否则降权插件全线 EACCES（详见 `docs/DEPLOY_TEST.md` §1.1）。
* **编译与测试串行（红线）**：违反即返工。
  * 同一时间只允许一路 `cargo` 编译或测试（`fmt` / `clippy` / `test` / `build` 及任意 `cargo` 子命令），不分会话、不分开发者。**必须经包装/闸门执行**：`PATH` 靠前的 `tools/bin/cargo.cmd` → `tools/cargo-gate.ps1` → `tools/gate.ps1`，由全局命名互斥体 `Global\SkiffBuild` 发放唯一令牌；拿不到令牌立即以退出码 2 拒绝，**不排队等待**。根 workspace 与 `plugins/` 共用同一把令牌，插件侧命令按 `--manifest-path` 记为 `workspace=plugins` 租约。
  * 第二层是单次调用内部：`.cargo/config.toml` 固化 `[build] jobs = 1`，同一时刻只跑一路 `rustc`；`plugins/` 无本地 `.cargo/config.toml`，继承根配置，闸门的插件任务另显式带 `--jobs 1`。`cargo fmt` 不取 cargo 的 target 锁，只能靠包装入口拦住，因此 `fmt` 同样必须走该入口。
  * 禁止为了并行而使用自定义 `CARGO_TARGET_DIR`——它会绕过 cargo 自带的 target 目录锁，是已发生过的真实事故成因。
  * 闸门 `pwsh tools/gate.ps1` 的退出码：`0` 成功、`2` 锁被占用、`3` 内存准入拒绝、`4` 需要 `-AllowFull`。根 workspace 的 `clippy` / `test` / `build` 带 `--workspace` 或 `--all-targets` 且不带 `-Pkg` 时属全量，**必须显式 `-AllowFull`**；`exec` 透传模式不拦，需自行判断。全量测试优先交 CI，本地只跑受影响 crate 的快速回归。
  * 内存准入只看可用物理内存绝对下限（默认 1024MB，可用 `SKIFF_GATE_MIN_AVAILABLE_MB` 调整），不看 commit 比例：本机是动态内存虚拟机，实测物理 **4.92GB / 2 核 4 线程**。
  * 覆盖不到的入口，一律按红线人工遵守：`%USERPROFILE%\.cargo\bin\cargo.exe` 这类绝对路径、rust-analyzer / IDE 内部调用、CI 与其它机器。`.trae/` 只有 `rules/`，没有 shell 前置钩子，工具层无法再加一道拦截。
  * **包装是否生效，重启后先自检**：Trae 宿主的 `PATH` 是启动时的快照，仓库 `tools\bin` 可能不在其中，裸敲 `cargo` 会解析到 `cargo.exe` 并绕过包装；必须重启 Trae（含其终端）才继承新 `PATH`。重启后在新 shell 里执行 `where.exe cargo`，首行应为 `<仓库>\tools\bin\cargo.cmd`；若先出现 `%USERPROFILE%\.cargo\bin\cargo.exe`，说明包装未生效，先修 `PATH` 再编译。
* **git 写操作串行（红线）**：与编译共用同一把 `Global\SkiffBuild` 令牌，同样必须经包装入口 `tools\bin\git.cmd` 执行。
  * 写命令（`add` / `commit` / `merge` / `rebase` / `reset` / `checkout` / `switch` / `restore` / `stash` / `gc` / `prune` / `mv` / `rm` / `tag` / `push` / `fetch` 等）拿不到令牌即退出码 2，不排队；只读命令（`status` / `log` / `diff` / `show` 等）不占令牌，但会在 `gate.log` 记一条 `AUDIT git-read`。
  * 仓库已关闭后台自动维护（`gc.auto=0`、`maintenance.auto=false`），避免无人值守时重建 `packed-refs` / 写 `maintenance.lock`；需要回收时人工在闸门里跑一次 `git gc`。
  * 提交与推送只由唯一负责会话执行；其它会话只读，不并发写 refs，避免 stale lock 与 `AUTO_MERGE` / `rebase-merge` 残留。

### 3.2 测试要求

* **目录存放**：单元测试随模块（`#[cfg(test)]`），集成测试放 `crates/<name>/tests/`；全局/跨模块测试放根目录 `tests/`。
* **环境干净**：测试中间产物、日志及临时文件不得污染源码目录。
* **回归补充**：核心逻辑变更或 Bug 修复必须补充对应的测试用例。
* **快速回归**：判定逻辑改动先跑 `cargo test -p skiff-policy`（零 Docker、零 SQLite 依赖）。

### 3.3 三级文档体系

| 层级 | 路径 | 职责范围 |
| --- | --- | --- |
| **L1 根文档** | `AGENTS.md` | 仓库全局规则与通用规范，不记录子项目细节。 |
| **L2 项目文档** | `crates/<name>/AGENTS.md` | crate 专属业务、配置、接口与开发说明（当前均未建）。 |
| **L3 专项文档** | `docs/` | 全局架构、部署、CI 等跨 crate 专题文档。 |

* **命名规则**：`docs/sections/` 正文用「两位章号-小写中划线.md」（如 `08-engineering.md`）；`docs/` 顶层专题文档用语义化短名（如 `DESIGN.md`、`CI.md`）。
* **子文档拆分**：crate 内复杂专题可放 `crates/<name>/docs/`，但必须在对应 `AGENTS.md` 登记入口。

## 四、文档索引

| 文档 | 相对路径 | 说明 |
| --- | --- | --- |
| 设计主文档 | `docs/DESIGN.md` | 一页结论、文档地图、文档约定与变更记录（总入口） |
| 设计正文 | `docs/sections/` | §1–§12 共 13 个文件，文件名前缀 = 章号，章节增删以 `docs/DESIGN.md` 文档地图为准 |
| 架构审查 | `docs/ARCH_REVIEW.md` | 六轮审查记录与 P0–P2 行动清单 |
| 部署与测试 | `docs/DEPLOY_TEST.md` | 两种部署形态装机、配置矩阵、验收用例、实测记录与权限口径 |
| 持续集成 | `docs/CI.md` | 工作流门槛、工具链与 MSRV、发布产物流向 |
| 开发机环境 | `docs/DEV_ENV.md` | 本机 Windows 的推送认证（GCM）、卡死 push 占锁的排障顺序、无 C 编译器导致的可编译范围 |
| 迭代验证回路 | `docs/ITERATION.md` | 自动化迭代方案：分层编译位置、回滚协同协议、验收口径；测试机访问信息见 `docs/TEST_ENV.md` |
| 测试环境访问信息 | `docs/TEST_ENV.md` | 测试机地址、面板 URL/入口前缀/TLS、账号与口令文件读取方式、SSH、当前部署版本与 Release 资产摘要、最近验收结论（唯一来源） |
| 测试台账 | `docs/TEST_LEDGER.md` | 只记「正在处理 / 待处理」两栏的未闭环项；PASS 即删，不保留历史 |
| 测试矩阵 | `docs/TEST_MATRIX.md` | 按内核功能域组织的回归清单：稳定编号、触发条件、前置/步骤/预期/判定/代码位置；每章附最小回归命令 |
| 官方插件集规划 | `docs/PLUGIN_CATALOG.md` | 竞品对照、官方插件清单、6 个（guard/monitor/files/cron/ftp/firewall，原 nginx 已内核化为「网站」）落地情况与开箱即用交付线；实现与约定见 `plugins/README.md` |
| 菜单契约 | `docs/MENU_CONTRACT.md` | 三级菜单模型与归属、manifest 声明、L3 渲染与 iframe 边界、菜单设置/布局持久化、固定页槽位、旧包迁移与验收矩阵 |
| 插件 UI 契约 | `docs/PLUGIN_UI_CONTRACT.md` | 插件信息/管理页与功能页的路由分工、`view` 视图模型与静态文件映射、内核信息页字段与操作、沙箱/CSP、旧包 `[[ui]]` 回落与测试矩阵 |
| URL 契约 | `docs/URL_CONTRACT.md` | 全仓库页面与接口路径的唯一依据：前缀与例外、资源命名、层级与页面内接口同构、动作动词表、方法与副作用、查询参数、旧路径兼容与下线 |
| 内核 UI 交互基准 | `docs/UI_CONTRACT.md` | 页面标题/提示条/主次操作/工具栏/表格/空态/弹窗抽屉/危险确认/窄屏断点/类名前缀的统一规范，附现有页面盘点与待对齐清单 |
| 网站运行环境契约 | `docs/SITE_RUNTIME_CONTRACT.md` | 运行环境与站点模型、三类建站（运行环境/静态/反向代理）、PHP 专属配置、Docker 页边界、路由权限审计、P1/P2 范围与测试矩阵；可点预览 `docs/ui/site-runtime-preview.html` |
| 内核定时任务契约 | `docs/KERNEL_SCHEDULER_CONTRACT.md` | 任务模型（kind/source/调度/超时/重试/并发）、内存缓冲与四种回执、执行语义、插件唤醒链路、内置任务、插件分工（cron 降级为 UI）、API/权限/迁移与测试矩阵 |
| 内核文件系统契约 | `docs/KERNEL_FS_CONTRACT.md` | 管理员与插件双主体授权、操作面与原子写/路径锁、fs 审计、回收站与配额、受保护路径、站点目录归属、页面与测试矩阵 |
| 内核定时任务实现说明 | `docs/KERNEL_SCHEDULER_IMPL_NOTES.md` | `skiff-sched` 的任务模型、内存缓冲与回执、tick 语义、审计结构、与契约差异、SQLite/Web 接线边界与顺序 |
| 内核文件系统实现说明 | `docs/KERNEL_FS_IMPL_NOTES.md` | `skiff-fs` 的授权与路径解析、原子写/路径锁、回收站与配额、审计分级、与契约差异与接线边界 |
| 内核 FTP 契约 | `docs/KERNEL_FTP_CONTRACT.md` | 新增 `ftp` 能力面的主体与授权（manifest→`perm:ftp:<verb>`）、审计口径、站点账号与 uid/gid 对齐（`MinUID` 只降不升）、安全边界、宿主 pure-ftpd 取舍与测试矩阵；fail2ban 已收口到防火墙插件 |
| 内核防火墙契约 | `docs/KERNEL_FIREWALL_CONTRACT.md` | 新增 `firewall` 能力面的主体与授权（manifest→`perm:firewall:<verb>`）、方法清单、自有作用域（`inet skiff` / `SKIFF` 链 / `skiff-*.conf`）、审计口径、特权通道、interop 接口与测试矩阵 |
| SDUI 内核适配手册 | `docs/SDUI_HANDBOOK.md` | 面向 JSON Schema 驱动服务端 UI 的内核适配：现状耦合清单（业务/展示/传输归类）、资源与动作契约、权限校验审计挂接、分阶段迁移与回滚、开发手册与待定项；设计与迁移规范，不代表已实施 |
| Skiff-server 分发方案 | `docs/SKIFF_SERVER.md` | 中心化静态数据分发：商店/应用模板/运行环境/内核升级四类清单的模型、JSON Schema 与 Ed25519 签名清单、内容寻址与 `seq` 防回滚、镜像同步与多源切换、发布审核与吊销、客户端拉取与商店安装链路、P0–P3 落地与签名发布/镜像同步命令、静态服务契约（`skiff-server serve`，§16）、发布面 `/api/**`（§17，可选独立子命令，R0/R1 已实施、R2 待做）；托管来源待决策（§15） |
| 变更日志 | `docs/changes/` | 详细变更记录存放目录（`YYYY-MM-DD.md`） |

> 新增全局专项文档须同步在此登记。

## 五、变更记录

详细变更写入 `docs/changes/`（一事一条，按日期归档为 `2026-11-07.md` 这类文件名）。`docs/DESIGN.md` 的「变更记录」表只承载设计正文的结构性/结论性变化；同一事实只在一处维护，另一处留日期引用。
