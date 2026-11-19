# 仓库总览与开发约定

> 本文档为仓库全局通用规范（L1 根文档）。执行任务前**必须**优先读取并严格遵循以下约定。

## 一、项目概述

- **项目名称**：Skiff（轻舟）
- **核心定位**：Rust 极简 Linux 服务器管理面板，只做三核心：**Web 管理服务 / Docker 管理 / 插件体系**。内核持 fs 能力面的管理员客户端（「工具 → 文件管理」，IO 全经 `skiff-fs`）；防火墙、主机监控等宿主功能由插件在能力面（docker/identity/fs/net/ftp/firewall）实现。
- **技术栈**：Rust（根 workspace + 第二 workspace `plugins/`）、SQLite、Docker、UDS 插件协议；交付以宿主机 systemd 直装为主，1Panel 商店 Docker 包为受限形态（能力面收缩为 docker/identity）。

## 二、仓库结构

| 路径 | 类型 | 说明 |
| --- | --- | --- |
| `crates/` | 子项目 | 根 workspace 的 18 个 crate（分层与依赖方向见 `docs/sections/08-engineering.md`） |
| `plugins/` | 子项目 | 第二 workspace（`exclude = ["plugins"]`）：`common`、6 个官方插件、testkit 与 `pack.sh` |
| `xtask/` | 工具 | `check`（1Panel 变量、Compose 字段子集）与 `verify-pkg` |
| `tools/` | 工具 | `bin/`（`cargo.cmd` / `git.cmd` 闸门）、`gate.ps1`、`linux-check.sh`、`iter.sh`、`perf/`、`skiff-server/` |
| `deploy/` | 交付 | 安装/卸载/打包脚本、systemd unit、Dockerfile、1Panel 商店包 |
| `tests/` | 测试目录 | 全局及跨模块测试与测试产物（按需建立） |
| `docs/` | 全局文档 | 主题式组织：`<topic>.md` 为入口、同名 `<topic>/` 存细节（contract / impl / test / ops / plugin / sdui / archive / sections / changes） |
| `.github/workflows/` | CI | `ci.yml` / `release.yml` / `audit.yml` / `iter.yml` |

> **维护规则**：新增、删除或重命名顶层目录时，**必须**同步更新上表。`target/`、`skiff-run/`、`.workbuddy/` 是本地产物/工作数据，已 gitignore，不得入库。

## 三、常用命令

> 一律经 `tools\bin\cargo.cmd` / `tools\bin\git.cmd`（全局串行令牌，拿不到即以退出码 2 拒绝，不排队）；命令与判定口径的唯一来源是 `docs/ops/CI.md` §2、§6，此处只作速查。

- **安装依赖**：`cargo fetch`；插件 workspace：`cargo fetch --manifest-path plugins/Cargo.toml`（`plugins/Cargo.lock` 必须入库）。
- **运行测试**：`cargo test --workspace --locked --no-fail-fast`；插件：`cargo test --manifest-path plugins/Cargo.toml --locked`。**只在 CI 或 Linux 测试机执行**，Windows 开发机不跑（§4.3）。
- **构建/启动**：`cargo build --workspace`；本地起服务 `cargo run -p skiff --bin skiff -- --dev serve`（`/healthz` 探活）。
- **本地门禁（只到编译级）**：`cargo fmt --all --check` → `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`。`--all-targets` 已把测试目标纳入编译，能查出类型错误、失效引用与死代码，**不再追加 `cargo test`**。`plugins/` 是第二 workspace，根门禁不覆盖，须用 `--manifest-path plugins/Cargo.toml` 另跑一遍。
- **其他校验**：`cargo run -p xtask -- check`（1Panel / Compose）、`cargo run -p skiff-server -- check`（商店清单）、`shellcheck -S error deploy/*.sh plugins/*.sh`。

## 四、开发与交付约定

### 4.1 架构优先与变更原则

- **全局视野，拒绝临时凑合**：修 Bug 或做小功能也**严禁**绕过既有架构、硬编码或破坏模块分层。**分层禁令**：`policy` / `pkgfmt` / `wire` 不得依赖 `store` / `docker`；`app` 不含路由与模板；`impl` 必须与类型同 crate。
- **边界防御**：引入跨模块调用、新增公共方法或改变数据流向，必须先理清上下游依赖并遵循既定设计。**单一来源**：插件↔内核协议只在 `skiff-wire`；TOFU 判定只在 `plugin::trust::resolve`；动词→Docker 分发只在 `Docker::execute`；角色→权限矩阵只在 `skiff_model::Permission`；路径授权代数只在 `skiff_model::path`（唯一有意例外：`skiff-fs` 的 I/O 感知解析器，见 `docs/impl/KERNEL_FS_IMPL_NOTES.md` §2.2.1）。
- **重构与修复并重**：既有技术债导致 Bug 频发时，在合理范围内内聚修复，或标记并列入「待决策问题」。
- **数字纪律**：未实测的数字与能力声明只标「设计目标 / 待实测」，不得当已验证事实引用。
- **通道唯一（无例外）**：宿主机进程执行只能经 `skiff_host`（`crates/host`），内核自身的文件写只能经 `AdminFs::write_kernel` / `remove_kernel`（`crates/app/src/admin_fs.rs`）。`crates/app` / `crates/core` 的非测试代码**不得出现 `Command::new`**；新增直接 `std::fs::write`/`remove_file`/`rename` 会让 `cargo run -p xtask -- check-channel` 失败（基线只减不增）。理由不是授权而是**记录**：`exec` 进运行记录、`fs` 进审计，绕过通道的行为事后查不到——这正是用「全量记录」替代「逐功能权限设计」的前提。

### 4.2 工作流与决策机制

- **同步更新（强约束）**：任何代码或核心逻辑变更，**必须**在同一次任务中同步更新对应的**测试用例**与**相关文档**，禁止只改代码。
- **延迟汇总决策**：常规/低风险问题（命名、局部实现、常规边界处理）按最佳实践直接处理，并在汇报中简述；高风险/阻塞性决策（架构变动、破坏性接口修改、跨模块依赖调整、需求歧义）**暂缓该分支**，先完成无阻塞部分，任务结束时集中列出「待决策问题」。
- **待决策问题**：必须给出方案、优劣对比与建议，不带问号交付。
- **迁移兼容**：当前为测试开发阶段，无正式环境，不做旧版本兼容迁移；旧版失效文档也无需保留。
- **部署留痕**：任何一次向测试机部署后，必须更新 `docs/test/TEST_ENV/` 下的 `02-DEPLOY_STATE.md`（当前部署）、`03-DEPLOY_LOG.md`（流水）与 `04-ACCEPTANCE.md`（验收结论），并在汇报中交代「地址 + 账号」；缺这步视为部署未完成。

### 4.3 测试规范

- **目录隔离**：单元测试随模块（`#[cfg(test)]`）；集成测试放 `crates/<name>/tests/`；全局/跨模块测试放根 `tests/`。
- **零污染原则**：中间产物、日志与临时文件不得生成在源码目录（已生成须在任务结束前清理）。
- **用例照写，不在本机跑**：新增功能、核心逻辑变更或 Bug 修复**必须**同批补用例；本地只保证它们编得过（`clippy --all-targets`），**用例结果一律以 CI 为准**（`test` / `test-win` / `plugins` job，`docs/ops/DEV_ENV.md` §3）。改判定逻辑需同步 `skiff-policy` 用例——那组零 Docker、零 SQLite，在 CI 上跑。
- **Windows 开发机禁止跑 `cargo test`**：Windows 不是交付形态——进程模型、权限位（0711/0660）、Unix socket、Docker 语义都与部署形态不同，本机跑绿不代表可用；本机能否编译还取决于外部 C 工具链探测（`docs/ops/DEV_ENV.md` §3），跑红也常是环境问题而非代码问题。用例结果一律看 CI 的 `test` / `test-win` / `plugins` job。
- **汇报口径**：只有 CI 跑过的用例才能说「已验证 / 全绿」；本地只做了编译级校验时，必须写明「fmt + clippy 通过，用例交 CI」，不得暗示已回归。

### 4.4 其他约定

- **编译与 git 写操作串行**：同一时间只允许一路 `cargo`（含 `fmt` / `clippy` / `test` / `build` 及任意子命令），根 workspace 与 `plugins/` 共用一把全局令牌，不排队；git 写命令与编译共用同一把令牌，提交与推送只由唯一负责会话执行。退出码 `2` 占用 / `3` 内存拒绝 / `4` 需 `-AllowFull`；禁止自定义 `CARGO_TARGET_DIR`；重启后先 `where.exe cargo` 确认首行为 `tools\bin\cargo.cmd`。细节见 `docs/ops/DEV_ENV.md` §4。
- **权限口径**：数据根与 `plugins/` 保持 **0711**（可穿越不可列），勿改回 0700，否则降权插件全线 EACCES（见 `docs/test/DEPLOY_TEST/01-PREGATE.md` §1.1）。

## 五、三级文档体系与管理规范

### 5.1 层级定义

| 层级 | 路径 | 职责范围 |
| --- | --- | --- |
| **L1 根文档** | `AGENTS.md` | 仓库全局规则、工作流约束与文档索引；不记录子项目细节。 |
| **L2 专项文档** | `docs/<topic>.md` + `docs/<topic>/` | 跨模块设计、契约、实现说明、交付验证；父文档是该主题的导航索引。 |
| **L3 子项目文档** | `crates/<name>/AGENTS.md`、crate 内 `docs/` | 模块专属约定（当前均未建）；局部优先，但不得与 L1 冲突。 |

### 5.2 编写与拆分硬性限制

- **命名规范**：主题入口用小写短名（`docs/test.md`），目录同其名；细节文档沿用现有口径——契约/实现类大写下划线（`*_CONTRACT.md`、`*_IMPL_NOTES.md`），章节文档 `NN-<topic>.md`，变更记录 `changes/YYYY-MM-DD.md`。
- **同题合一**：讲同一件事的多份文档归一个主题——一个入口文档 + 同名目录存细节，`docs/` 顶层不为同一主题散放多份文件。
- **精简原则**：保持内容高密度、可执行；发现过期或冗余信息时**主动清理**。
- **体积红线（触发即拆分）**：单份 Markdown **不得超过 30KB 且总行数不得超过 500 行**。
- **拆分方式**：单份文档触及红线时，建立同名目录存放子文档，原文件降级为目录索引。
  - *示例*：`docs/pro-abc.md` 超限 → 新建 `docs/pro-abc/` 存放细节文档，并将 `docs/pro-abc.md` 改写为子文档导航索引。
- **索引唯一**：全局索引只有本文「六、文档索引」，只登记到主题入口；`docs/<topic>.md` 是主题内的**局部导航**，不得充当全局索引。

## 六、文档索引

| 文档名称 | 相对路径 | 说明 |
| --- | --- | --- |
| 总体设计 | `docs/DESIGN.md` | 定位、范围边界、设计正文地图（改设计先读） |
| 设计正文 | `docs/sections/` | 13 个文件覆盖 §1–§12（§4 分两册）：01 需求 … 12 待实测清单 |
| 契约 | `docs/contract.md` | 9 篇契约入口：URL / 菜单 / 插件页 / 站点运行环境 + 内核 fs·调度·FTP·防火墙·exec（改行为先读） |
| 实现说明与审计 | `docs/impl.md` | 实际怎么做、与契约差在哪（两篇 IMPL_NOTES + 两篇审计稿） |
| 交付与验证 | `docs/test.md` | 装机 → 迭代 → 坐标 → 用例 → 未闭环（上测试机按此顺序读） |
| 工程环境与流水线 | `docs/ops.md` | CI workflow 与门槛命令、开发机闸门与退出码（出包 / CI 红 / 编译异常） |
| 插件与分发 | `docs/plugin.md` | 官方插件集、商店清单与签名、`serve`/`api` 发布面 |
| SDUI / budding | `docs/sdui.md` | 设计稿：资源 / 动作 / 迁移阶段、接入边界、上游能力台账 |
| 历史存档 | `docs/archive.md` | 架构审查、UI 设计过程与已收口评估稿（只读，不参与路由） |
| 插件工程 | `plugins/README.md` | 第二 workspace、插件开发 / 签名 / 打包 |
| 变更记录目录 | `docs/changes/` | 每日详细变更记录存放目录（只查历史，不作规范） |

> **维护规则**：新增、删除或移动**文档**（含主题入口与目录）时，**必须**同步本表；主题内部的子文档增减只改该主题的入口文档，不必回到本表。

## 七、变更记录规范

- 完成实质性代码修改、重构或配置变更后，**必须**将详细变更写入 `docs/changes/YYYY-MM-DD.md`（一事一条，按日期归档；当天已存在则追加，勿覆盖历史）。
- 涉及跨模块行为、契约或交付流程的改动，同一条记录写清「改了什么 / 为什么 / 验证方式」。
- `docs/DESIGN.md` 只放设计正文与索引，**不设「变更记录」章节**；设计正文的结构性/结论性变化写入 `docs/changes/YYYY-MM-DD.md`，同一事实只在一处维护。
