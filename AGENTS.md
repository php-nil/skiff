# 仓库总览与开发约定

> 本文档为仓库全局通用规范。子项目可维护独立规范，但不得与本规范冲突；同一信息仅在一处维护，其他位置保留相对路径引用。
> **执行约定：任何任务动手前必须先阅读本文件**，再按「四、文档索引」选读对应专项文档，然后执行。

## 一、项目概述

* **项目名称**：Skiff（轻舟）—— Rust 极简 Linux 服务器管理面板
* **核心定位**：只做三核心——Web 管理服务 / Docker 管理 / 插件体系。文件管理、防火墙、网站管理、主机监控等宿主功能不进内核，由插件层在宿主能力面（docker/identity/fs/net）实现。主交付形态为宿主机 systemd 直装；1Panel 商店 Docker 包是可选受限形态（能力面收缩为 docker/identity）。

## 二、仓库结构

| 路径 | 类型 | 说明 | 文档入口 |
| --- | --- | --- | --- |
| `crates/` | 子项目 | Rust workspace 的 14 个 crate（`xtask` 在顶层单独列出），成员清单以根 `Cargo.toml` 为准；crate 分层与职责见文档入口 | `docs/sections/08-engineering.md` |
| `plugins/` | 子项目 | **第二个 workspace**（`exclude = ["plugins"]`，不受根 `--workspace` 覆盖）：共享库 `common`、官方插件 `guard` / `monitor` / `nginx` / `files` / `cron` 与 `pack.sh` 打包脚本；发布包 `official/` 的产出源头 | `plugins/README.md` |
| `xtask/` | 工程校验 | `cargo run -p xtask -- check` 固化两类校验（1Panel 变量、Compose 字段子集）；`verify-pkg` 为独立子命令，校验 `.wbpkg` 结构/摘要/签名 | `docs/sections/08-engineering.md` |
| `deploy/` | 部署物料 | 安装/卸载/打包脚本、systemd unit、Dockerfile、1Panel 商店包、`config.toml.example` | `docs/DEPLOY_TEST.md` |
| `docs/` | 全局文档 | 全仓库专项文档 | 见「四、文档索引」 |
| `tools/` | 开发机工具 | `linux-check.sh`（本地 Linux 目标编译级复核）、`iter.sh`（单轮迭代编排：门禁/取包/装/验收/回滚协同） | `docs/ITERATION.md` |
| `.github/workflows/` | CI/CD | `ci.yml` / `release.yml` / `audit.yml` / `iter.yml` 四个工作流 | `docs/CI.md` |

> 顶层目录变动时，须同步更新上表；子项目新增独立规范（`crates/<name>/AGENTS.md`）须在上表登记入口。`target/`、`skiff-run/`、`.workbuddy/` 等是本地产物/工作数据，已被 gitignore，不得入库。

## 三、全局约定

### 3.1 开发与交付

* **同步更新**：代码、逻辑变动必须与测试用例、相关文档同步更新。
* **交付门槛**：所有变更须本地三关全绿后才可交付合入：`cargo fmt --all`、`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`、`cargo test --workspace --locked`，口径与 CI 对齐（`docs/CI.md`）。
  * 动 `plugins/` 时，根三关**不覆盖它**（第二 workspace），另需 `cargo fmt --manifest-path plugins/Cargo.toml --all --check`、`cargo clippy --manifest-path plugins/Cargo.toml --all-targets --locked -- -D warnings`、`cargo test --manifest-path plugins/Cargo.toml --locked`。
* **红线**（违反即返工）：
  * 分层禁令：`policy` / `pkgfmt` / `wire` 不得依赖 `store` / `docker`；`app` 不含路由与模板；`impl` 必须与类型同 crate。
  * 单一来源：插件↔内核 UDS 协议只在 `skiff-wire`；TOFU 判定只在 `plugin::trust::resolve`；动词→Docker 分发只在 `Docker::execute`；角色→权限矩阵只在 `skiff_model::Permission`；路径授权代数只在 `skiff_model::path`；插件能力面实现只在 `crates/plugin`。
  * 数字纪律：未实测的数字与能力声明只标「设计目标 / 待实测」，不得当已验证事实引用。
  * 权限口径：数据根与 `plugins/` 保持 0711（可穿越不可列），勿改回 0700，否则降权插件全线 EACCES（详见 `docs/DEPLOY_TEST.md` §1.1）。

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
| 迭代验证回路 | `docs/ITERATION.md` | 自动化迭代方案：分层编译位置、回滚协同协议、验收口径、测试机凭据约定 |
| 官方插件集规划 | `docs/PLUGIN_CATALOG.md` | 竞品对照、官方插件清单、首批 5 个（guard/monitor/nginx/files/cron）落地情况与开箱即用交付线；实现与约定见 `plugins/README.md` |
| 变更日志 | `docs/changes/` | 详细变更记录存放目录（`YYYY-MM-DD.md`） |

> 新增全局专项文档须同步在此登记。

## 五、变更记录

详细变更写入 `docs/changes/YYYY-MM-DD.md`（一事一条，按日期归档）。`docs/DESIGN.md` 的「变更记录」表只承载设计正文的结构性/结论性变化；同一事实只在一处维护，另一处留日期引用。
