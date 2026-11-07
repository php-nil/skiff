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
| 入口前缀 | 随机 8 位，**每次重装可能变化**；最近记录 `swn4nbhe`（2026-11-05 重置后人工恢复，2026-11-07 覆盖安装保留库与前缀） |
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
| tag / 构建提交 | `v0.1.0` = `c44eeb5`（Release 由该提交构建，资产与之一一对应） |
| 部署时间 | 2026-11-07 20:50（Asia/Shanghai，Release 资产覆盖时间；上机时间以当轮记录为准） |
| 部署方式 | GitHub Release 资产 `sha256sum -c` → 解压 → `sh install.sh` 覆盖安装；未清库，站点/任务/插件数据保留 |
| 数据库 | `PRAGMA user_version=11` |
| 服务 | `skiff.service` active、`NRestarts=0` |

## 6. Release 资产（v0.1.0，2026-11-07 最终版）

| 资产 | 大小 | sha256 |
| --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 8,020,711 B | `b16cb72e4a4b3b606e86634fb9940d9df4742e395ad01cb4cb5c48ae8bea7601` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 7,675,401 B | `a780dd7ff5adefb0a194a8925ffb6e65df6c561c9d32c1bd83bf9186d9257d11` |

- 下载端点：`https://github.com/php-nil/skiff/releases/download/v0.1.0/<资产名>`；测试机直连 `objects.githubusercontent.com` 会超时，改用 GitHub API 资产端点或本机代理下发。
- 包内 `official/` 含 6 个 `.wbpkg`（guard / monitor / files / cron / ftp / testkit）与 `public-key.txt`；`checksums.txt` 逐个校验。
- GitHub Release **说明正文尚未列 `ftp`**（正文仍写 4 个插件）；以包内 `official/` 与本文为准，待人工修正 Release 说明。

## 7. 最近部署记录

| 日期 | 场景 | 版本 / 坐标 | 结果 |
| --- | --- | --- | --- |
| 2026-11-05 | 重置库后全新安装（schema v7） | x86_64 6,987,420 B，`sha256:25559d7ec362625cee4fb608710aa842049447f7c76588cfb921c8e348988862` | 安装成功；新口令写入 `/root/iter/admin-pw.txt`；三类站与运行环境验收通过 |
| 2026-11-07 早轮 | Docker / FTP 修复轮 | 资产 `f9a0b30`，x86_64 `sha256:87427aa6…` | DOCKER-01～08 复验通过；该资产随后被同日最终版覆盖 |
| 2026-11-07 | v0.1.0 最终版覆盖安装 | tag/构建 `c44eeb5`；x86_64 `b16cb72e…` | FTP 主链路、DEPLOY-02/07/08 通过；详见 §8 |

## 8. 最近一次全链路验收结论（2026-11-07）

**通过**
- Release 资产 `sha256sum -c`、`install.sh`、`skiff.service` active、`NRestarts=0`、`integrity=ok`。
- FTP：装机与插件重装验签（grants=9）；建号 uid/gid 82 与容器 `www-data` 一致、`MinUID=82`；`STOR` 226；PHP 容器内读写；删号后登录 530；审计含 `ftp.*` 且无密码。
- DEPLOY-02 重启自启、DEPLOY-07 同包覆盖安装保留数据、DEPLOY-08 默认卸载保留数据，均通过。
- DOCKER-01～08（含受保护容器拒操作与删除入口）；SITE / WEB / MENU / TASK 的 HTTP 级验收见 [TEST_MATRIX.md](TEST_MATRIX.md)。

**未闭环（详见 [TEST_LEDGER.md](TEST_LEDGER.md)）**
- FTP-01 卸载回收；FTP-05 越界/跨站、fail2ban 三次封禁、`ftp.deny.*`、显式 TLS、浏览器访问上传的 `probe.php`。
- aarch64 仅核对资产摘要，未实机装机。
- 真实 Release 下载超时回退、1Panel 受限形态、G 组长时观测等无环境项。
- 真实浏览器视觉：430/860px、iframe 交互、站点详情四标签。

## 9. 数据、备份与清理

| 项 | 路径 / 约定 |
| --- | --- |
| 数据根 | `/www/skiff`（0711）、`/www/sites`、`/www/php`、`/www/runtime` |
| 配置 | `/etc/skiff/config.toml`（0644） |
| 数据库 | `/www/skiff/skiff.db`（0600） |
| 运维备份 | `/root/iter/backups/`、`/root/backups/`；保留 `builtin:db-vacuum` 与运维备份 |
| 清理 | 测试完成后清理临时站点、容器、账号与压测文件，机器恢复可用状态 |
| GitHub 出网 | 内网代理与 git 全局配置见 [ITERATION.md](ITERATION.md) §7.1（不在本文重复） |
