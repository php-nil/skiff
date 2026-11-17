# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-17 13:00 就地覆盖安装 `ab70234` Release 资产（当前生效；库/配置/插件/站点全保留）**
> - 背景：`feat(ui): 三级菜单改横向菜单（非面包屑），站点/运行环境详情接 L3 子视图`（`ab70234`）出包：删掉上一轮的三级路径条，改页面内横向三级菜单（有子视图才出现、底色取内容区、吸顶遮罩）；站点详情拆 6 个、运行环境详情拆 6 个子视图；插件 L3 改由 `/ui/v1/menu` 的 `views` 驱动。本机 Windows 无 C 编译器，编译与全量单测由 CI 承担。
> - CI：`release.yml` dispatch run `44018296064`（head `ab70234`）success，资产覆盖 `v0.1.0`；tag 未移动。
> - 资产：`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **12,144,387 B** `8c53a1eb35e8a519363bedadbf95350190fbde9fc7a64fdf9e07f50327df5580`；`checksums.txt` 434 B digest `01e7caec085a722411571193a5d1320211dcd2df7754d35b9248a35b60c59225`。开发机走 GitHub API 资产端点（`Accept: application/octet-stream`）取回，本地 sha256 / 侧车 `.sha256` / `checksums.txt` 三方一致；`scp` 到 `/root/iter/rel-ab70234/` 后 `sha256sum -c` OK。
> - 安装：解包后 `sh install.sh --no-docker --tls off --listen 127.0.0.1:18080`，**exit 0**（覆盖安装而非清库重装）。库 `user_version=21`、`integrity_check=ok`；配置 `sha256=5bfb00d9552e9889…e16af8` 未变；权限 `711 /www/skiff`、`711 plugins/` 保持。
> - 新二进制：`/usr/local/bin/skiff` **19,851,024 B** `73a34d776a3b98660ce9ddf3…`（旧件 `2477dcd9…8b212` / 19,855,120 B，本轮体积 −4,096 B）。
> - 配置（形态三）：`listen=127.0.0.1:18080`、`allow_insecure_http = true`、`tls_enabled = false`、`allow_reduced_sandbox = true`。`skiff-http-fwd.service` active，`/healthz` 18080 与 18081 均 200。
> - 面板：入口前缀 **`cb5mgdk7` 未变**（启动日志 13:00:24 复述），管理员 `admin` / `SkiffPass2026x`（`/root/iter/admin-pw.txt` 仍 14 B / 0600，覆盖安装未改口令；浏览器实测可登录）。
> - 插件：7 个官方插件覆盖安装后仍在（`cron/files/firewall/ftp/guard/monitor/testkit`）；插件 L3 由 `/ui/v1/menu` 的 `views` 驱动，主机监控四视图实测可切。
> - 现场：站点 `a.com`（运行环境 `php-8.5`，运行中）；Web 服务已装成（`skiff-web-nginx` running）；运行环境 1 个（`php-8.5` 8.5.12）。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此
「某轮的资产 sha」只在该轮有效；要对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表，不要在这里堆流水账。

- **当前资产来源**：Release run `44018296064`（dispatch `ref=main`，head `ab70234`，`publish=true`、`release_tag=v0.1.0`），success。当前生效一轮的包大小/摘要与二进制摘要见 §5。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`2847580`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果，不是异常。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `checksums.txt` | 434 B | `01e7caec085a722411571193a5d1320211dcd2df7754d35b9248a35b60c59225` | `757660215` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,144,387 B | `8c53a1eb35e8a519363bedadbf95350190fbde9fc7a64fdf9e07f50327df5580` | `757660251` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | 111 B | （侧车，内容同上摘要） | `757660212` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 11,631,477 B | `788a7a95a184c7f5e4f9eb471083ff7ace93a93816a22a4e4c77d1d24b5150d3` | `757660249` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz.sha256` | 112 B | （侧车） | `757660221` |
| `skiff-server-x86_64-unknown-linux-musl` | 3,936,992 B | `ade332ee8245ce2c8816844d6764546bd198d0bdbae2c06c374746c1bb3c6a8a` | `757660209` |
| `skiff-server-aarch64-unknown-linux-musl` | 3,182,736 B | `1b2358d6b9dcdac68bb879c12d0206d5a7a9fdd0a6253e67a98eaba79beb3652` | `757660220` |

- **下载与核对**：对外端点 `https://github.com/php-nil/skiff/releases/download/v0.1.0/<资产名>`；开发机走内网代理 `http://10.66.10.22:1010` 打 `api.github.com/repos/php-nil/skiff/releases/assets/<id>` + `Accept: application/octet-stream`（缓存 `%TEMP%\skiff-ops\rel-<sha7>\`）。装机前必须**本地 sha256 / 侧车 `.sha256` / `checksums.txt` / API `digest` 四方一致**，`scp` 到测试机后再 `sha256sum -c`。
- **包内容**：`official/` 含 7 个官方插件包（`com.skiff.{cron,files,firewall,ftp,guard,monitor,testkit}`）+ `public-key.txt`；aarch64 两件本轮未实机验证（sha 取自同轮 `checksums.txt`）。
