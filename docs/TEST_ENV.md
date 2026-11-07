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
| URL | `https://10.10.15.4:18080/<entry_prefix>/login` |
| 入口前缀 | 随机 8 位，**每次重装可能变化**；最近记录 `8p9yyvg4`（2026-11-07 干净系统重装，全新库生成） |
| 前缀读取 | 测试机执行 `journalctl -u skiff -n 50 --no-pager \| grep 安全入口`，或登录后查看设置页；不要沿用旧记录直接试 |
| TLS | 自签证书，同端口 HTTPS；HTTP 访问同端口不降级 |
| 证书 | `/etc/skiff/tls/fullchain.pem`（0644）、`/etc/skiff/tls/privkey.pem`（0600）；SAN `IP:10.10.15.4, IP:127.0.0.1, DNS:skiff.local`，有效期 10 年 |
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
| tag / 构建提交 | `v0.1.0` = `a22296f`（Release 由该提交构建，资产与之一一对应） |
| 部署时间 | 2026-11-08 00:18（Asia/Shanghai；在旧资产 `c44eeb5` 装出的 v11 库上覆盖安装） |
| 部署方式 | 旧资产 `c44eeb5` 先装出 `user_version=11` 并留 2 条任务 → 新资产按 `sha256sum -c` → 解压 → `sh install.sh` 覆盖（同一轮既走升级路径又复验四处修复）；6 个官方插件随包同步 |
| 数据库 | `PRAGMA user_version=12`（v11→v12 自动迁移，`idx_scheduled_tasks_kind_source` 已删） |
| 服务 | `skiff.service` active、`NRestarts=0`、`/healthz` 200 |
| 配置 | `/etc/skiff/config.toml`：`listen=0.0.0.0:18080` + TLS 自签、`allow_reduced_sandbox=true`（landlock/cgroups 未实现，测试机显式降级） |

## 6. Release 资产（v0.1.0 = `a22296f`，2026-11-07 最终版）

| 资产 | 大小 | sha256 |
| --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 8,027,834 B | `2e8aa3d5afb18acc5e43bc96a9e721ade8b9c3dd946dd016ceea26cadff98b4e` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 7,674,270 B | `ac517feef6393c6e1966b2890de4116eaf2f33d8711c83c59049e833926d0a43` |

- 下载端点：`https://github.com/php-nil/skiff/releases/download/v0.1.0/<资产名>`；测试机直连 `objects.githubusercontent.com` 会超时，改用 GitHub API 资产端点或本机代理下发。
- 包内 `official/` 含 6 个 `.wbpkg`（guard / monitor / files / cron / ftp / testkit）与 `public-key.txt`；`checksums.txt` 逐个校验。
- GitHub Release **说明正文尚未列 `ftp`**（正文仍写 4 个插件）；以包内 `official/` 与本文为准，待人工修正 Release 说明。

## 7. 最近部署记录

| 日期 | 场景 | 版本 / 坐标 | 结果 |
| --- | --- | --- | --- |
| 2026-11-05 | 重置库后全新安装（schema v7） | x86_64 6,987,420 B，`sha256:25559d7ec362625cee4fb608710aa842049447f7c76588cfb921c8e348988862` | 安装成功；新口令写入 `/root/iter/admin-pw.txt`；三类站与运行环境验收通过 |
| 2026-11-07 早轮 | Docker / FTP 修复轮 | 资产 `f9a0b30`，x86_64 `sha256:87427aa6…` | DOCKER-01～08 复验通过；该资产随后被同日最终版覆盖 |
| 2026-11-07 | v0.1.0 最终版覆盖安装 | tag/构建 `c44eeb5`；x86_64 `b16cb72e…` | FTP 主链路、DEPLOY-02/07/08 通过；详见 §8 |
| 2026-11-07 22:05 | 干净系统全新装机（Release 资产） | tag/构建 `c44eeb5`；x86_64 8,020,711 B `b16cb72e…`，`sha256sum -c` OK | 全新库 `user_version=11`；入口前缀 `8p9yyvg4`；6 个官方插件 CLI 安装成功（grants 默认含 `path:/www` 与 scheduler 权限）；`doctor` 全 ok、`/healthz` 200 |
| 2026-11-08 00:18 | v11→v12 升级 + 最终版复验（覆盖安装） | 旧 `c44eeb5` → 新 `a22296f`；x86_64 8,027,834 B `2e8aa3d5…`，`sha256sum -c` OK | 库自动升 12、原任务保留、唯一索引消失、同 target 建第二条成功；FTP 零账号首装/卸载回收、PHP 扩展重装、删站级联逐项通过；浏览器视觉项实测（详见 §8） |

## 8. 最近一次全链路验收结论（2026-11-08 `a22296f` 升级 + 复验轮）

**通过**
- 升级路径：旧资产 `c44eeb5` 装出 `user_version=11` 并留 2 条任务（`builtin:db-vacuum`、`plugin:com.skiff.cron:upgradeprobe1`）；新资产覆盖后自动升到 12，原任务保留，`idx_scheduled_tasks_kind_source` 消失，同 target+method 第二条（`upgradeprobe2`）建成功。
- FTP（零账号首装 → 全链路 → 卸载回收）：清空 `/etc/pure-ftpd` 模拟干净系统后 `ftp.install` 200、`pure-ftpd` active、`auth/50pure` 生成、`pureftpd.passwd` 0600 空、`pureftpd.pdb` 2056 B、`:21` 监听；建号 uid/gid 82 与容器 `www-data` 一致；`STOR` 226；容器内 www-data 读上传内容并回写；删号后重登 530；`uninstall`（`removed_package:false`）后 `inactive`、`:21` 消失、`/etc/pure-ftpd/auth` 已回收。
- PHP 扩展卸载/重装：安装 `redis` → 卸载后 `php -m` 无、`redis.so` 无、`pecl list` 无 → 重装约 15 s 成功。
- 删站级联：删 `cascade-probe.test` 后 `scheduled_tasks` 中 `source=site:cascade-probe.test` 行消失，站点目录删除，日志无新 WARN。
- 浏览器视觉（Chromium `--ignore-certificate-errors` + CDP 精确视口）：
  - 430px / 860px：站点列表与详情无页面级横向溢出（430px 列表表格 504 px 宽，但包在 `.table-wrap` 内滚动，页面 `scrollWidth=430`）。
  - 站点详情实际 **7 个标签**（概览/域名/配置文件/日志/目录/SSL/管理），真实点击「管理」落到 `?tab=manage` 并出现停止/删除按钮。
  - 直达 `/sites/env/{id}`：扩展操作按钮齐全（安装所选/卸载所选/安装 PECL 扩展/重新安装/已安装）。
  - 插件 iframe：`/plugins/com.skiff.cron/ui/list.html` 内 `h1=0`、`h2=1`、无横向溢出（标题由父页面渲染）。
  - 1024 视口抽屉实测宽 720 px、`clippedChildren=0`；与 CSS `92vw`/840 px 预期不符，已记台账待复核。
- 其余沿用通过项：文件管理内置页、cron 预设互转、受保护容器 403、官方插件随包同步、站点「管理」页签、PERF-03、运行环境 digest/APK 缓存、FTP 主链路、PHP 参数生效与越界拒绝。

**未闭环（详见 [TEST_LEDGER.md](TEST_LEDGER.md)）**
- WEB-02：`worker_connections=2048` 保存成功但容器 `nginx -t` 仍告警。
- Docker 通用 create / CPU·内存限制表单 / Compose 运行入口仍缺（UI 设计未定，按用户要求留台账）。
- FTP fail2ban 三次封禁：`ignoreself` 使本机无法构造，需第二台客户端。
- 1024 视口抽屉实测 720 px 与 CSS 预期不符，待真实 1024 窗口复核。
- aarch64 仅核对资产摘要；1Panel 受限形态、G 组长时观测等无环境项。

**收尾状态（本轮结束）**
- 站点：`/www/sites` 为空；容器仅 `skiff-web-nginx`；三个 PHP 环境为种子行（`stopped`，目录已清）。
- 任务：仅 `builtin:db-vacuum`；`user_version=12`；服务 active、`NRestarts=0`。
- FTP 已彻底卸载（包状态 `rc`、`/etc/pure-ftpd` 删除、`:21` 无监听）；测试账号与上传文件已清。
- 保留 `/root/iter/admin-pw.txt`、`/root/iter/backups`、`/root/backups`。


## 9. 数据、备份与清理

| 项 | 路径 / 约定 |
| --- | --- |
| 数据根 | `/www/skiff`（0711）、`/www/sites`、`/www/php`、`/www/runtime` |
| 配置 | `/etc/skiff/config.toml`（0644） |
| 数据库 | `/www/skiff/skiff.db`（0600） |
| 运维备份 | `/root/iter/backups/`、`/root/backups/`；保留 `builtin:db-vacuum` 与运维备份 |
| 清理 | 测试完成后清理临时站点、容器、账号与压测文件，机器恢复可用状态 |
| GitHub 出网 | 内网代理与 git 全局配置见 [ITERATION.md](ITERATION.md) §7.1（不在本文重复） |
