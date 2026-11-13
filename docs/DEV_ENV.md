# 开发机环境与本地操作限制

> 只写「本机（Windows 开发机）能做什么、踩过哪些坑」；目标机与测试机口径见 [DEPLOY_TEST.md](DEPLOY_TEST.md)、[TEST_ENV.md](TEST_ENV.md)。
> 编译串行、闸门链路与退出码、内存准入、`PATH` 包装、git 写操作串行的**操作细节只在本文维护**；[AGENTS.md](../AGENTS.md) §三 只保留红线摘要。

## 1. 推送认证：凭据未命中就会弹 GitHub 登录框

- **现象**：`git push` 弹出「Connect to GitHub」，命令挂在认证上不返回，也不报错。
- **原因**：全局 `credential.useHttpPath = true` 会让 git 把仓库路径一并交给 helper，GCM 的查找键因此变成带 `/owner/repo` 的形式；凭据管理器里实际存在的是 `git:https://github.com`（无 path），两者不匹配，GCM 对未命中的默认动作是弹窗而不是失败。
- **做法**：只对 GitHub 关掉路径匹配——`git config --global credential.https://github.com.useHttpPath false`。不要全局关，其它 host 可能靠路径区分仓库。
- **验证**：`GCM_INTERACTIVE=never git ls-remote origin`，命中会直接返回，未命中以错误退出而不弹窗；若报 `401/403` 则是 token 失效或对目标仓库无权限，与键匹配无关。
- **注意**：GitHub 的 HTTPS 认证不收账号密码，弹窗里填密码不会生效，只能用 PAT 或 OAuth。

## 2. 卡死的 push 会占住全局锁

- **现象**：push 卡在认证后，后续所有 `git` / `cargo` 一律 `BLOCKED：编译闸门被占用`，退出码 2。
- **原因**：闸门按「持有进程」发放 `Global\SkiffBuild`，挂起的 push 不退出就一直持有；包装器不排队，后来者直接拒绝。
- **做法**：先看 [gate.log](../.workbuddy/locks/gate.log) 末尾是否有 `ACQUIRED task=git` 而没有对应的 `RELEASED`，再看是否残留 `git` / `git-remote-https` / `git-credential-manager` 进程或 GCM 窗口；确认是本次 push 卡住后再终止该进程树，不要连带杀掉正在编译的进程。

## 3. 本机没有 C 编译器：一部分 crate 本机编不了

- **现象**：`cargo check/test` 停在 `libsqlite3-sys` / `zstd-sys` / `ring` 的构建脚本，报找不到 `cl.exe`。
- **原因**：`rusqlite` 用 `bundled`（自带 SQLite 源码编译）、`zstd` 走 `zstd-sys`、`rustls` 启用 `ring`，三者都需要 C 编译器，本机未装 MSVC C++ 生成工具。
- **做法**：本机只跑编得动的子集——`skiff-model` / `skiff-policy` / `skiff-cron` / `skiff-sched` / `skiff-compose` / `skiff-docker` / `skiff-wire` / `skiff-fs`；`skiff-store` / `skiff-pkgfmt` / `xtask` 以及传递依赖它们的 `skiff-identity` / `skiff-app` / `skiff-web` / `skiff`（`crates/core`）/ `skiff-plugin`（`crates/plugin`）/ `skiff-sdk` 本机编不了，只能交 CI 的 `test-win` 与 `test` job。
- **插件侧同理**：`plugins/` 经 path 依赖到 `skiff-sdk`，本机也编不了，改动交 CI 的 `plugins` job。
- **分工**：本机只做编得动的快速回归，Linux 产物一律由 workflow 编译发布，本机不出发布物。

## 4. 编译与 git 写操作闸门

### 4.1 包装入口与串行令牌

- 同一时间只允许一路 `cargo` 编译或测试（`fmt` / `clippy` / `test` / `build` 及任意 `cargo` 子命令），不分会话、不分开发者。必须经包装入口：`PATH` 靠前的 `tools/bin/cargo.cmd` → `tools/cargo-gate.ps1` → `tools/gate.ps1`，由全局命名互斥体 `Global\SkiffBuild` 发放唯一令牌。
- 拿不到令牌立即以退出码 2 拒绝，**不排队等待**。根 workspace 与 `plugins/` 共用同一把令牌；插件侧命令按 `--manifest-path` 记为 `workspace=plugins` 租约。
- 第二层是单次调用内部：`.cargo/config.toml` 固化 `[build] jobs = 1`，同一时刻只跑一路 `rustc`。`plugins/` 无本地 `.cargo/config.toml`，继承根配置；闸门的插件任务另显式带 `--jobs 1`。
- `cargo fmt` 不取 cargo 的 target 锁，只能靠包装入口拦住，因此 `fmt` 同样必须走该入口。
- 禁止为了并行使用自定义 `CARGO_TARGET_DIR`——它会绕过 cargo 自带的 target 目录锁，是已发生过的真实事故成因。

### 4.2 退出码与全量判定

`pwsh tools/gate.ps1` 的退出码：

| 退出码 | 含义 |
|---|---|
| `0` | 成功 |
| `2` | 锁被占用 |
| `3` | 内存准入拒绝 |
| `4` | 需要 `-AllowFull` |

- 根 workspace 的 `clippy` / `test` / `build` 带 `--workspace` 或 `--all-targets` 且不带 `-Pkg` 时属全量，**必须显式 `-AllowFull`**。
- `exec` 透传模式不拦，需自行判断。
- 全量测试优先交 CI，本地只跑受影响 crate 的快速回归。

### 4.3 内存准入、本机规格与提交泄漏

- 内存准入只看可用物理内存绝对下限（默认 1024 MB，可用 `SKIFF_GATE_MIN_AVAILABLE_MB` 调整），不看 commit 比例。可用物理内存低于 1024 MB 时闸门直接以退出码 3 拒绝：先释放内存，不要通过调低阈值把门降下来。
- 本机实测配置：Dell Vostro 15 3510，11th Gen Intel Core i5-1135G7（4 核 8 线程），物理 **8GB**（约 7.75GB 可用），页面文件 24GB、提交上限 31.75GB。
- 若出现**系统提交量远大于各进程提交量之和**（曾实测 31.19GB/31.75GB vs 进程合计 5.25GB），且编译报 LLVM OOM，属约 25GB 无法归属进程的提交泄漏（疑似 rustc `0xC0000409` 异常退出残留的内核/会话提交），杀进程无效，**重启回收**；不要靠改 build 参数或 `CARGO_TARGET_DIR` 绕过。
- 覆盖不到的入口一律按红线人工遵守：`%USERPROFILE%\.cargo\bin\cargo.exe` 这类绝对路径、rust-analyzer / IDE 内部调用、CI 与其它机器。`.trae/` 只有 `rules/`，没有 shell 前置钩子，工具层无法再加一道拦截。
- **包装是否生效，重启后先自检**：Trae 宿主的 `PATH` 是启动时的快照，仓库 `tools\bin` 可能不在其中，裸敲 `cargo` 会解析到 `cargo.exe` 并绕过包装；必须重启 Trae（含其终端）才继承新 `PATH`。重启后在新 shell 里执行 `where.exe cargo`，首行应为 `<仓库>\tools\bin\cargo.cmd`；若先出现 `%USERPROFILE%\.cargo\bin\cargo.exe`，说明包装未生效，先修 `PATH` 再编译。
- `tools\bin` 不在 `PATH` 里时，裸敲 `cargo` / `git` 会解析到各自的原生 exe 并绕过闸门；`tools\bin\cargo.cmd` 依赖 PowerShell 7（`pwsh`）。

### 4.4 git 写操作串行

- 与编译共用同一把 `Global\SkiffBuild` 令牌，同样必须经包装入口 `tools\bin\git.cmd` 执行。
- 写命令（`add` / `commit` / `merge` / `rebase` / `reset` / `checkout` / `switch` / `restore` / `stash` / `gc` / `prune` / `mv` / `rm` / `tag` / `push` / `fetch` 等）拿不到令牌即退出码 2，不排队；只读命令（`status` / `log` / `diff` / `show` 等）不占令牌，但会在 `gate.log` 记一条 `AUDIT git-read`。
- 仓库已关闭后台自动维护（`gc.auto=0`、`maintenance.auto=false`），避免无人值守时重建 `packed-refs` / 写 `maintenance.lock`；需要回收时人工在闸门里跑一次 `git gc`。
- 提交与推送只由唯一负责会话执行；其它会话只读，不并发写 refs，避免 stale lock 与 `AUTO_MERGE` / `rebase-merge` 残留。
