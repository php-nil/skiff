# 测试机坐标：机器、面板访问、账号与 SSH（§1–4）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §1–4，正文未改（2026-11-16 整理）。

---

## 1. 测试机

| 项 | 值 |
| --- | --- |
| 地址 | `10.10.15.4`（内网 VM，本机可直达） |
| 系统 | Debian GNU/Linux 13 (trixie)，内核 `6.12.113+deb13-amd64` |
| 规格 | 4 vCPU / 3.9 GB 内存（可用约 3.5 GB）/ 120 GB 根分区 |
| 用途 | 干净系统装机、升级、插件与能力面验收；**不在测试机上编译** |
| 并行约束 | 多会话共用；涉及改 admin 口令、TOTP、重启服务、卸载、重置库等操作时使用同机隔离实例，避免影响其他并行任务 |
| 回滚 | 人工全量回滚到 T0/T1，按 [ITERATION/01-ROUND.md](../ITERATION/01-ROUND.md) §4.1 执行 |

## 2. 面板访问

| 项 | 值 |
| --- | --- |
| URL | `http://10.10.15.4:18080/5e3yz2yr/login`（**形态三：直接明文 HTTP，仅限内网**） |
| 入口前缀 | 随机 8 位，**每次重装可能变化**；当前 `5e3yz2yr`（2026-11-26 干净重装生成，此后三次就地升级未变） |
| 前缀读取 | 测试机执行 `journalctl -u skiff -n 50 --no-pager \| grep 安全入口`，或登录后查看设置页；不要沿用旧记录直接试 |
| 明文通道（形态三，当前） | `listen="0.0.0.0:18080"` + `tls_enabled=false` + `allow_insecure_http=true`（**安装后人工追加**：安装脚本只写 `listen` + `tls_enabled`，`allow_insecure_http` 与插件安装要用的 `allow_reduced_sandbox` 均需人工补，见 §6.2 裸奔防护）；**无转发层**，直连 18080。Cookie 名 `skiff_dev_session`（无 `Secure`）。**仅供内网验证，勿对外暴露** |
| 证书 | 本轮全新安装未生成 TLS 证书（`tls_enabled=false`）；历史自签证书路径与 SAN 见 §2.1，需要 TLS 时按形态一/二重新生成或回切 |
| 健康检查 | `GET /healthz` 200；裸 `GET /` 404，根路径只做前缀跳转 |

### 2.1 三种 TLS 形态与回切路径（2026-11-08 已逐形态验证）

| 形态 | 生效配置 | 访问地址 | 备份与回切 |
| --- | --- | --- | --- |
| 一：TLS 直连（`install.sh --tls on`） | `listen="0.0.0.0:18080"`、`tls_enabled=true`、`tls_cert=/etc/skiff/tls/fullchain.pem`、`tls_key=/etc/skiff/tls/privkey.pem` | `https://10.10.15.4:18080` | 需先自签证书（当前未生成）。以 `/root/iter/config-dev-20261118-fresh.toml` 为基线改这三项 → `systemctl restart skiff` |
| 二：反代终止 TLS（`trusted_proxies`） | `listen="0.0.0.0:18080"`、`allow_insecure_http=true`、`trusted_proxies=["127.0.0.1/32"]`、`tls_enabled=false` | `https://10.10.15.4:18443`（需自备 `skiff-tls-proxy.service`，当前未部署） | 同上；停 `skiff-http-fwd`、起 TLS 代理、重启 `skiff` |
| 三：直接明文（当前） | `listen="0.0.0.0:18080"`、`allow_insecure_http=true`、`tls_enabled=false` | `http://10.10.15.4:18080`（直连，无转发层） | `/etc/skiff/config.toml`（当前生效；`allow_insecure_http` 为安装后人工追加） |

- **机器还原后，上一轮的配置备份、TLS 证书与 `skiff-tls-proxy` 均已不存在**；上表只描述切换方法，形态一/二需要先重新生成证书/代理。基线就是 `/etc/skiff/config.toml`（2026-11-26 干净重装生成 + 人工追加 `allow_insecure_http` 与 `[catalog]` 段；此后三次就地升级均逐字节相同）；**2026-11-25 重装前现场已备份**在测试机 `/root/iter/backup-20261125/`（`config.toml.bak`、`skiff-data.tgz`、`sites-old/`、`php-old/`、`runtime-old/`），回滚即反向覆盖。
- 三形态切换均只需覆盖配置 + 重启 `skiff`；历史三形态实测结论（3→1→3 恢复 `/healthz` 200、无残留监听）仍然有效。
- Cookie 断言：形态一/二为 `__Host-skiff_session`（`Secure; HttpOnly; SameSite=Lax`）；形态三为 `skiff_dev_session`（`HttpOnly; SameSite=Lax`，无 `Secure`）。
- 形态二下直连 `:18080` 明文端口：`GET /<prefix>/login` 仍 200 但不写会话，`POST /<prefix>/login` 返回 403「跨站登录请求被拒绝」；`curl -k https://<host>:18080/healthz` 返回 000（该端口无 TLS）。

### 2.2 商店目录镜像（本机托管，2026-11-25）

| 项 | 值 |
| --- | --- |
| 镜像基址 | `http://10.66.52.7:8080`（**开发机**，即 `[catalog].mirrors` 的一期默认值） |
| 服务进程 | 开发机 `target/debug/skiff-server.exe serve --dist target/skiff-server/dist --listen 0.0.0.0:8080 --http --access-log .workbuddy/server-access.log`（明文，仅供内网；Windows 无 systemd，进程随会话存续） |
| 产物 | `target/skiff-server/dist`（`publish` 生成，含 `channels/stable/{manifest.json,.sig,index.json,runtime-index/{php,node,rust}.json}` 与 `blobs/sha256/*`）。当前 `seq=1`、`key_id=707cede3…ac2`、`index.json` 8,303 B；三份清单 php 1,030 B / node 777 B / rust 214 B |
| 安装包（客户端侧） | `v0.1.0` Release 的 `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz`：**12,542,260 B**、`sha256=c038e313c9c4bcabdfeaef1da61811cc245f8c3e95ccbad9e39f75c1dce0279b`（Release run `45688148056`，提交 `a915b6e`，CI run `45687189385` 全绿）。包内 `official/` **7 个**（`com.skiff.files` 已删） |
| 客户端配置 | 测试机 `/etc/skiff/config.toml` 的 `[catalog]`：`enabled = true`、`channel = "stable"`、`mirrors = ["http://10.66.52.7:8080"]`、`public_key = <测试发布公钥 7a9cb5d3…>`、`timeout_secs = 10`。**公钥每轮换密钥都要同步改**，否则面板拒绝该渠道。就地升级（`sh install.sh --no-start`，不传 `--listen/--tls`）**不会改这份配置**——脚本只在文件不存在时才放示例配置（2026-11-25 实测 `diff` 无差异） |
| 默认关闭 | 面板出厂 `[catalog] enabled = false`，装完不显示商店入口；要验对接必须手工开启（MIRROR-05） |
| 发布密钥 | 测试期用临时 Ed25519 密钥对，私钥只在发布机环境变量里（`SKIFF_SIGNING_KEY`），公钥写进上面 `public_key`；仓库与文档都不保存私钥 |
| 用例归属 | 本册只记坐标；验收步骤与预期见 [TEST_MATRIX/08-SELFHOST_MIRROR.md](../TEST_MATRIX/08-SELFHOST_MIRROR.md) |

## 3. 账号与口令

| 项 | 值 / 读取方式 |
| --- | --- |
| 管理员账号 | `admin` |
| 当前口令 | 见测试机 `/root/iter/admin-pw.txt`（2026-11-26 干净重装取自 `/www/skiff/initial-admin.txt`，`accept.sh` B3 改密后又改回原值；口令明文不落文档） |
| 口令文件 | `/root/iter/admin-pw.txt`（`0600`）。**只存口令本身**（无换行）。首装时取自 `/www/skiff/initial-admin.txt` 第 3 行 `口令: …`；`accept.sh` 的 B3 改密会把口令换回原值，重装/重置后必须同步更新本文件 |
| 读取方式 | SSH 登录后 `cat /root/iter/admin-pw.txt`；重置/轮换口令后必须同步更新该文件 |
| 首登改密 | 测试期默认 `must_change_password=0`，初始口令可直接登录；强制改密流程见 [ITERATION/01-ROUND.md](../ITERATION/01-ROUND.md) §5.2.1 |

## 4. SSH

| 项 | 值 |
| --- | --- |
| 连接 | `ssh -i C:\Users\note3510\.ssh\skiff-iter root@10.10.15.4`（端口 22）；ssh 客户端在 `C:\pf\Git\usr\bin\ssh.exe`（不在 PATH，需写绝对路径） |
| 私钥 | 当前开发机 `C:\Users\note3510\.ssh\skiff-iter`（ed25519，2026-11-13 生成；指纹 `SHA256:a77iflFQPsuDrBvneyFo9MTOhlsowtCvxlE58azsGtA`） |
| 公钥 | `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIESI78egMhyPjNWCMhZUf4aQRAhhU6eXd3fVgADRfYaV skiff-iter@dev`（已加入测试机 `authorized_keys`） |
| root 口令 | `1`（**内网测试机专用，仅用于验收**；不属于通用凭据口径，禁止外扩）。注意：口令无法经脚本注入——工具层把「把口令写进环境变量/脚本」判为 critical 并拒绝执行，日常一律走密钥 |
| 前置顺序 | 公钥必须先加入 `authorized_keys`，再打基线快照；顺序反了回滚后即失联 |
