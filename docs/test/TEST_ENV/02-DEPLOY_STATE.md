# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-18 21:15 就地覆盖安装 `d0c8cc4`（当前生效；库/配置/插件/站点/运行环境全保留）**
> - 背景：本轮修三项（登录页回车不提交、PHP 扩展首装超时、Docker 写路径错误分类），Release run `44310262459`（head `d0c8cc4`）success（同提交 CI `44310244598`）。前两轮（`12d8ea2` / `bcdfd22`）见 03-DEPLOY_LOG。
> - 机器状态：还原后为干净系统——无旧二进制/配置/库、18080/18081 空闲、Docker 29.7.2 在位、`/etc/skiff-baseline` 不存在；按 T1 口径（Docker 已装）安装。
> - 资产：`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **11,951,277 B** `fc2269cb2afdf7477eb287c1ffb256c8bc1a7ea261d31f7fb664ce858b6069b2`（API `digest` / 本地 sha256 / 测试机 `sha256sum` 三方一致）。
> - 安装：`install.sh --no-docker --tls off --listen 127.0.0.1:18080` **exit 0**（就地覆盖）。`skiff.service` 与 `skiff-http-fwd.service`（`0.0.0.0:18081 → 127.0.0.1:18080`，脚本 `/root/iter/skiff-http-fwd.py`）均 active。
> - 二进制：`/usr/local/bin/skiff` **19,146,512 B** `d7a49eaa4805dcf1cffa1c42388faefe4b15f06a39d58655c197edf0ddb11f53`。
> - 库：`user_version=21`、`integrity_check=ok`；users 1 / plugins 7 / sites 2（`uiaccept.local` 静态、`matrix.local` PHP）/ site_runtimes 1（`php-8.5` running）。配置 `sha256=5bfb00d9552e988993816e9298b8714fcc495dfecdea000293e32aea44e16af8` 升级前后逐字节未变；权限 `711 /www/skiff`、`711 plugins/`、`600 skiff.db`、`711 /run/skiff` 保持。
> - 面板：入口前缀 **`2u3uiv85`**；管理员 `admin` / `skiff-accept-1795001381`（`/root/iter/admin-pw.txt`，0600；accept.sh B3 改密后按审计 `ts` 取回）。`/healthz` 18080 与 18081 均 **200**，登录页 **200**。
> - 插件：首装不自动装官方插件，需 `skiff plugin install /www/skiff/official/*.wbpkg --yes`（先 `allow_reduced_sandbox=true`，否则全部被「要求完整沙箱」拒装）；7 个全部装成，升级后随包同版本重装。
> - 站点与 Web：Web 服务经 `POST /ui/v1/runtimes/web/action/install` 装成（`skiff-web-nginx` running）；站点 `uiaccept.local`（静态，根 `/www/sites/uiaccept.local`）+ 别名 `www.uiaccept.local`，Host 直连 80 端口 **200**，停站 **404**、启站 **200**。
> - 浏览器实测（1440×900，真实资产无拦截）：25 页 **0 console error、0 4xx**；壳页脚本为 `budding-1.8.1-r2.min.js?v=1.8.1-r2` + `form-enter-submit.js?v=6` + `budding-boot.js?v=6`（登录页为 `theme-init.js` + `budding-{ver}.min.js` + `form-enter-submit.js?v=6` + `budding-login.js`），以上均 200；`.bud-admin` 计算样式 `border:0 / radius:0`；`/sites`（2 站点）、`/sites/matrix.local`（概览/域名/配置/证书/备份/日志）、`/runtimes`、`/runtimes/php-8.5`（基础信息/配置/扩展/站点/管理/日志）均渲染真实内容、0 console error。

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
