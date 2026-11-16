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
| URL | `http://10.10.15.4:18081/cb5mgdk7/login`（**当前为形态三：直接明文 HTTP，仅限内网**） |
| 入口前缀 | 随机 8 位，**每次重装可能变化**；当前 `cb5mgdk7`（2026-11-16 23:22 全新安装生成，2026-11-17 00:06 覆盖安装后未变） |
| 前缀读取 | 测试机执行 `journalctl -u skiff -n 50 --no-pager \| grep 安全入口`，或登录后查看设置页；不要沿用旧记录直接试 |
| 明文通道（形态三，当前） | `listen=127.0.0.1:18080` + `allow_insecure_http=true` + `tls_enabled=false`（loopback dev HTTP），对外由 `skiff-http-fwd.service`（`python3 /root/iter/skiff-http-fwd.py`，`0.0.0.0:18081 → 127.0.0.1:18080`，unit 在 `/etc/systemd/system/`）TCP 转发；Cookie 名 `skiff_dev_session`（无 `Secure`）。**仅供内网验证，勿对外暴露** |
| 证书 | 本轮全新安装未生成 TLS 证书（`tls_enabled=false`）；历史自签证书路径与 SAN 见 §2.1，需要 TLS 时按形态一/二重新生成或回切 |
| 健康检查 | `GET /healthz` 200；裸 `GET /` 404，根路径只做前缀跳转 |

### 2.1 三种 TLS 形态与回切路径（2026-11-08 已逐形态验证）

| 形态 | 生效配置 | 访问地址 | 备份与回切 |
| --- | --- | --- | --- |
| 一：TLS 直连（`install.sh --tls on`） | `listen="0.0.0.0:18080"`、`tls_enabled=true`、`tls_cert=/etc/skiff/tls/fullchain.pem`、`tls_key=/etc/skiff/tls/privkey.pem` | `https://10.10.15.4:18080` | 需先自签证书（当前未生成）。以 `/root/iter/config-dev-20261108-fresh.toml` 为基线改这三项 → `systemctl restart skiff` |
| 二：反代终止 TLS（`trusted_proxies`） | `listen="0.0.0.0:18080"`、`allow_insecure_http=true`、`trusted_proxies=["127.0.0.1/32"]`、`tls_enabled=false` | `https://10.10.15.4:18443`（需自备 `skiff-tls-proxy.service`，当前未部署） | 同上；停 `skiff-http-fwd`、起 TLS 代理、重启 `skiff` |
| 三：直接明文（当前） | `listen="127.0.0.1:18080"`、`allow_insecure_http=true`、`tls_enabled=false` | `http://10.10.15.4:18081`（`skiff-http-fwd.service` 转发） | `/root/iter/config-dev-20261108-fresh.toml`（当前生效） |

- **机器还原后，本轮之前的配置备份、TLS 证书与 `skiff-tls-proxy` 均已不存在**；上表只描述切换方法，形态一/二需要先重新生成证书/代理。当前备份只有 `/root/iter/config-dev-20261108-fresh.toml`。
- 三形态切换均只需覆盖配置 + 重启 `skiff`；历史三形态实测结论（3→1→3 恢复 `/healthz` 200、无残留监听）仍然有效。
- Cookie 断言：形态一/二为 `__Host-skiff_session`（`Secure; HttpOnly; SameSite=Lax`）；形态三为 `skiff_dev_session`（`HttpOnly; SameSite=Lax`，无 `Secure`）。
- 形态二下直连 `:18080` 明文端口：`GET /<prefix>/login` 仍 200 但不写会话，`POST /<prefix>/login` 返回 403「跨站登录请求被拒绝」；`curl -k https://<host>:18080/healthz` 返回 000（该端口无 TLS）。

## 3. 账号与口令

| 项 | 值 / 读取方式 |
| --- | --- |
| 管理员账号 | `admin` |
| 当前口令 | `SkiffPass2026x`（2026-11-15 23:20 本轮安装后按 AUTH-07 流程修改；仅内网测试机） |
| 口令文件 | `/root/iter/admin-pw.txt`（`0600`）。**只存口令本身**（16 B，无换行），取自 `/www/skiff/initial-admin.txt` 第 3 行 `口令: …` |
| 读取方式 | SSH 登录后 `cat /root/iter/admin-pw.txt`；重置/轮换口令后必须同步更新该文件 |
| 首登改密 | 测试期默认 `must_change_password=0`，初始口令可直接登录；强制改密流程见 [ITERATION/01-ROUND.md](../ITERATION/01-ROUND.md) §5.2.1 |

## 4. SSH

| 项 | 值 |
| --- | --- |
| 连接 | `ssh -i C:\Users\note3510\.ssh\skiff-iter root@10.10.15.4`（端口 22）；ssh 客户端在 `C:\pf\Git\usr\bin\ssh.exe`（不在 PATH，需写绝对路径） |
| 私钥 | 当前开发机 `C:\Users\note3510\.ssh\skiff-iter`（ed25519，2026-11-13 生成；指纹 `SHA256:9tEAKeYXnnSC2nUO7+jDjCilgB+Bdz+w+P+OQJ8o4Ck`） |
| 公钥 | `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIESI78egMhyPjNWCMhZUf4aQRAhhU6eXd3fVgADRfYaV skiff-iter@dev`（已加入测试机 `authorized_keys`） |
| root 口令 | `1`（**内网测试机专用，仅用于验收**；不属于通用凭据口径，禁止外扩）。注意：口令无法经脚本注入——工具层把「把口令写进环境变量/脚本」判为 critical 并拒绝执行，日常一律走密钥 |
| 前置顺序 | 公钥必须先加入 `authorized_keys`，再打基线快照；顺序反了回滚后即失联 |
