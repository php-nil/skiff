# 持续集成与发布流水线

> 状态：**CI 已在 GitHub 全绿**（首绿 2026-10-31 run 41219623804；CI 最新 41278450106）。
> Release 流水线首跑 2026-11-01（run 41268959656）musl 双架构全绿、`docker` job 暴露
> Dockerfile 过期 COPY；修复后复跑 **run 41270406760 全绿**；发布物去签名（决策 12）后
> **run 41278461323 全绿**（产物两件套：tar.gz + .sha256，无 `.sig`）。§5 实测列已按 run 结果回填。
>
> 相关：工程结构与 `xtask` 固化校验见 [§8 工程结构](sections/08-engineering.md)；
> 装机步骤与真机验收用例见 [DEPLOY_TEST.md](DEPLOY_TEST.md)。

## 1. 三个工作流各管什么

**当前是开发/测试阶段：不发新版，但交付链路必须可用。** 测试期只走手动 dispatch
（版本沿用 0.1.0、不新建 tag、资产覆盖到同一 Release），用来真实验证「Release 安装包 →
下载 → 解压 → install.sh」；正式发版仍走标签推送（§4.2）。

| 文件 | 触发 | 管什么 |
|---|---|---|
| `.github/workflows/ci.yml` | push（main/master）、PR、手动；**纯文档改动不触发** | 质量门槛：格式、lint、单测、MSRV、契约校验、插件包链路、脚本、冒烟；musl 静态产物**仅手动** |
| `.github/workflows/release.yml` | 手动 dispatch（测试期唯一入口）+ `v*` 标签推送（正式发版） | 出可交付安装包：amd64 / arm64 musl 静态包 + `.sha256` + `checksums.txt`。测试期沿用 0.1.0，`publish` 勾选即把资产**覆盖到同一既有 Release**，不新建 tag |
| `.github/workflows/audit.yml` | 每周一 03:00 UTC、手动 | `cargo audit` 查 RustSec 公告 |

依赖升级不走 Dependabot：`dependabot.yml` 已删（2026-10-31），minor/patch 升级人工按需
`cargo update` + 合入前三关；漏洞通报仍由 `audit.yml` 周扫兜底。

**audit 首次报红（2026-11-02 schedule run 41428921272）**：`zstd 0.13.3` / `zstd-safe 7.3.0` 命中三条 RUSTSEC
（0391 复用 context 时残留流数据串到下一流、0403 legacy 帧流式解压失败暴露未初始化内存、0398 `Seekable::decompress`
把 `Vec` 长度设过容量）。修复版分别为 `zstd ^0.13.4` / `zstd-safe ^7.4.0`，2026-11-03 执行 `cargo update -p zstd`
（只动这两个包，`zstd-sys 2.1.2` 未变）后消除。`zstd` 是 `pkgfmt` 解 `.wbpkg` 的直接依赖，属**不可信输入路径**，
不是纯工具链组件，不能按「只是编译期依赖」放过。

## 2. 门槛与判定口径

**没有一个 job 用 `continue-on-error`。** 红了就是没通过，不存在“失败也算过”的兜底。

| job | 命令 | 对应验收项 |
|---|---|---|
| `fmt` | `cargo fmt --all --check` | 零格式漂移 |
| `clippy` | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 零告警（“release 零警告”是验收项，所以这里 `-D warnings`） |
| `test` | `cargo test --workspace --locked --no-fail-fast` | 全量单测 + 文档测试；**unix-only 用例第一次真正执行**（见 §3） |
| `test-win` | `cargo test --workspace --locked --no-fail-fast`（windows-latest） | `cfg(not(unix))` stub 路径有人管；「Windows 本地三关」交付口径在 CI 有复核（平台盲区见 §5） |
| `msrv` | `cargo check --workspace --all-targets --locked`（工具链 1.89） | `rust-version` 不说谎 |
| `verify` | `cargo run --locked -p xtask -- check` | 1Panel 变量对齐 / Compose 字段子集；另以空参加 `xtask verify-pkg`，验证缺包时「跳过而非报错」 |
| `pkg-chain` | `skiff-plugin` keygen → new → build → pack → verify；再跑 `plugins/pack.sh` 把 7 个官方插件包（6 个插件 + testkit）真打一遍 | keygen 落盘/公钥输出 + 插件包链路真签真验；**另跑两条反例**（错公钥、无公钥）必须被拒；官方插件包（发布包 `official/` 的来源）逐个验签 |
| `plugins` | `cargo {fmt,clippy,test} --manifest-path plugins/Cargo.toml …` | 官方插件 workspace（第二 workspace，根 `--workspace` 不覆盖它）的 fmt / `-D warnings` / 单测 |
| `scripts` | `shellcheck -S error`、`git ls-files` 查 CRLF、Dockerfile COPY 源存在性 | 部署脚本在 Linux 上跑得起来；Dockerfile 引用的上下文源真实存在（资源挪位不再漏到发版才炸） |
| `smoke` | 起 `skiff --dev serve`，探 `/healthz`、`/<入口>` 跳登录、裸 `/` 与未知路径 404 | 服务真能起来，且入口前缀/错误页没退化 |
| `build` | musl 静态编译 + `readelf` 查 `PT_INTERP` | 产物不挑宿主 glibc（**仅 workflow_dispatch**，不随 push 跑） |

`pkg-chain` 的两条反例是重点：只测“正例通过”会把验签写成摆设——错公钥也过、缺公钥也过，
直到有一天有人把私钥打进包才发现。反例断言的是**必须失败**。

**官方插件是第二个 workspace，根 `--workspace` 管不到它。** 根 `Cargo.toml` 里有
`exclude = ["plugins"]`，所以 `cargo fmt/clippy/test --workspace` 的三关对 `plugins/`
**完全没有覆盖**——漏了 `plugins` 这个 job，插件代码就是没人复核的盲区。
插件 crate 的 `--locked` 依赖 `plugins/Cargo.lock`（必须入库）。
注意 `cargo fmt --manifest-path plugins/Cargo.toml --all` 会顺着 path 依赖格式化到根
workspace（插件依赖 `crates/sdk`），所以本地跑它等于把根和插件一起格式化，`--check` 也可能
报根 crate 的漂移：这是预期，不是插件 workspace 的错。

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

**交付路径固定为「从 GitHub Release 下载安装包 → 校验 → 解压 → 执行 `install.sh`」。**
开发机工作树直传测试机、目标机就地编译都只算开发调试，**不是交付路径**（见 [DEPLOY_TEST.md](DEPLOY_TEST.md) §0.2）。

> 本节（尤其 §4.1 / §4.2 的触发方式）是**规定**，不是对某次操作的现状描述。实际操作与它不一致时
> （例如测试期用强移 tag 代替手动 dispatch），差异记在 [TEST_ENV.md](TEST_ENV.md) 的部署记录或
> `docs/changes/` 里，不要回头把规定改成现状。

### 4.1 测试期：手动 dispatch，覆盖同一个 Release

测试阶段**不新建 tag、不改版本号**。版本沿用 `0.1.0`，产物上传/覆盖到同一个既有 Release：

```text
GitHub → Actions → Release → Run workflow
  version:      0.1.0                      # 沿用现有版本
  publish:      true                       # 上传/覆盖 Release 资产
  release_tag:  v0.1.0                     # 目标 Release 标签（留空则用 v<version>）
  skip_plugins: false                      # 正式安装包必须带 7 个官方插件包（6 个插件 + testkit）
```

`publish` job 的行为是**只覆盖资产、绝不建 tag**：

1. 先用 `gh api repos/$GITHUB_REPOSITORY/git/ref/tags/<tag>` 校验标签已存在，不存在直接报错（不创建）。
   runner 的 checkout 只拉单 ref，`git ls-remote` 会误判标签不存在，故改用 API（run 41841016534）；
2. Release 已存在 → `gh release upload <tag> dist/* --clobber`（**同名资产直接覆盖**，
   反复 dispatch 不会因「资产已存在」失败）；
3. Release 不存在但标签存在 → `gh release create --verify-tag`（同样不创建标签）；
4. `night` 占位版本拒绝发布。

> 前置条件：仓库里得先有目标标签/Release。本流水线不负责创建它们；当前已有
> `v0.1.0` tag 与 Release（2026-11-07），资产坐标与摘要见 [TEST_ENV.md](TEST_ENV.md) §5、§6。
> 目标标签或 Release 不存在时，`publish=true` 会在第 1 步明确报错并给出提示，不会伪造发布成功。

只想验证构建链路、不碰 Release 时：`publish=false`，产物从 run 页 Artifacts 取（7 天保留）。

### 4.2 正式发版（当前不使用）

工作流同时保留 `push.tags: ["v*"]` 触发。真要发版时改版本号并推送标签即可（测试期不要这么做）：

```sh
# 1. 改版本（Cargo.toml 的 workspace.package.version）
# 2. git tag v0.1.0 && git push origin v0.1.0   # 标签触发能跑，但测试期禁止
# 或手动 dispatch：version 填真实版本号、publish=true（night 会被拒绝）
```

### 4.3 官方插件签名：不强制，缺私钥不阻塞出包

`build` job 检测 `secrets.PLUGIN_SIGNING_KEY`（ed25519 私钥种子，64 位 hex）：

| 场景 | 行为 |
|---|---|
| Secret 已配置 | 写入 `$RUNNER_TEMP/plugin-signing.key`（`umask 077`），交给 `pack-release.sh --plugin-key`；`plugins/pack.sh` 在临时目录回填 `manifest.key_id` 后打包，私钥不进产物/仓库 |
| Secret 缺失 | **不失败**：`pack-release.sh` 生成临时签名身份并打 `::warning::`，包仍可装可验，但签名人不稳定，**仅供测试**；正式发行前必须配置正式私钥 |
| `skip_plugins=true` | 出「裸包」，`::warning::` 明确标注不能作为正式发布物 |

`build` 自检会数 `official/` 下是否 **6** 个 `.wbpkg`（guard/monitor/files/cron/ftp/testkit），读取
`official/public-key.txt`（64 位 hex）逐个 `skiff-plugin verify --arch <矩阵架构>`——
既验签名，也复核「载荷架构没错位」。插件构建一律从仓库根用
`--manifest-path plugins/Cargo.toml` 调（`cross` 按 cwd 判定挂载哪个 workspace，
cwd 落在 `plugins/` 会报 `failed to find a workspace root`，run 41309171057 双架构同红）。

首次配置签名身份（只做一次；私钥只存在离线机与 secret 里）：

```sh
skiff-plugin keygen --out skiff-official-signing.key   # 0600，打印公钥(hex)与指纹(key_id)
gh secret set PLUGIN_SIGNING_KEY < skiff-official-signing.key
```

这把私钥是**发布身份**：换掉等于换签名人，已信任旧公钥的实例会拒装新包。
安装时 `official/public-key.txt` 随包落到 `/www/skiff/official/`，**由首启自动信任（TOFU）**，无需手工导入。

### 4.4 产物清单与安装包布局

`build` 矩阵并发编译 `x86_64-unknown-linux-musl` 与 `aarch64-unknown-linux-musl`
（`cross` 自带交叉工具链镜像），逐个跑 `deploy/pack-release.sh`（同一份脚本，CI 只换
`CARGO=cross` 与 `--target`）。`publish` 汇总后生成 Release 级 `checksums.txt`。

| 资产 | 说明 |
|---|---|
| `skiff-<ver>-<target>.tar.gz` | 安装压缩包（两个架构各一份），命名含版本与 target |
| `skiff-<ver>-<target>.tar.gz.sha256` | 单包摘要，装机前先校验 |
| `checksums.txt` | Release 级汇总摘要（覆盖所有 `.tar.gz`），`sha256sum -c checksums.txt` |

压缩包内布局（解压后 `cd` 进去直接 `sh install.sh`）：

| 文件 | 说明 |
|---|---|
| `skiff` / `skiff-plugin` | musl 静态主程序（`ldd` 应为 `not a dynamic executable`）与插件工具 |
| `install.sh` / `uninstall.sh` / `docker-bootstrap.sh` | 安装/卸载/前置引导（不依赖仓库 `deploy/` 路径） |
| `skiff.service` / `config.toml.example` | systemd unit 与配置样例 |
| `official/*.wbpkg` | 官方插件包 7 个（guard/monitor/files/cron/ftp/firewall + testkit），已签名；只复制到 `/www/skiff/official/`，**不自动安装** |
| `official/public-key.txt` | 上面那批包的签名公钥，首启自动信任（TOFU） |
| `checksums.txt` | **包内**所有文件的 SHA-256（不含自身），解压后可 `sha256sum -c` 复核 |
| `README.txt` | 简短安装说明（含 Docker 选项与 official 用法） |

**发布物不做 detached 签名**（§11 决策 12，2026-11-01）：只用 SHA-256 摘要防传输损坏；
`RELEASE_SIGNING_KEY_HEX` secret、`.sig` 产物与 `skiff-plugin verify-file` 链路已整体移除——
插件包 `.wbpkg` 自己的签名/TOFU 不受影响。


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

本地执行一律经闸门入口，串行约束、退出码（`2` 锁被占用 / `3` 内存拒绝 / `4` 需要 `-AllowFull`）与 `-AllowFull` 适用条件见 [AGENTS.md](../AGENTS.md) §3.1，本文不重复。全量测试优先交 CI。

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test  --workspace --locked
cargo check --workspace --all-targets --locked     # 想验 MSRV 就加 `+1.89.0`
cargo run --locked -p xtask -- check
shellcheck -S error deploy/*.sh plugins/*.sh

# 官方插件（第二 workspace）：根三关不覆盖，必须单独跑
cargo fmt    --manifest-path plugins/Cargo.toml --all --check
cargo clippy --manifest-path plugins/Cargo.toml --all-targets --locked -- -D warnings
cargo test   --manifest-path plugins/Cargo.toml --locked
```

以上带 `--workspace` / `--all-targets` 的命令属全量，本地执行须经闸门显式 `-AllowFull` 放行；CI 不受此限。CLI flag 有变（如某天要 `--all-features` 之外的新组合），以 CI 的命令为准——本文件只是索引。
