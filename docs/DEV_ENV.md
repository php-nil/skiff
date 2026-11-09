# 开发机环境与本地操作限制

> 只写「本机（Windows 开发机）能做什么、踩过哪些坑」；目标机与测试机口径见 [DEPLOY_TEST.md](DEPLOY_TEST.md)、[TEST_ENV.md](TEST_ENV.md)。
> 编译串行、闸门退出码、内存准入、`PATH` 包装这些规则见 [AGENTS.md](../AGENTS.md) §3.1，本文不重复，只补操作层面的经验。

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

## 4. 闸门的两个前置

- 可用物理内存低于 1024 MB 时闸门直接以退出码 3 拒绝：这是准入判断，先释放内存，不要通过调低 `SKIFF_GATE_MIN_AVAILABLE_MB` 把门降下来。
- `tools\bin` 不在 `PATH` 里时，裸敲 `cargo` / `git` 会解析到各自的原生 exe 并绕过闸门；`tools\bin\cargo.cmd` 依赖 PowerShell 7（`pwsh`）。自检与重启后的修复步骤见 [AGENTS.md](../AGENTS.md) §3.1。
