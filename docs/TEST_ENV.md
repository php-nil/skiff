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
| URL | `http://10.10.15.4:18081/<entry_prefix>/login`（**当前为明文 HTTP，仅限内网**） |
| 入口前缀 | 随机 8 位，**每次重装可能变化**；最近记录 `8p9yyvg4`（2026-11-07 干净系统重装，全新库生成） |
| 前缀读取 | 测试机执行 `journalctl -u skiff -n 50 --no-pager \| grep 安全入口`，或登录后查看设置页；不要沿用旧记录直接试 |
| 明文通道 | 2026-11-08 起为关闭 TLS 做视觉复核：`listen=127.0.0.1:18080` + `allow_insecure_http=true`（loopback dev HTTP），对外由 `skiff-http-fwd.service`（`python3 /tmp/skiff-http-fwd.py`，`0.0.0.0:18081 → 127.0.0.1:18080`）TCP 转发。**仅供内网验证，勿对外暴露** |
| TLS 恢复 | 原配置备份 `/root/iter/config-https-20261108.toml`（`listen=0.0.0.0:18080` + `tls_cert/tls_key`）；`cp` 回 `/etc/skiff/config.toml` 并 `systemctl stop skiff-http-fwd && systemctl restart skiff` 即恢复 HTTPS |
| 证书 | `/etc/skiff/tls/fullchain.pem`（0644）、`/etc/skiff/tls/privkey.pem`（0600）；SAN `IP:10.10.15.4, IP:127.0.0.1, DNS:skiff.local`，有效期 10 年（文件保留，未删除） |
| 健康检查 | `GET /healthz` 200；裸 `GET /` 404，根路径只做前缀跳转 |

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
| tag / 构建提交 | `v0.1.0` = `1ead5fe`（Release 由该提交构建；其后仅有文档回填提交，不影响资产）；CI run `42446837173`、Release run `42446842893` 均 success |
| 部署时间 | 2026-11-08 12:38（Asia/Shanghai；在 `8ede9d2` 装出的 v13 库上按 Release 资产覆盖安装） |
| 部署方式 | `v0.1.0` Release 资产 → `sha256sum -c checksums.txt`（双架构均 OK）→ 解压 → `sh install.sh`；输出 `CONFIG_UNCHANGED`，未传本地二进制 |
| 数据库 | `PRAGMA user_version=13`（v12→v13 迁移在 `8ede9d2` 轮完成，本轮覆盖后保持 13；`site_runtimes.origin` 与 seed 行 `socket_path=NULL` 均保留） |
| 服务 | `skiff.service` active、`/healthz` 200；`skiff-http-fwd.service` 运行中 |
| 配置 | `/etc/skiff/config.toml`：`listen=127.0.0.1:18080` + `allow_insecure_http=true`（明文 dev HTTP，安装前后 sha256 不变；TLS 原配置已备份，见 §2）；`allow_reduced_sandbox=true`（landlock/cgroups 未实现，测试机显式降级） |

## 6. Release 资产（v0.1.0 = `1ead5fe`，2026-11-08）

构建来源：Release workflow run `42446842893`（`push` tag `v0.1.0`，`head_sha=1ead5fe`）。
`git merge-base --is-ancestor` 校验：`a22296f`/`8ede9d2`/`3030648`/`1ead5fe` 对 `v0.1.0` 退出码均为 0（全部在 tag 内），构建时 tag 与 `origin/main` 同为 `1ead5fe`。

| 资产 | 大小 | sha256 |
| --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 8,124,749 B | `55caa6d28dce68a978b8f5ec3ca2e486e2a31cc99806ecca16f5a6906c3c6bae` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 7,767,601 B | `dea9b016eab805f0f403ff220ebaabf8b59534f1210861811ac889a437f6c8d7` |

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

## 8. 最近一次全链路验收结论（2026-11-08 `1ead5fe` 重发轮）

**通过**
- 装机与升级：Release 资产双架构 `sha256sum -c` OK；`install.sh` 输出 `CONFIG_UNCHANGED`；`user_version=13`；`site_runtimes` 保留 `p8.5`/`php-8.5`/`visual`/`rust-rust-probe` 为 `instance`，`php-8.3`/`php-8.4` 仍为 `seed` 且 `socket_path=NULL`；`php-8.5` `is_default=1`；`builtin:db-vacuum` 与站点 `visual.test`/`rust.test` 保留；旧前缀 `sk9v13x` 302 → `8p9yyvg4`，`/healthz` 200。
- 插件 iframe P0（本轮修复）：`curl -D-` 确认插件功能页响应头 `X-Frame-Options: SAMEORIGIN`（原为 `DENY`，`entry_guard` 覆盖了 handler 设置）；浏览器实测计划任务菜单页 iframe 渲染出 `builtin:db-vacuum` 行、内置任务无删除按钮；FTP 菜单页渲染出账号表与运行状态。根因是 `entry_guard` 无条件写 XFO，改为 `entry().or_insert(DENY)` 后 handler 的 `SAMEORIGIN` 生效（回归测试 `entry_guard_keeps_handler_x_frame_options`）。
- 登录 `Origin: null`：Chromium 无 Fetch Metadata 时同源表单 POST 带 `Origin: null`，原逻辑按跨站拒绝（403）；修复后回落双提交令牌，浏览器真实点击登录 303 进入 `/8p9yyvg4`，无登录表单残留。
- `postDrawerForm` urlencoded：运行环境详情抽屉「重启」请求为 `application/x-www-form-urlencoded;charset=UTF-8` + `X-Requested-With: fetch`，抽屉提示「操作已完成」（原 multipart 导致 csrf 取不到、整单 403）。
- DOCKER-11：网络页新增「可附加」列（可附加/不可附加），子网、网关、`类型`（外部/内部）、容器挂载均在列；Docker 不可用时不显示「暂无数据」。
- 新建网站下拉：3 个运行中实例可选，默认选中 `p8.5`；`option` 值与 `selected` 分隔正常；无就绪实例时显示引导卡片（沿用上一轮结论）。
- 插件 UI 弹窗：计划任务「新建任务」弹窗打开且顶层 URL 不变（不跳页）；预设 `daily` → `0 3 * * *`；表达式 `*/15 * * * *` 预览「每 15 分钟执行」；内置任务无删除入口。FTP「服务配置」弹窗（2121 / required / guard 开），非法端口 70000 → `监听端口需为 1–65535 的整数`，弹窗保留、字段未清空；「新建 FTP 账号」弹窗列出 `visual.test（PHP）`，密码不一致 → `两次输入的密码不一致`，账号与两次密码均保留。
- 视觉：430/860/1024 下运行环境、Web 服务、新建网站弹窗、任务弹窗、FTP 服务配置与新建账号弹窗均无页面级横向溢出；iframe 内视口 396/826 px，弹窗 364/722 px，`clippedChildren=0`。
- 文件管理：`GET /{p}/files/api/list?path=/www` 200；`POST /{p}/m/core/{p}/files/api/list` 405/404（重复段路由不存在）；受保护路径 403 沿用上一轮结论。
- 其余沿用通过项：PANEL-02/03/04/05/06、运行环境种子接管、Rust 1.99 建站、FTP-06～09 与 TLS/封禁/审计、DOCKER-11 取不到时显示「未知」。

**未闭环（详见 [TEST_LEDGER.md](TEST_LEDGER.md)）**
- WEB-02：`worker_connections=2048` 保存成功但容器 `nginx -t` 仍告警。
- Docker 通用 create / CPU·内存限制表单 / Compose 运行入口仍缺（UI 设计未定，按用户要求留台账）。
- FTP fail2ban 三次封禁：`ignoreself` 使本机无法构造，需第二台客户端。
- aarch64 仅核对资产摘要；1Panel 受限形态、G 组长时观测等无环境项。

**收尾状态（2026-11-08 `1ead5fe` 复验后）**
- 面板：明文 HTTP `http://10.10.15.4:18081/8p9yyvg4` 可用；`skiff.service` active；`skiff-http-fwd.service` 运行中（测试用途）。
- 视觉/功能测试资产：站点 `visual.test`、`rust.test`、PHP 运行环境 `visual`/`p8.5`/`php-8.5`、Rust 运行环境 `rust-rust-probe` 仍在，供后续复测。
- FTP：`pure-ftpd` active、`:2121` 监听，账号 `skiff_visual-test_fa92_ftpuanchor`（用于上传/禁用/改密验证）保留；`fail2ban` jail `skiff-pureftpd.conf` 保留手工 `[sshd] enabled=false`。
- 保留 `/root/iter/admin-pw.txt`、`/root/iter/config-https-20261108.toml`、`/root/iter/backups`、`/root/backups`、`/root/iter/rel-1ead5fe`。


## 9. 数据、备份与清理

| 项 | 路径 / 约定 |
| --- | --- |
| 数据根 | `/www/skiff`（0711）、`/www/sites`、`/www/php`、`/www/runtime` |
| 配置 | `/etc/skiff/config.toml`（0644） |
| 数据库 | `/www/skiff/skiff.db`（0600） |
| 运维备份 | `/root/iter/backups/`、`/root/backups/`；保留 `builtin:db-vacuum` 与运维备份 |
| 清理 | 测试完成后清理临时站点、容器、账号与压测文件，机器恢复可用状态 |
| GitHub 出网 | 内网代理与 git 全局配置见 [ITERATION.md](ITERATION.md) §7.1（不在本文重复） |
