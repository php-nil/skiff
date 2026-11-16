# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-17 00:06 就地覆盖安装 `2847580` Release 资产（当前生效；库/配置/插件/站点全保留）**
> - 背景：`test(docker): 更新 409 用例断言并去重`（`2847580`）出包，含上一提交 `fb280ea` 的 Docker 409 结构化（`domain.conflict`）。本机 Windows 无 C 编译器（`skiff-store` 及下游不可编），编译与全量单测由 CI `43899773015` 承担，12 个 job 全绿（`编译（x86_64 musl 静态）` 按设计仅手动）。
> - CI：`release.yml` dispatch run `43902115907`（head `2847580`）四 job 全绿，资产覆盖 `v0.1.0`；tag 未移动。dispatch 后 `main` 只多了文档与 `deploy/accept.sh`（验收脚本不进安装包），**资产内容 = `2847580`**。
> - 资产：`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **12,130,261 B** `f90676b4a0aaad2ed0f0510b0e481078927a54e5ecda5a135cc396ce8eb86894`；`checksums.txt` 434 B `604dde76…b9a3`。开发机走 GitHub API 资产端点（内网代理 `10.66.10.22:1010`，`Accept: application/octet-stream`）取回，本地 sha256 / 侧车 `.sha256` / `checksums.txt` / API digest 四方一致；`scp` 到 `/root/iter/rel-2847580/` 后 `sha256sum -c` OK。
> - 安装：解包后 `sh install.sh --no-docker --tls off --listen 127.0.0.1:18080`，**exit 0**（覆盖安装而非清库重装，用于同时验证升级路径）。配置 `/etc/skiff/config.toml` 与安装前备份 `diff` 无差异；库 `user_version=21`、`integrity_check=ok`、`users=[('admin','admin')]`；权限 `711 /www/skiff`、`711 plugins/`、`600 skiff.db`、`711 /run/skiff`、`660 kernel.sock` 全部保持。
> - 新二进制：`/usr/local/bin/skiff` **19,822,352 B** `5e4aa2978c1b0689eb42bd711bf64db366a541afa0785c3d8531b823909ff2b5`（旧件 `0d82bc95…581b`，同尺寸不同内容——本轮只改错误映射与用例）。
> - 配置（形态三）：`listen=127.0.0.1:18080`、`allow_insecure_http = true`、`tls_enabled = false`、`allow_reduced_sandbox = true`（本机 landlock/cgroups 未实现，装插件必须降级授权）。`skiff-http-fwd.service` active，`/healthz` 18080 与 18081 均 200。
> - 面板：入口前缀 **`cb5mgdk7` 未变**（覆盖安装不动 `security.entry_prefix`），管理员 `admin` / `SkiffPass2026x`（`/root/iter/admin-pw.txt`，14 B / 0600）。本轮 B3c 改密后按 AUTH-07 改回：因密码已轮换为未知值，采用「停服 → 清 `sessions`/`users` → 启服由 bootstrap 重建 admin 并写 `initial-admin.txt` → AUTH-07 改回 `SkiffPass2026x`」，改密成功后产品自动删除 `initial-admin.txt`（已确认不存在）。
> - 插件：7 个官方插件覆盖安装后仍全部 `installed + enabled`（`cron/files/firewall/ftp/guard/monitor/testkit`，随包版本校验通过）。
> - 现场：未建站点、无 `skiff-web-nginx` 容器；`skiff-sites`（172.19.0.0/16）为产品网络。验收用的 `regress409-net` 网络与 `regress409-vol` 卷均已删除，`docker network ls` 仅 `bridge/host/none/skiff-sites`、`docker volume ls` 为空。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此
「某轮的资产 sha」只在该轮有效；要对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表，不要在这里堆流水账。

- **当前资产来源**：Release run `43902115907`（dispatch `ref=main`，head `2847580`，`publish=true`、`release_tag=v0.1.0`），四 job 全绿。当前生效一轮的包大小/摘要与二进制摘要见 §5。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`2847580`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果，不是异常。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `checksums.txt` | 434 B | `604dde76ea5372bd5c877723afb49d99feecdbe2d3be7e64e8d34d6cca1bc9a3` | `755483968` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,130,261 B | `f90676b4a0aaad2ed0f0510b0e481078927a54e5ecda5a135cc396ce8eb86894` | `755483967` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | 111 B | `0a38d7ef875236d38b72cb80e15e94105c04b176dbc8a5bddbbcd735bcc22c69` | `755483970` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 11,617,637 B | `b0af7cb63b1b37d9fd73fd2141961b85b4c556d4218bc13a1a7f1c2ba2445571` | `755484019` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz.sha256` | 112 B | `5616a820fdf42f15ac190e3399ba7b7e86f6f12df2b11ce8a3f9fbd90510db96` | `755483971` |
| `skiff-server-x86_64-unknown-linux-musl` | 3,936,992 B | `ade332ee8245ce2c8816844d6764546bd198d0bdbae2c06c374746c1bb3c6a8a` | `755483966` |
| `skiff-server-aarch64-unknown-linux-musl` | 3,182,736 B | `1b2358d6b9dcdac68bb879c12d0206d5a7a9fdd0a6253e67a98eaba79beb3652` | `755484020` |

- **下载与核对**：对外端点 `https://github.com/php-nil/skiff/releases/download/v0.1.0/<资产名>`；开发机走内网代理 `http://10.66.10.22:1010` 打 `api.github.com/repos/php-nil/skiff/releases/assets/<id>` + `Accept: application/octet-stream`（缓存 `%TEMP%\skiff-ops\rel-<sha7>\`）。装机前必须**本地 sha256 / 侧车 `.sha256` / `checksums.txt` / API `digest` 四方一致**，`scp` 到测试机后再 `sha256sum -c`。
- **包内容**：`official/` 含 7 个官方插件包（`com.skiff.{cron,files,firewall,ftp,guard,monitor,testkit}`）+ `public-key.txt`；aarch64 两件本轮未实机验证（sha 取自同轮 `checksums.txt`）。
