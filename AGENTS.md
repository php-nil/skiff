# 仓库总览与开发约定

> 本文档为仓库全局规范，同一信息仅在一处维护，其他位置保留相对路径引用；子项目规范不得与本文件冲突。
> **执行约定：任何任务动手前先读本文件，再按「五、先读哪一篇」选读专项文档。** 全量索引、分类与状态见 [docs/README.md](docs/README.md)。

## 一、项目定位

Skiff（轻舟）—— Rust 极简 Linux 服务器管理面板，只做三核心：**Web 管理服务 / Docker 管理 / 插件体系**。内核持有 fs 能力面的**管理员客户端**（「工具 → 文件管理」，IO 全部经 `skiff-fs`），但不把宿主文件业务塞回内核业务层；防火墙、主机监控等其余宿主功能由插件层在宿主能力面（docker/identity/fs/net/ftp/firewall）实现。主交付形态为宿主机 systemd 直装；1Panel 商店 Docker 包是可选受限形态（能力面收缩为 docker/identity）。细节见 [DESIGN.md](docs/DESIGN.md) §0。

## 二、仓库地图

| 路径 | 说明 | 文档入口 |
| --- | --- | --- |
| `crates/` | 根 workspace 的 16 个 crate；分层、职责与依赖方向 | [sections/08-engineering.md](docs/sections/08-engineering.md) |
| `plugins/` | 第二个 workspace（`exclude = ["plugins"]`）：`common`、6 个官方插件、testkit 与 `pack.sh` | [plugins/README.md](plugins/README.md) |
| `xtask/` | `check`（1Panel 变量、Compose 字段子集）与 `verify-pkg` | [sections/08-engineering.md](docs/sections/08-engineering.md) |
| `tools/` | `linux-check.sh`、`iter.sh`、`perf/`、`skiff-server/`（根 workspace 成员） | [ITERATION.md](docs/ITERATION.md)、[SKIFF_SERVER.md](docs/SKIFF_SERVER.md) |
| `deploy/` | 安装/卸载/打包脚本、systemd unit、Dockerfile、1Panel 商店包 | [DEPLOY_TEST.md](docs/DEPLOY_TEST.md) |
| `docs/` | 全仓库专项文档 | [docs/README.md](docs/README.md) |
| `.github/workflows/` | `ci.yml` / `release.yml` / `audit.yml` / `iter.yml` | [CI.md](docs/CI.md) |

> 顶层目录变动须同步本表；新增 `crates/<name>/AGENTS.md` 或 crate 内 `docs/` 须在上表登记入口。`target/`、`skiff-run/`、`.workbuddy/` 是本地产物/工作数据，已 gitignore，不得入库。

## 三、硬性红线（违反即返工）

- **分层禁令**：`policy` / `pkgfmt` / `wire` 不得依赖 `store` / `docker`；`app` 不含路由与模板；`impl` 必须与类型同 crate。
- **单一来源**：插件↔内核 UDS 协议只在 `skiff-wire`；TOFU 判定只在 `plugin::trust::resolve`；动词→Docker 分发只在 `Docker::execute`；角色→权限矩阵只在 `skiff_model::Permission`；路径授权代数只在 `skiff_model::path`（唯一有意例外：`skiff-fs` 的 I/O 感知解析器，见 [KERNEL_FS_IMPL_NOTES.md](docs/KERNEL_FS_IMPL_NOTES.md) §2.2.1）；插件能力面实现只在 `crates/plugin`。
- **数字纪律**：未实测的数字与能力声明只标「设计目标 / 待实测」，不得当已验证事实引用。
- **权限口径**：数据根与 `plugins/` 保持 0711（可穿越不可列），勿改回 0700，否则降权插件全线 EACCES（见 [DEPLOY_TEST.md](docs/DEPLOY_TEST.md) §1.1）。
- **编译与测试串行（红线）**：同一时间只允许一路 `cargo`（含 `fmt` / `clippy` / `test` / `build` 及任意子命令），必须经包装入口 `tools\bin\cargo.cmd`；根 workspace 与 `plugins/` 共用同一把全局令牌，拿不到立即以退出码 2 拒绝、不排队；全量 `--workspace` / `--all-targets` 本地须显式 `-AllowFull`；禁止自定义 `CARGO_TARGET_DIR`。闸门链路、退出码、内存准入、`PATH` 自检与故障处置见 [DEV_ENV.md](docs/DEV_ENV.md) §4。
- **git 写操作串行（红线）**：与编译共用同一把令牌，写命令必须经 `tools\bin\git.cmd`，只读命令不占令牌；提交与推送只由唯一负责会话执行，其它会话只读。细节见 [DEV_ENV.md](docs/DEV_ENV.md) §4.4。

## 四、交付与测试纪律

- **同步更新**：代码、逻辑变动必须与测试用例、相关文档同步更新。
- **迁移兼容**：当前为测试开发阶段，没有正式环境，不需要兼容旧版本支持迁移。旧版失效文档也无需保留，一切放在当前已有功能上。
- **交付门槛**：命令与判定口径只在 [CI.md](docs/CI.md) §2、§6 维护；本文件不重复。动 `plugins/` 时，根三关**不覆盖它**（第二 workspace），须另跑插件 workspace 三关。
- **测试机部署留痕**：任何一次向测试机部署后，必须更新 [TEST_ENV.md](docs/TEST_ENV.md) 的部署记录与验收结论，并在汇报中交代「地址 + 账号」；缺这步视为部署未完成。
- **测试要求**：单元测试随模块（`#[cfg(test)]`），集成测试放 `crates/<name>/tests/`，全局/跨模块测试放根 `tests/`；中间产物、日志与临时文件不得污染源码目录；核心逻辑变更或 Bug 修复必须补充用例；判定逻辑改动先跑 `cargo test -p skiff-policy`（零 Docker、零 SQLite 依赖）。

## 五、先读哪一篇

| 我要做什么 | 先读 | 读到什么程度 |
| --- | --- | --- |
| 改 URL / handler / 表单动作 | [URL_CONTRACT.md](docs/URL_CONTRACT.md) | 路径、动词、方法与副作用 |
| 改菜单 / 插件页 / iframe | [MENU_CONTRACT.md](docs/MENU_CONTRACT.md) → [PLUGIN_UI_CONTRACT.md](docs/PLUGIN_UI_CONTRACT.md) | 归属、声明、`view`、CSP |
| 改内核页面或文案 | [UI_CONTRACT.md](docs/UI_CONTRACT.md) | 标题、提示、工具栏、空态、危险确认 |
| 改站点 / 运行环境 | [SITE_RUNTIME_CONTRACT.md](docs/SITE_RUNTIME_CONTRACT.md) | 站点模型、建站类型、PHP/Node |
| 改 fs / 调度 / FTP / 防火墙 | `docs/KERNEL_*_CONTRACT.md` → `docs/KERNEL_*_IMPL_NOTES.md` | 契约先读，实现差异后读 |
| 出包 / CI 红 / 发版 | [CI.md](docs/CI.md) | §2 门槛、§4 发布、§6 本地复现 |
| 上测试机 / 验收 / 回滚 | [DEPLOY_TEST.md](docs/DEPLOY_TEST.md) → [ITERATION.md](docs/ITERATION.md) → [TEST_ENV.md](docs/TEST_ENV.md) → [TEST_MATRIX.md](docs/TEST_MATRIX.md) → [TEST_LEDGER.md](docs/TEST_LEDGER.md) | 步骤 → 编排 → 坐标 → 用例 → 未闭环 |
| 改商店 / 分发 / `skiff-server` | [SKIFF_SERVER.md](docs/SKIFF_SERVER.md) → [PLUGIN_CATALOG.md](docs/PLUGIN_CATALOG.md) | 清单、签名、`serve` / `api` 契约 |
| 接 budding / SDUI | [SDUI_BUDDING_INTEGRATION.md](docs/SDUI_BUDDING_INTEGRATION.md) → [SDUI_HANDBOOK.md](docs/SDUI_HANDBOOK.md) | 接入边界、迁移方法 |

> 详细变更写入 `docs/changes/YYYY-MM-DD.md`（一事一条，按日期归档）；[DESIGN.md](docs/DESIGN.md) 的「变更记录」只承载设计正文的结构性/结论性变化，同一事实只在一处维护，另一处留日期引用。
