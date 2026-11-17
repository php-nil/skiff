# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-17 19:51 就地覆盖安装 `0816073` Release 资产（当前生效；库/配置/插件/站点全保留）**
> - 背景：`feat(testkit): 功能页迁到随包 schema，壳页新增 RPC 结果图`（`0816073`）出包：testkit 四个视图改随包 schema（壳内原生渲染、无 iframe），`skiffPluginRpc` 新增 `inspect`，壳脚本与样式共用 `SHELL_VERSION`（`v=3`）。本机 Windows 无 C 编译器，编译与全量单测由 CI 承担。
> - CI：`release.yml` dispatch run `44075496980`（head `0816073`）success，资产覆盖 `v0.1.0`；tag 未移动。
> - 资产：`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **12,153,912 B** `5f6bcbc7f8112fb307c8128eb00b1c7c4ec622d48e5400d15e1014375bb1ee38`；`checksums.txt` 434 B digest `24038115140d127679143d18a7100dcb783d9e25668685e259906e323c4695f8`。开发机走 GitHub API 资产端点（`Accept: application/octet-stream`）取回，本地 sha256 / 侧车 `.sha256` / `checksums.txt` 三方一致；`scp` 到 `/root/iter/rel-0816073/` 后 `sha256sum -c` OK。
> - 安装：解包后 `sh install.sh --no-docker --tls off --listen 127.0.0.1:18080`，**exit 0**（覆盖安装）。库 `user_version=21`、`integrity_check=ok`；配置 `sha256=5bfb00d9552e9889…e16af8` 未变；权限 `711 /www/skiff`、`711 plugins/` 保持。
> - 新二进制：`/usr/local/bin/skiff` **19,855,120 B** `075645490e3b1cf873cd021b…`（旧件 `73a34d77…` / 19,851,024 B，本轮 +4,096 B）。
> - 配置（形态三）：`listen=127.0.0.1:18080`、`allow_insecure_http = true`、`tls_enabled = false`、`allow_reduced_sandbox = true`。`skiff-http-fwd.service` active，`/healthz` 18080 与 18081 均 200。
> - 面板：入口前缀 **`cb5mgdk7` 未变**（启动日志 19:51:00 复述），管理员 `admin` / `SkiffPass2026x`（`/root/iter/admin-pw.txt` 仍 14 B / 0600；浏览器实测可登录）。
> - 插件：7 个官方插件覆盖安装后仍在；**`com.skiff.testkit` 由「同版本重装」路径自动升级为 `api = 1.3` + 四个随包 schema**（日志 `官方插件内容已随发布包更新（同版本重装） plugin=com.skiff.testkit`，`/www/skiff/official/com.skiff.testkit-0.1.0.wbpkg` 329,175 B），壳内四视图实测无 iframe。
> - 现场：站点 `a.com`（运行环境 `php-8.5`，运行中）；Web 服务已装成（`skiff-web-nginx` running）；运行环境 1 个（`php-8.5` 8.5.12）。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此
「某轮的资产 sha」只在该轮有效；要对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表，不要在这里堆流水账。

- **当前资产来源**：Release run `44075496980`（dispatch `ref=main`，head `0816073`，`publish=true`、`release_tag=v0.1.0`），success。当前生效一轮的包大小/摘要与二进制摘要见 §5。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`2847580`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果，不是异常。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `checksums.txt` | 434 B | `24038115140d127679143d18a7100dcb783d9e25668685e259906e323c4695f8` | `758831579` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,153,912 B | `5f6bcbc7f8112fb307c8128eb00b1c7c4ec622d48e5400d15e1014375bb1ee38` | `758831577` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | 111 B | （侧车，内容同上摘要） | `758831580` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 11,634,878 B | `ea59fb697881a724fea58474caafdb89560f931c4ec6f8afa17edbe17f74e299` | `758831630` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz.sha256` | 112 B | （侧车） | `758831576` |
| `skiff-server-x86_64-unknown-linux-musl` | 3,936,992 B | `4f371427413d6c43849746a32dcc47ac009dd3268171c2388d16f1a6aa2703d9` | `758831631` |
| `skiff-server-aarch64-unknown-linux-musl` | 3,182,736 B | `741f6aa7cf420f53759c34fca3df00d8db9df5918e318473a5097dfdeb3788dc` | `758831574` |

- **下载与核对**：对外端点 `https://github.com/php-nil/skiff/releases/download/v0.1.0/<资产名>`；开发机走内网代理 `http://10.66.10.22:1010` 打 `api.github.com/repos/php-nil/skiff/releases/assets/<id>` + `Accept: application/octet-stream`（缓存 `%TEMP%\skiff-ops\rel-<sha7>\`）。装机前必须**本地 sha256 / 侧车 `.sha256` / `checksums.txt` / API `digest` 四方一致**，`scp` 到测试机后再 `sha256sum -c`。
- **包内容**：`official/` 含 7 个官方插件包（`com.skiff.{cron,files,firewall,ftp,guard,monitor,testkit}`）+ `public-key.txt`；aarch64 两件本轮未实机验证（sha 取自同轮 `checksums.txt`）。
