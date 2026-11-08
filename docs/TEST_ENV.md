# 测试环境访问信息（唯一来源）

> 本文是测试机访问信息、当前部署坐标与最近验收结论的**唯一来源**。`ITERATION.md`、`DEPLOY_TEST.md`、`docs/changes/` 不再重复地址、账号、入口前缀、证书与资产摘要，只留相对路径引用。
> 部署约定见 [AGENTS.md](../AGENTS.md) §3.1：**任何一次向测试机部署后，必须更新本文，并在汇报中交代「地址 + 账号」；缺这步视为部署未完成。**

## 1. 测试机

| 项 | 值 |
| --- | --- |
| 地址 | `10.10.15.4`（内网 VM，本机可直达） |
| 系统 | Debian GNU/Linux 13 (trixie)，内核 `6.12.113+deb13-amd64` |
| 规格 | 4 vCPU / 3.9 GB 内存（可用约 3.5 GB）/ 120 GB 根分区 |
| 用途 | 干净系统装机、升级、插件与能力面验收；**不在测试机上编译** |
| 并行约束 | 多会话共用；涉及改 admin 口令、TOTP、重启服务、卸载、重置库等操作时使用同机隔离实例，避免影响其他并行任务 |
| 回滚 | 人工全量回滚到 T0/T1，按 [ITERATION.md](ITERATION.md) §4.1 执行 |

## 2. 面板访问

| 项 | 值 |
| --- | --- |
| URL | `http://10.10.15.4:18081/<entry_prefix>/login`（**当前为形态三：直接明文 HTTP，仅限内网**） |
| 入口前缀 | 随机 8 位，**每次重装可能变化**；最近记录 `8p9yyvg4`（2026-11-07 干净系统重装，全新库生成，本轮未变） |
| 前缀读取 | 测试机执行 `journalctl -u skiff -n 50 --no-pager \| grep 安全入口`，或登录后查看设置页；不要沿用旧记录直接试 |
| 明文通道（形态三，当前） | `listen=127.0.0.1:18080` + `allow_insecure_http=true` + `tls_enabled=false`（loopback dev HTTP），对外由 `skiff-http-fwd.service`（`python3 /tmp/skiff-http-fwd.py`，`0.0.0.0:18081 → 127.0.0.1:18080`）TCP 转发；Cookie 名 `skiff_dev_session`（无 `Secure`）。**仅供内网验证，勿对外暴露** |
| 证书 | `/etc/skiff/tls/fullchain.pem`（0644）、`/etc/skiff/tls/privkey.pem`（0600）；SAN `IP:10.10.15.4, IP:127.0.0.1, DNS:skiff.local`，有效期 10 年（文件保留，未删除） |
| 健康检查 | `GET /healthz` 200；裸 `GET /` 404，根路径只做前缀跳转 |

### 2.1 三种 TLS 形态与回切路径（2026-11-08 已逐形态验证）

| 形态 | 生效配置 | 访问地址 | 备份与回切 |
| --- | --- | --- | --- |
| 一：TLS 直连（`install.sh --tls on`） | `listen="0.0.0.0:18080"`、`tls_enabled=true`、`tls_cert=/etc/skiff/tls/fullchain.pem`、`tls_key=/etc/skiff/tls/privkey.pem` | `https://10.10.15.4:18080` | `/root/iter/config-tls-20261108-c.toml`（本轮 9aa0381 复验版；`-b` 同形，旧版 `config-tls-20261108.toml`）。回切：`cp` 覆盖 `/etc/skiff/config.toml` → `systemctl stop skiff-http-fwd skiff-tls-proxy` → `systemctl restart skiff` |
| 二：反代终止 TLS（`trusted_proxies`） | `listen="0.0.0.0:18080"`、`allow_insecure_http=true`、`trusted_proxies=["127.0.0.1/32"]`、`tls_enabled=false` | `https://10.10.15.4:18443`（`skiff-tls-proxy.service`，脚本 `/root/iter/skiff-tls-proxy.py`，转发 `127.0.0.1:18080`） | `/root/iter/config-proxy-20261108-b.toml`（本轮；无后缀 `config-proxy-20261108.toml` 同形）。回切：`cp` → `systemctl stop skiff-http-fwd` → `systemctl start skiff-tls-proxy` → `systemctl restart skiff` |
| 三：直接明文（当前） | `listen="127.0.0.1:18080"`、`allow_insecure_http=true`、`tls_enabled=false` | `http://10.10.15.4:18081`（`skiff-http-fwd.service` 转发） | `/root/iter/config-dev-20261108-c.toml`（当前生效；`-b` 同形）。回切：`cp` → `systemctl stop skiff-tls-proxy` → `systemctl start skiff-http-fwd` → `systemctl restart skiff` |

- 旧自签 HTTPS 直连配置另存 `/root/iter/config-https-20261108.toml`（`listen=0.0.0.0:18080` + `tls_cert/tls_key`），需要时可直接回切。
- 三形态切换均只需覆盖配置 + 重启 `skiff`，实测来回 3→1→3 均恢复 `/healthz` 200，无残留监听。
- Cookie 断言：形态一/二为 `__Host-skiff_session`（`Secure; HttpOnly; SameSite=Lax`）；形态三为 `skiff_dev_session`（`HttpOnly; SameSite=Lax`，无 `Secure`）。
- 形态二下直连 `:18080` 明文端口：`GET /<prefix>/login` 仍 200 但不写会话，`POST /<prefix>/login` 返回 403「跨站登录请求被拒绝」；`curl -k https://<host>:18080/healthz` 返回 000（该端口无 TLS）。

## 3. 账号与口令

| 项 | 值 / 读取方式 |
| --- | --- |
| 管理员账号 | `admin` |
| 口令文件 | `/root/iter/admin-pw.txt`（`0600`）；文档不保存明文口令 |
| 读取方式 | SSH 登录后 `cat /root/iter/admin-pw.txt`；重置/轮换口令后必须同步更新该文件 |
| 首登改密 | 测试期默认 `must_change_password=0`，初始口令可直接登录；强制改密流程见 [ITERATION.md](ITERATION.md) §5.2.1 |

## 4. SSH

| 项 | 值 |
| --- | --- |
| 连接 | `ssh -i ~/.ssh/skiff-iter root@10.10.15.4`（端口 22） |
| 私钥 | `~/.ssh/skiff-iter`（已生成） |
| 公钥 | `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM6Y+VSKG/QjciBaXQIObQwyg99CEEV+YKhJ5Op68Nrh skiff-iter@DESKTOP-5P3Q9PS` |
| 前置顺序 | 公钥必须先加入 `authorized_keys`，再打基线快照；顺序反了回滚后即失联 |
| 密码回退 | 不在文档留存；需要时由机器所有者提供，或走 `SSH_ASKPASS` 一次性注入 |

## 5. 当前部署

| 项 | 值 |
| --- | --- |
| 版本 | `v0.1.0` |
| tag / 构建提交 | `v0.1.0` = `9aa0381`（Release 由该提交构建；`origin/main` 与 HEAD 同为 `9aa0381`）；CI run `42452614639`、Release run `42452619537` 均 success |
| 部署时间 | 2026-11-08（Asia/Shanghai；TLS 三形态轮，在 `1ead5fe` 装出的 v13 库上按 Release 资产覆盖安装） |
| 部署方式 | `v0.1.0`（`9aa0381`）Release 资产 → `sha256sum -c checksums.txt`（双架构均 OK）→ 解压 → `sh install.sh --no-docker --listen 0.0.0.0:18080 --tls on --tls-cert … --tls-key …`；未传本地二进制 |
| 数据库 | `PRAGMA user_version=13`（覆盖安装后保持 13；站点、运行环境、内置任务均保留） |
| 服务 | `skiff.service` active、`/healthz` 200；`skiff-http-fwd.service` active（形态三对外转发）；`skiff-tls-proxy.service` inactive（形态二验证用，脚本保留） |
| 配置 | `/etc/skiff/config.toml`：`listen=127.0.0.1:18080` + `allow_insecure_http=true` + `tls_enabled=false`（形态三明文 dev HTTP；sha256 `0103e75af4cc497d697be2baeedb9da349f006079a888517ed2c0b0298f053e1`）；`allow_reduced_sandbox=true`（landlock/cgroups 未实现，测试机显式降级）。三形态备份见 §2.1 |

## 6. Release 资产（v0.1.0 = `9aa0381`，2026-11-08）

构建来源：Release workflow run `42452619537`（`push` tag `v0.1.0`，`head_sha=9aa0381`；CI run `42452614639` success）。
`git merge-base --is-ancestor` 校验：`1ead5fe`/`46848d7`/`8ede9d2`/`3030648` 对 `v0.1.0` 退出码均为 0（全部在 tag 内），构建时 tag、`origin/main` 与 HEAD 同为 `9aa0381`。资产摘要经 GitHub API 复核。

| 资产 | 大小 | sha256 |
| --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 8,124,497 B | `244ab407398f0429e52bd11efe4d2b9da06834296e18753bc3f0933ea178a4ee` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 7,768,972 B | `bd9993b61cfa732475eb6f8b227d2fb662623f190e372c3483faf611fdae5cf4` |

- 下载端点：`https://github.com/php-nil/skiff/releases/download/v0.1.0/<资产名>`；测试机直连 `objects.githubusercontent.com` 会超时，用 `curl --retry 5 --retry-all-errors` 或在资产端点重试。
- 包内 `official/` 含 6 个 `.wbpkg`（guard / monitor / files / cron / ftp / testkit）与 `public-key.txt`；`checksums.txt` 逐个校验。
- GitHub Release **说明正文尚未列 `ftp`**（正文仍写 4 个插件）；以包内 `official/` 与本文为准，待人工修正 Release 说明。

## 7. 最近部署记录

| 日期 | 场景 | 版本 / 坐标 | 结果 |
| --- | --- | --- | --- |
| 2026-11-05 | 重置库后全新安装（schema v7） | x86_64 6,987,420 B，`sha256:25559d7ec362625cee4fb608710aa842049447f7c76588cfb921c8e348988862` | 安装成功；新口令写入 `/root/iter/admin-pw.txt`；三类站与运行环境验收通过 |
| 2026-11-07 早轮 | Docker / FTP 修复轮 | 资产 `f9a0b30`，x86_64 `sha256:87427aa6…` | DOCKER-01～08 复验通过；该资产随后被同日最终版覆盖 |
| 2026-11-07 | v0.1.0 最终版覆盖安装 | tag/构建 `c44eeb5`；x86_64 `b16cb72e…` | FTP 主链路、DEPLOY-02/07/08 通过；详见 §8 |
| 2026-11-07 22:05 | 干净系统全新装机（Release 资产） | tag/构建 `c44eeb5`；x86_64 8,020,711 B `b16cb72e…`，`sha256sum -c` OK | 全新库 `user_version=11`；入口前缀 `8p9yyvg4`；6 个官方插件 CLI 安装成功；`doctor` 全 ok、`/healthz` 200 |
| 2026-11-08 00:18 | v11→v12 升级 + 最终版复验（覆盖安装） | 旧 `c44eeb5` → 新 `a22296f`；x86_64 8,027,834 B `2e8aa3d5…`，`sha256sum -c` OK | 库自动升 12、原任务保留；FTP、PHP 扩展、删站级联逐项通过 |
| 2026-11-08 08:30 | 关闭 TLS 切明文 HTTP + 浏览器视觉复测 | 无重装：`listen=127.0.0.1:18080` + `allow_insecure_http=true`，`skiff-http-fwd` 转发 `0.0.0.0:18081 → 127.0.0.1:18080` | `/healthz` 200、`/<prefix>/login` 200；完成 430/860/1024 视觉项与插件 iframe 复核 |
| 2026-11-08 上午 | v12→v13 升级轮（覆盖安装） | 资产 `8ede9d2`；x86_64 8,123,372 B `3c2d498c…`，`sha256sum -c` OK | `user_version=13`；`site_runtimes.origin` 落 `instance`/`seed`，seed 行 `socket_path` 清空；种子行可被同名新建接管并继承 `is_default` |
| 2026-11-08 12:38 | `1ead5fe` 重发版覆盖安装 + 复验 | 资产 `1ead5fe`；x86_64 8,124,749 B `55caa6d2…`，双架构 `sha256sum -c` OK | 库保持 13、任务/站点/运行环境全部保留；插件 iframe P0、登录 `Origin: null`、抽屉表单、网络页「可附加」、新建网站下拉均复验通过 |
| 2026-11-08（三形态轮） | `9aa0381` 覆盖安装 + TLS 三形态 / 视觉 / 工作台复验 | 资产 `9aa0381`；x86_64 8,124,497 B `244ab407…`、aarch64 7,768,972 B `bd9993b6…`，双架构 `sha256sum -c` OK | 库保持 13；三形态逐项通过（Cookie 名/属性、浏览器登录、切回）；430/860/工作台通过；WAF 核查为「功能不存在」；详见 §8 |

## 8. 最近一次全链路验收结论（2026-11-08 `9aa0381` 三形态轮）

**本轮通过（三形态轮）**
- 装机：`9aa0381` 资产双架构 `sha256sum -c` OK；`install.sh --no-docker --listen 0.0.0.0:18080 --tls on --tls-cert … --tls-key …` 退出码 0；配置落 `tls_enabled=true`、`tls_cert=/etc/skiff/tls/fullchain.pem`、`tls_key=/etc/skiff/tls/privkey.pem`（修复前 `tls_key` 曾被函数参数覆盖成证书路径）；`user_version=13`、`NRestarts=0`。
- 形态一（TLS 直连 `:18080`）：`/healthz` 200（loopback 与 `10.10.15.4` 均通）；表单登录 303 + `set-cookie: __Host-skiff_session=…; Path=/; HttpOnly; SameSite=Lax; Max-Age=43200; Secure`；认证后 `/8p9yyvg4`（概览）、`/containers`、`/plugins`、`/settings` 均 200；Chromium（忽略证书错误）真实登录成功，Cookie `secure=true; httpOnly=true; SameSite=Lax`。
- 形态二（反代终止 TLS）：`trusted_proxies=["127.0.0.1/32"]` + `allow_insecure_http=true`；经 `https://10.10.15.4:18443` 登录 303 + 同名同属性 `__Host-skiff_session`（Secure），认证后页面 200，浏览器登录可用；直连明文 `:18080` 的 `POST /<prefix>/login` 返回 403「跨站登录请求被拒绝」，`curl -k https://<host>:18080/healthz` 为 000（该端口无 TLS）。
- 形态三（直接明文 `tls_enabled=false`）：经 `http://10.10.15.4:18081` 登录 303 + `skiff_dev_session=…; HttpOnly; SameSite=Lax`（无 `Secure`、无 `__Host-`），认证后页面 200；改密路径 `POST /8p9yyvg4/password`（旧口令错误）返回 303 `settings?err=当前口令不正确`，表单校验链路可用；浏览器登录成功并显示「明文 HTTP 开发模式」提示。
- 三形态往返：3→1→3 各覆盖配置 + `systemctl restart skiff`，两端 `/healthz` 均 200，Cookie 名随形态正确切换，无残留监听；备份路径见 §2.1。
- 430px：概览、应用（containers）、插件中心三页 `document.documentElement.scrollWidth=430`、无子元素越界（`bad=0`）；容器列表 `.table-wrap` `overflowX=auto`（`scrollWidth=668 / clientWidth=364`），修复随 `9aa0381` 生效。
- 860px：菜单抽屉初始 `x=0 w=0 aria=false` → 展开 `x=16 w=828 aria=true` → 再收起归零；「网站」分组展开 `items=3`，点二级菜单进入 `/m/core/sites/list`（标题「网站列表 · Skiff」）。
- 工作台（cron）：两个标签 `plugin/com.skiff.cron/jobs/list*` 与 `…/jobs/new`；`.workbench-tabs` 吸顶 `position=sticky top=0px`，页头滚出后 `rectTop` 由 126 变 0（`scrollY=589` 仍吸顶）；点「新建」URL 变为 `…/jobs/new`、活动标签切换、iframe `src` 指向 `new.html?...&tab=new`，切回列表恢复 `list.html?tab=list`。
- WAF：**功能不存在**。代码仅有预留（`crates/app/src/sites.rs:170` `KEY_WAF_MODE`、`:288-289` `waf_rules_dir`，`crates/core/src/bootstrap.rs:210` 注释「WAF 规则目录仅预留，本期不加载、不拦截」），无模板、无 UI、无规则加载与拦截逻辑，无法构造可触发拦截的请求；不按缺陷处理。

**沿用通过（`1ead5fe` 轮，本轮未回归）**
- 插件 iframe `X-Frame-Options: SAMEORIGIN`（`entry_guard` 改 `entry().or_insert(DENY)`）、登录 `Origin: null` 回落双提交令牌、`postDrawerForm` 改 urlencoded、DOCKER-11 网络页「可附加」列、新建网站运行环境下拉、插件 UI 弹窗校验、430/860/1024 弹窗无溢出、文件管理绝对路径 API、PANEL-02～06、运行环境种子接管、Rust 1.99 建站、FTP-06～09 与封禁/审计。

**未闭环（详见 [TEST_LEDGER.md](TEST_LEDGER.md)）**
- WAF 拦截能力未实现（仅配置字段与目录预留），需产品定稿规则来源与开关后再验。
- WEB-02：`worker_connections=2048` 保存成功但容器 `nginx -t` 仍告警。
- Docker 通用 create / CPU·内存限制表单 / Compose 运行入口仍缺（UI 设计未定，按用户要求留台账）。
- FTP fail2ban 三次封禁：`ignoreself` 使本机无法构造，需第二台客户端。
- aarch64 仅核对资产摘要；1Panel 受限形态、G 组长时观测等无环境项。

**收尾状态（2026-11-08 `9aa0381` 三形态轮后）**
- 面板：形态三明文 `http://10.10.15.4:18081/8p9yyvg4` 可用；`skiff.service` active；`skiff-http-fwd.service` active（`systemd-run` 临时单元）；`skiff-tls-proxy.service` inactive（`systemd-run` 临时单元，脚本 `/root/iter/skiff-tls-proxy.py` 保留）。
- 视觉/功能测试资产：站点 `visual.test`、`rust.test`、PHP 运行环境 `visual`/`p8.5`/`php-8.5`、Rust 运行环境 `rust-rust-probe` 仍在，供后续复测。
- FTP：`pure-ftpd` active、`:2121` 监听，账号 `skiff_visual-test_fa92_ftpuanchor`（用于上传/禁用/改密验证）保留；`fail2ban` jail `skiff-pureftpd.conf` 保留手工 `[sshd] enabled=false`。
- 保留 `/root/iter/admin-pw.txt`、`/root/iter/tls-src/`、三形态配置备份、`/root/iter/rel-1ead5fe`、`/root/iter/rel-9aa0381`、`/root/iter/backups`、`/root/backups`。


## 9. 数据、备份与清理

| 项 | 路径 / 约定 |
| --- | --- |
| 数据根 | `/www/skiff`（0711）、`/www/sites`、`/www/php`、`/www/runtime` |
| 配置 | `/etc/skiff/config.toml`（0644） |
| 数据库 | `/www/skiff/skiff.db`（0600） |
| 运维备份 | `/root/iter/backups/`、`/root/backups/`；保留 `builtin:db-vacuum` 与运维备份 |
| 清理 | 测试完成后清理临时站点、容器、账号与压测文件，机器恢复可用状态 |
| GitHub 出网 | 内网代理与 git 全局配置见 [ITERATION.md](ITERATION.md) §7.1（不在本文重复） |
