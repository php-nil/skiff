# 持续集成与发布流水线

> 状态：**CI 已在 GitHub 全绿**（首绿 2026-10-31 run 41219623804，最新 41270386660）。
> Release 流水线首跑 2026-11-01（run 41268959656）musl 双架构全绿、`docker` job 暴露
> Dockerfile 过期 COPY；修复后复跑 **run 41270406760 全绿**（含 docker 构建 + 自检，
> 产物 release-amd64 / release-arm64 已上传）。§5 实测列已按 run 结果回填。
>
> 相关：工程结构与 `xtask` 三类固化校验见 [§8 工程结构](sections/08-engineering.md)；
> 装机步骤与真机验收用例见 [DEPLOY_TEST.md](DEPLOY_TEST.md)。

## 1. 三个工作流各管什么

**当前是开发/测试阶段（2026-10-31 起）：不发新版。** 发布链路改成「可跑但默认不发」——
用来验证 musl/cross/打包/签名全链路，而不是产出对外发行物。

| 文件 | 触发 | 管什么 |
|---|---|---|
| `.github/workflows/ci.yml` | push（main/master）、PR、手动；**纯文档改动不触发** | 质量门槛：格式、lint、单测、MSRV、契约校验、插件包链路、脚本、冒烟；musl 静态产物**仅手动** |
| `.github/workflows/release.yml` | **仅手动**（`v*` 标签触发已摘） | 出发布物验证包：amd64 / arm64 静态包 + 摘要 + 签名。默认只传 workflow 产物，`publish` 勾选才建 GitHub Release |
| `.github/workflows/audit.yml` | 每周一 03:00 UTC、手动 | `cargo audit` 查 RustSec 公告 |

依赖升级不走 Dependabot：`dependabot.yml` 已删（2026-10-31），minor/patch 升级人工按需
`cargo update` + 合入前三关；漏洞通报仍由 `audit.yml` 周扫兜底。

## 2. 门槛与判定口径

**没有一个 job 用 `continue-on-error`。** 红了就是没通过，不存在“失败也算过”的兜底。

| job | 命令 | 对应验收项 |
|---|---|---|
| `fmt` | `cargo fmt --all --check` | 零格式漂移 |
| `clippy` | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 零告警（“release 零警告”是验收项，所以这里 `-D warnings`） |
| `test` | `cargo test --workspace --locked --no-fail-fast` | 全量单测 + 文档测试；**unix-only 用例第一次真正执行**（见 §3） |
| `test-win` | `cargo test --workspace --locked --no-fail-fast`（windows-latest） | `cfg(not(unix))` stub 路径有人管；「Windows 本地三关」交付口径在 CI 有复核（平台盲区见 §5） |
| `msrv` | `cargo check --workspace --all-targets --locked`（工具链 1.89） | `rust-version` 不说谎 |
| `verify` | `cargo run --locked -p xtask -- check` | 1Panel 变量对齐 / Compose 字段子集 / 包格式 |
| `pkg-chain` | `skiff-plugin` keygen → new → build → pack → verify | 插件包链路真签真验；**另跑两条反例**（错公钥、无公钥）必须被拒 |
| `scripts` | `shellcheck -S error`、`git ls-files` 查 CRLF、Dockerfile COPY 源存在性 | 部署脚本在 Linux 上跑得起来；Dockerfile 引用的上下文源真实存在（资源挪位不再漏到发版才炸） |
| `smoke` | 起 `skiff --dev serve`，探 `/healthz`、`/<入口>` 跳登录、裸 `/` 与未知路径 404 | 服务真能起来，且入口前缀/错误页没退化 |
| `build` | musl 静态编译 + `readelf` 查 `PT_INTERP` | 产物不挑宿主 glibc（**仅 workflow_dispatch**，不随 push 跑） |

`pkg-chain` 的两条反例是重点：只测“正例通过”会把验签写成摆设——错公钥也过、缺公钥也过，
直到有一天有人把私钥打进包才发现。反例断言的是**必须失败**。

## 3. 刻意决策

**工具链锁 1.98.1，不跟 stable 浮动。** 新 stable 会带新 lint，某天一个跟本次改动无关的
PR 突然变红，排查成本很高。升级方式：改 `ci.yml` / `release.yml` 顶部的 `RUST_VERSION`，
本地同版本跑一遍 `cargo clippy ... -D warnings` 再推。

**MSRV 是 1.89，不是 1.85。** 原来 `Cargo.toml` 写 1.85，实测用 1.85 连依赖解析都过不去：
`rusqlite 0.41.1 requires rustc 1.89.0`、`libsqlite3-sys 0.39` / `sha2 0.11` / `icu_*` 要 1.88。
装 1.89 后 `cargo check --workspace --all-targets --locked` 通过，因此把 `rust-version`
改成 1.89、`DEPLOY_TEST.md §0.1` 同步更正。`msrv` 这个 job 就是防止它再悄悄往上爬。

**CI 里删掉 `.cargo/config.toml`。** 仓库自带的那份把 crates.io 换成了国内镜像（开发机直连被阻），
而 CI runner 在境外、也没有镜像可用性保证。所以每个跑 cargo 的 job 先 `rm -f .cargo/config.toml`
走 crates.io。**只删工作区里的这一份，不动仓库内容**；本地开发照旧用镜像。
`release.yml` 的 `docker` job 是同一口径的变体：镜像内 cargo 构建也必须走 crates.io，
但不能 `rm`——Dockerfile 的 `COPY .cargo ./.cargo` 要求源存在，删掉会死在 COPY。
所以 build 前把文件**置空**（保留空文件），效果等同。

**开发阶段省 runner 时间（2026-10-31）。** 三处：

1. **纯文档改动不触发 CI**（`paths-ignore`：`docs/**`、`**.md`、`.gitignore`、`.workbuddy/**`）。
   ⚠ 若分支保护把 CI 检查设为 required，md-only 的 PR 会因「检查没跑」被卡住——
   那种情况去掉 `paths-ignore`，或只对 push 生效。
2. **不再 `apt-get install`**。`ubuntu-24.04` 镜像已预装 gcc / make / pkg-config / shellcheck
   （清单见 runner-images 的 Ubuntu2404-Readme，已核对），每次 job 省 30~60s。
   shellcheck 仍加了 `command -v` 存在性断言：万一镜像拿掉了，宁可红也不要静默跳过。
3. **同工具链 job 共用 rust-cache（`shared-key: ci`）**，避免 5 份 target 目录吃满
   10GB 缓存配额；msrv / musl 因工具链或目标不同各用独立 key。

## 4. 发布流程

**开发阶段（现在）：不发版。** 验证发布链路用手动 dispatch：

```sh
# GitHub → Actions → Release → Run workflow
#   version: 留空即可（开发阶段没有版本号，产物自动用 night 占位命名）
#   publish: 保持 false → 只构建 + 自检 + 传 workflow 产物，不建 Release
```

产物在 run 页的 Artifacts 里下载（7 天保留），验证包结构/摘要/签名即可，
Release 页保持干净。等真要发版时：

```sh
# 1. 改版本（Cargo.toml 的 workspace.package.version）
# 2. 打标签推送（先把 release.yml 里注释掉的 push.tags 放开）
git tag v0.1.0 && git push origin v0.1.0
# 或仍然手动 dispatch，但 version 必须填真实版本号、publish 勾成 true
#   （night 占位版本会被 publish job 直接拒绝）
```

`build` 矩阵并发编译 `x86_64-unknown-linux-musl` 与
`aarch64-unknown-linux-musl`（用 `cross`，自带交叉工具链镜像），逐个跑
`deploy/pack-release.sh`；`publish`（仅 `publish=true` 时跑）汇总两套产物 +
`SHA256SUMS` 建成 GitHub Release。

签名靠仓库 secret **`RELEASE_SIGNING_KEY_HEX`**：内容是 `skiff-plugin keygen` 打印的私钥那 32 字节 hex。
没配也能出包，但会打 `::warning::` 并把产物标成未签名——**不伪造“已签名”**。
正式发布前必须配上，并且把公钥指纹与公钥一起公布。

发布物结构（与手工 `pack-release.sh` 完全一致，同一份脚本产出）：

| 文件 | 说明 |
|---|---|
| `skiff-<ver>-linux-<arch>.tar.gz` | `skiff` + `skiff-plugin` + `install.sh` / `uninstall.sh` / `skiff.service` / `config.toml.example` |
| `.tar.gz.sha256` | 装机前先校验 |
| `.tar.gz.sig` | detached ed25519 签名（hex），`skiff-plugin verify-file` 验 |

## 5. 哪些地方是“推上去才知道”（实测列已回填）

| 项 | 风险 | 首次运行的判断依据 | 实测结论 |
|---|---|---|---|
| unix-only 用例 | `crates/sdk/tests/kernel_contract.rs` 在 Windows 上只编译不执行，CI 是**首次真跑** | `test` job 红 → 看具体断言，多半是 UDS 路径/时序假设 | 2026-10-31 首绿（run 41219623804）起 `test` job 多轮全绿，用例真跑通过 |
| `cross` 拉镜像 | 首次要拉 `ghcr.io/cross-rs/*` 镜像，arm64 尤其慢（可能 10 分钟以上） | `release.yml` 的 `build` 矩阵卡在 “Pulling image” 属正常 | Release 首跑（41268959656）两套 build job 均绿，拉镜像+编译在 60min 超时内完成 |
| arm64 musl + ring | `ring` 的汇编在 aarch64-musl 下需要正确的 clang 目标；cross 镜像通常自带 | 报 `unknown target triple` 之类 = 交叉工具链问题，不是代码问题 | 41268959656 的 aarch64 build 绿：ring 交叉编译过，产物过静态链接自检 |
| `smoke` 非 root 启动 | 首次在非 root 下起服务：`kernel.sock` / 数据目录 chown 失败只应告警，不应退出 | 日志里出现 `kernel.sock 服务未启动` 是**预期**（无插件即无影响） | 41268216736 等多轮 smoke 全绿：`/healthz` 20s 内就绪，服务非 root 起得来 |
| Dockerfile | 此前两处**必崩**（`-p skiff-core` 包名不存在、少 `COPY xtask`）已修；第三次是资源挪位类 | `docker` job 红在 `COPY ... not found` = 仓库布局变了、Dockerfile 引用没跟上 | Release 首跑（41268959656）暴露第三处：`COPY web-assets` 过期（10-31 迁位未同步）→ 已修复并加 COPY 源静态校验，复跑 41270406760 的 docker job 全绿（构建 + `version`/`--help` 自检） |
| 预装假设 | 删掉 `apt-get` 后押注镜像预装 gcc/pkg-config/shellcheck；镜像升级可能拿掉某个 | 对应 job 红在「找不到命令」→ 把安装步骤加回去，别改成静默跳过 | 截至 41268216736 全绿，预装清单成立（shellcheck 仍有 `command -v` 断言兜底） |

## 6. 本地复现 CI

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test  --workspace --locked
cargo check --workspace --all-targets --locked     # 想验 MSRV 就加 `+1.89.0`
cargo run --locked -p xtask -- check
shellcheck -S error deploy/*.sh
```

CLI flag 有变（如某天要 `--all-features` 之外的新组合），以 CI 的命令为准——本文件只是索引。
