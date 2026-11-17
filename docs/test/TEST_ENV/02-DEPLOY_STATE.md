# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-17 08:14 就地覆盖安装 `fc43f3c` Release 资产（当前生效；库/配置/插件/站点全保留）**
> - 背景：`feat(ui): 隐藏右侧顶部通栏改内核三级路径条，网站下四页精修`（`fc43f3c`）出包：壳层 chrome 改造（D17，顶栏隐藏 + 内核三级路径条 + 账号移设置页）+ 网站下四页精修 + Web 服务拆三级子视图 + files 插件 schema 改版。本机 Windows 无 C 编译器（`skiff-store` 及下游不可编），编译与全量单测由 CI 承担。
> - CI：`release.yml` dispatch run `43984373863`（head `fc43f3c`）success，资产覆盖 `v0.1.0`；tag 未移动。**资产内容 = `fc43f3c`**。
> - 资产：`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **12,143,038 B** `fd782a013ef5adba73641e41adfd85b25a0993ba75c653c944bfee183179cd7f`；`checksums.txt` 434 B digest `fcc14475…a9a7`。开发机走 GitHub API 资产端点（内网代理 `10.66.10.22:1010`，`Accept: application/octet-stream`）取回，本地 sha256 / 侧车 `.sha256` / `checksums.txt` / API digest 四方一致；`scp` 到 `/root/iter/rel-fd782a0/` 后 `sha256sum -c` OK。
> - 安装：解包后 `sh install.sh --no-docker --tls off --listen 127.0.0.1:18080`，**exit 0**（覆盖安装而非清库重装）。库 `user_version=21`、`integrity_check=ok`；配置 `sha256=5bfb00d9552e988993816e9298b8714fcc495dfecdea000293e32aea44e16af8`（本轮起记录指纹，覆盖安装不动配置）；权限 `711 /www/skiff`、`711 plugins/`、`600 skiff.db`、`711 /run/skiff`、`660 kernel.sock` 全部保持。
> - 新二进制：`/usr/local/bin/skiff` **19,855,120 B** `2477dcd9d9f6772894345a1fe26e2663ef591ea4ce0c5f0cd7afb3850908b212`（旧件 `5e4aa297…ff2b5` / 19,822,352 B）。
> - 配置（形态三）：`listen=127.0.0.1:18080`、`allow_insecure_http = true`、`tls_enabled = false`、`allow_reduced_sandbox = true`（本机 landlock/cgroups 未实现，装插件必须降级授权）。`skiff-http-fwd.service` active，`/healthz` 18080 与 18081 均 200。
> - 面板：入口前缀 **`cb5mgdk7` 未变**（覆盖安装不动 `security.entry_prefix`），管理员 `admin` / `SkiffPass2026x`（`/root/iter/admin-pw.txt`，14 B / 0600；本轮浏览器实测可登录）。
> - 插件：7 个官方插件覆盖安装后仍全部 `installed + enabled`（`cron/files/firewall/ftp/guard/monitor/testkit`）；files 插件新 schema（`plugins/files/ui/schema/main.json`）随包更新后**已生效**（站点文件页分卡实测通过），manifest 版本仍 `0.1.3` 未升（内容更新按 hash 驱动）。
> - 现场：站点 `a.com`（运行环境 `php-8.5`，运行中——上轮 UI 迭代验收时所建）；Web 服务已装成（`skiff-web-nginx` + openresty/1.31.8.1 running，安装完成 05:55:03）；`skiff-sites`（172.19.0.0/16）为产品网络；运行环境 1 个（`php-8.5` 8.5.12）。


## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此
「某轮的资产 sha」只在该轮有效；要对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表，不要在这里堆流水账。

- **当前资产来源**：Release run `43984373863`（dispatch `ref=main`，head `fc43f3c`，`publish=true`、`release_tag=v0.1.0`），success。当前生效一轮的包大小/摘要与二进制摘要见 §5。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`2847580`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果，不是异常。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `checksums.txt` | 434 B | `fcc14475e8dd5a07a5741060d85409bf5533a5341870df8977c7f4ea56baa9a7` | `756965230` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,143,038 B | `fd782a013ef5adba73641e41adfd85b25a0993ba75c653c944bfee183179cd7f` | `756965253` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | 111 B | `0d4ab001054eb37b9c4b74a8d99423827b407181ef77d21c2dd25bcd78d37d55` | `756965224` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 11,628,804 B | `8509e27f069dc49997fd8495d1c906a5d3fa8a426e9925ad34ee7d0feda27654` | `756965262` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz.sha256` | 112 B | `503269113115764c73cc4001f4d896fc4ce8c37a187af5c93bba54c798d65f2b` | `756965226` |
| `skiff-server-x86_64-unknown-linux-musl` | 3,936,992 B | `ade332ee8245ce2c8816844d6764546bd198d0bdbae2c06c374746c1bb3c6a8a` | `756965228` |
| `skiff-server-aarch64-unknown-linux-musl` | 3,182,736 B | `1b2358d6b9dcdac68bb879c12d0206d5a7a9fdd0a6253e67a98eaba79beb3652` | `756965227` |

- **下载与核对**：对外端点 `https://github.com/php-nil/skiff/releases/download/v0.1.0/<资产名>`；开发机走内网代理 `http://10.66.10.22:1010` 打 `api.github.com/repos/php-nil/skiff/releases/assets/<id>` + `Accept: application/octet-stream`（缓存 `%TEMP%\skiff-ops\rel-<sha7>\`）。装机前必须**本地 sha256 / 侧车 `.sha256` / `checksums.txt` / API `digest` 四方一致**，`scp` 到测试机后再 `sha256sum -c`。
- **包内容**：`official/` 含 7 个官方插件包（`com.skiff.{cron,files,firewall,ftp,guard,monitor,testkit}`）+ `public-key.txt`；aarch64 两件本轮未实机验证（sha 取自同轮 `checksums.txt`）。
