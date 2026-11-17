# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-18 06:10 就地覆盖安装 `36840f2` Release 资产（当前生效；库/配置/插件/站点全保留）**
> - 背景：`fix(firewall): 端口与 fail2ban 数值参数兼容表单字符串`（`36840f2`）。schema 表单提交的端口是字符串，原实现只认 JSON 数字，UI 点「放行 / 添加规则」会报 `端口需在 1..65535`。本机 Windows 无 C 编译器，编译与全量单测由 CI 承担。
> - CI / Release：`release.yml` dispatch run `44185563501`（head `36840f2`）**success**；资产覆盖 `v0.1.0`，tag 未移动。
> - 资产：`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **12,121,107 B** `7ac7f9cea1fe1fca4d23ced07deb24a609b5f286fb204aea072bdc9c297a321e`（API `digest` / 本地 sha256 / 测试机 `sha256sum` 三方一致）。
> - 安装：解包后 `sh install.sh --no-docker --tls off --listen 127.0.0.1:18080` **exit 0**（覆盖安装）；`skiff.service` 与 `skiff-http-fwd.service` 均 active。
> - 新二进制：`/usr/local/bin/skiff` **19,646,224 B** `2aede7a7f13e65737b18207e0895dd1f3b73825aa8a40650e776d2d13552877a`（上一件 `63641009…`）。
> - 库：`user_version=21`、`integrity_check=ok`；users 1 / plugins 7 / sites 3 / site_runtimes 3 / plugin_grants 66 / fs_quotas 0；配置 `sha256=fad99a3dbcfa13eda538671cb2965919c96ee0a80c3386e940ac5a80499ed958` 未变；权限 `711 /www/skiff`、`711 plugins/` 保持。
> - 面板：入口前缀 **`kyuyjr44`**（本次机器还原后全新安装生成）；管理员 `admin` / `SkiffPass2026x`（`/root/iter/admin-pw.txt`，0600）。`/healthz` 18080 与 18081 均 **200**，登录页 **200**。
> - 插件：7 个官方插件全在；启动日志 `官方插件内容已随发布包更新（同版本重装） plugin=com.skiff.firewall`（防火墙端口修复随包生效）。`com.skiff.files` 授权保留 3 个 `path:` 站点根（php/node/rust）；`com.skiff.ftp` 授权含 `mv/chown/chmod`、无 `find`。
> - 站点与运行环境：`php.local` / `node.local` / `rust.local` 均 `running`；运行环境 `php-8.5`（PHP 8.5.14）、`node-22`（端口 9200）、`rust-1.98`（端口 9100）；三站点经 Host 头直连 OpenResty 均 **200**。Web 服务（OpenResty）已安装。
> - 防火墙 / FTP：nftables 为当前后端，受管表 `inet skiff` 存在（验收后链为空）；fail2ban 已安装 · 运行中；pure-ftpd active，账号 `skiff_php-local_866107650_ftp-php`（home `/www/sites/php.local/index`）。
> - 浏览器实测（1440×900，0 console error）：概览、网站列表、站点详情、Web 服务、文件管理、站点文件、设置、界面设置（5 套预设 / 15 个色卡）、菜单设置、审计、防火墙、FTP 共 12 页全部为新壳；侧栏 27 个条目逐一点击均落在新 UI 路由。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此「某轮的资产 sha」只在该轮有效；对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表。

- **当前资产来源**：Release run `44185563501`（dispatch `ref=main`，head `36840f2`，`publish=true`、`release_tag=v0.1.0`），success。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`36840f2`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,121,107 B | `7ac7f9cea1fe1fca4d23ced07deb24a609b5f286fb204aea072bdc9c297a321e` | `760823956` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | 111 B | （侧车，内容同上摘要） | 同轮上传 |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 同轮 | 见同轮 `checksums.txt` | 同轮上传 |
| `skiff-server-{x86_64,aarch64}-unknown-linux-musl` | 同轮 | 见同轮 `checksums.txt` | 同轮上传 |

- **下载与核对**：开发机走内网代理 `http://10.66.10.22:1010` 打 `api.github.com/repos/php-nil/skiff/releases/assets/<id>` + `Accept: application/octet-stream`（缓存 `%TEMP%\skiff-ops\rel-<sha7>\`）；装机前本地 sha256 / API `digest` 一致，`scp` 到测试机后 `sha256sum` 再核对。
- **包内容**：`official/` 含 7 个官方插件包（`com.skiff.{cron,files,firewall,ftp,guard,monitor,testkit}`）+ `public-key.txt`；aarch64 两件本轮未实机验证。
