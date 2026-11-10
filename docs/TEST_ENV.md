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
| URL | `http://10.10.15.4:18081/w3qz4vc2/login`（**当前为形态三：直接明文 HTTP，仅限内网**） |
| 入口前缀 | 随机 8 位，**每次重装可能变化**；最近记录 `w3qz4vc2`（2026-11-09 21:52 全新库生成） |
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
| 口令文件 | `/root/iter/admin-pw.txt`（`0600`）；文档不保存明文口令。**只存口令本身**（17 B），由 `/www/skiff/initial-admin.txt` 第 3 行 `口令: …` 提取 |
| 读取方式 | SSH 登录后 `cat /root/iter/admin-pw.txt`；重置/轮换口令后必须同步更新该文件 |
| 首登改密 | 测试期默认 `must_change_password=0`，初始口令可直接登录；强制改密流程见 [ITERATION.md](ITERATION.md) §5.2.1 |

## 4. SSH

| 项 | 值 |
| --- | --- |
| 连接 | `ssh -i ~/.ssh/skiff-iter root@10.10.15.4`（端口 22）；**当前开发机没有该私钥**，改用 root 口令 + `SSH_ASKPASS` 非交互登录 |
| 私钥 | `~/.ssh/skiff-iter` 属于上一台开发机（`DESKTOP-5P3Q9PS`）；当前开发机 `DESKTOP-IRMPI7F` 上不存在，ssh-agent 为空 |
| 公钥 | `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM6Y+VSKG/QjciBaXQIObQwyg99CEEV+YKhJ5Op68Nrh skiff-iter@DESKTOP-5P3Q9PS` |
| 前置顺序 | 公钥必须先加入 `authorized_keys`，再打基线快照；顺序反了回滚后即失联 |
| 密码回退 | 口令不在文档留存；由机器所有者提供后经环境变量注入 `SSH_ASKPASS` 临时脚本（脚本只回显环境变量，不落盘口令，用完即删） |

## 5. 当前部署

| 项 | 值 |
| --- | --- |
| 版本 | `v0.1.0`（`790c01c`：修复面板设置写 `config.toml` 把顶层键追加到文件末尾导致重启解析失败（升级阻断）、nginx worker 换代检测（procfs `-r` + 非空判断、`openresty\|nginx` 匹配）、证书对写入与 reload 共用进程内锁；库 **v18**，本轮无迁移） |
| tag / 构建提交 | 构建提交 `790c01c745a80f83af083e9e57fb95ba323c7952`；tag `v0.1.0` **未移动，仍指 `ef7058b`**，由 dispatch（`ref=main`、`version=0.1.0`、`publish=true`、`release_tag=v0.1.0`、`skip_plugins=false`）构建并覆盖上传同一 Release；Release run `42916533236` **四个 job 全绿**；同提交 CI 全绿（12 项 check-run：11 success + `编译（x86_64 musl 静态）` skipped） |
| 部署时间 | 2026-11-11 06:44–06:46（Asia/Shanghai；先备份二进制/库/配置，再保库覆盖安装；本轮无 schema 迁移；`skiff.service` ActiveEnter `2026-11-11 06:46` 前后，`NRestarts=0`） |
| 部署方式 | `790c01c` Release 资产，本地 sha256 `1c932e7fa57e2fac5972e1fcb3299eca6059bee711e10db9e7f42731f194aec7`（9,918,314 B）与 API digest、`checksums.txt`、逐资产侧车四方一致 → 经内网代理走 API 资产端点下载（缓存 `%TEMP%\skiff-ops\rel-790c01c\`）→ `scp` 到 `/root/iter/rel-790c01c/` → 测试机 `sha256sum -c` OK → 解压（包内顶层目录 `skiff-0.1.0-x86_64-unknown-linux-musl/`）→ `sh -n install.sh` **exit 0** → `sh install.sh --no-docker` **退出码 0**；安装后 `/usr/local/bin/skiff` 14,709,264 B、sha256 `550c0fe69fe6fabd56e2ebfc4aecc2e0e73ea6bea79033d1270ac5d410a8c453`（与包内解压二进制一致）。**备份口径说明**：`/root/iter/skiff.bin.before-790c01c` 实测与新装二进制同 sha（`550c0fe6…`，部署脚本被重跑导致备份发生在安装之后），**不能**作为回退点；真实回退点取 `/root/iter/rel-a5dcdeb/skiff-0.1.0-x86_64-unknown-linux-musl/skiff`（a5dcdeb `271aa90c…`）或 `/root/iter/skiff.bin.before-a5dcdeb`（bf0c4fe `bd364d33…`）；升级前库快照 `/root/iter/skiff.db.before-790c01c`、配置备份 `/root/iter/config.toml.before-790c01c` |
| 数据库 | 库保持 **v18**（本轮无迁移）：`PRAGMA integrity_check=ok`；计数 plugins 7 / plugin_grants 58 / sites 2 / site_aliases 0 / site_runtimes 1 / users 1 / `user_prefs` 0；配置文件 sha256 随面板设置复验从部署时的 `4d320464fc54706c48cf289db483f0f188e14878a963299a6a6d7b1e7c45e72c` 变为 **`eb8ba7f35590e77270613298d5b0b326826f89d9809f8e4fcf407053b995f89a`**（唯一差异：`tls_enabled = false` 由第 98 行移到第 1 行，其余段与顺序不变），保存后重启 active、`NRestarts=0`，详见 §8 |
| 插件 | 7 个插件在装且全部 `enabled=1`；`install.sh` 按包内 `official/` 重放安装；本轮 FTP RPC（`status`/`config.save`/`create`/`delete`/`accounts`）与视图实测见 §8 |
| 服务 | `skiff.service` active、`NRestarts=0`、`/healthz` 200（`127.0.0.1:18080` 与转发 `:18081` 均 200）；`skiff-http-fwd.service`、`skiff-catalog-test.service`（18090）、`skiff-tunnel-keepalive.service` 均 active；`fail2ban.service` active（jail `pure-ftpd`）；宿主 `pure-ftpd` running（`/etc/pure-ftpd/conf/Umask` = `113 002`，本轮 `install` 重放后生效；`pureftpd.passwd` 0 行）、`skiff-web` = 201000:201000 已由 `install.sh` 幂等创建；站点容器 `skiff-php-php-8.5`（PHP 站 `a.com`）与 `skiff-web-nginx` 正常运行 |
| 配置 | `/etc/skiff/config.toml`：`listen="127.0.0.1:18080"` + `allow_insecure_http=true` + `tls_enabled=false`（形态三明文 dev HTTP）+ `allow_reduced_sandbox=true`（landlock/cgroups 未实现），并含 `[catalog]`：`enabled=true`、`channel="stable"`、`mirrors=["https://65f3f271cb611d89-139-227-130-152.serveousercontent.com/cur"]`（受信 Let's Encrypt 证书，serveo 域名每轮重建；本轮收尾时该隧道已 502，`/store` 按设计降级为「离线 + 上次成功快照」）、`public_key="315eacce321fd67ca0faeda0de04567017b3b352ba95180451e4536af4c74a28"`（临时测试密钥公钥，私钥种子只在测试机 `/root/iter/catalog-test/seed.hex`）、`timeout_secs=20`；面板设置复验保存后，顶层键 `tls_enabled = false` 位于**第 1 行**（首个表头 `[catalog]` 在第 98 行之前），`entry_prefix="w3qz4vc2"` 与 `listen` 均在顶层、`[catalog]` 段内容逐行未变、文件尾部无追加；sha256 **`eb8ba7f35590e77270613298d5b0b326826f89d9809f8e4fcf407053b995f89a`** |

**备份口径（硬性）**：部署前备份必须先用**可信来源**（Release 资产 `sha256sum -c`、`checksums.txt`、已安装二进制 `sha256sum`）确认内容版本，再按「`<内容> = <版本/提交> + <sha256>`」落名；禁止只按执行先后命名为 `before-<目标提交>` 而不校验内容。

- 正例：`/root/iter/rel-a5dcdeb/skiff-0.1.0-x86_64-unknown-linux-musl/skiff`（随资产校验，a5dcdeb `271aa90c…`）、`/root/iter/skiff.bin.before-a5dcdeb`（实测 bf0c4fe `bd364d33…`）。
- 反例（2026-11-11）：`/root/iter/skiff.bin.before-790c01c` 因部署脚本被重跑、备份发生在安装之后，实测 sha 与新装二进制相同（`550c0fe6…`），**不是** 790c01c 之前那一版，**不能**作为回退点。
- 操作要求：备份后立即 `sha256sum <备份文件>` 与预期来源比对，并把「备份路径 = sha256」写进本节/台账；不一致就重做备份，或明确标注该备份作废，不得留歧义文件名。

## 6. Release 资产（v0.1.0 = `790c01c`，2026-11-11）

构建来源：Release workflow run `42916533236`（dispatch `ref=main`，head_sha `790c01c745a80f83af083e9e57fb95ba323c7952`；tag `v0.1.0` 未移动，**该 tag 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`**，资产覆盖同一 Release）。**该 run 四个 job 全绿**：x86_64 / aarch64 构建、`docker` 镜像可构建、`发布到 GitHub Release` 均 success。x86_64 资产在开发机经内网代理走 API 资产端点下载，本地 sha256 与 API digest、`checksums.txt`、逐资产侧车四方一致；`scp` 到测试机后再 `sha256sum -c` 复核一致。

| 资产 | 大小 | sha256 |
| --- | --- | --- |
| `checksums.txt` | 223 B | `a9b56764f8b732a6c40f4119777160fe9664969691b168588d16f148b15eb382` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 9,918,314 B | `1c932e7fa57e2fac5972e1fcb3299eca6059bee711e10db9e7f42731f194aec7` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 9,457,485 B | `02ca77f52ed7dee9f11ee0408f6c4d666a245870c737c103801093b1240858d8` |

- 包内二进制：x86_64 `skiff` 14,709,264 B `550c0fe69fe6fabd56e2ebfc4aecc2e0e73ea6bea79033d1270ac5d410a8c453`（包内解压与安装后 `/usr/local/bin/skiff` 两处 `sha256sum` 一致）；aarch64 二进制未在本轮实机验证（sha 取自同轮 `checksums.txt`）。
- 资产 id：x86_64 主包 `733247661`、侧车 `733247657`（111 B）；aarch64 主包 `733247660`、侧车 `733247659`（112 B）；`checksums.txt` `733247658`。
- 下载端点：`https://github.com/php-nil/skiff/releases/download/v0.1.0/<资产名>`；当前开发机经内网代理 `http://10.66.10.22:1010` 走 `api.github.com/.../releases/assets/<id>` + `Accept: application/octet-stream` 下载（缓存 `%TEMP%\skiff-ops\rel-790c01c\`），再 `scp` 到测试机 `/root/iter/rel-790c01c/`。
- 包内 `official/` 含 7 个 `.wbpkg` 与 `public-key.txt`（本轮未改 `plugins/`）；解压后顶层目录为 `skiff-0.1.0-x86_64-unknown-linux-musl/`。
- 测试期只保留 `v0.1.0` 一个 tag，后续发版一律覆盖同一 tag 的 Release 资产；**tag 指向（`ef7058b`）与资产构建提交（`790c01c`）不同**，这是测试期「不移动 tag + dispatch 覆盖资产」的约定结果，不是异常。

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
| 2026-11-08 15:20 | `48f57dc` 收口发版覆盖安装 + v13→v14 升级与五组复验 | 资产 `48f57dc`；x86_64 8,453,244 B `786018dc…`、aarch64 8,068,111 B `e34089f2…`，双架构 `sha256sum -c` OK | `user_version=14`，`trash_entries`/`fs_quotas` 建出、旧数据保留；FS-10～12、DOCKER-12～14、monitor/guard 内核调度器、WEB-02 全部通过；详见 §8 |
| 2026-11-08 16:55 | `8716e51` 小批收口覆盖安装 + 配置保护 / 插件日志 / 上一批回归 | 资产 `8716e51`；x86_64 8,472,637 B `dee454fd…`、aarch64 8,086,197 B `8cffabc3…`，双架构 `sha256sum -c` OK | 库保持 v14、数据全部保留、配置 sha 未变；配置保护 403/配额 403、插件日志与崩溃尾巴、回收站/配额与 Docker 三入口均通过；详见 §8 |
| 2026-11-08 19:59 | 机器还原后**全新安装**（空库直建 v15） | 资产 `1862702`；x86_64 8,785,237 B `640603ca…`、aarch64 8,378,566 B `4c1232c9…`，双架构 `sha256sum -c` OK | `sh install.sh --no-docker` 完成；空库直建 **v15**，`jobs`/`job_logs`/`trash_entries`/`fs_quotas` 均建出；入口前缀 `udkwxz96`；6 个官方插件安装成功；`/healthz` 200、登录 303 换会话成功 |
| 2026-11-08 20:47 | `9daabd3` 复核轮覆盖安装（两处缺陷修复） | 资产 `9daabd3`；x86_64 8,786,193 B `ff71be79…`、aarch64 8,380,846 B `a2945c66…`，双架构 `sha256sum -c` OK | 库保持 v15、插件与运行环境保留；php.ini 坏语法被拒且原文件不变、合法值即生效；插件只读 403 文案不再追加授权提示；详见 §8 |
| 2026-11-08 20:47–21:55 | 工作树构建二进制覆盖（写放大三处修复 A/B 复测，**非 Release 资产**） | 同工作树 musl 构建替换 `/usr/local/bin/skiff`（备份 `/root/skiff.before-archfix`，即 `9daabd3` 资产 13,005,584 B）；monitor 修复以现编插件临时替换（备份 `/root/plugin.monitor.before-archfix`，961,864 B） | 三处 A/B 复测完成，数字见 `docs/TEST_LEDGER.md` 与 `docs/changes/2026-11-08.md`；收尾已还原官方 `com.skiff.monitor-0.1.0.wbpkg` 与官方插件二进制（961,864 B，22:09），删除 `samples.jsonl`，删除测试站 `audit-env.local`，`PRAGMA integrity_check=ok`、`/healthz` 200 |
| 2026-11-08 21:20 | 本地工作树构建二进制覆盖（超时可配验证用，**非 Release 资产**） | `cargo build --release --target x86_64-unknown-linux-musl -p skiff`（工作树含未提交的超时可配改动）；`/usr/local/bin/skiff` 12,749,696 B `md5 86e8f7ca06730120bcf2131600f567fb` | 仅用于验证任务中心超时路径；验完 `config.toml` 已恢复默认（无 `ext_*_timeout_secs` 键），服务 active、`/<prefix>/login` 200 |
| 2026-11-08 22:48 | `456e040` 三线收口发版覆盖安装（Release 资产） | 资产 `456e040`；x86_64 8,798,637 B `5ba0a822…`、aarch64 8,391,898 B `315ddaa2…`，双架构与逐资产 `sha256sum -c` 全部 OK | 解压 `sh install.sh --no-docker` 成功；库保持 v15、配置 sha 未变；`/usr/local/bin/skiff` 与包内二进制 sha256 一致；6 个官方插件按新包重装（monitor 新包 358,905 B，进程然后由内核调度器拉起） |
| 2026-11-08 22:48–23:12 | `456e040` Release 资产五组复验 | 同资产 | 插件面 EDQUOT **507**、超时 `error_class=timeout`、430/860/1024 三档 UI、性能三项、monitor `samples.jsonl` 追加+压缩 全部通过；详见 §8「收口轮复验」 |
| 2026-11-09 00:31 | `97edf87` 工作台三项复验版覆盖安装（Release 资产） | 资产 `97edf87`；x86_64 8,806,266 B `e10a0e9e…`、aarch64 8,401,420 B `e3e23735…`，双架构与逐资产 `sha256sum -c` 全部 OK | 解压 `sh install.sh --no-docker` 成功；库保持 v15、配置 sha `ebcccae6…` 未变；`/usr/local/bin/skiff` 13,034,256 B `8ae4eb97…` 与包内一致；6 个官方插件包字节与 md5 未变 |
| 2026-11-09 00:35–01:05 | `97edf87` 工作台三项 + 任务/文件主链路复验 | 同资产 | 四插件 × 三档 iframe 高度收敛（430 封顶后页级滚动条 0、滚轮内部滚动有效）、单视图老包占位消失与明暗底衬一致、停 Docker 后概览 + 四资源页 gate 且恢复后自动解除、任务中心与文件管理主链路 200；详见 §8「工作台三项复验」 |
| 2026-11-09 01:05 | `d197041` testkit 探针复验版覆盖安装（Release 资产） | 资产 `d197041`；x86_64 8,807,640 B `563a0fc4…`（单资产 sha256 与 API digest 一致） | 解压 `sh install.sh --no-docker` 成功；库保持 v15；`/usr/local/bin/skiff` 13,034,256 B `9e96f596…` 与包内一致；testkit 新包 322,007 B `df8e361b…` 同步到 `/www/skiff/official/`，全新安装 grants 6（含 `scheduler:read`）并种入默认停用探针任务 |
| 2026-11-09 01:05–01:12 | `d197041` 台账四条复验 | 同资产 | WEB-04 四条候选校验、SITE-10 三种删除路径、files 授权默认/重装、BusyGuard 403 全部通过；详见 §8「台账四项复验」 |
| 2026-11-09 01:23–01:35 | `9e2c6b1` 修复轮覆盖安装 + 复验（Release 资产） | 资产 `9e2c6b1`；x86_64 8,808,110 B `23c5127d…`（与 API digest 一致） | 解压 `sh install.sh --no-docker` 成功；库保持 v15、配置 sha `ebcccae6…` 未变；二进制 `df1ba659…` 与包内一致；停止态植入非法配置启动被拒且线上 `nginx -t` 通过，还原后启动成功、站点恢复；详见 §8「`9e2c6b1` 修复轮」 |
| 2026-11-09 凌晨 | `2f63aeb` FTP jail 修复版覆盖安装（Release 资产） | 资产 `2f63aeb`；x86_64 8,808,264 B `9bb0467f…`、aarch64 8,401,553 B `5bfd3cb1…` | 库保持 v15；二进制 `54f660a1…` 与包内一致、`/healthz` 200；jail 写入 `port = 2121`、fail2ban 单 jail、nft 仅 `tcp dport 2121`；详见 §8 |
| 2026-11-09 03:25 | `607d09b` 发版覆盖安装（Release 资产） | 资产 `607d09b`；x86_64 8,812,519 B `1284a6cb…4c5c009b`、aarch64 8,403,005 B `fb4c7aa5…4dead7a`，`sha256sum -c` 与外层 `checksums.txt` 均 OK | 解压 `sh install.sh --no-docker` 成功；库保持 v15、配置 sha `ebcccae6…` 未变；`/usr/local/bin/skiff` 13,050,640 B `01ad573e…985d7348` 与包内一致；`/healthz` 200；详见 §8「`607d09b` 发版复验」 |
| 2026-11-09 03:59 | `8acc83f` 收口发版覆盖安装（Release 资产） | 资产 `8acc83f`；x86_64 8,812,469 B `ac4bc53f…1519fd1`、aarch64 8,402,947 B `fefae2ac…140bc38`，`sha256sum -c` 与外层 `checksums.txt` 均 OK | 解压 `sh install.sh --no-docker` 成功；库保持 v15、配置 sha `ebcccae6…` 未变；`/usr/local/bin/skiff` 13,050,640 B `5959d021…64a72b8` 与包内一致；`/healthz` 200、`NRestarts=0`；详见 §8「`8acc83f` 收口发版复验」 |
| 2026-11-09 04:10–04:25 | monitor 网速口径修复的插件临时替换（**非 Release 资产**） | 现编 musl 插件 `730,592 B` `dfb96a8a…` 替换 `versions/0.1.0/bin/amd64/plugin`（官方版 `b113ddd9…` 备份 `/root/monitor-plugin-official.bin`）；官方包暂移 `/root/monitor-official.wbpkg.bak`；UI 两文件替换（备份 `/root/monitor-{overview.html,app.js}.bak`） | 已知量对照：16s 下载 119.1 MB，期望 tx 7,428,904 B/s、插件报 7,437,136 B/s（0.11%），rx 44,709 vs 44,739 B/s；历史行已带 `net_tx_rate`、双基线 `prev.json`/`prev_ui.json` 独立。收尾已还原官方插件 `b113ddd9…`、官方包与 UI（`c-net-tx` 消失），删 `prev_ui.json` 与临时文件，单进程、`/healthz` 200；详见 `docs/changes/2026-11-09.md` |
| 2026-11-09 04:29 | `9a656d5` 下载端点 404 修复版覆盖安装（Release 资产） | 资产 `9a656d5`；x86_64 8,812,918 B `928fb07f…fc71837a`、aarch64 8,403,389 B `7efbcb6c…98300a7`，`sha256sum -c` 与外层 `checksums.txt` 均 OK | 解压 `sh install.sh --no-docker` 成功；库保持 v15、配置 sha `ebcccae6…` 未变；`/usr/local/bin/skiff` 13,050,640 B `eb8ac795…523401d` 与包内一致；`/healthz` 200、`NRestarts=0`；详见 §8「`9a656d5` 台账清理轮复验」 |
| 2026-11-09 05:10–05:28 | `0c99464` monitor 出站速率 + testkit 缓冲探针复验版覆盖安装（Release 资产） | 资产 `0c99464`；x86_64 8,817,181 B `f969910d…ccfe6c9a`、aarch64 8,408,470 B `d16801d8…4ff243a0a7`，双架构 `sha256sum -c` 与外层 `checksums.txt` 均 OK | 解压 `sh install.sh --no-docker` 成功；库保持 v15、配置 sha `ebcccae6…` 未变；`/usr/local/bin/skiff` 13,050,640 B `92b2bd2b…2f26b17` 与包内一致；6 包全部 `verify` 通过（指纹 `ed73de3e…`）；testkit 同版本升级保留旧 grants，`remove`+`install` 后才得 `scheduler:manage`，busy-probe 任务行需删后重种；详见 §8「`0c99464` monitor 与 TASK-06 复验」 |
| 2026-11-09 21:20–21:55 | 机器还原后**全新安装**（`491da40`，空库直建 v15） | 资产 `491da40`；x86_64 8,921,336 B `6d66d6ed…2fc7154ff9`（与 API digest 一致，`sha256sum` 复核 OK）、aarch64 8,510,237 B `b41e2d5a…454117a17c` | 机器还原后无 `/etc/skiff`、无 `skiff.service`、Docker 已就绪；解压 `sh install.sh --no-docker` 成功；空库直建 **v15**；入口前缀 `w3qz4vc2`；`/usr/local/bin/skiff` 13,323,920 B `6288c1b0…f5695253db` 与包内一致；6 个官方插件 CLI 装成；重建形态三转发，`/healthz` 200、登录 303；AUTH/FS/PLUGIN/TASK/审计 35 项、站点+nginx 7 项全 PASS；详见 §8「`491da40` 全新安装轮」 |
| 2026-11-10 08:05–08:40 | `483f898` 防火墙 + Node 运行时发版覆盖安装（Release 资产） | 资产 `483f898`；x86_64 9,395,086 B `dae3157b…c97641`（与 API digest 一致，测试机 `sha256sum -c` OK）、aarch64 8,955,032 B `40322313…21d8f438` | 解压 `sh install.sh --no-docker` 成功；库保持 v15、配置 sha `3133d44b…` 未变、入口前缀仍 `w3qz4vc2`；`/usr/local/bin/skiff` 13,528,720 B `255846ea…07baee` 与包内一致；7 个插件（`com.skiff.firewall` 新装，grants 12）；AUTH/FS/SITE/TASK/PLUGIN/审计/firewall/fail2ban/监控/Node 全 PASS，FTP→firewall 升级路径对象授权 FAIL（见台账）；详见 §8「`483f898` 防火墙 + Node 运行时轮」 |
| 2026-11-10 09:03–09:15 | `c7a06b4` 升级授权补齐复验版覆盖安装（Release 资产） | 资产 `c7a06b4`；x86_64 9,396,976 B `98bca683…a492242`（与 API digest 一致，测试机 `sha256sum -c` OK）、aarch64 8,957,823 B `e0e2863d…a94021ae` | 解压 `sh install.sh --no-docker` 成功；库保持 v15、配置 sha `3133d44b…` 未变、入口前缀仍 `w3qz4vc2`；`/usr/local/bin/skiff` 13,536,912 B `e94c5c88…1d64e9a` 与包内一致；旧包 ftp 升级后 grants 11→13、`plugin.grant_upgrade` 审计可查、`protection.enable` 200、`interop.call` 6 ok / 0 deny / 0 fail；AUTH/FS/SITE/TASK/firewall 回归 PASS；详见 §8「`c7a06b4` 升级授权补齐复验」 |
| 2026-11-10 13:25 | `2defdc7` 内核健壮性轮升级覆盖安装（Release 资产，**保留旧库**验迁移） | 资产 `2defdc7`；x86_64 9,464,715 B `3f2668a1…98a0`（与 API digest 一致，测试机 `sha256sum -c` OK）、aarch64 9,017,580 B `094e779e…b8ca`；旧二进制备份 `/root/skiff.before-2defdc7`、升级前库快照 `/root/iter/skiff.db.before-2defdc7` | schema **v15→v16** 自动迁移成功（`user_version=16`、`fs_quotas.truncated` 建出）、数据全部保留、配置 sha `3133d44b…` 未变、前缀仍 `w3qz4vc2`；`/usr/local/bin/skiff` 13,688,592 B `e766e84e…d3a05` 与包内一致；`/healthz` 200、`NRestarts=0`；audit-prune / 配额后端 / Compose partial / 重入拦截 / AUTH·FS·SITE+nginx·TASK·firewall·Node 回归 PASS；配额页 UI 与 Release docker job 两处 FAIL（见台账）；详见 §8「`2defdc7` 内核健壮性轮」 |
| 2026-11-10 14:09–14:25 | `38b0bb9` 两处 FAIL 修复复验版覆盖安装（Release 资产） | 资产 `38b0bb9`；x86_64 9,464,733 B `fc13b656…c5aa20`（与 API digest 一致，测试机 `sha256sum -c` OK）、aarch64 9,018,441 B `ce279cf3…04acc1`；旧二进制备份 `/root/skiff.before-38b0bb9`、升级前库快照 `/root/iter/skiff.db.before-38b0bb9` | 解压 `sh install.sh --no-docker` 成功；库保持 **v16**（同版本覆盖，无迁移）、配置 sha `3133d44b…` 未变、前缀仍 `w3qz4vc2`；`/usr/local/bin/skiff` 13,692,688 B `5cf0eadc…81b6` 与包内一致；`/healthz` 200、`NRestarts=0`；配额页 truncated 文案、回收站四动作、AUTH/FS/SITE/TASK 回归全 PASS；详见 §8「`38b0bb9` 两处 FAIL 修复复验」 |
| 2026-11-10 15:28 起 | `0ee9065` 商店目录（Skiff-server P1+P2）覆盖安装 + 链路实测（Release 资产） | 资产 `0ee9065`；x86_64 9,770,768 B `31cdc789…94b0`（与 API digest 一致，测试机 `sha256sum -c` OK）、aarch64 9,312,509 B `8696818a…b365`；旧二进制备份 `/root/skiff.before-0ee9065`、升级前库快照 `/root/iter/skiff.db.before-0ee9065` | 解压 `sh install.sh --no-docker` 成功；库保持 **v16**（无迁移）、前缀仍 `w3qz4vc2`；`/usr/local/bin/skiff` 14,331,792 B `1f6c08f9…a723` 与包内一致；`/healthz` 200、`NRestarts=0`；商店四类条目、testkit 商店安装全链路与四条反例按预期（两处审计缺口与换钥后缓存不失效记台账）；详见 §8「`0ee9065` 商店目录轮」 |
| 2026-11-10 18:38 起 | `5c15b6b` 健壮性收口轮覆盖安装 + 五条重点复验（Release 资产） | 资产 `5c15b6b`；x86_64 9,812,106 B `4dc3ddb9…7b80d59c`（与 API digest 一致，测试机 `sha256sum -c` OK）、aarch64 9,352,764 B `2adf3b7b…47a6af`；旧二进制备份 `/root/skiff.before-5c15b6b`、升级前库快照 `/root/iter/skiff.db.before-5c15b6b` | 解压 `sh install.sh --no-docker` 成功；库保持 **v16**（无迁移）、前缀仍 `w3qz4vc2`；`/usr/local/bin/skiff` 14,446,736 B `f22c3f33…96b954d` 与包内一致；`/healthz` 200、`NRestarts=0`；插件启停一致性 / 文件页 26 项 / 审计三条路径 / 镜像退避 15 项 / 镜像 digest 五条全部 PASS；详见 §8「`5c15b6b` 健壮性收口轮」 |
| 2026-11-10 19:46 起 | `c8bcb6e` 商店 P3 升级只读段 + 暂存 TTL 覆盖安装 + 四条重点复验（Release 资产） | 资产 `c8bcb6e`；x86_64 9,821,046 B `fceae2cb…c576f09`（与 API digest 一致，测试机 `sha256sum -c` OK）、aarch64 9,363,268 B `df3e0bc6…d41d59`；旧二进制备份 `/root/skiff.before-c8bcb6e`、升级前库快照 `/root/iter/skiff.db.before-c8bcb6e`、配置备份 `/root/iter/config.before-c8bcb6e.toml` | 解压 `sh install.sh --no-docker` 成功；库保持 **v16**（无迁移）、前缀仍 `w3qz4vc2`；`/usr/local/bin/skiff` 14,475,536 B `540a715a…b8a693` 与包内一致；`/healthz` 200、`NRestarts=0`；升级提示 / 下载校验 / 暂存 TTL / 商店与插件回归全部 PASS；详见 §8「`c8bcb6e` 内核升级只读段轮」 |
| 2026-11-10 23:03–次日 00:20 | `eea2969` 六块大范围上机验收覆盖安装（Release 资产，**保留旧库**验 v16→v17） | 资产 `eea2969`；x86_64 9,848,676 B `68046ca5…e156c66`（与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK）、aarch64 9,389,549 B `4352f483…55fb8f62`；旧二进制备份 `/root/skiff.before-eea2969`、升级前库快照 `/root/iter/skiff.db.before-eea2969`、配置备份 `/root/iter/config.before-eea2969.toml` | `sh -n install.sh` OK；库 **v16→v17**（`jobs.exit_code`）迁移成功、数据全保留、`integrity_check=ok`；宿主 `skiff-web` 201000:201000 创建且二次安装复用；站点统一身份 / URL 契约 / 任务中心 / fail2ban / 菜单回归共 30 项 PASS、4 项 FAIL（Debian PHP `listen.group=nobody`、重试作业永挂 queued、fail2ban 缺失态 400 非 503、装后 sshd jail 未即时关闭；见台账）；详见 §8「`eea2969` 六块上机验收」 |
| 2026-11-11 00:19–00:57 | `db7e579` 四条 FAIL 修复复验版覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `db7e579`；x86_64 9,902,038 B `a3103511…b29fce`（与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK）、aarch64 9,438,191 B `43754121…13772dce`；旧二进制备份 `/root/skiff.before-db7e579`、升级前库快照 `/root/iter/skiff.db.before-db7e579`、配置备份 `/root/iter/config.before-db7e579.toml` | `sh -n install.sh` OK、`sh install.sh --no-docker` exit 0；库保持 **v17**（无迁移）、计数一致、`integrity_check=ok`；`/usr/local/bin/skiff` 14,655,760 B `c639e3f4…b57254f` 与包内一致；`/healthz` 200、`NRestarts=0`；Debian PHP / 重试作业 / RPC 状态码 / fail2ban 装后 / 回归五块 PASS（另见 Debian 扩展安装 Alpine-only 等观察，记台账）；详见 §8「`db7e579` 四条 FAIL 修复复验」 |
| 2026-11-11 00:52–01:06 | `a735a91` SITE-15 / FW-19 修复复验版覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `a735a91`；x86_64 9,903,059 B `35ba6da1…e180e1`（与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK）、aarch64 9,440,485 B `e77306a7…bb3e`；旧二进制备份 `/root/skiff.before-a735a91`、升级前库快照 `/root/iter/skiff.db.before-a735a91`、配置备份 `/root/iter/config.before-a735a91.toml` | `sh -n install.sh` OK、`sh install.sh --no-docker` exit 0；库保持 **v17**（无迁移）、计数一致、`integrity_check=ok`；`/usr/local/bin/skiff` 14,663,952 B `669942eb…c5218` 与包内一致；`/healthz` 200、`NRestarts=0`；FW-19 PASS，SITE-15 **FAIL**（`PHP_INI_SCAN_DIR` 重复扫描致 PECL 扩展安装判失败，非本轮引入）；详见 §8「`a735a91` SITE-15 / FW-19 复验」 |
| 2026-11-11 01:04–01:35 | `c4e42f0` SITE-15 前导冒号修复复验版覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `c4e42f0`；x86_64 9,902,995 B `a9ab05c9…53a2a68`（与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK）、aarch64 9,439,333 B `42aef729…dfac51f`；旧二进制备份 `/root/skiff.before-c4e42f0`、升级前库快照 `/root/iter/skiff.db.before-c4e42f0`（3,948,544 B `b3402161…`）、配置备份 `/root/iter/config.before-c4e42f0.toml` | `sh -n install.sh` OK、`sh install.sh --no-docker` exit 0；库保持 **v17**（无迁移）、计数一致、`integrity_check=ok`；`/usr/local/bin/skiff` 14,663,952 B `b7acc80b…a6b51` 与包内一致；`/healthz` 200、`NRestarts=0`；**SITE-15 PASS**（重建两家族环境后 `php --ini` 两目录各一次，`imagick`/`redis`/`msgpack` 与批量 `redis+zip`/`zip+intl` 全部 exit 0）；详见 §8「`c4e42f0` SITE-15 复验」 |
| 2026-11-11 03:45–03:58 | `e2d6240`（含 `912c1f3`）菜单下移 + FTP 权限对齐复验版覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `e2d6240`；x86_64 9,900,537 B `1949ae54…50227a5`（与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK）、aarch64 9,437,650 B `af0d659f…d0466b`；旧二进制备份 `/root/skiff.before-e2d6240`（`2951777a…`）、升级前库快照 `/root/iter/skiff.db.before-e2d6240`（4,263,936 B `ff52fc79…`）、配置备份 `/root/iter/config.before-e2d6240.toml` | `sh -n install.sh` OK；库保持 **v18**（无迁移）、`integrity_check=ok`、计数不变（sites 2 / plugins 7 / grants 58 / users 1）；`/usr/local/bin/skiff` 14,672,144 B `45fa416b…02cc05a4` 与包内一致；`/healthz` 200、`NRestarts=0`；菜单 41 项 + FTP 权限 16 项断言全 PASS；工具分组实际项与顶栏「应用」菜单两处口径差异见 §8 与台账；详见 §8「`e2d6240` 菜单下移 + FTP 权限对齐」 |
| 2026-11-11 04:08–04:20 | `97694b4` 站点/任务纯结构拆分回归覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `97694b4`；x86_64 9,900,885 B `00f4efdc…14db6c9`（与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK）、aarch64 9,437,602 B `345e26d0…1914f45`；旧二进制备份 `/root/iter/skiff.bin.before-97694b4`（`45fa416b…`）、升级前库快照 `/root/iter/skiff.db.before-97694b4`、配置备份 `/root/iter/config.toml.before-97694b4` | `sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **exit 0**（真实退出码，无 `Bad substitution`）；库保持 **v18**（无迁移）、`integrity_check=ok`、计数一致（sites 2 / plugins 7 / grants 58 / users 1）；`/usr/local/bin/skiff` 14,672,144 B `18c59a0c…72d2509` 与包内一致；`/healthz` 200、`NRestarts=0`；站点全流程 47/48、任务中心 32/32（1 项 FAIL 为既有 `set_site_default` UNIQUE 冲突，非本轮引入，见 §8 与台账） |
| 2026-11-11 04:47–04:52 | `244ea76` 默认站点切换修复 + `97694b4` 两处结构拆分回归覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `244ea76`；x86_64 9,907,191 B `d150659f…3dabcec99`（与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK）、aarch64 9,439,433 B `4dc7f908…4895b4beb3`；旧二进制备份 `/root/iter/skiff.bin.before-244ea76`（14,672,144 B `18c59a0c…72d2509`）、升级前库快照 `/root/iter/skiff.db.before-244ea76`（4,382,720 B）、配置备份 `/root/iter/config.toml.before-244ea76` | `sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **真实退出码 0**（重定向 + `rc=$?`，`Bad substitution` 未复现）；库保持 **v18**（无迁移）、`integrity_check=ok`、计数一致（sites 2 / plugins 7 / grants 58 / users 1 / `user_prefs` 0）；`/usr/local/bin/skiff` 14,684,560 B `d35af82…16b7cc62` 与包内一致；`/healthz` 200（18080 与 18081 均 200）、`NRestarts=0`；默认站 48/48、FTP/TLS 35/36、并发写 17/17、站点全流程 48/48、任务中心 32/32；唯一 FAIL 为 `tls.pem` 中间文件 0644（目录 0700 兜底），详见 §8 |
| 2026-11-11 05:30–05:31 | `bf0c4fe` TLS 临时件权限 + 唯一临时名收尾覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `bf0c4fe`；x86_64 9,904,582 B `53a4b6c3…a165b3f59`（与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK）、aarch64 9,439,362 B `da5550ce…e5c74abb40d`；旧二进制备份 `/root/iter/skiff.bin.before-bf0c4fe`（14,684,560 B `d35af82…16b7cc62`）、升级前库快照 `/root/iter/skiff.db.before-bf0c4fe`（4,530,176 B）、配置备份 `/root/iter/config.toml.before-bf0c4fe` | `sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **退出码 0**；库保持 **v18**（无迁移）、`integrity_check=ok`、计数一致（sites 2 / plugins 7 / grants 58 / users 1 / `user_prefs` 0）；`/usr/local/bin/skiff` 14,684,560 B `bd364d33…f1cc51dbe0` 与包内一致；`/healthz` 200、`NRestarts=0`；FTP TLS 权限 14/14、唯一临时名并发 26/26、回滚路径 5/5 全 PASS；详见 §8 |
| 2026-11-11 06:44–06:46 | `790c01c` 配置写入修复 + reload 检测/证书成对重验覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `790c01c`；x86_64 9,918,314 B `1c932e7f…f194aec7`（与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK）、aarch64 9,457,485 B `02ca77f5…b1240858d8`；备份 `/root/iter/skiff.db.before-790c01c`、`/root/iter/config.toml.before-790c01c`（**注意**：`skiff.bin.before-790c01c` 因脚本重跑实为新装二进制，不作回退点） | `sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **退出码 0**；库保持 **v18**（无迁移）、`integrity_check=ok`、计数一致（sites 2 / plugins 7 / grants 58 / users 1 / `user_prefs` 0）；`/usr/local/bin/skiff` 14,709,264 B `550c0fe6…0a8c453` 与包内一致；`/healthz` 200、`NRestarts=0`；配置写入 **42/42 PASS**（保存后可重启、顶层键在首个表头前、`[catalog]` 不动、只读失败路径 sha 不变）、reload 优先 **31/33 + 回退补测 8/8**（单次不 restart、并发 213 次全 303/0 非 200、坏配置仍回滚、回退 restart 有效）、证书 **24/25**（12 次交替 + 30s 混合全 PASS，私钥 rename 失败路径仍留 HTTPS 声明导致 `nginx -t` 失败，台账保留）；详见 §8 |


## 8. 最近一次全链路验收结论（截至 2026-11-11 `v0.1.0` / `790c01c`）

> 本节只保留验收断言、实测数字与限制，是「测试机上发生了什么」的唯一来源；各轮的代码改动、根因与提交链见 [changes/2026-11-10.md](changes/2026-11-10.md)、[changes/2026-11-09.md](changes/2026-11-09.md)，用例编号与预期结果见 [TEST_MATRIX.md](TEST_MATRIX.md)，未闭环项见 [TEST_LEDGER.md](TEST_LEDGER.md)。

**`790c01c` 配置写入修复 + reload 优先 / 证书成对重验（2026-11-11 06:44 起，Release 资产覆盖升级，保留库，无迁移）**

本轮出包目标为上轮升级阻断项（面板设置写 `config.toml` 追加到文件末尾）与 reload 检测器（worker 换代恒空）的修复版本，验收三块：配置写入、reload 优先、证书成对 + 回归。

- 出包 / 部署：同提交 CI 12 项 check-run 全绿（11 success + `编译（x86_64 musl 静态）` skipped）；Release run `42916533236` 四个 job 全绿（head_sha `790c01c`）；tag `v0.1.0` 未移动（仍指 `ef7058b`）；x86_64 9,918,314 B `1c932e7f…f194aec7` 与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK；`sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **退出码 0**；`/usr/local/bin/skiff` 14,709,264 B `550c0fe6…0a8c453` = 包内解压二进制；库保持 **v18**（无迁移）、`integrity_check=ok`、计数一致（sites 2 / site_aliases 0 / site_runtimes 1 / plugins 7 / plugin_grants 58 / users 1 / `user_prefs` 0）；`/healthz` 200、`NRestarts=0`。
- **① 配置写入（升级阻断项，42/42 PASS，`/root/iter/m18-config.py`，日志 `m18-config.log`）**
  - 保存与落位：从部署后的 `config.toml`（sha `4d320464…`）出发，面板保存 `tls_enabled`（删除后重建）→ 文件 sha 变为 `eb8ba7f3…`，**唯一差异是 `tls_enabled = false` 由第 98 行移到第 1 行**；`[catalog]`（第 98 行起）逐行未变（`len=6/6`）、顶层键落在首个表头之前（`first_header=98`、`tops=[0,15,97]`）、文件以换行结尾、尾部无多余追加。
  - 覆盖面：同值保存幂等（sha 不变）；`listen` `18080→18082→18080` 往返均落对位置；反向场景（删除 `tls_enabled` 后再保存顶层键，且 `[catalog]` 在末尾）新键仍插到 `[catalog]` 之前而非其内，段头集合与顺序保持不变（`['[catalog]']`）。
  - 核心判定「保存后能重启」：`systemctl restart skiff` 后 `active`、`NRestarts=0`、`/healthz` 200、journal 无 `TOML parse error`。
  - 失败路径：`chattr +i /etc/skiff/config.toml` 后保存 → 页面返回 `替换 /etc/skiff/config.toml 失败: Operation not permitted (os error 1)`，原文件 sha 保持 `eb8ba7f3…` 不变；`chattr -i` 后恢复可保存且语义一致。
  - 面板可写键范围（回复父会话）：面板设置只写 **6 个顶层键** `listen`、`entry_prefix`、`tls_enabled`（auto 模式删该行）、`tls_cert`、`tls_key`，不触碰 `[catalog]`；本轮复验实际改动仅 `listen`、`entry_prefix`、`tls_enabled` 三项，`tls_cert`/`tls_key` 路径为空未写。
- **② reload 优先（SITE-17 重验，31/33 + 回退补测 8/8 PASS，`m18-reload.py` / `m18-fallback.py`，日志 `m18-reload.log` / `m18-fallback.log`）**
  - 单次保存：303（0.23s）；容器 ID 与 `StartedAt` **不变**（`2026-11-10T22:30:58.514728739Z`）；worker PID `7 8 9 10 → 71 72 73 74`（50ms 采样观察到 3 代 `['7 8 9 10', '7 8 9 10 71 72 73 74', '71 72 73 74']`）；服务日志 `INFO nginx 已平滑 reload（worker PID 换代） before=7 8 9 10 after=7 8 9 10 71 72 73 74`；探针 `/m18-probe` 200 + `X-M18`、站点 200、`nginx -t` 通过；恢复基线 `e50ca07d…`。
  - 4 会话 × 35s 并发保存：`{303: 213}`，**0 次非 303 / 0 次错误页 / 0 次 5xx / 0 次健康配置回滚**；容器未 restart；站点非 200 次数 **0**（`bad_polls=0`）；vhost 花括号配平且为完整提交（`len=549`）、conf 目录无 `.tmp`、`nginx -t` 通过；结束后恢复基线 sha 一致。
  - 坏配置仍被拦：注入 `zz-m18-bad.conf`（候选隔离校验能过、真实 nginx 起不来）→ 保存返回 **200**（2.7s）文案「配置已回滚到保存前内容：nginx 未能保持运行」，conf 回到 `e50ca07d…`、无 `.tmp`；清理后 `nginx -t` 通过、站点 200。
  - 回退路径：从宿主 `docker inspect -f '{{.State.Pid}}'` 取容器 init 宿主 PID 并 `kill -STOP`（`stat=Ts`），令 reload 无法换代后保存 → 303（12.5s）、`StartedAt` `22:49:30.565548665Z → 22:52:32.585954361Z`（确认回退 restart）、容器 `running`、日志 `WARN nginx reload 后 worker 未换代，回退重启容器 before=5643 5644 5645 5646`、站点 200、`nginx -t` 通过、恢复基线。
  - 说明：`m18-reload.py` 的两个 FAIL 是故障注入无效（容器命名空间内对 PID 1 `kill -STOP` 被内核忽略），已由 `m18-fallback.py` 用宿主 PID 重做并 8/8 通过，不单列缺陷。
- **③ 证书成对（24/25，`m18-cert.py` / `m18-cert-detail.py`，日志 `m18-cert.log`）**
  - 12 次交替替换（自签 generate ↔ 上传 4 组预生成自签对）：**12/12 303**，每次 `openssl modulus` 成对、站点 200（`bad=0`）、`nginx -t` 通过。
  - 30s 混合压测（2 线程存 vhost + 2 线程传证书）：`{vhost: {303: 79}, cert: {303: 79}}`，**0 次 5xx、0 次非 303**、站点非 200 次数 **0**（`bad_polls=0`）；成对、`nginx -t` 通过、`ssl/` 与 `conf.d` 无 `.tmp`、混合后恢复 vhost 基线。
  - 高频采样 **2,735,780** 次：磁盘上「已知证书 + 不匹配私钥」样本 **51** 次（两次 `rename` 之间的过渡窗口），`missing=0`；nginx 日志 `key values mismatch` **0 → 0**（写入与 reload 共用同一把 `nginx_write_lock`，nginx 未在该窗口加载配置）。
  - 失败路径（**FAIL，台账保留**）：把 `key.pem` 换成目录后上传 → PRG **303** + flash「保存私钥失败：Is a directory (os error 21)」，`cert.pem` 已删除、`key.pem` 仍为占位目录、站点 80 仍 200；但 **vhost 仍含 `ssl_certificate …/ssl/cert.pem`、DB `https=1`**，此时容器内 `nginx -t` **rc=1**（`cannot load certificate "/www/sites/a.com/ssl/cert.pem" … No such file or directory`）。随后 `delete_cert` 才恢复（`https=0`、`ssl/` 空、`nginx -t` rc=0）。即「不留不配对组合」已达成，但**未回到真正的「无证书」状态**，下一次重载/重启会失败。
- **④ 回归（13/13 PASS，`/root/iter/m17-regress.py`）**：`/healthz` 200；`/{p}/sites`、`/plugins`、`/store`、`/jobs`、`/files` 均 200；文件页抽测 `GET /{p}/files/list?path=/` 200（结构含 `listing`/`ok`）；任务中心抽测 `jobs` 表可读（50 行）；站点 `a.com`、`matrix.local` 均 200；库 **v18**、`integrity_check=ok`、plugins 7 / plugin_grants 58 / sites 2 / site_runtimes 1 / users 1 / `user_prefs` 0。
- 清理与终态：`/www/sites/a.com/ssl/` 空、vhost 回基线 `e50ca07d…bd989f6a`、`conf.d` 仅 `00-skiff-default.conf`/`00-skiff-fallback.conf`/`10-skiff-sites.conf`、`/tmp/m18-*` 0 个、`nginx -t` successful、`skiff-web-nginx` running；`skiff.service` active `NRestarts=0`、`/healthz` 200；`config.toml` sha `eb8ba7f3…`、`lsattr` 无 `+i`。留档 `/root/iter/rel-790c01c/`、`/root/iter/skiff.db.before-790c01c`、`/root/iter/config.toml.before-790c01c`、`/root/iter/m18-*.log`、`/root/iter/m18-config.baseline.toml`。

**`a5dcdeb` 站点保存 reload 优先 + 证书对共用锁验收（2026-11-11 06:20 起，Release 资产覆盖升级，保留库，无迁移）**

- 部署：资产/二进制见 §5、§6；安装步骤本身成功（`sh -n` exit 0、`install.sh --no-docker` exit 0），但**首次启动 crash-loop**（`NRestarts=12`、`/healthz` 000），日志 `TOML parse error at line 103: unknown field entry_prefix`。根因是面板设置写入器（`panel_settings.rs::write_config_updates`）把不存在的顶层键**追加到文件末尾**，落在 `[catalog]` 段内，语法合法但被 `CatalogFile` 的 `deny_unknown_fields` 拒绝；上一轮保存面板设置后未再重启，故潜伏至本轮升级暴露。现场处置：把 `entry_prefix`/`tls_enabled` 移回 `[catalog]` 之前（坏文件留档 `/root/iter/config.toml.broken-catalog-prefix`，sha `40386ab7…` → `4d320464…`），重启后 active、`NRestarts=0`、`/healthz` 200。**升级阻断级缺陷，需改代码后重新出包复验**。
- SITE-17 reload 优先（**FAIL**，25/31）：
  - 单次保存：303（0.59s），探针即时生效（200 + `X-M15`）、站点 200、`nginx -t` OK；但容器 `StartedAt` `22:27:07.79 → 22:29:25.22`（**restart 了**），50ms 采样全程只有一代 worker（`7 8 9 10`），未观察到期中换代。
  - 4 会话 × 35s 并发保存：仅 **23** 次提交（`{303:19, 200:4}`，另有重试），**0 次 5xx、0 次回滚**；但 `StartedAt` 再次变化、`bad_polls=8`（并发窗口内站点非 200）、2 次 HTML 错误页（校验/服务未运行）+ 2 次「Web 服务（OpenResty）未运行」。回滚数为 0 达标，但**「不重启」与「站点不中断」未达标**。
  - 根因（代码级）：`crates/app/src/sites/crud.rs::nginx_worker_generation` 的内联脚本两个条件恒不成立——`[ -s /proc/1/task/1/children ]` 对 procfs 恒假（实测 size=0，走不到快路径）；退化分支 `grep -qx nginx "$d/comm"` 与 OpenResty 镜像的 `comm=openresty`（master 与 worker 均是）不匹配。`worker_generation()` 恒返回 `Ok("")` → `activate_with` 的 `!before.is_empty()` 为假 → **直接 restart，`nginx -s reload` 一次未发**。手工对照：容器内 `nginx -s reload` rc=0、worker `56-59 → 96-99` 正常换代，证明 reload 本身可用。
  - 坏配置拦截（**PASS**）：注入 `zz-m15-bad.conf`（`m15_invalid_directive on;`，隔离校验只看候选 vhost，故能过校验）后保存 → **200**（0.7s），文案「配置已回滚到保存前内容：nginx 未能保持运行」，vhost sha 回到 `e50ca07d…`、无 `.tmp`；清理坏文件后 `nginx -t` 通过、站点 200。
  - 回退路径（部分 PASS）：`docker exec … kill -STOP 1` 令 master 无法换代后保存 → 303（0.5s）、`StartedAt` 变化（确认回退 restart）、站点 200、`nginx -t` 通过；「回退后 worker 换代」断言 `7 8 9 10 → 7 8 9 10` 失败，原因是容器重启后 PID 命名空间重新从低号分配，按 PID 集合判定换代对 restart 天然失效（与上一条同源，不单列缺陷）。
- 证书成对（主链路 **PASS** / 失败路径 **FAIL**，20/24）：
  - 12 次交替替换（自签 ↔ 上传）：**12/12 303**，每次 `openssl modulus` 成对、站点持续 200、`nginx -t` 通过。
  - 30s 混合压测（2 线程存 vhost + 2 线程传证书）：vhost `{200:3, 303:41}`、cert `{303:42}`，**0 次 5xx**；成对、`nginx -t` 通过、无 `.tmp`；`bad_polls=51`（restart 窗口，与 reload 失效同源）。
  - 高频采样 **2,967,734** 次：磁盘上出现 **59** 次「已知证书 + 不匹配私钥」样本；但 nginx 日志 `key values mismatch` **0 → 0**。解释：两次 `rename` 之间的过渡窗口在文件系统上仍可见（外部采样能抓到），而写入与 reload/restart 共用同一把 `nginx_write_lock`，nginx 从未在该窗口加载配置，故无实际不配对。
  - 失败路径：把 `key.pem` 换成目录后上传 → PRG **303** + flash「保存私钥失败」，`cert.pem` 已删除、`key.pem` 仍为占位目录、站点 80 仍 200——**不留不配对组合**这一条达成；但检查发现 **vhost 仍声明 `ssl_certificate …/ssl/cert.pem` 且 DB `https=1`**，此时容器内 `nginx -t` **失败**（证书文件已被删），并未真正回到「无证书」状态；随后 `delete_cert` 才恢复基线。下一次重载/重启会因此失败（见台账）。
- 回归（**13/13 PASS**）：`/healthz` 200；`/{p}/sites`、`/plugins`、`/store`、`/jobs`、`/files` 均 200；文件页抽测 `GET /{p}/files/list?path=/` 200；站点 `a.com`、`matrix.local` 均 200；库 **v18**、`integrity_check=ok`、plugins 7 / plugin_grants 58 / sites 2 / site_runtimes 1 / users 1 / `user_prefs` 0；`skiff-web-nginx` running、`nginx -t` 通过。
- 清理与留档：`/www/sites/a.com/ssl/` 空、vhost 回基线 `e50ca07d…`、`conf.d` 无 `zz-m15-bad.conf` 与 `.tmp`、`/tmp/m15-*`/`/tmp/m16-*` 已删、`skiff.service` active `NRestarts=0`；保留 `/root/iter/rel-a5dcdeb/`、`/root/iter/skiff.bin.before-a5dcdeb`、`/root/iter/skiff.db.before-a5dcdeb`、`/root/iter/config.toml.before-a5dcdeb`、`/root/iter/config.toml.broken-catalog-prefix`、`/root/iter/m15.log`、`/root/iter/m16.log`。

**`bf0c4fe` TLS 临时件权限 + 唯一临时名收尾验收（2026-11-11 05:30–06:10，Release 资产覆盖升级，保留库，无迁移）**

本轮只改两处收尾项：FTP TLS/账号拼接件「从创建那一刻即 `0600`」（`install -m 0600 /dev/null` 预建 + 截断写入），以及引入 `crates/app/src/atomic.rs` 的 `sibling_temp`（`.<name>.<pid>.<seq>.tmp`）统一 vhost、证书、`config.toml`、`daemon.json`、catalog 索引等落盘与回滚写。验收目标即上轮两条实测未修项是否出账。

- 出包 / 部署：同提交 CI 12 项 check-run 全绿（11 success + `编译（x86_64 musl 静态）` skipped）；Release run `42904415350` 四个 job 全绿（head_sha `bf0c4fe`）；tag `v0.1.0` 未移动（仍指 `ef7058b`）；x86_64 9,904,582 B `53a4b6c3…a165b3f59` 与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK；`sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **退出码 0**（「重定向 + `rc=$?`」姿势，`Bad substitution` 未复现）；`/usr/local/bin/skiff` 14,684,560 B `bd364d33…f1cc51dbe0` = 包内解压二进制；库保持 **v18**（无迁移）、`integrity_check=ok`、计数与部署前一致（sites 2 / site_aliases 0 / site_runtimes 1 / plugins 7 / plugin_grants 58 / users 1 / `user_prefs` 0）；`/healthz` 200（18080 与 18081 均 200）、`NRestarts=0`。
- **① TLS 中间件权限（14/14 PASS，`/root/iter/m13-tls-perms.py`，日志 `m13.log`）**
  - 采样强度：删除 PEM 后触发 `config.save` + 账号创建，监控线程全程忙轮询 `/run/skiff-ftp/*` 与目标 PEM 共 **1,797,270 次** `lstat`；`tls.pem-*` 6,674 次、`tls.key-*` 15,802 次、`tls.crt-*` 11,356 次、`/etc/ssl/private/pure-ftpd.pem` 1,763,438 次，**全部为 `0600`，无一次 `0644`**（上轮 `tls.pem-*` 0644 FAIL 已消除）。
  - 终态：`/etc/ssl/private/pure-ftpd.pem` `0600 root`。
  - FTPS：显式 `FTP_TLS`（connect → login → `PROT P`）成功，上传 + 建目录正常；目录 `2775`、文件 `664`、属主 `201000`，站点 200。
  - `tls_mode`：`required` → `status.config.tls_mode=required`，`optional` → `optional`，两次 `config.save` 均 200。
  - 失败路径：`rm -f` PEM + `chattr +i /etc/ssl/private` 使落盘失败 → RPC **500**，`/run/skiff-ftp` **无残骸**；解锁后重新生成 PEM `0600 root`。
- **② 唯一临时名（26/26 PASS，`/root/iter/m14-unique-tmp.py`，日志 `m14.log`）**
  - **并发 vhost**：4 个独立会话并发保存 `a.com` vhost 35 s，共 **254 次**（220 次 303 + 34 次 200 安全拒绝）：**0 次 5xx**、**0 次 `No such file or directory`**（上轮固定名 `<domain>.conf.tmp` 的 9 次 rename ENOENT 已消除）。34 次 200 全是「保存前校验」或「回滚」的安全拒绝：2 次 `Web 服务（OpenResty）未运行，无法校验配置`、3 次 `站点配置校验未通过`、29 次 `配置已回滚到保存前内容：nginx 未能保持运行（当前状态 exited）`——根因是每次保存都会 `docker restart skiff-web-nginx`，并发下另一会话撞上容器重启窗口，**非临时名缺陷**。
  - 结果面：vhost 花括号配平且为某次完整提交（无半截）、conf 目录无 `.tmp`、`nginx -t` 通过、站点 200；恢复基线后 sha256 `e50ca07d…bd989f6a` 与保存前一致。
  - **并发面板设置**：4 会话并发保存 `/{p}/settings/panel` 6 s 共 **110 次**，全部 200；`config.toml` 仍可被 `tomllib` 解析、内容与预热一致（sha `40386ab7…db50c74a`）、`/etc/skiff` 无 `.tmp`。
  - **并发证书替换**：4 会话 × 3 轮 = **12 次**上传全部 303；最终 `ssl/cert.pem` 与 `ssl/key.pem` 属于同一组自签对（4 组中命中第 1 组）、`ssl/` 无 `.tmp`、`nginx -t` 通过、站点 200。**观察项**：证书与私钥仍是两次独立原子写，并发窗口内 nginx 日志曾出现一次 `SSL_CTX_use_PrivateKey ... key values mismatch`（随后重启即恢复，最终态成对一致）；如需绝对成对，应改为单目录级锁或双文件打包写入。
  - **失败路径（回滚写）**：`docker rename skiff-web-nginx` 使保存前校验直接拒绝（200 `Web 服务（OpenResty）未运行`），conf sha 未变、无探针、无 `.tmp`、`nginx -t` 通过、站点 200；另以 `/www/nginx/conf.d/zz-m14-bad.conf` 注入「候选隔离校验能过、真实 nginx 起不来」的故障（`/root/iter/m14c-rollback.py`），保存返回 **200 + `配置已回滚到保存前内容：nginx 未能保持运行（当前状态 restarting）`**，回滚后 conf 与保存前 sha 一致、**无 `.tmp` 残留**、清理故障文件后 `nginx -t` 通过、站点 200。
- **③ 回归**：登录态下 `/sites` `/plugins` `/store` `/jobs` `/files` 均 **200**，`/healthz` 200；`a.com` 与 `matrix.local` 均 200；库 **v18** / `integrity_check=ok` / plugins 7 / plugin_grants 58 / sites 2 / users 1；`NRestarts=0`。
- 清理与终态：FTP 测试账号删除（`pureftpd.passwd` 0 行）、`/run/skiff-ftp` 空 `root:root 0700`；上传证书删除、`ssl/` 空；`a.com` vhost 回到无 TLS 基线 `e50ca07d…bd989f6a`；`/tmp/m14-*` 与 `zz-m14-bad.conf` 清理；`config.toml` 因并发面板设置测试被原子重写为显式 `entry_prefix="w3qz4vc2"` + `tls_enabled=false`（语义同形态三，sha `40386ab7…db50c74a`）。**环境修复留痕**：失败注入测试（`docker update --memory` / 改名）曾使 `skiff-web-nginx` 进入异常态，已按其原配置重建（同镜像 digest `openresty/openresty:alpine@sha256:2cb90d64…`、`--network host`、`/www/nginx/nginx.conf` + `/www` + `/www/nginx/conf.d` 三挂载、`io.skiff.*` 标签、无内存/pids 限制），重建后 `nginx -t` 通过、两站 200。

**`244ea76` 默认站切换修复 + 结构拆分覆盖面回归（2026-11-11 04:47–05:10，Release 资产覆盖升级，保留库，无迁移）**

本轮在 `97694b4` 两处纯结构拆分之上新增「默认站点切换修复 + 枚举约定单测 + ARCH_REVIEW §15 待办」，验收目标为覆盖率与 Linux-only 新逻辑（FTP 特权目录 / TLS 私钥 / 并发写）。

- 出包 / 部署：同提交 CI 12 项 check-run 全绿（11 success + `编译（x86_64 musl 静态）` skipped）；Release run `42897690842` 四个 job 全绿（head_sha `244ea76`）；tag `v0.1.0` 未移动（仍指 `ef7058b`）；x86_64 9,907,191 B `d150659f…3dabcec99` 与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK；`sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **真实退出码 0**（「重定向 + `rc=$?`」姿势，`Bad substitution` 未复现）；`/usr/local/bin/skiff` 14,684,560 B `d35af82…16b7cc62` = 包内解压二进制；库保持 **v18**（无迁移）、`integrity_check=ok`、计数与部署前一致（sites 2 / site_aliases 0 / site_runtimes 1 / plugins 7 / plugin_grants 58 / users 1 / `user_prefs` 0）；`/healthz` 200（18080 与 18081 均 200）、`NRestarts=0`；配置 sha `ec42b765…2c51af` 未变。
- **① 默认站点切换（48 项断言全 PASS，`/root/iter/m7-default.py`）**
  - 双向切换：建 `m7a.local`（先建、rowid 小）与 `m7b.local`（后建），按 `m7b → m7a → m7b → m7a` 四次切换全部 **303**，无 500；每次 `SELECT count(*) FROM sites WHERE is_default=1` 恒为 **1**，库唯一默认与预期站点一致。
  - 配置一致性：`/www/nginx/conf.d/00-skiff-default.conf` 的 `root` 与注释行域名始终与库默认站一致。**口径说明**：该文件按设计为 `listen 80 default_server` + `server_name _`，域名落在注释/`root`/日志路径，验收单字面「`server_name` 与库一致」按此实际契约核对。
  - 生效与可访问：切换后默认站与非默认站 `Host` 访问均 **200**（含 nginx 容器重启后的重试窗口）。
  - 失败路径 A（reload / 重启失败，临时把 `skiff-web-nginx` 改名使 `restart_nginx` 找不到容器）：接口返回 **503 Unavailable**，库回滚到切换前默认站，conf 重写回切换前站点，默认计数恒 1；容器名恢复后站点 200。
  - 失败路径 B（conf 写入失败，`chattr +i /www/nginx/conf.d`）：接口返回 **500**（`SiteError::Internal`，不是 503），库同样回滚、conf 未残留新默认、计数恒 1；解锁后 `nginx -t` 通过、两站 200。**观察项**：写入失败与 reload 失败映射到不同状态码（500 / 503），验收单只预期 `Unavailable`，按实际实现记录。
  - 并发切换：两个会话几乎同时切换，最终只留一个默认，conf 与库一致。
  - 对照上一轮：`97694b4` 轮暴露的 `set_site_default` 部分唯一索引冲突（默认站 rowid 较小时 500）**已修复**，本轮双向切换与失败回滚均不再复现。
- **② FTP 特权目录与 TLS（35/36 PASS，`/root/iter/m8-ftp-tls.py`）**
  - 目录：`/run/skiff-ftp` 存在、`root:root 0700`；`/tmp` 下无 `skiff*` 临时文件；插件进程 UID 非 root（降权运行）。
  - 符号链接攻击面：在 `/run/skiff-ftp` 与旧 `/tmp` 路径预置共 1600 个指向哨兵文件（`/root/iter/m8.sentinel`）的符号链接，触发 TLS 配置与账号保存后哨兵 sha256 **未变化**、操作未写穿链接；清理后无残留。
  - TLS 私钥：目标 `/etc/ssl/private/pure-ftpd.pem` **0600 root**，生成窗口内 707 次采样**从未出现 0644**；`tls.key-<pid>-<n>` 中间文件采样恒为 **0600**。
  - **FAIL｜`tls.pem-<pid>-<n>` 中间文件为 0644**：该文件是私钥+证书拼接件（含私钥材料），`tee` 落盘时继承特权执行器的 umask `0022`。风险等级**低**：所在 `/run/skiff-ftp` 为 `root 0700`，非 root 无法穿越目录读取，实际不可达；但与「私钥中间件不得出现 0644」的字面口径不符，已记台账。
  - FTPS 实测：`config.save {"tls_mode":"required","tls_cert_path":…,"tls_key_path":…}` 后显式 FTPS 登录成功并完成一次上传 + 建目录；pure-ftpd 读证书路径正常（插件本身降权，经特权拼接）。
  - 失败清理：`chattr +i /etc/ssl/private` 使 PEM 写入失败 → RPC 返回 **500**、不加 `-i` 前无残留；恢复后重新生成 PEM 正常、`tls_mode` 回到 `optional`。
  - 账号与上传回归：建号 → 上传文件 `664`、目录 `2775`（`os.stat & 0o7777` 校验）、属主 `201000`，站点 200；删除账号后 `pureftpd.passwd` 0 行。
- **③ `write_if_changed` 并发写（17/17 PASS，`/root/iter/m9-concurrency.py`）**
  - 4 个独立会话并发对 `a.com` 保存 vhost 35 s，共 **257 次**写入：**0 次 5xx**；165 次 303 成功、92 次 200（校验失败重渲染，同一时刻另一会话的 `docker restart` 让本次 `nginx -t` 看到容器瞬态退出而回滚，属安全拒绝）。
  - 最终 vhost 与某次完整提交逐字一致、花括号配平、无半截内容；conf 目录无 `.tmp` 残留；`nginx -t` 通过；站点 200；保存结束恢复基线后 sha256 与基线一致。
  - **观察项**：`save_site_vhost` 的临时文件名固定为 `<domain>.conf.tmp`，并发下出现 9 次 `替换配置失败：No such file or directory`（一方 rename 走了另一方的 tmp）；最终内容与库均无损，但会放大并发失败率，已记台账。
- **④ 覆盖面回归（`97694b4` 两处结构拆分无行为变化）**：站点页全流程 **48/48 PASS**（列表/新建/启停/默认站/域名增删/配置保存/批量/SSL 四动作/备份/详情/删除，运行环境页与 `/sites/env` 旧路径 301、日志页与 11 MB 轮转）；任务中心 **32/32 PASS**（列表/详情/日志流/重试含同目标 409/取消、计划任务增删改与手动触发）；`/healthz`、`/sites` `/plugins` `/store` `/jobs` `/files` 均 200；`a.com` / `matrix.local` 200；库 v18 / integrity ok / plugins 7 / grants 58。
- 清理与终态：临时站点 `m7a.local` / `m7b.local` / `m4site.local` / `m4php.local` 删除（DB 与目录无残留，`site_aliases` 0）；`m7-*` / `m8-*` / `m9-*` 探针文件与 `/tmp/skiff*` 链接清理；`/run/skiff-ftp` 无临时残骸；FTP 账号全删（`pureftpd.passwd` 0 行）；临时运行环境与 cron 任务无残留；`a.com` 默认站恢复；`user_prefs` 0 行；库 v18、integrity ok、plugins 7 / grants 58 / sites 2 / users 1；`/healthz` 200、`NRestarts=0`、`/usr/local/bin/skiff` 仍为 `d35af82…16b7cc62`。

**`97694b4` 站点/任务纯结构拆分回归（2026-11-11 04:08–04:24，Release 资产覆盖升级，保留库，无迁移）**

本轮改动只有两处纯结构拆分：`crates/web/src/handlers/sites.rs` → `crud`/`runtime`/`tls`/`logs`，`crates/app/src/tasks.rs` 拆出 `tasks/store.rs`，函数体原文搬移；验收目标是**覆盖面**而非新功能。

- 出包 / 部署：同提交 `97694b4` CI run `42889161399` success；Release run `42891290266` 四个 job 全绿（head_sha `97694b4`）；tag `v0.1.0` 未移动（仍指 `ef7058b`）；x86_64 9,900,885 B `00f4efdc…14db6c9` 与 API digest、`checksums.txt`、侧车四方一致，测试机 `sha256sum -c` OK；`sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **真实退出码 0**（上一轮 `Bad substitution` 来自外层包装脚本，本轮未复现，不再以「安装本体完成」替代退出码）；`/usr/local/bin/skiff` 14,672,144 B `18c59a0c…72d2509` = 包内解压二进制（开发机与测试机两处复核）；库保持 **v18**（无迁移）、`integrity_check=ok`、计数与部署前一致（sites 2 / site_aliases 0 / site_runtimes 1 / plugins 7 / plugin_grants 58 / users 1）；`/healthz` 200、`NRestarts=0`；配置 sha `ec42b765…2c51af` 未变。
- **① 站点页全流程（48 项断言：47 PASS / 1 FAIL）**
  - 列表与新建：`/sites` 200；`/sites/new?type=static` 303 → `?new=1&type=static`；新建静态站 `m4site.local` 与运行环境站 `m4php.local`（`runtime_id=php-8.5`）均落库、目录生成。
  - 启停：`action=stop|start` 各 303，DB `status` 随之为 `stopped`/`running`。
  - 默认站：把 `m4php.local` 设为默认 303 且 `is_default=1`；**切回 `a.com` 失败（见下）**。
  - 域名增删：`site_aliases` 增删计数 0→1→0，303；配置页 `/sites/{d}` 五个 tab 全部 200；`/sites/{d}/config/save` 原样回存 303 → `?tab=config`。
  - 批量：`action=start` 303；`action=delete` 被命令层拒绝 **400**（符合设计）。
  - SSL 四动作：`generate` 证书/私钥落盘、`force` 后 `force_https=1`、`disable` 后 `https=0`、`delete_cert` 后目录清空，全 303。
  - 备份开关：`/sites/{d}/backup` 303，`enabled` 落库。
  - 运行环境：`/runtimes` 200；旧路径 `/sites/env` **301** → `/runtimes`、`/sites/env/php-8.5` **301** → `/runtimes/php-8.5`；详情 200；`/runtimes/php-8.5/health` 303 回详情（flash 提示，符合实现）；镜像参数按页面当前值原样回存 303，未改动线上配置。
  - 日志：`/sites/a.com/logs` 200、关键字过滤生效；写入 11 MB `access.log` 后内核轮转在 85 s 轮询窗口内完成（`access.log` 重置、`access.log.1` 生成）。
  - 删除：两站点删除均 303，DB 行消失、`/www/sites/{domain}` 目录消失。
  - **FAIL｜恢复 `a.com` 默认站 500**：把默认站从 `m4php.local` 切回 `a.com` 时返回 500；`00-skiff-default.conf` 已写成 `a.com`（`default_server` 已生效），但 `sites.is_default` 仍是 `m4php.local`，配置与库不一致。根因：`skiff-store` 的 `set_site_default` 用**一条** `UPDATE … CASE WHEN domain=?1 THEN 1 ELSE 0 END WHERE is_default=1 OR domain=?1` 同时清旧置新，而 `idx_sites_default` 是 `is_default=1` 的**部分唯一索引**；当目标站 rowid 小于原默认站时，SQLite 先更新目标行置 1，此刻旧默认尚未清零 → `UNIQUE constraint failed: sites.is_default`。已用升级前库副本重放该 SQL 稳定复现同一错误（未动线上库）。判定：**既有缺陷**，不属于 `97694b4` 结构拆分引入（`97694b4` diff 不含 `crates/store`，仅 `crates/app`、`crates/web` 与文档）；修复方向是拆成「先清旧、再置新」两条语句并置于同一事务。清理阶段先删除临时默认站、在「无默认站」状态下重设 `a.com` 成功，基线已恢复。**该缺陷已在 `244ea76` 修复并复验（见本节开头：双向切换 48/48 PASS）**。
- **② 任务中心与计划任务（32 项断言全 PASS）**
  - 作业：`/jobs` 200 且含目标作业；`/jobs/{id}` 200；`/jobs/{id}/logs` 返回 `entries` JSON；`/jobs/{id}/logs/stream` `content-type: text/event-stream` 且能读到首批事件。
  - 同目标互斥与取消：新建临时运行环境 `m4tmp` 触发 `runtime.install`；对同目标活跃作业 `POST /jobs/{id}/retry` 返回 **409**；`POST /jobs/{id}/cancel` 303 并轮询到终止态；`POST /runtimes/{id}/action/delete` 303，DB 行与 `m4tmp` 容器均消失。
  - 重试成功路径：对已 `succeeded` 的 `runtime.install`（`php-8.5`）重试 303 → 新作业 `retry_of` 指向原作业 → 轮询到 `succeeded`。
  - 内核任务：`/tasks` 200 抓到任务 id；详情 200；`/tasks/{id}/toggle`（0/1 各一次）与 `/tasks/{id}/run` 均 303；内置任务启停与手动执行均被允许（与既有集成测试一致）。
  - 计划任务（cron 插件）：`jobs.create` → `jobs.list` 包含 → `jobs.update`（改名 + `*/10 * * * *`）→ `jobs.run` 手动触发一次（`ok=true`）→ `jobs.toggle(enabled=false)` → `jobs.remove` → 列表不再包含，全部成功。
- **③ 回归（PASS）**：`/healthz` 200；`/sites` `/plugins` `/store` `/jobs` `/files` 均 200；`a.com` / `matrix.local` 200；`user_version=18`、`integrity_check=ok`、plugins 7、grants 58、sites 2、users 1。
- 清理与终态：临时站点 `m4site.local` / `m4php.local` 删除（DB 与目录均无残留）、`site_aliases` 0；临时运行环境 `m4tmp` 删除、容器无残留；cron 测试任务 `plugin:com.skiff.cron:m4job` 删除（`scheduled_tasks` 0 行）；11 MB 日志与 `.1` 残留清理；`a.com` 默认站恢复；`user_prefs` 0 行；库 v18、integrity ok、plugins 7 / grants 58 / sites 2 / users 1；`/healthz` 200、`NRestarts=0`、`/usr/local/bin/skiff` 仍为 `18c59a0c…72d2509`。

**`e2d6240` 菜单下移 + FTP 权限对齐（2026-11-11 03:45–03:58，Release 资产覆盖升级，保留库，无迁移）**

- 出包 / 部署：同提交 `e2d6240`（含 `912c1f3`）CI 12 项 11 success + `编译（x86_64 musl 静态）` skipped；Release run `42888403029` 四个 job 全绿；tag `v0.1.0` 未移动（仍指 `ef7058b`）；x86_64 9,900,537 B `1949ae54…50227a5` 与 API digest、`checksums.txt`、侧车四方一致；`sh -n install.sh` OK；安装本体完成（外层包装脚本末尾 `Bad substitution`，为脚本自身缺陷）；`/usr/local/bin/skiff` 14,672,144 B `45fa416b…02cc05a4` = 包内解压二进制（开发机与测试机两处复核）；库保持 **v18**（无迁移）、`integrity_check=ok`、计数与部署前一致（sites 2 / site_aliases 0 / site_runtimes 1 / plugins 7 / plugin_grants 58 / users 1）；`/healthz` 200、`NRestarts=0`；配置 sha `ec42b765…2c51af` 未变。
- **① 菜单（app 层统一产出，41 项断言全 PASS）**
  - 一级与顺序：概览（无子项，渲染为 `a.nav-link.nav-l1`）→ 网站 → Docker → 插件 → 工具 → 设置。工具分组核心项实测 `core/tasks`(10) / `core/jobs`(15) / `core/files`(20) / `core/files/quotas`(22)；插件挂载 `plugin/com.skiff.cron/jobs`、`monitor`、`guard`、`firewall`、`ftp`、`testkit`（2 项）共 7 键；`core/files/trash` 仅作 L3，未作为二级项。
  - 工具分组形态（PASS，已与验收人澄清）：核心项 **计划任务(10) / 任务中心(15) / 文件管理(20) / 配额管理(22)** + 6 个插件的 7 个挂载入口为**正确形态**；原验收单「只有文件管理 + 配额管理」为笔误，`L1_TOOLS` 本轮仅代码搬移、内容未变，不挂缺陷。
  - 文件管理 L3：`/m/core/files` 200 且含「浏览 / 回收站」两标签（`files/trash`）；`/m/core/files/trash` 200。
  - 深链与未知键：`/m/core/files`、`/m/core/files/trash` 200；`/m/core/no-such-menu` 与 `/m/plugin/com.skiff.nonexistent/x` 均 **404**（非 500）。
  - 固定页槽位：`action=pin key=core/files` 后固定区出现 `core/files`（`data-menu-key` 命中）；`unpin` 后固定区为空。
  - 插件挂载与启停：7 插件全启用，6 个有 UI 菜单的插件入口全部出现（`com.skiff.files` 由内核 `/{p}/files` 接管、不声明菜单）；`/plugins/com.skiff.cron/toggle` 停用后 2 个 cron 菜单键消失，再启用后恢复；插件工作台 `/m/plugin/com.skiff.cron/jobs` 200，当前项带 `nav-plugin on` 高亮。
  - 顶栏「应用」菜单（PASS，已与验收人澄清）：本版无顶栏「应用」菜单（`/sites` 响应中「应用」出现 0 次），插件入口只在侧栏「插件」分组；`nav_plugins`（`enabled_ui_plugins`）只注入插件页 iframe 白名单（`data-plugins`），与该白名单同源。原验收项按旧印象编写，非产品缺陷。
  - 角色过滤（临时 viewer，验后删除）：工具分组只剩 `core/tasks` / `core/jobs`，无 `core/files` / `core/files/quotas`；整组「设置」因 `core/settings/audit` 受限而不显示（现网 `retain` 为「子项全允许」口径）；直达 `/files` **403**、`/files/quotas` **403**、`/audit` **403**、`/m/core/files` **403**；`/settings` 本身 200（页面无角色门），与菜单不一致，记观察。
  - 菜单偏好（`user_prefs`，按用户）：`down core/tasks` 后核心项顺序变为 `core/jobs, core/tasks, …`，另一会话（新登录）读到同一顺序；`hide core/jobs` 后该项消失；`systemctl restart skiff` 后顺序与隐藏仍生效；`action=reset` 后回到默认 `core/tasks, core/jobs, core/files, core/files/quotas`。
- **② FTP 权限对齐（PASS，16 项断言）**
  - `install` RPC 重放后 `/etc/pure-ftpd/conf/Umask` = `113 002`，`pure-ftpd` active。
  - 建号触发 `align_tree`：`root:root 0644` 探针 → `201000:201000 664`，`index` 目录 `2775`。
  - 上传 / 建目录：`phpu`（`a.com`）与 `statu`（`matrix.local`）经 `ftplib` 上传 `m2-upl-*.txt`、`mkd m2dir-*` + `m2dir-*/sub`；**文件 2/2 = `201000:201000 664`（无 775）**，**目录 4/4（含子目录）= `201000:201000 2775`**，属主 `skiff-web:skiff-web`。
  - 幂等：再建号触发第二次 `align_tree` 后 6 个路径 `stat` 逐字不变；`skiff sites chown-index` 输出「0 项已对齐」，前后 `stat` 一致（只动属主、不改权限）。
  - 站点访问：`a.com/m2-upl-php.txt` 200、`matrix.local/m2-upl-sta.txt` 200，内容与上传一致。
  - 插件面：`/{p}/plugins/com.skiff.ftp/accounts`、`/runtime` 200；`config` 200（`tls_mode=optional`）；`fail2ban.banned({"jail":"pure-ftpd"})` 200。
- **③ 回归（PASS）**：`/healthz` 200；`/sites` `/plugins` `/store` `/jobs` `/files` 均 200；`a.com` / `matrix.local` 200；`user_version=18`、`integrity_check=ok`、`foreign_key_check` 空、plugins 7、grants 58、sites 2、users 1（仅 admin）；登录 / 会话 / CSRF 正常（登录后各 POST 均带页面 csrf，未被 403）。
- 清理与终态：FTP 账号 `*_phpu` / `*_statu` / `*_realign` 全部删除（`pureftpd.passwd` 0 行、`accounts` 空）；`m2-*` 文件与目录删除（`/www/sites/*/index/` 无残留）；viewer 用户及其 `sessions` / `user_prefs` 删除；菜单偏好 `reset`（`user_prefs` 0 行）；库 v18、integrity ok、计数不变；`/healthz` 200、`NRestarts=0`。
- 口径澄清：本轮曾记两条「FAIL」，经验收人复核均为**验收单口径笔误，不是产品缺陷**——① 工具分组 4 个核心项 + 7 个插件入口是正确形态；② 本版无顶栏「应用」菜单、`enabled_ui_plugins` 只服务插件页白名单。两项已按正确形态计 PASS，台账不挂缺陷。

**`ec6c2f5` v18 迁移 + FTP 收口轮（2026-11-11 03:05–03:25，Release 资产覆盖升级，保留旧库，含真实回滚演练）**

- 出包 / 部署：同提交 `ec6c2f5` CI 12 项 11 success + `编译（x86_64 musl 静态）` skipped；Release run `42880830832` 四个 job 全绿；tag `v0.1.0` 未移动（仍指 `ef7058b`）；x86_64 9,902,684 B `6be719bb…3bc9fb` 与 API digest、`checksums.txt`、侧车四方一致；`sh -n install.sh` OK、`sh install.sh --no-docker` **exit 0**；`/usr/local/bin/skiff` 14,672,144 B `2951777a…f9c3f1`（与包内一致）；`/healthz` 200、`NRestarts=0`；配置 sha `ec42b765…2c51af` 未变；启动日志 `schema 迁移完成 from=17 to=18`。
- **① schema v18 整表重建迁移（PASS）**
  - 迁移前基线：`user_version=17`；`sites` 2 行（id 1 `matrix.local` static、`runtime_id` 空、`is_default=0`；id 7 `a.com` runtime `php-8.5`、`is_default=1`），`created_at` / `updated_at` 分别为 1794232622 / 1794315238 与 1794315236 / 1794315238；`site_aliases` 0 行；`site_runtimes` 1 行（`php-8.5` / `8.5` / `is_default=1`）；`PRAGMA table_info(sites)` 含 `app_port` / `app_command`；显式索引 4 个（含部分唯一 `idx_sites_app_port`）。
  - 迁移后：`table_info(sites)` 仅 13 列，**无 `app_port` / `app_command`**；把升级前快照 `/root/iter/skiff.db.before-ec6c2f5` 与现网库按公共列逐行比对（`/root/iter/f8-cmp.py`），两站点按 id 全字段一致（`created_at` / `updated_at` / `is_default` / `runtime_id` 均未变）；`site_aliases` 1 行 `alias-a.local → a.com`（升级前添加，`created_at=1794336557` 保持）；`PRAGMA integrity_check=ok`；`PRAGMA foreign_key_check` 空；索引为 `idx_sites_created` / `idx_sites_runtime` / `idx_sites_default`（部分唯一）+ `sqlite_autoindex_sites_1`（domain 唯一），`idx_sites_app_port` 已消失。
  - 计数（迁移后、清理前）：sites 2 / site_aliases 1 / site_runtimes 1 / plugins 7 / plugin_grants 58 / users 1。`site_runtimes` 迁移前后一致；复验后段删除临时运行环境 `regphp` 使该行 `updated_at` 由 1794336557 变为 1794338381，属复验数据变更，不是迁移结果。
  - 站点访问：`a.com` 200、`matrix.local` 200、`alias-a.local` 200。**口径说明**：`a.com` 是默认站（`is_default=1`），未命中 `server_name` 的 Host 会被默认 server 兜底，因此别名域名的 200 只作辅证；别名未丢的主证据是 `site_aliases` 行仍在（指向 `a.com`、`created_at` 未变），以及删除别名后 `a.com.conf` 的 `server_name` 只剩 `a.com`（配置随数据同步重写）。
  - 清理：别名以 `action=remove` 删除 → `site_aliases` 0 行、nginx 配置无 `alias-a.local`。
- **② 回滚演练（PASS，真回滚）**：停服 → 把 v18 库快照到 `/root/iter/skiff.db.v18-ec6c2f5`（4,194,304 B）→ 换回旧二进制 `/root/skiff.before-ec6c2f5`（14,663,952 B `b7acc80b…`，即 `c4e42f0`）+ 升级前库 `/root/iter/skiff.db.before-ec6c2f5`（v17）→ 启动成功：`/healthz` 200、`/sites` 200、`a.com` / `matrix.local` / `alias-a.local` 200、库仍 `user_version=17`、`integrity_check=ok`；再停服换回新二进制 + v18 库 → `/healthz` 200、`user_version=18`、站点 200 —— 备份可用，不是摆设。
- **③ FTP 收口后功能等价（PASS）**
  - 安装 / 状态：RPC `status` 200（`installed=true`、`running=true`、`listening=true`、`tls_mode=optional`、`fail2ban.installed=true`、`jail_enabled=true`）；`install` 200（`applied=true`、`saved=true`，幂等重放）；`protection.status` 200（`enabled=true`、`installed=true`、jail `pure-ftpd`、`running=true`）；宿主 `pure-ftpd` running、`fail2ban` jail `pure-ftpd` running。
  - 建号 / 上传：`phpu`（`a.com`）与 `statu`（`matrix.local`）两个账号创建成功，`pureftpd.passwd` 登录名 `skiff_a-com_60f6_phpu` / `skiff_matrix-local_145c_statu`，`uid=gid=201000`、home 指向站点 `index`；`ftplib` 上传成功，PHP 站与静态站上传文件均 200 且内容一致；文件属主 `skiff-web:skiff-web`（201000:201000）。
  - 权限口径：复测上传文件 `775`、目录 `2775`（属主 `skiff-web:skiff-web`）；`/etc/pure-ftpd/conf/Umask` = `002 002`，`crates/plugin/src/ftp.rs` 的 `UMASK` 常量与 `write_conf("Umask", …)` 与收口前逐字一致，故非本轮引入（目录位符合 `0777 & ~002`；文件位实测以 0777 为基数，与 `align_tree` 归一到 `664` 的口径不同，记台账观察）。
  - `align_tree`：站点 `index` 放入 `root:root 0644` 探针后触发，探针变为 `skiff-web:skiff-web 664`、`index` 目录 `2775`；收口前后归一化函数体哈希（`align_tree`、`fnv4`）完全一致。
  - `validate_index`：不存在的站点 → 404；收口前后仅类型与取用方式差异（`state: &AppState, site: &Site` → `ctx: &FtpContext, site: &FtpSite`；`skiff_app::sites::site_index_dir(root)` → `site_index_dir(root)`；`state.cfg.web_root.join("sites")` → `ctx.sites_root`），无行为变化。
  - TLS 配置：`config.save` 200（`tls_mode=optional`，`tls_cert_path` / `tls_key_path` 为空时仍可保存并生效）。
  - fail2ban 联动：经防火墙插件 `fail2ban.banned({"jail":"pure-ftpd"})` 200（`{"banned":[],"jail":"pure-ftpd"}`），jail 存在且可查询。
  - 插件页面：`/{p}/plugins/com.skiff.ftp/accounts` 与 `/runtime` 均 200；全部 FTP RPC 无 404/500；本轮未改 `plugins/`、SDK 与 wire 协议，`official/` 7 包字节与上一轮一致。
- **④ 站点与运行时回归（PASS）**
  - 新建 `/ 删除`：`reg-static.local`（static）与 `reg-php.local`（runtime）创建 200、站点 200、详情页 200；新建运行环境 `regphp`（alpine 8.5）容器 `running`、`/runtimes` 200；验证后站点与运行环境删除均 200。
  - 日志轮转：向站点日志追加 11,536,607 B（> 10 MiB）后由 30s 巡检改名为 `.1`（11,536,607 B）、新 `access.log` 归零，`docker exec skiff-web-nginx nginx -s reopen` 后新日志继续增长（190 B）——`sites.rs` 拆分后行为不变。
  - 终态：`user_version=18`、`integrity_check=ok`、`foreign_key_check` 空、sites 2 / site_aliases 0 / site_runtimes 1 / plugins 7 / plugin_grants 58 / users 1；`a.com` 与 `matrix.local` 200、`/healthz` 200、`NRestarts=0`；容器仅 `skiff-php-php-8.5` + `skiff-web-nginx`，镜像仅 `openresty/openresty:alpine` + `php:8.5-fpm-alpine`；`/etc/pure-ftpd/pureftpd.passwd` 0 行、`accounts` 空；nginx `server_name` 只剩 `a.com` / `matrix.local`（别名已清）。
- 本轮 **FAIL 0**；唯一观察项为 FTP 上传文件权限 `775`（见 ③，收口前后配置一致，非本轮引入）。

**`c4e42f0` SITE-15 复验（2026-11-11 01:04–01:35，Release 资产覆盖升级，保留库）**

- 出包 / 部署：同提交 `c4e42f0` CI 12 项 11 success + `编译（x86_64 musl 静态）` skipped；Release run `42865002942` 四个 job 全绿，tag `v0.1.0` 未移动（仍指 `ef7058b`）；x86_64 9,902,995 B `a9ab05c9…53a2a68` 与 API digest、`checksums.txt`、侧车四方一致；`sh -n install.sh` OK、`sh install.sh --no-docker` **exit 0**；库保持 **v17**（无迁移）、`integrity_check=ok`、plugins 7 / grants 58 / sites 2 / runtimes 1 / users 1 与部署前一致；`/healthz` 200、`NRestarts=0`；配置 sha `ec42b765…2c51af` 未变；安装后二进制 `b7acc80b…a6b51`（14,663,952 B）。
- **① 重建两家族运行环境后的 `php --ini`（PASS）**：`php:8.5-fpm` 新建 `php85deb4`、`php:8.5-fpm-alpine` 新建 `php85al4` / `php85al5`，三者均为 `Scan for additional .ini files in: "/usr/local/etc/php/conf.d:/usr/local/etc/php/conf.d-skiff"` —— `conf.d` 与 `conf.d-skiff` **各出现一次**、镜像目录在前、无前导冒号、无空段。`Additional .ini files parsed` 中每个 ini 只列一次（Debian：`docker-fpm` + `imagick` / `intl` / `msgpack` / `sodium` / `zip`；Alpine：`docker-fpm` + `redis` / `sodium` / `zip`），无重复条目。扩展 ini 由 ipe 写入镜像自带 `conf.d`，Skiff 托管目录 `conf.d-skiff` 本轮为空（宿主 `/www/runtime/php/<name>/php/conf.d` 的挂载点）。
- **环境变量固化验证**：升级前已有的 `skiff-php-php-8.5` 容器 env 仍是旧值 `PHP_INI_SCAN_DIR=:/usr/local/etc/php/conf.d:/usr/local/etc/php/conf.d-skiff`，未随覆盖安装变化 —— 证实「env 在建容器时固化」，必须重建运行环境才能拿到新值（本轮全部用新建环境复验）。
- **② 单扩展安装（PASS）**：
  - Debian `php85deb4`（站点 `id-c4deb.local`）装 `imagick` → 任务 `succeeded`、`exit_code=0`（`job-1794332209-2809afaa1`），`php -m` 有 `imagick`，日志 `already loaded` **0** 次、无 `主路径失败` 回退、`cannot create /etc/apk` 0 次；站点仍 `http=200`。
  - Alpine `php85al5` 装 `redis` → `succeeded`、`exit_code=0`（`job-1794332363-3567159d7`），`php -m` 有 `redis`，`already loaded` 0 次、无回退。
  - 自定义 PECL：Debian `php85deb4` 走 `/runtimes/{id}/pecl` 装 `msgpack` 3.0.0 → `succeeded`、`exit_code=0`（`job-1794332264-378eec592`），`php -m` 有 `msgpack`，`already loaded` 0 次、无回退。
- **③ 批量安装（PASS）**：
  - 干净 Alpine `php85al4`（站点 `id-c4al.local`）先做批量 `redis+zip` → `succeeded`、`exit_code=0`（`job-1794332330-25e8af9d5`），`php -m` 同时有 `redis`、`zip`，`already loaded` 0 次、**未回退依赖表**、`cannot create /etc/apk` 0 次；站点 `http=200`。
  - 纯内置批量 Debian `php85deb4` `zip+intl` → `succeeded`、`exit_code=0`（`job-1794332280-52347483`），`php -m` 有 `intl`、`zip`，`already loaded` 0 次。
- **④ 回归（PASS）**：`/healthz` 200；`/sites` `/plugins` `/store` `/jobs` 均 200；`a.com` / `matrix.local` 200；`user_version=17`、`integrity_check=ok`、plugins 7、grants 58；收尾后 sites 2、runtimes 1，容器仅 `skiff-web-nginx` + `skiff-php-php-8.5`，镜像仅 `php:8.5-fpm-alpine`（本轮 pull 的 `php:8.5-fpm` 已删）；`NRestarts=0`。临时站点 `id-c4deb.local` / `id-c4al.local`、运行环境 `php85deb4` / `php85al4` / `php85al5` 已删（目录与容器一并清除），`php_image` 已还原 `php:8.5-fpm-alpine`。

**`a735a91` SITE-15 / FW-19 复验（2026-11-11 00:52–01:06，Release 资产覆盖升级，保留库）**

- 出包 / 部署：同提交 `a735a91` CI 12 项 11 success + `编译（x86_64 musl 静态）` skipped；Release run `42859923757` 四个 job 全绿，tag `v0.1.0` 未移动（`git/ref/tags` 仍 `ef7058b`）；x86_64 9,903,059 B `35ba6da1…e180e1` 四方一致；`sh -n install.sh` OK、`sh install.sh --no-docker` **exit 0**；库保持 **v17**（无迁移）、`integrity_check=ok`、plugins 7 / grants 58 / sites 2 / runtimes 1 与部署前一致；`/healthz` 200、`NRestarts=0`；配置 sha `ec42b765…2c51af` 未变。
- **① SITE-15 两家族扩展安装（FAIL）**：a735a91 的家族判定生效——四个 Debian 安装日志中 `cannot create /etc/apk` **0** 次（仅脚本回显里出现 `/etc/apk/config` 字样），Alpine 侧行为不变。但**编译型/PECL 扩展仍装不上**：
  - Debian `php:8.5-fpm`：运行环境 `php85deb2` + 站点 `id-a73deb.local`，单装 `imagick` → 任务 `failed`、`exit_code=1`；`imagick.so` 已编出并落到 extension_dir，随后 ipe `Check if the imagick module can be loaded… Error loading the "imagick" extension: Warning: Module "imagick" is already loaded in Unknown on line 0` → `[skiff] 主路径失败（exit 1），已回退手写依赖表：imagick` → 回退 `pecl install imagick` 因「already installed and is the same as the released version 3.8.1」非 0，未执行 `docker-php-ext-enable` → `php -m` 无 imagick；站点本身仍 200。
  - Alpine：运行环境 `php85al2` + `id-a73al.local`，单装 `redis` → 同样 `exit_code=1` + `already loaded` + 回退失败 + `php -m` 无 redis；站点 200。
  - 自定义 PECL：`/runtimes/{id}/pecl`（`msgpack` 3.0.0）→ `exit_code=1`，同一 `already loaded`，回退路径执行。
  - 批量：干净 Alpine 运行环境 `php85al3` 批量 `redis+zip` → `failed exit_code=1`，两者均未加载；纯内置扩展批量（Debian `zip+intl`、Alpine `zip+bcmath`）→ `succeeded exit_code=0` 且 `php -m` 可见。
  - **根因（已定位，非 a735a91 引入；`git log -S PHP_INI_SCAN_DIR_VALUE` 指向 `1862702`）**：`crates/app/src/sites.rs:110` 的 `PHP_INI_SCAN_DIR_VALUE = ":/usr/local/etc/php/conf.d:/usr/local/etc/php/conf.d-skiff"`。按 PHP 语义，前导冒号表示「再叠加编译期默认扫描目录」，而默认目录正是 `/usr/local/etc/php/conf.d`；显式再列一次 → conf.d 被扫描两次，每个扩展 ini 加载两次并产生 `Module "X" is already loaded`。ipe 的 `checkModuleWorking` 以「写临时 ini 前后 stderr 是否变化」判定，必然命中该警告而返回失败；内置扩展（zip/intl/bcmath）不走该自检，故只有 PECL/编译型扩展受影响。
  - 对照实测：同一容器内改 `PHP_INI_SCAN_DIR=/usr/local/etc/php/conf.d:/usr/local/etc/php/conf.d-skiff`（去掉前导冒号）重跑 `install-php-extensions redis` → **exit 0**，`php -m` 出现 redis。
  - 修复在途：实现会话已在工作区改 `crates/app/src/sites.rs`（去掉前导冒号 + 单测 `php_ini_scan_dir_lists_each_directory_once`），**尚未提交**，故 `a735a91` 的 Release 资产不含该修复；下一轮需等其推送后重新出包，并**先重建两家族 PHP 运行环境**（环境变量在建容器时固化）再复验。
  - 修复方向（一行）：改为 `":/usr/local/etc/php/conf.d-skiff"`（保留前导冒号语义、删掉重复的显式 conf.d）或 `/usr/local/etc/php/conf.d:/usr/local/etc/php/conf.d-skiff`（去掉前导冒号）；`c4e42f0` 采纳后者，复验 PASS（见上节）。
  - 复现路径：新建任一家族运行环境 → `POST /{p}/runtimes/{id}/extensions/imagick/install`（或 `.../pecl`、`.../extensions-batch`）→ 任务 `exit_code=1`，`php -m` 不可见。
- **② FW-19 返回体一致性（PASS）**：`/etc/fail2ban/jail.d/zz-sshd.conf`（`[sshd] enabled = true`，排序在 `skiff-defaults.conf` 之后）在场并重启后 `fail2ban-client status` 为 `pure-ftpd` + `sshd`；RPC `fail2ban.start` → 200，返回 `jails:["pure-ftpd","sshd"]`、`residual_jails:["sshd"]`、**无 `disabled_jails`**，`disabled_jails ∩ jails = ∅`，sshd 只出现在 `jails` 与 `residual_jails`，不再同时挂在 `disabled_jails`。移除 `zz-sshd.conf` 并重启后再调 `start` → `jails:["pure-ftpd"]`、`residual_jails` 空、`disabled_jails` 空，`jail.d` 恢复三文件。
- **③ 回归（PASS）**：`/healthz` 200；`/sites` `/plugins` `/store` `/jobs` 均 200；`a.com` / `matrix.local` 200；`user_version=17`、`integrity_check=ok`、plugins 7、grants 58；收尾后 sites 2、runtimes 1，容器仅 `skiff-web-nginx` + `skiff-php-php-8.5`，镜像仅 `php:8.5-fpm-alpine`；fail2ban installed + running（`pure-ftpd`）；`NRestarts=0`。临时站点 `id-a73deb.local` / `id-a73al.local`、运行环境 `php85deb2` / `php85al2` / `php85al3`、`php:8.5-fpm` 镜像与 `zz-sshd.conf` 已清理。

**`db7e579` 四条 FAIL 修复复验（2026-11-11 00:19–00:57，Release 资产覆盖升级，保留库）**

- 出包 / 部署：同提交 `db7e579` CI 12 项检查 11 success + `编译（x86_64 musl 静态）` skipped；Release run `42852172418` 四个 job 全绿，tag `v0.1.0` 未新建未移动（仍指 `ef7058b`）；x86_64 9,902,038 B `a3103511…b29fce` 与 API digest、`checksums.txt`、侧车四方一致；`sh -n install.sh` OK；解压后 `sh install.sh --no-docker` **exit 0**，`/usr/local/bin/skiff` 14,655,760 B `c639e3f4…b57254f`；库保持 **v17**（无迁移）、`integrity_check=ok`、计数与部署前一致（plugins 7 / grants 58 / sites 2 / runtimes 1 / jobs 13 / job_logs 86 / users 1）；`scheduled_tasks` 5→6（安装重跑内置种子）；`/healthz` 200、`NRestarts=0`、前缀 `w3qz4vc2`、配置 sha `ec42b765…` 未变。
- **① Debian PHP 站点（PASS）**：`docker pull php:8.5-fpm` 后把 `php_image` 切到 Debian 并建运行环境 `php85deb`（`runtime.install` succeeded，容器 `skiff-php-php85deb` `Up`）。容器内 `getent group nobody` → `nobody:x:65534:`，`getent group nogroup` → `nogroup:x:65534:`，`awk -F: '$3==65534'` 同时列出 `nogroup` / `nobody`；`id -u skiff-web` = 201000；`/proc` 扫描：master `Uid=0`，pool www 三个 worker `Uid=201000`；池配置 `user/group = skiff-web`、`listen.group = nobody`（gid 65534 由别名解析）；日志 `cannot get gid` / `user not found` 各 **0** 次。站点 `id-db7deb.local`：`index/` 与 `index.php` 201000:201000；会话写入返回 `sid=… ok=1`，容器 `/tmp/sess_*` 属主 `skiff-web`。**重启（不重建）**：`docker restart` 后 status running、镜像引用不变；`nobody` 别名仍在（初始化脚本幂等重放）、worker 仍 201000、再次会话写入成功、日志 0 报错。alpine 回归：`php85al` 运行、`nobody:65534`、worker 201000、站点 `id-db7al.local` 会话写入 200、0 报错。测试机无存量 Debian 实例（上轮已清），故「旧实例重启恢复」以新建实例 + `docker restart` 路径覆盖。
  - 观察（不在本轮五项内，已记台账）：Debian 镜像上 PHP 扩展安装不可用——`runtime.ext.install` 立即 `failed exit_code=2`，日志 `sh: 5: cannot create /etc/apk/config: Directory nonexistent`（安装脚本无条件写 Alpine 的 `/etc/apk/config`）；Alpine 不受影响。
- **② 重试作业（PASS）**：停 alpine 容器后提交 `redis` 扩展安装 → `failed`；启动容器后 `POST /jobs/{id}/retry` → 303，新任务带 `retry_of`，**首次轮询即 `running`（`started_at` 已置位）**，未出现 queued 滞留；结束后 `exit_code=1`、日志 33 行。并发：同目标已有运行中任务时，表单再次提交 → 303 + flash「该对象已有任务在执行（job-…（排队）），请等待完成或取消后再试」，`retry` → **409** 且正文同一文案；`cancel` 后 3–6 s 转 `cancelled`（释放索引），再次提交 → 303，新任务立即 `running` 并正常结算（exit_code 1、28 行日志）。
- **③ RPC 状态码（PASS）**：卸载 fail2ban 后 `status` 200（`state=missing`），`start` / `stop` / `uninstall` 均 **503**，页面标题「503 服务不可用」，正文含「内核返回 HTTP 503: fail2ban 未安装，无法启动/停止/卸载；请先安装」；不存在站点 `GET /sites/nope.local` **404**；不存在插件 RPC（`com.skiff.nope`）**404**（正文「插件不存在」）；冲突 **409**（重试，见②）；依赖不可用 **503**（fail2ban 缺失）。500 / 504 未构造。
- **④ fail2ban 装后生效（PASS）**：面板 `fail2ban.install` → 200，返回 `{"installed":true,"jails":["pure-ftpd"],"running":true,…}`（未带 `disabled_jails`：`skiff-defaults.conf` 在 apt 安装前已写 `[sshd] enabled=false`，重启后本就不存在非预期 jail）；`fail2ban-client status` 仅 `pure-ftpd`；`jail.d` 三个文件，`skiff-defaults.conf` 内容为 `[sshd] enabled=false`。**宿主自装保护**：移走 `skiff-defaults.conf`（`defaults-debian.conf` 自带 `[sshd] enabled=true`）并重启 → jails `pure-ftpd, sshd`；调 `fail2ban.start` → 200，jails 仍 `pure-ftpd, sshd`，**未静默关闭**宿主 jail（`banned 0`）；还原托管文件并重启 → 仅 `pure-ftpd`。**`disabled_jails` 路径**：人为放晚排序的 `zz-sshd.conf` 启用 sshd，`start` 返回 `disabled_jails:["sshd"]` 并追加 `[sshd] enabled=false`；因该覆盖文件排序在 `skiff-defaults.conf` 之后，返回体 `jails` 仍如实列出 `pure-ftpd, sshd`，移除覆盖文件并重启后 sshd 消失（该偏差只在「宿主文件排在同名托管配置之后」时出现，记台账）。
- **⑤ 回归（PASS）**：`/healthz` 200；`/sites` `/plugins` `/store` `/jobs` 均 200；`a.com` / `matrix.local` 200；登录页 200；库 `user_version=17`、`integrity_check=ok`、plugins 7、grants 58；清理后 sites 2、runtimes 1。**收尾**：删 `id-db7deb.local` / `id-db7al.local` 站点、`php85deb` / `php85al` 运行环境、`php:8.5-fpm` 镜像、`zz-sshd.conf` 与 `/www/sites/id-db7*` 残留；`php_image` 还原 `php:8.5-fpm-alpine`；fail2ban 保持 installed + running（jail `pure-ftpd`）；nginx / php 容器各一，镜像仅 `php:8.5-fpm-alpine`。

**`eea2969` 六块上机验收（2026-11-10 23:03–次日 00:20，Release 资产覆盖升级，保留库）**

- 出包 / 部署：同提交 CI run `42838814264` success（11 success + 1 skipped）；Release run `42840177829` 四个 job 全绿，tag `v0.1.0` 未新建未移动（实际指向 `ef7058b`）；x86_64 9,848,676 B `68046ca5…e156c66` 四方一致；`sh -n install.sh` 语法检查 OK；`sh install.sh --no-docker` 一次成功，`/usr/local/bin/skiff` 14,536,976 B `91e1e21d…63adf24`；库 **v17**、`integrity_check=ok`、计数与 v16 基线一致（见 §5）、`/healthz` 200、`NRestarts=0`、前缀 `w3qz4vc2`。
- **① 部署与迁移（PASS）**：`install.sh` 首次执行创建 `skiff-web` 201000:201000；随后再跑一次 `--no-docker`，输出「已存在」并复用（`getent passwd/group` 前后一致，二次安装 exit 0）。preflight 用 `nginx:alpine` 容器隔离（只读挂载 `install.sh` 与枚举 `/etc/passwd|group` 的 getent 垫片）验证：预置 gid 201000 的 `othergrp` → 拒绝，报「GID 201000 已被组 othergrp 占用；站点统一身份要求固定 skiff-web，请先处理该组」；预置 uid 201000 的 `otheruser` → 拒绝，报「UID 201000 已被账号 otheruser 占用…」；两例 `install_exit=1`，现网未受影响。v16→v17：`jobs.exit_code` 建出，jobs 8 / job_logs 61 / plugins 7 / grants 55 / sites 2 / runtimes 1 全保留；升级启动补授 fail2ban 三条 grants（55→58，`plugin.grant_upgrade` ok ×3）。
- **② 站点统一身份**：alpine **PASS**、Debian 系 **FAIL**。alpine（`php:8.5-fpm-alpine`，实例 `php85b`、站点 `id-eea.local`）：`index/` 全树 201000:201000（目录 2775、文件 664）；`php-fpm` worker `Uid=201000`；容器日志 `user not found` **0** 次；PHP 会话（`session.save_path=/tmp`）写入 200、`/tmp/sess_*` 属主 201000；FTP（pure-ftpd 1.0.50）建号上传 226、`ftpup.txt` 201000；面板文件管理 upload / rename / copy / extract 后全树 201000；插件 `com.skiff.files` RPC `write` 写文档根后 201000；旧站点 `skiff sites chown-index --dry-run` 不改属主，实跑 `a.com` / `matrix.local` 全树 201000、symlink 与 `/etc/hostname` 未被改、`chattr +i` 失败站回滚 0:0 且其它站成功、逐站审计 ok / fail、退出码 1；Node 站点 `id-eea-node.local` 文档根占位文件 201000，`/www/runtime/node/node24` 属主 0:988（root，符合契约）；Rust 未单独建实例（与 Node 同为进程型运行环境，目录创建走同一段代码）。**FAIL**：切 `php_image=php:8.5-fpm`（Debian）建实例 `php85deb` → `runtime.install` failed（阶段「启动 PHP-FPM」），容器 `Restarting (78)`，日志 `ERROR: [pool www] cannot get gid for group 'nobody': Success (0)` + `FPM initialization failed`；根因是内核池模板写死 `listen.group = nobody`，Debian 系该组名为 `nogroup`（最小复现：仅把该行改为 `nogroup` 即启动成功；uid 201000 + `session.save_path=/tmp` 下会话可写）；已删除 php85deb 并还原 `php:8.5-fpm-alpine`。
- **③ URL 规范（PASS）**：`/{p}/files/api/list` → 308（Location `/{p}/files/list`，跟随 200）；`/{p}/sites/env` 与 `/{p}/sites/env/{id}` → 301（`/{p}/runtimes`、`/{p}/runtimes/{id}`，跟随 200）；`/{p}/docker/settings` → 308（`/{p}/settings/docker`，跟随 200）；`/{p}/sites/env/add`、`/{p}/runtimes/add` GET **405**、POST **303**（只作 `/runtimes/create` 别名）；`/{p}/images/{tag}/remove` 与 `/delete` 均 200 且镜像实际删除（`php:8.5-fpm`、`nginx:alpine` 各走一条）。商店安装改 POST + CSRF：确认页 200（token + 权限/资源勾选项 + 文案「安装改为 POST 提交」）；旧 GET → 303 回 `/store` 不安装；缺 CSRF POST → **403** 不安装；带 CSRF POST → 200 确认页 → `POST /plugins/confirm` 303 → 已安装；审计 `store.install ok reason=prepared`、`plugin.install ok signed=true key_id=ed73de3e…`。
- **④ 任务中心**：**部分 FAIL**。PASS：`runtime.ext.install` 作业 `job-1794324324-b4ae0220` 落 `exit_code=1` 并在详情页显示；`/jobs/{id}/logs` 返回 entries+seq，88 行安装器真实输出；重试生成新作业并显示「重试 · 原任务 job-1794324324-b4ae0220」，原作业显示「手动提交 · admin」；清空日志后详情页显示「暂无日志」。**FAIL**：重试作业 `job-1794324364-1b2f0d861` 创建后 `status=queued`、`started_at=NULL`，等待 **301 s** 仍未被拾起（容器内无残留 install-php 进程、journal 无报错）；该 queued 行永久占用同目标活跃唯一索引，`php85b` 再次提交扩展安装被拒（页面 flash「该对象已有任务在执行…」），只能手工取消才释放。代码核对：[jobs.rs](../crates/web/src/handlers/jobs.rs) 的 retry 只调 `jobs().retry(...)` 建行，未像正常提交路径那样 spawn 执行器。
- **⑤ fail2ban 管理**：**部分 FAIL**。PASS：三态浏览器实测（headless Chromium）：运行中 → 绿色「运行中」+「停止 fail2ban」「卸载 fail2ban」；停止 → 「已安装未运行」+「启动/卸载」，无「停止」；未安装 → 「未安装」+ 仅「安装 fail2ban」；提示「新装 fail2ban 时默认关闭 sshd jail，避免误封宿主机 SSH」可见。安装 → 启动 → 停止 → 卸载全链路走通；卸载只删 `skiff-defaults.conf`，预置的 `skiff-testkeep.conf` / `custom-other.conf` 保留；审计 `firewall.fail2ban.{status,install,start,stop,uninstall}` 有 ok / fail（缺失态 start 记 fail，detail 含「fail2ban 未安装，无法启动；请先安装」）。**FAIL 1**：缺失态经面板 RPC 调 start / stop / uninstall 返回 **HTTP 400**（正文仍带「内核返回 HTTP 503: fail2ban 未安装…」），非约定的 503——[plugins.rs](../crates/web/src/handlers/plugins.rs) 的 `rpc_error` 只透传 403/404/409/507。**FAIL 2**：`fail2ban.install` 后立即查询 `jails=[pure-ftpd, sshd]`，`fail2ban-client status` 确认 sshd 实际启用；stop → start 后 sshd 才消失。根因：Debian 包 postinst 在写 `skiff-defaults.conf` 之前已自启服务，`systemctl enable --now` 对已运行实例不重启，配置里的 `[sshd] enabled=false` 要到下次重启才生效。
- **⑥ 菜单与回归（PASS）**：`/{p}/files/trash` 200 且工作台标签为「浏览 / 回收站」，侧栏导航无独立「回收站」入口；`/{p}/m/core/files` 200；`/healthz` 200；商店页 200（隧道 502 时按设计降级为离线 + 上次快照）；插件页 200、`toggle` 两次 200（enabled 1→0→1）；站点 `a.com` / `matrix.local` / `id-eea.local`（Host 头）均 200。
- 收尾：删除测试站点 `id-eea.local` / `id-eea-node.local`、运行环境 `php85b` / `node-node24`，删除 `php:8.5-fpm`、`nginx:alpine`、`node:24-alpine` 镜像；FTP 测试账号已删（`getent` 无 `eeaftp` 残留）；`/www/sites/zz-fail*` 已清；fail2ban 恢复到「已安装并运行」（stop → start 清 sshd，jail `pure-ftpd`）；testkit 因商店隧道 502 用本地官方包 `com.skiff.testkit-0.1.0.wbpkg` + 全量 `--grant/--resource` 回装（grants 8；其 manifest 定时任务随卸载未回种，scheduled_tasks 6→5）；库终态 v17、2 站点 / 1 运行时 / 7 插件 / 58 grants / `integrity_check=ok`，`/healthz` 200。

**`c8bcb6e` 内核升级只读段轮（2026-11-10 19:46 起，Release 资产覆盖升级，保留库）**

- 出包 / 部署：同提交 CI run `42807944935` success；Release run `42809009928` 四个 job 全绿，tag `v0.1.0` 未新建未移动（**该 tag 实际指向 `ef7058b`**）；x86_64 资产 `fceae2cb…c576f09` 在测试机 `sha256sum -c` OK；同版本覆盖，库保持 **v16**、7 插件 / 55 grants / 6 任务 / 1 站点 / 1 运行时全部保留；`/healthz` 200、`NRestarts=0`、前缀仍 `w3qz4vc2`；`/usr/local/bin/skiff` 14,475,536 B `540a715a…b8a693`。
- 目录夹具：沿用测试机 `/root/iter/catalog-test/`（Ed25519 种子只在 `/root/iter/catalog-test/seed.hex`，公钥 `315eacce…4a28`）；本轮用自写 `mkcat.py` 生成并签名多套变体（`up` 0.2.0 / `same` 0.1.0 / `lower` 0.0.9 / `none` 删除 `upgrades` / `badsig` 签名不匹配），每次 seq 递增；测试包 10,752 B `a0607c69e8b6f45542cc3dc1dcc31c92bdfaa108340497191fb009b375aa09b2`。serveo 隧道本轮再次 502，重开换新域名 `https://acde8a7b…serveousercontent.com/cur`，同步 `[catalog].mirrors` 并重启后恢复。
- **①升级提示（PASS）**：`up`（version 0.2.0 / arch `x86_64` / channel `stable` / min_from 0.1.0）→ `/store?refresh=1` 的「内核升级」段显示当前 `0.1.0` → 可用 **`0.2.0`**，含 changelog 与下载入口（按渠道 + 架构匹配、取最高一版）；`same`（0.1.0）与 `lower`（0.0.9）→「无可用升级」且无下载入口；`none`（删除 `upgrades` 条目）→ 同上；`badsig`（清单 key_id 与固定公钥不匹配）→ 页面 **200**、状态「校验失败」、提示「目录校验失败，暂不提供升级下载」+「无可用升级」，审计 `catalog.refresh fail reason=signature`，无 500 无白屏；离线（`mirrors` 改为 `https://127.0.0.1:9/cur` 且删掉 `/www/skiff/catalog` 后重启）→ 页面 **200**、「离线 / 无可用快照」+「无可用升级」，恢复镜像后重新显示 `0.2.0`。
- **②下载与校验（PASS）**：inotify 观测到 `upgrade/incoming/` 下先建 `skiff-0.2.0-x86_64.tar.part`（`CREATE` → `CLOSE_WRITE` → `ATTRIB`（落 `0600`）→ `MOVED_FROM`）再 `MOVED_TO` 为 `skiff-0.2.0-x86_64.tar.gz`，落盘 sha256 `a0607c69…aa09b2` 与清单一致，审计 `upgrade.download ok reason=staged … reused=false`；重复请求复用本地文件（`reused=true`，目录侧无新 blob GET）；本地暂存追加 1 字节后重下 → journal `暂存升级包校验失败，丢弃后重新下载 error=大小不符：清单 10752 字节，本地 10753 字节`，重下后摘要恢复、`reused=false`。远端包截断到 100 B → 审计 `upgrade.download fail reason=digest`（`包大小与目录不符：目录 10752 字节，实得 100 字节`），无 `.tar.gz` 与 `.part` 残留；远端包等长改 1 字节 → `fail reason=digest`（`包摘要与目录不符…实得 1b16b471…`），同样无残留。同版本 `0.1.0` 与降级 `0.0.9` → `fail reason=downgrade`（`拒绝降级或同版本重下：当前 0.1.0，目标 0.1.0/0.0.9`）；清单无 `0.3.0` → `fail reason=not_found`；三类均为 0 次 blob GET。移除 `[catalog].public_key` 重启 → `GET /store/upgrades/0.2.0/download` **400**、审计 `deny reason=no_key`，目录侧访问日志请求数不变（不触网）；`viewer` 角色账号（复制 admin `password_hash` 插入后验完即删）→ **403**、审计 `deny reason=permission`。
- **③暂存 TTL（PASS）**：安装前在 `upgrade/incoming/` 造 25 h 前的 `skiff-0.9.9-x86_64.part`、`skiff-0.8.0-x86_64.tar.gz` 各一条，另放 1 h 前的 `.tar.gz` 一个与自建 `notes.txt`；`install.sh` 后服务启动即清（journal `已清理过期升级暂存包 path=/www/skiff/upgrade/incoming/skiff-0.9.9-x86_64.part age_secs=90000 ttl_secs=86400` 及 `.tar.gz` 一条），条目 4 → 2，1 h 包与 `notes.txt` 保留；再触发一次下载前的清理（journal `… skiff-0.8.0-x86_64.tar.part age_secs=90002 …`），年龄按 mtime 计算，路径与 `ttl_secs` 均在日志内。
- **④回归（PASS）**：商店安装 `com.skiff.testkit` 全链路仍通过（确认页 → `POST /plugins/confirm` → 插件页「已安装 7 个 · 已启用 7」，审计 `store.install ok` / `plugin.install ok`）；插件启停一致性 10/10；`/healthz` 200（18080 与转发 18081）；库仍 **v16**、7 插件全部 `enabled=1`（guard / monitor `running`）、grants **55**（cron 6 / files 7 / firewall 12 / ftp 13 / guard 6 / monitor 3 / testkit 8）、6 任务 / 1 站点 / 1 运行时 / 8 jobs 不变。
- 收尾：目录恢复为 `none`（seq 9），面板终态「无可用升级」；`upgrade/incoming/` 清空；清理验收造的 `viewer-test` 用户与其会话；`/root/iter/rel-c8bcb6e/`、`/root/skiff.before-c8bcb6e`、`/root/iter/skiff.db.before-c8bcb6e`、`/root/iter/config.before-c8bcb6e.toml`、`/root/iter/catalog-test/` 保留。

**`5c15b6b` 健壮性收口轮（2026-11-10 18:38 起，Release 资产覆盖升级，保留库）**

- 出包 / 部署：同提交 CI run `42798491760` success；Release run `42799634298` 四个 job 全绿，tag `v0.1.0` 未新建未移动（**该 tag 实际指向 `ef7058b`**）；x86_64 资产 `4dc3ddb9…7b80d59c` 在测试机 `sha256sum -c` OK；同版本覆盖，库保持 **v16**、7 插件 / 55 grants / 6 任务 / 1 站点 / 1 运行时全部保留；`/healthz` 200、`NRestarts=0`、前缀仍 `w3qz4vc2`；`/usr/local/bin/skiff` 14,446,736 B `f22c3f33…96b954d`。
- **①插件启停状态一致性（PASS，10/10）**：Web 停用 / 启用 `com.skiff.cron` 各一次，DB 中 `enabled` 与 `status` 在同一次 `UPDATE` 内一起变化（停用 → `enabled=0 status=disabled`，启用 → `enabled=1 status=installed`），`status_detail` 同步清空；`systemctl restart skiff` 后不漂移（`guard` / `monitor` 由内核调度器重新拉起为 `running`，其余保持 `installed`，无出现「enabled=1 但 status=disabled」）；审计两条 `plugin.toggle`（`detail=enabled=false|true`）；CLI `disable` / `enable` 与 Web 停用态一致，不会把管理员停用的插件重新改回启用。
- **②文件管理页回归（PASS，26/26）**：roots / tree / list / read / upload / download（单文件与目录 zip）/ stat / mkdir / rename / copy / chmod / chown / 回收站 remove→list→restore→remove→purge / 配额 get→set→cancel→scan 全部通过，响应形状与状态码不变（`{ok,listing{path,parent,entries,truncated}}`、`{ok,file{path,text,size,truncated}}`）；16 MiB 文件按 256 KiB 分 64 片上传成功，下载后 sha256 与源一致；单次 >256 KiB 上传 413 `files.too_large`；8 读 + 1 写共 96 并发请求无 5xx，单请求最大耗时无明显抬升（Δmax ≈ 0，同步 IO 已移出 async worker）。
- **③审计完整性（PASS）**：Web 侧审计照常落库；exec WebSocket 的 `docker.containers.exec` 在 start / end（正常容器 `skiff-exec-oktest`）与 fail（容器已停止）三种结果均有记录；`audit_block BEFORE INSERT ON audit ... RAISE(ABORT)` 模拟审计表不可写时，文件操作仍返回 200、无新审计行、仅日志报错（`文件操作审计写入失败` / `审计写入失败（操作已按原结果继续）`），移除触发器后审计恢复落库。
- **④商店多源退避（PASS，15/15）**：双镜像夹具（`mirrors[0]` 固定失败、`mirrors[1]` 正常）下，自动刷新遇失败镜像按 `10s → 20s → 40s` 退避（上限 300s），退避期内该镜像请求数为 0；两个镜像全部退避且本地有缓存时页面直接出缓存（耗时 ≈ 0，无额外请求）；手动 `?refresh=1` 立即重试并在成功后清零退避。常量与判定在 `crates/app/src/catalog`（`MIRROR_BACKOFF_BASE=10s`、`MIRROR_BACKOFF_MAX=300s`）。
- **⑤Docker 镜像 digest（PASS，单测 + 机上双仓库补证）**：单测 `client::tests::primary_digest_matches_repo_not_first_entry` 本地 gate 通过；机上把 `settings.sites.php_image` 切到 `127.0.0.1:5000/php2:8.5` 并重建运行环境，`site_runtimes.image_digest` 登记为 `127.0.0.1:5000/php2@sha256:e136a4ea…`（与镜像所在仓库匹配），切回 `php:8.5-fpm-alpine` 重建后登记 `php@sha256:4c281807…`。**限制**：同一 image id 同时挂两个不同 RepoDigest 的现场在单机无法构造（本地 registry 拉取后镜像 ID 与 RepoDigest 各自独立），该分支以单测覆盖，E2E 只验到「双仓库各一条 digest」。
- 夹具 / 收尾：收尾复核时 serveo 旧域名返回 502，`/store` 按设计「离线：…manifest HTTP 502；展示上次成功快照」，失败审计 `catalog.refresh | stable | fail | reason=offline … HTTP 502` 如实保留；重开隧道为 `https://a3af2489…serveousercontent.com/cur` 并同步 `[catalog].mirrors` 后恢复「目录未更新（304）· 展示本地缓存」。回收站已清空；测试容器 `skiff-exec-oktest`、`skiff-reg`、`registry:2` 与 `127.0.0.1:5000/php*` 镜像已删；`fs_quotas` 两条无上限跟踪行已清理；`PRAGMA integrity_check=ok`；`/root/iter/rel-5c15b6b/`、`/root/skiff.before-5c15b6b`、`/root/iter/skiff.db.before-5c15b6b`、`/root/iter/catalog-test/` 保留。

**`dc44024` 商店目录修复轮（2026-11-10 16:05 起，Release 资产覆盖升级，保留库）**

- 出包 / 部署：同提交 CI run `42778317275` success；Release run `42779553406` 四个 job 全绿，tag `v0.1.0` 未新建未移动；x86_64 资产 `47ec728c…40cae6` 在测试机 `sha256sum -c` OK；同版本覆盖，库保持 **v16**、7 插件 / 55 grants / 6 任务 / 1 站点 / 1 运行时全部保留；`/healthz` 200、`NRestarts=0`、前缀仍 `w3qz4vc2`。
- 夹具重建：上轮 serveo 域名已过期，新开受信 https（`https://682fdb…serveousercontent.com/cur`）；静态服务换成带条件请求的自写服务器（`ETag`/`If-None-Match`/`If-Modified-Since` → **304**，逐请求记录 `inm=`/`ims=` 与状态码），`catalog.mirrors` 仍只接受 https、**未放开协议校验**；新增 `skiff-catalog-test.service`（18090）与 `skiff-tunnel-keepalive.service`（每 8 s 探测 `cur/schema/plugin.schema.json`，防免费隧道空闲掉线）。
- 冷启动升级：旧缓存 `meta.json` 无 `key_id` → 首次刷新判定「公钥已变更」并强制重拉（journal `cached_key="-" current_key="d530cc…"`），同步 seq 2 成功，四类条目各 1 项（插件 / 应用模板 / 运行环境 / 升级通道）、1 个安装入口。
- **目录拒绝审计（PASS，三条 `reason` 均正确）**：
  - 错误公钥（换成另一把合法 Ed25519 公钥并重启）：缓存按 `key_id` 失效，清单请求**不带**任何条件头（服务端记 `inm=None ims=None`，200）→ WARN `reason=signature detail=验签失败: 清单 key_id 与固定公钥不匹配（清单 d530cc…，公钥 044d3d…）`；审计 `catalog.refresh | stable | fail | reason=signature …`；页面「校验失败」、0 条目；`GET /store/plugins/com.skiff.testkit/install` 落 `store.install fail reason=not_found`。
  - 篡改索引（`index.json` 追加 8 字节，manifest 未变）：`reason=digest detail=索引大小与 manifest 声明不符：声明 2364 字节，实得 2372 字节`；审计同码。
  - `seq` 回退（改回 seq=1 产物）：`reason=seq detail=清单 seq 1 小于已见值 2（防回滚）`；审计同码。
  - 另测到网络失败路径同样落审计：切错隧道域名（旧 serveo 域名 502）时 `catalog.refresh fail reason=offline … manifest HTTP 502`。
  - 后两条拒绝都保留上次成功快照与 4 类条目（页面「已拒绝该清单并保留上次成功快照」），缓存未被覆写。
- **审计写失败不阻断拒绝（PASS）**：`ALTER TABLE audit RENAME TO audit_tmp` 后重放「篡改索引」→ journal `ERROR 审计写入失败（目录判定已按原结果继续） error=sqlite: no such table: audit action="catalog.refresh" channel=stable`，页面仍「校验失败」并保留旧快照，`audit` 表无新行；改回表名后审计恢复。
- **换公钥失效缓存（PASS）**：热缓存下 `?refresh=1` 命中条件请求 **304**（日志 `inm="<etag>" ims=<date> 304`）；换公钥重启后同一请求变为 **200 且无 `If-None-Match`/`If-Modified-Since`**，旧条目不展示、不可安装；换回原公钥后缓存重新可用（文件未被破坏，304 复用）。
- **暂存清理（PASS）**：`.incoming` 放 mtime 25 h 前的 `stale-test.wbpkg` + `stale-test.part` 与一个新建文件 → 安装准备触发清理（journal `已清理过期插件暂存包 removed=2 ttl_secs=86400`），两个过期文件消失、新下载的 327,839 B 包与自建新文件保留；再造一对 25 h 文件后 `systemctl restart skiff`，启动后同样被清（removed=2），其余保留。
- **安装正向链路回归（PASS）**：`GET /store?refresh=1`（快照未被拒绝）→ `GET /store/plugins/com.skiff.testkit/install` 确认页（下载 327,839 B，摘要 / 验签 / 兼容性通过，列出 6 项权限 + 对象 `path:/proc`、`plugin:com.skiff.cron`）→ `POST /plugins/confirm` 200 → 审计 `store.install ok reason=prepared version=0.1.0 channel=stable source=https://…/cur` 与 `plugin.install ok signed=true key_id=ed73de3e… grants=8`；已安装仍 7 个。
- 备注：放弃确认页仍会遗留 `.incoming/<token>.wbpkg`，现由 24 h TTL 兜底；本轮结束时该目录有 4 个 327,839 B 包（含上轮遗留 1 个，均 <24 h，未误删）与 1 个人工新建小文件，作为到期回收样本保留。
- 终态：恢复完整 `[catalog]`（`cur` + 原公钥 + `enabled=true`），商店页四类条目正常；`/root/iter/rel-dc44024/`、`/root/skiff.before-dc44024`、`/root/iter/skiff.db.before-dc44024`、`/root/iter/catalog-test/`（seed、`http/{cur,old,bad}`、`access.log`、`tunnel.url`）保留；免费隧道域名随会话失效。

**`0ee9065` 商店目录轮（2026-11-10 15:28 起，Release 资产覆盖升级，保留库）**

- 出包：同提交 CI success；Release run `42774595031` 四个 job 全绿；tag `v0.1.0` 未新建未移动；x86_64 资产 `31cdc789…94b0` 在测试机 `sha256sum -c` OK。
- 部署：同版本覆盖，库保持 **v16**（无迁移）、前缀仍 `w3qz4vc2`；`/healthz` 200（18080/18081）、`NRestarts=0`；7 插件 / 55 grants / 6 任务 / 1 站点 / 1 运行时全部保留。
- 目录托管（测试夹具，不入仓库）：临时 Ed25519 密钥对在测试机生成（种子只在 `/root/iter/catalog-test/seed.hex`；公钥 `315eacce…4a28`，`key_id=sha256(pub)=d530cc47…b5d3`）；用仓库 `tools/skiff-server gen --catalog … --sign-key <seed>` 生成三套产物：`cur`（seq=2）、`old`（seq=1）、`bad`（`cur` + blob 尾部追加 `tampered`，327,847 B）；插件条目复用官方已签名 `com.skiff.testkit-0.1.0.wbpkg`（327,839 B，签发键 `ed73de3e…`，与 DB 信任库一致）。静态托管：`python3 -m http.server 18090 --bind 127.0.0.1 --directory /root/iter/catalog-test/http` + `ssh -R 80:localhost:18090 serveo.net` 反向隧道，对外为**受信 Let's Encrypt 证书的 https**（`https://e1d0…serveousercontent.com/cur`；隧道重建后域名会变）。
- **HTTPS 限制（限制项，未改协议校验）**：`UreqTransport` 用 ureq 默认根（webpki-roots，无 native certs、无 `danger_accept_invalid_certs`），自签证书不可能被信任；且 `catalog.mirrors` 只接受 `https://`。实测：`mirrors=["http://127.0.0.1:18090/cur"]` 启动即拒（journal「catalog.mirrors 必须是 https:// 地址」，服务不启动）；`mirrors=["https://127.0.0.1:18443/cur"]`（自签 RSA 2048，`curl -k` 返回 200）→ 商店页「校验失败：离线…`invalid peer certificate: CaUsedAsEndEntity`」，0 条目、无缓存落盘。因此正向链路只能借外部受信 https 隧道验证，本地 http/自签仅能验证拒绝路径。
- 正向（PASS）：`/w3qz4vc2/store` 200，状态「目录已更新 · **seq 2**」；四类各 1 项、合计 **4 项**（插件 `com.skiff.testkit 0.1.0`；应用模板 `com.example.hello-app 0.1.0`；运行环境 `php:8.5-fpm-alpine`；升级通道 `0.1.1 stable x86_64`）；导航出现「商店」（`/w3qz4vc2/m/core/plugins/store`）；冷启动拉取按 `manifest.json` → `manifest.json.sig` → `index.json` 顺序 GET（各 200）。
- 安装链路（PASS）：`POST /plugins/com.skiff.testkit/remove`（`delete_data=off`，grants 清零）→ `GET /store/plugins/com.skiff.testkit/install` 200 确认页（下载 blob 327,839 B，摘要 / 验签 / 兼容性通过，列出 6 项权限 + 对象 `path:/proc`、`plugin:com.skiff.cron`）→ `POST /plugins/confirm`（csrf + token + 全选 grant/resource）**303** → 插件回装 0.1.0、grants **8**；审计 `store.install` **ok** `reason=prepared`（含 version/channel/source）、`plugin.install` **ok** `signed=true key_id=ed73de3e… grants=8`、`plugin.uninstall` ok。
- 反例（PASS，均拒绝）：
  - 错误公钥（冷缓存）：清 `catalog/channels/` 后换成另一把合法 Ed25519 公钥 → 「校验失败：验签失败：清单 key_id 与固定公钥不匹配」，0 条目、0 安装入口、不写缓存。**观察**：热缓存下只发条件请求并复用 304，换公钥后仍展示旧条目（缓存未按 `key_id` 失效，已记台账）。
  - blob 篡改（`bad` 镜像）：商店页仍列条目（清单/索引未变），安装确认页拒绝并 flash `[digest]`（包大小期望 327,839 B、实际 327,847 B）；审计 `store.install` **fail** `reason=digest`。
  - `seq` 回退（`old` 镜像 seq=1、缓存 seq=2；`touch` 使服务端返 200 而非 304）：商店页「目录未更新」并继续用 seq 2 缓存与 4 条目，journal WARN 记录回退拒绝，缓存未被覆写。
  - 未配公钥：删除 `[catalog].public_key` → 商店页 200 但 **0 安装入口**、提示固定公钥未配置；`GET /store/plugins/<id>/install` **400**，审计 `store.install` **deny** `reason=no_key`。
  - `enabled=false`：`/store` **404**、侧栏/菜单无「商店」链接；静置 6 s 静态托管访问日志请求数不变（42→42），缓存 mtime 不变（不触网）。
  - **审计缺口（FAIL）**：目录验签失败与 `seq` 回退只落 journal WARN，**没有审计事件**（`crates/app/src/catalog` 无 audit 写入）；安装侧 ok / fail / deny 三种结果均有 `store.install`。已记台账。
  - 观察：未提交的安装确认页会遗留 `/www/skiff/plugins/.incoming/<token>.wbpkg`（无 TTL 清理），本轮残留 1 个 327,839 B 包，不影响后续安装。
- 边界：缓存落 `/www/skiff/catalog/channels/stable/`（manifest / manifest.json.sig / index.json / meta.json + `blobs/sha256/`）；条目里的 `key_id` 是 schema 必填但客户端不读取，包信任判定只用包内 `manifest.toml.key_id` 对 DB 信任库。
- 终态：恢复完整 `[catalog]`（`cur` + 正确公钥 + `enabled=true`）后商店页 200、cache seq 2；`skiff.service` active、`NRestarts=0`、前缀 `w3qz4vc2`；`/root/iter/catalog-test/`（seed、`http/{cur,old,bad}`、日志）、`/root/iter/rel-0ee9065/`、`/root/skiff.before-0ee9065`、`/root/iter/skiff.db.before-0ee9065` 保留；serveo 隧道进程仍在（测试临时，域名可随时失效）。

**安装链路（`1862702` 全新安装轮）**
- 机器已还原：无 `/etc/skiff`、无 `admin` 用户、无旧数据根；按 Release 资产安装，不传本地二进制。
- 资产校验：`checksums.txt` 两个架构 `sha256sum -c` 均 OK；解压后 `sh install.sh --no-docker` 一次成功。
- 空库直建：`user_version=15`；`jobs`（含 `cancel_requested`/`log_seq`/`retry_of`）、`job_logs`、`trash_entries`、`fs_quotas` 全部建出；users 1 / plugins 0（安装前）/ site_runtimes 1。
- 初始管理员口令生成于 `/www/skiff/initial-admin.txt`（已转存 `/root/iter/admin-pw.txt`，0600）；安全入口随机生成 `udkwxz96`。
- 插件：为降级沙箱在 `config.toml` 追加 `allow_reduced_sandbox=true` 后重启，6 个官方插件全部 `已验签` 安装成功，grants 与 manifest 一致（`plugin list` 已核对）。
- 访问：形态三 `http://10.10.15.4:18081`（`skiff-http-fwd.service`），`/healthz` 200、登录页 200、`POST` 登录 303 并下发 `skiff_dev_session`。

**分组验收（`1862702` 全新安装轮）**
- **任务中心**：PHP 扩展安装全程走作业中心（`runtime.install` / 扩展安装作业），SSE 带 `Last-Event-ID` 可从断点续传、`/jobs/{id}/logs?after=` 可补历史；`docker exec` 阶段取消只置 `cancel_requested`，作业保持 running 到当前步骤结束，最终 `cancelled` 且 `error_detail` 为「已取消，扩展状态可能未生效，请核对后重试」；失败作业保留安装器尾部输出；同目标重复提交文案含 job id 与阶段；重试新建 job 且 `retry_of` 指旧 id。**超时失败未实测**（2026-11-08 晚已补实测，见下方「三条补充实测」）：安装器脚本每轮被内核重写，改写 `sleep` 探针无效。
- **文件管理**：`4755`/`6000`/`7777` → **403** 且理由含 setuid；`2775`/`1777` 生效；chown root 返回 `root_owned=true` 且审计 `root_owner=true`；zip / tar.gz 往返内容一致；穿越包 **403**、软链包 `skipped_symlinks=1` 不落链接；配额超限解压 **507** 且目标目录无残留；目录打包下载 zip 条目正确。
- **Web 五条**：明文访问无危险横幅；PHP 站点根有 `index.php`，Rust 站点无 `index.php` 且容器编译运行站点源码（Cargo.toml + src/main.rs）；两处入口均无 PHP 扩展区块；flash 只显示一次、刷新不重放，Location 无 `?notice=`/`?error=`；php.ini 合法值保存后 `php -i` 即生效。**发现缺陷：坏语法被放行**（PHP 只把 `syntax error` 打到 stdout、退出码仍 0）→ 已修并复核。
- **删站**：确认页展示 `677 B · 4 个文件`、绑定任务 0 个、绑定运行环境；确认域名填错 **400**；批量 `action=delete` **400**；正确删除 **303** 且目录移除。
- **FS-14**：内核写 `/etc`、`/usr/bin` → **403** `fs.read_only`「…只读，无法写入（宿主策略限制）」；构造 64 KB tmpfs 写满 → **507** `fs.no_space`；插件能力面写 `/etc` → **403** 且文案含「只读」（当时附带「不在已授权范围」误导提示 → 已修）。`EDQUOT` 构造不出 → **未实测**（2026-11-08 晚已补实测，见下方「三条补充实测」）。

**复核轮修复（`9daabd3`）**
- PHP 配置校验：坏内容 `memory_limit = 64M` + `[` 提交后 **303** flash「配置语法校验失败，已拒绝保存：PHP: syntax error, unexpected end of file…」；文件 md5 前后一致、`php -i` 仍为原值；合法 `memory_limit = 128M` / `upload_max_filesize = 32M` 保存后立即生效。
- 插件 403 文案：只读路径返回「该路径所在文件系统为只读，无法写入（宿主策略限制）」，不再追加「不在已授权范围」；复验后 files 授权恢复默认（grants=7）。

**三条「未实测」补充实测（2026-11-08 21:36–21:45，运行的是 §7 21:20 本地工作树二进制）**

- **任务中心超时（`error_class=timeout`）**：把 `ext_exec_timeout_secs = 6` 临时写入 `/etc/skiff/config.toml` 后重启，安装未装的 Xdebug（`bcmath/exif/gd/intl/sodium` 已装，重复安装是 no-op，故必须换扩展才能触发）。作业 `job-1794145039-1218b5c90`（`runtime.ext.install`）结果：`status=failed`、`error_class=timeout`、`phase_label=失败`、`progress=15`、`created_at=started_at=1794145039`、`finished_at=1794145045`（6 秒）；`error_detail` = 「扩展 Xdebug 执行超过 6 秒 未结束，已按超时失败结算；Docker 无法中断 exec，容器内脚本可能仍在运行，建议重载或重建运行环境。已采集输出尾部见任务日志。」；`job_logs` 共 **24** 条，seq 1–23 为安装器 `out` 行（含 `Warning: Module "intl"/"sodium" is already loaded`、`Updating channel "pecl.php.net"`、`fetch https://dl-cdn.alpinelinux.org/…`），seq 24 为同文案 `note` — 输出尾巴确实保留。验完删除配置键并重启，`config.toml` 无 `ext_*_timeout_secs`、`/<prefix>/login` 200。
- **EACCES / EPERM（403「权限不足」）**：`mkdir /www/iter-eacces` + `chattr +i` 造「可进入不可写」路径。内核管理员面 `POST /{p}/files/api/write` → **403**，`{"error":{"code":"fs.denied","message":"权限不足: /www/iter-eacces/.probe.txt.tmp-66300-0: Operation not permitted (os error 1)"},"ok":false}`；审计 `actor=admin action=fs.write result=deny code=fs.denied`。插件能力面（`com.skiff.files` RPC `fs.write`）→ **HTTP 403**，页面正文「内核返回 HTTP 403: 权限不足: /www/iter-eacces/probe.txt: Operation not permitted (os error 1)」；审计 `actor=plugin:com.skiff.files` 同文案 `result=deny`。两面都不是 500。验后 `chattr -i` 并删除目录。
- **EDQUOT（507「文件系统配额已用尽」）**：8 MB loop ext4（`mkfs.ext4 -O quota` 内建配额）+ `setquota` KB 级硬限。**关键构造点**：skiff 以 uid 0 运行且 `CapEff=000001ffffffffff` 含 `CAP_SYS_RESOURCE`，配额对 root 直接豁免（前三次构造写 20 KB 都返回 200，`repquota` 显示已超限也没用）；用 `setpriv --reuid=65534` 写同一挂载点会失败，证明配额本身生效。临时 drop-in `[Service] CapabilityBoundingSet=~CAP_SYS_RESOURCE` + `daemon-reload` + restart（`CapEff` 降为 `000001fffeffffff`）后：内核管理员面写 20 KB → **507**，`{"error":{"code":"fs.no_space","message":"文件系统配额已用尽，无法写入: /www/iter-quota/.big.bin.tmp-75024-0"},"ok":false}`；审计 `code=fs.no_space result=fail`。插件能力面同一次写 → **HTTP 400**，正文「…协议错误: 内核返回 HTTP 507: 文件系统配额已用尽，无法写入: /www/iter-quota/big2.bin」——代理层 `rpc_error` 当时只透传 403/404/409。已补 `AppError::NoSpace`（507）+ `rpc_error` 透传 507 + 单测 `rpc_error_passes_kernel_status_through` 覆盖；**插件面 507 已于 `456e040` Release 资产复验通过，见下方「收口轮复验」**。验后移除 drop-in、`daemon-reload` + restart（`CapEff` 回 `000001ffffffffff`）、`quotaoff`+`umount` 并删除镜像与目录。

**沿用通过（`8716e51` 小批收口轮与 `48f57dc` 收口轮，未复跑全部断言）**
- 装机/升级：Release 资产双架构 `sha256sum -c` OK；`sh install.sh --no-docker` 完成；库保持 **v14**，站点 2 / settings 7 / users 1 / plugins 6 全部保留；配置 sha256 保持 `0103e75a…f053e1`，`/healthz` 200。
- 配置保护组：删 / 改名 / 覆盖写 `/etc/skiff/config.toml` 全部 **403**「内核运行配置或数据目录受保护…（命中 /etc/skiff/config.toml）」；对 `/etc/skiff` 设配额 **403**（命中 `/etc/skiff`）；`/etc/hosts` 读 200；保护区外 `/www/iter-protect-probe.txt` 写/改名/读/删均 200（`/etc` 内其它路径不撞保护判定，写入被 systemd 只读挡回 500）；重启后服务 active、`/healthz` 与登录 200、配置 sha 前后一致。
- 插件日志组：journal 可见 monitor 启动横幅（`WARN … stream="stderr"`）；`kill -9` 子进程后 RPC **55 ms** 返回 200 并拉起新 pid，journal 先「上一次运行的 stderr 尾部」后新横幅，新日志不覆盖旧尾巴；初始化失败探针（临时替换二进制为 `exit 9`，已恢复、md5 `6c5ea70b…`）RPC **133 ms** 返回 400，正文、journal `ERROR` 与审计 `plugin.spawn` fail 的 `detail` 均带 `stderr 尾部: BOOT-8716e51 init failed probe`；恢复后 `status=running`、`status_detail` 清空。
- 上一批回归：回收站 删除→列表→恢复（内容仍为 `hello-a`）→彻底删除，回收站清空归零；配额 100B → 300B 写入 **507**「目录容量配额超限：… 307 字节 > 上限 100 字节」→ 重扫/取消后写回 200；Docker `GET /containers`、`GET /containers/new` 200，创建 `iter-lim-8716e51` 后 `docker inspect` 为 `mem=128MB / cpu=512 / pids=64`，限额更新后即时变 `256MB / 1024 / 128`，删除后容器消失；`GET /compose` 200，合法 YAML「校验通过」、非法 YAML 报错；FS-10～12 全链路（含同名冲突 409、父目录缺失 403、审计 `fs.recycle.*`/`fs.quota.*`）与 DOCKER-12～14 全链路（坏镜像回显、cgroup `pids.max`、Compose `up/down` 保护标签与卷策略）。
- monitor/guard 内核调度器（`driver=kernel`、停用任务不采样、卸载清任务、重装重 seed）与 WEB-02 `worker_connections` 无告警。
- TLS 三形态、430/860px 视觉、工作台标签与吸顶、插件 iframe 等前轮结论继续有效。

**`456e040` 收口轮复验（Release 资产，2026-11-08 22:48–23:12）**

- **插件面 EDQUOT → 507（FS-14 收尾）**：8 MB loop ext4 `-O quota` + `setquota` KB 级硬限，临时 `[Service] CapabilityBoundingSet=~CAP_SYS_RESOURCE` 剥掉 root 配额豁免（`CapEff` 由 `000001ffffffffff` 降为 `000001fffeffffff`）。内核 `POST /files/api/write` → **507** `fs.no_space`「文件系统配额已用尽，无法写入」；插件 `POST /plugins/com.skiff.files/rpc` → **HTTP 507**（上轮为 400），页面标题「507 空间不足」、正文「内核返回 HTTP 507: 文件系统配额已用尽」。验后移除 drop-in、`daemon-reload` + restart（`CapEff` 回 `000001ffffffffff`）、`quotaoff`+`umount` 并删除镜像与目录；服务 active、`/healthz` 200。
- **任务中心超时（`error_class=timeout`）**：Release 资产 + 临时 `ext_exec_timeout_secs = 6` 重启，安装 Xdebug。作业 `job-1794149311-af61efa0`（`runtime.ext.install`）：`status=failed`、`error_class=timeout`、`phase_label=失败`、`progress=15`、`created_at=started_at=1794149311`、`finished_at=1794149317`（6 秒）；`error_detail`「扩展 Xdebug 执行超过 6 秒 未结束，已按超时失败结算；Docker 无法中断 exec，容器内脚本可能仍在运行，建议重载或重建运行环境。已采集输出尾部见任务日志。」；`job_logs` **33** 条含安装器输出尾巴。验完删键重启，`config.toml` 无 `ext_*_timeout_secs`，服务 active、`/healthz` 200。
- **三档 UI（430/860/1024，测试机实测）**：headless Chromium（CDP）+ 会话 cookie 注入，7 页 × 3 档共 21 组合全部加载成功、0 条 JS 错误；`overflowPx` 最大 0（工作台/任务中心/插件/设置为负值），越界元素 offenders 全空；站点/容器/插件 `page-head .actions` 三档均在视口内；确认弹窗三档打开（宽 396/460/460、视口内、取消「取消」、返回 false）；提示条「任务已执行」0.5 s 可见、4.8 s 移除（430 与 1024）。截图 23 张（`d:\panel\.workbuddy\ui-456e040\`）。
- **性能三项**：3 GiB 站点日志页 **4.741 ms** 返回 200、下载 45,060 B，skiff `rchar` 增量 **2,097,152 B（恰 2 MiB）**（非 3 GiB），页面含尾标记与 199 行；30 s 巡检把 3,223,322,644 B 的 `access.log` 改名为 `.1` 并新建 0 B 新文件，skiff `write_bytes` 增量 **90,112 B**（约 88 KiB，与文件大小无关；复制方案需写 ~3 GB）；monitor 追加 3 点：11→14 行、2,461→2,546 B（+85 B）、inode 不变。
- **官方 monitor 包（`samples.jsonl`）**：新包装上后 `snapshot` 返回 `cpu=0.7`、`cpus=4`、`mem_avail_kb=3404336`、`net_rx_rate≈599.6 B/s`；`execute` 3 点纯追加（见上）；预置到 480 行再追加 6 点 → 压缩回 **240 行**（detail「追加 6 点，环形保留 240 点」，文件 4,758 B，属主仍 200115，无 `.tmp` 残留）；`history` 回读 240 点。验后清空 `samples.jsonl`（保留属主），**23:10:00 的 5 分钟 tick 实测追加 5 个真实点（1,132 B，属主不变）**，采样链路按新逻辑闭环。

**工作台三项复验（`97edf87` Release 资产，2026-11-09 00:35–01:05）**

- **iframe 高度（四插件 × 430/860/1024）**：860/1024 全部内容高＝iframe 高（cron 381/381、ftp 346/406、testkit 331/370、guard 304/304），`scrollHeight == innerHeight`，页级滚动 0；430 封顶生效（cron 561→513、ftp 635→453、testkit 466→453、guard 409→409），`innerWidth − clientWidth = 0`、`scrollHeight 900 / innerHeight 900`，上一轮那条 15px 页级滚动条彻底消失；在 iframe 中心发 `mouseWheel ΔY=400` 后整页截图变化、父页面 `scrollY` 仍为 0，内部滚动真实可用。截图与 `result.json` / `scroll-wheel.json` 在开发机 `.workbuddy/ui-65b10b9/`（不入库）。
- **主题占位**：单视图老包（testkit `single`，`tabs=0`）加载后 `::before` 为 `display:none`；面板与 iframe 底衬浅色 `rgb(246,248,250)`、暗色 `rgb(11,13,17)`，与 `--plugin-frame` 一致；`readystatechange=interactive` 时 `data-theme` 已是 `light`/`dark`，首帧 body 底色 `rgb(244,245,247)` / `rgb(14,17,22)`，无闪白。
- **Docker gate**：停 `docker.socket` + `docker` 8 秒后，概览 `/udkwxz96`（`-L` 后 200，gate 文案 4 处）、`/containers`、`/images`、`/networks`、`/volumes` 均 200、各 2 处「Docker 不可用」+ 1 处「前往 Docker 设置」、`hyper client error` **0**；恢复后全部 gate 归零、三容器 10 秒内自动 `Up`、skiff PID `122451` 前后一致（未重启面板）。guard 工作台页在 Docker 不可用时按 manifest `gates=["docker_available"]` 随菜单隐藏、深链 404（设计行为，非裸错误）。
- **探活开销**：30 s 内 skiff 进程 CPU（`utime+stime`）**40 ms**，≈0.133% 单核；3 秒一次 `ping` 无可见负担。
- **主链路回归**：`/files` 200、`/files/api/list?path=/www` 200、`/tasks` 200、`/jobs` 200；`POST /tasks/builtin:db-vacuum/run` 303，审计 `tasks.run target=builtin:db-vacuum result=ok`（内置任务同步执行、不落 `jobs` 行，故 `jobs` 表 0 命中属预期）。

**台账四项复验（`d197041` Release 资产，2026-11-09 01:05–01:12）**

- **WEB-04 站点配置候选校验四条**：
  - 语法失败拒绝：运行态保存含 `this_is_invalid_directive_xyz;` 的候选，HTTP 200 内联报错 `unknown directive "this_is_invalid_directive_xyz" … :4`（含指令名与行号）；vhost sha256 保存前后一致；站点仍 200、nginx 容器 running。
  - 替换后容器未保持运行则回滚：构造「隔离候选校验通过、但线上与默认站 `default_server` 冲突」的 vhost，保存后报 `配置已回滚到保存前内容：nginx 未能保持运行（当前状态 restarting）`；vhost sha256 恢复为保存前；4 秒后容器回到 running、站点 200、线上 `nginx -t` 通过。
  - 参数落库后应用失败回滚 DB：先在 `nginx.params_json` 落 `worker_connections=2048`，再在线上 include 目录植入一条非法指令使应用期 `nginx -t` 失败；提交 1024 被拒（`nginx -t 未通过`），DB 仍为 **2048**；清理植入文件后站点 200、容器 running。
  - 数字字段非法不回退默认：合法保存 2048 后提交 `worker_connections=abc`，页面提示 `worker_connections 需为 64–65535 的整数`，DB 保持 **2048**（未静默回退默认 1024）。
- **SITE-10 删除运行环境**：① rust 环境仍被 `iter-rust-1862702.test` 绑定时删除 → 303 + `运行环境仍被以下站点绑定…`，DB 行/容器/目录全部保留；② 对 `php-8.5` 发起 `install` 后在途立即删除 → 303 + `该运行环境有进行中的任务…`，DB 行保留；③ 安装完成后按无占用删除 → `docker ps -a` 无 `skiff-php-php-8.5`、`/www/runtime/php/php-8.5` 已删、`site_runtimes` 行消失，审计 `runtimes.delete` 成功。
- **插件授权默认值**：`com.skiff.files` 移除后全新安装 → **7** 项授权（6 个 `fs` 动词 + `object:path:/www`），与 manifest 一致；停用/启用后仍为 7；同版本覆盖重装后仍为 7。6 个插件逐项比对 manifest 与 `plugin_grants`：cron 6 / ftp 11 / guard 6 / monitor 2+`/proc` / files 6+`/www` / testkit 5+`/proc`，无任何超出声明集的授权。
- **BusyGuard 403**：新 testkit 包种入默认停用的 `plugin:com.skiff.testkit:busy-probe` 任务；任务中心启用后「立即执行」，运行历史显示 `执行期回调 scheduler 被拒绝：… 内核返回 HTTP 403: 插件正在执行任务，不能再调用 scheduler（BusyGuard 期望 403）`，审计 `scheduler.deny / deny / 插件正在作为任务执行目标`；验后任务已恢复停用。之前「缺上机入口」已由探针补齐。
- **回滚协同**：`deploy/docker-bootstrap.sh --check-only` 在 Docker 就绪机返回 **0** 并识别为已就绪；`/etc/skiff-baseline` 标记在本机不存在，`tools/iter.sh rollback-wait` 需真实快照回滚（外部 hypervisor）+ `.workbuddy/server.env`，本机无法构造，保持未闭环。

**`9e2c6b1` 修复轮（2026-11-09 01:23–01:35）**

- **现场缺陷（停止态复验时暴露）**：停止 `iter-rust-1862702.test`（`site_type=runtime`，绑定 Rust 1.99 环境）后点击启动 → **503**「运行环境 1.99 未运行，请先启动运行环境」，但容器 `Up`、`site_runtimes.last_status=running`。根因：`set_site_running` 的启动前置检查把「环境必须是 PHP 且有 FPM socket」套在所有 runtime 站点上，Rust 环境没有 socket 必然误判；只影响启动路径，建站直启与运行中站点不受影响。
- **修复（`9e2c6b1`）**：抽出 `ensure_runtime_ready_for_start`——所有运行环境只要求 `last_status == running`，PHP 额外要求 `socket_path` 存在且文件就绪；错误文案保持可读。单测 `runtime_site_start_guard_only_treats_php_as_socket_based` 覆盖 Rust running/stopped 与 PHP socket 缺失/就绪四条分支。
- **停止态三条补验**：①（d197041）停站后合法保存 → `配置已保存并通过校验（站点未运行，未重载）`，`.conf.disabled` sha 不变；②（d197041）停止态保存非法 vhost → 200 内联 `unknown directive … candidate:4`，文件 sha 不变；③（`9e2c6b1`）停止态磁盘植入非法配置后启动 → **409**「拒绝启动：站点配置未通过校验（… unknown directive … candidate:4）」，`.conf.disabled` sha 不变、未生成 `.conf`，线上 `nginx -t` 通过、容器 running。
- **修复后恢复**：还原合法配置再启动 → **303**，`.conf` 生成、DB `status=running`，站点正文 `Skiff Rust runtime ready haha version: 1.99 port: 9100`，`nginx -t` 通过、容器 running。
- **回归抽查**：运行态保存非法 vhost 仍 200 内联报错且 vhost sha 不变、站点 200；`worker_connections=abc` 仍提示「需为 64–65535 的整数」且 DB 保持 **2048**；安装二进制 `df1ba659…` 与包内一致、配置 sha 未变、6 插件 grants 不变（files 7 / cron 6 / ftp 11 / guard 6 / monitor 3 / testkit 6）。

**`2f63aeb` 三块环境受限项复验（2026-11-09 01:40–02:05）**

- **FTP fail2ban 三次封禁（FTP-05）**：第二客户端用 `skiff-ftp-probe-client`（`python:3.13-alpine`，自定义网络 `skiff-ftp-probe-net` 172.31.0.0/24，客户端 IP **172.31.0.2**，与宿主不同网段，绕过 `ignoreself`），目标 `10.10.15.4:2121`。
  - **缺陷复现**：把 `jail.d/skiff-pureftpd.conf` 退回旧内容（无 `port`）并重启 fail2ban，手动封探针 IP 后 nft 链为 `tcp dport { 20, 21, 989, 990 }`——`fail2ban-client status` 显示已封，但 2121 不被拦（旧实现漏封非 21 端口）。
  - **产品路径**：`config.save{listen_port:2121}` → jail 落盘含 `port = 2121`，fail2ban 重启，链变为 `tcp dport 2121`。
  - **三次错误密码**（`wrong1` 超时、`wrong2`/`wrong3` 530）→ `Currently banned: 1`（172.31.0.2）；`banip --with-time` 显示 `2026-11-09 01:54:49 + 600s`；第 4 次同一 IP 用正确口令 → **`ConnectionRefusedError`**（nft reject 生效）；面板 `status` 返回 `currently_banned=1`、`recent_ban` 带时间、`total_banned=2`、`listen_port=2121`、`running=true`。
  - **解封恢复**：`unbanip` 后同 IP 正确口令 `LOGIN_OK`（`pwd=/`）。
  - **审计**：`ftp.service.configure`、`ftp.status`、`ftp.uninstall` 有行；逐 IP 封禁不在内核审计（fail2ban journal/nft 为唯一证据），与契约一致。
  - 修复已由 `2f63aeb` 落地（jail 钉住 `listen_port`，单测 `jail_body_pins_configured_listen_port`）；本条是修复后的 2026-11-09 复验。
  - 清理：账号与 `ftp-ban-probe.test` 站点已删；FTP 服务 `uninstall`（`removed_accounts=1, stopped=true`）；全部解封；`jail.d` 仅剩 `[sshd] enabled=false`、`fail2ban` jail 数 0、无 f2b nft 规则；2121 已停。
- **1Panel 受限形态（DEPLOY-10）**：镜像 `skiff-panel-probe:2f63aeb`（`alpine:3.23` 运行阶段 + 发布包 `skiff`，与 `deploy/Dockerfile` 运行阶段等价，未跑 builder 阶段）；容器 `skiff-panel-probe`，`127.0.0.1:18082→18080`，`SKIFF_FORM=docker`、`SKIFF_ALLOW_INSECURE_HTTP=1`、`SKIFF_TRUSTED_PROXIES=172.16.0.0/12`、`/data` bind、`/var/run/docker.sock`。
  - 启动正常、`/healthz` **200**、`/skiff health` HEALTHCHECK **healthy**、登录 **303**、数据根 `/data` 0700、首启建库 v15 与 `initial-admin.txt`。
  - 能力面收缩实测（确认安装均 **400**）：`com.skiff.files`/`monitor`/`guard`/`testkit` →「插件需要能力 fs，当前部署形态不提供（能力面收缩见 §5.4）」；`com.skiff.ftp` →「插件需要能力 ftp…」；`com.skiff.cron` →「要求完整沙箱（requires_full_sandbox = true）…」；`plugins` 表为空。
  - 容器管理可用：`/containers/rows` 200 列出 5 个容器（含 `skiff-web-nginx`）；`/containers/{id}/stop|start` 均 **303**，`docker inspect` running→exited→running，审计 `docker.containers.stop|start result=ok`。
  - **文件管理**：入口仍可见且可用（容器内 `/`、`/www` 可列）；仅内核数据目录 `/data` 被拒（`fs.denied`「内核运行配置或数据目录受保护」）。**与「不可见或明确提示能力面未启用」的口径不一致**，记为待定项（见 [TEST_LEDGER.md](TEST_LEDGER.md)）。
  - 清理：容器/镜像/网络/`/root/iter/panel-probe` 已删；宿主面板 `/healthz` 200、三容器 running。
- **aarch64（DEPLOY-09）**：包内二进制 **10,702,888 B**、sha256 `8891cf4b7193f2835997b5ba07fb0b0780c88c6a9b90a575b2c64f55425e29ce`；`file` = `ELF 64-bit LSB executable, ARM aarch64, … statically linked, stripped`；`ldd` = `not a dynamic executable`；`qemu-aarch64-static`（10.0.13）下 `skiff version` → `skiff 0.1.0 / arch=arm64 os=linux`，`skiff --help` 正常列出命令，`skiff doctor` 全绿。
  **结论：qemu-user 冒烟通过，未实机装机，不记为装机通过。**（`--version` 非 CLI 旗标，版本用 `version` 子命令。）

**`607d09b` 发版复验（Release 资产，2026-11-09 03:25–03:55）**

- **安装**：`sha256sum -c` 与外层 `checksums.txt` 双重校验 OK；`sh install.sh --no-docker` 成功；库保持 `user_version=15`、配置 sha `ebcccae6…` 未变；二进制 `01ad573e…985d7348` 13,050,640 B 与包内一致；`/healthz` 200、`skiff version` = `skiff 0.1.0`。
- **Node/Go 摘除**：`/sites?new=1&type=runtime` 下拉只剩 `PHP 8.5` / `Rust 1.99`，页面无 Node/Go 字样；`site_runtimes` 仅 `iter-php-1862702`(php) 与 `rust-iter-rust-1862702`(rust)。弹窗「运行环境」副标题原为 `PHP / Node / Go / Rust`，已改 `PHP / Rust`（`crates/web/templates/site_new_dialog.html`，本轮修复）。
- **WAF 残留**：`/settings` 页面 waf 命中 **0**；`/etc/skiff/config.toml`、包内 `config.toml.example`、`README.txt` 均 **0**；无 `sites.waf_mode` / `waf_rules_dir` 键。
- **FTP jail `port`（全新安装路径）**：先 `uninstall{remove_accounts:true}`（jail 回落 `[sshd] enabled=false`），再 `install` → 新 jail 为 `[pure-ftpd] enabled=true / backend=systemd / port = 2121 / maxretry=3 / findtime=10m / bantime=10m`；`fail2ban-client status` 仅 1 个 `pure-ftpd` jail；强制封一个探针 IP 后 nft 链为 **`tcp dport 2121`**（无 20/21/989/990），`ss` 监听 `*:2121`。
- **FTP 主链路回归**：新建 `ftp-probe2-1862702.test`（PHP 运行环境）→ `create` 账号 → FTP 2121 上传 `ftp-probe2-upload.txt` → 站点容器 `skiff-php-iter-php-1862702` 读到内容 → 容器内写 `ftp-probe2-cw.txt` → FTP `RETR` 读回 `container-write-ok` → `delete` 账号（`remaining_accounts=0`）→ 删站点 303 且目录回收；审计含 `plugin.rpc` / `sites.delete`。
- **任务中心与文件管理主链路**：`/tasks` `/jobs` `/files` `/containers` `/sites` 均 **200**；`/files/api/list?path=/www` 200。**发现并修复**：`/{p}/m/core/jobs` 之前 404（`handlers/menu.rs::dispatch` 漏分支），本版已补分支并 200。
- **受限形态（DEPLOY-10，`SKIFF_FORM=docker`）**：探针镜像 `skiff-panel-probe2:607d09b`（`alpine:3.23` + 本版发布二进制，等价 `deploy/Dockerfile` 运行阶段），`127.0.0.1:18082→18080`，`SKIFF_ALLOW_INSECURE_HTTP=1`、`SKIFF_TRUSTED_PROXIES=172.16.0.0/12`、`/data` bind、`/var/run/docker.sock`。
  - 首页菜单：`core/files` / `core/files/trash` / `core/files/quotas` 链接 **0 条**，「文件管理 / 回收站 / 配额管理」文案 0 次；`core/tasks`、`core/jobs` 入口保留。
  - 直达拒绝（均 **403**，正文含「当前部署形态不提供文件管理能力（1Panel 受限形态能力面仅 docker/identity）」）：`/m/core/files`、`/m/core/files/trash`、`/m/core/files/quotas`、`/files`、`/files/trash`、`/files/quotas`、`/files/api/list`、`/files/api/tree`、`/files/api/trash`、`/files/api/quotas`；`POST /files/api/{mkdir,write,upload}` 同样 403（不再是列容器内 `/`、`/www`）。
  - 容器管理仍可用：`/containers` 200 并列出站点容器；临时容器 `skiff-p2-probe` 详情 200、`stop`/`start` 均 **303**，`docker inspect` running→exited→running，审计 `docker.containers.stop|start result=ok`。
  - 清理：探针容器/镜像/`/root/iter/panel-probe2` 与临时容器已删。

**`8acc83f` 收口发版复验（Release 资产，2026-11-09 04:30–04:55）**

- **安装**：双架构 `sha256sum -c` 与外层 `checksums.txt` 均 OK；包内逐文件校验 OK（含 `vendor/install-php-extensions`）；`sh install.sh --no-docker` 成功；库保持 `user_version=15`、配置 sha `ebcccae6…` 未变；`/usr/local/bin/skiff` 13,050,640 B `5959d021…64a72b8` 与包内一致；`skiff version` = `skiff 0.1.0 arch=amd64 os=linux`。
- **删站确认页（按钮顺序）**：`GET /sites/btn-probe-1862702.test/delete` **200**；限定表单 `data-type-confirm="btn-probe-1862702.test"` 内部比较，`cancel_idx=413 < delete_idx=501`；按钮为 `<a class="btn ghost">取消</a>`（左）与 `<button class="btn danger" type="submit" disabled data-type-confirm-submit>删除站点</button>`（右）；`data-type-confirm-input` 存在，输入域名前提交按钮 `disabled`。
- **绕过前端**：直接 `POST /sites/<domain>/delete` 空 `confirm` → **400**；`confirm=wrong.test` → **400**；`confirm=btn-probe-1862702.test` → **303**，站点记录与目录一并回收（余 `iter-rust-1862702.test`）。
- **插件安装确认页（`plugin_confirm.html`）**：临时卸载 `com.skiff.testkit` 后重新上传包，确认页 **200**；`cancel_idx=23500 < confirm_idx=23555`，行为 `<a class="btn ghost" href="/udkwxz96/plugins">取消</a>` 在左、`<button class="btn primary" type="submit">确认并安装</button>` 在右，隐藏 `token` 24 位；按原 6 项授权（`fs:list` / `fs:read` / `identity:self` / `interop:call` / `scheduler:read` + `resource=path:/proc`）提交后装回，6 插件与 grants 数量复原。
- **共享确认弹窗**：`/sites` 与 `/containers` 两页均 `dialog` 存在、DOM 顺序 `data-confirm-cancel`（`btn ghost`，「取消」）在 `data-confirm-ok`（`btn danger`，「确认执行」）之前；容器删除按钮走共享弹窗（`data-confirm="确认删除容器 skiff-php-iter-php-1862702？…"`、`data-confirm-verb="删除"`、`hx-post …/remove`）。
- **主链路回归**：`/sites`、`/tasks`、`/jobs`、`/files`、`/files/api/list?path=/www`、`/containers`、`/plugins` 全 **200**；`/healthz` **200**；`systemctl show skiff -p ActiveState,NRestarts` = `active` / **0**。
- **终态**：`user_version=15`；6 插件（cron / ftp / guard / monitor / files / testkit）；站点仅 `iter-rust-1862702.test` running；容器 `skiff-web-nginx` / `skiff-php-iter-php-1862702` / `skiff-rust-iter-1862702` 均 Up；探针站 `btn-probe-1862702.test` 的站点、目录与 vhost 已删除；远端 `/root/iter/v1.sh v2.sh v3.sh` 已清理。

**`9a656d5` 台账清理轮复验（Release 资产，2026-11-09 04:29–04:36）**

- **并发拉起（32 路）**：预热 `ping` 拉起 `com.skiff.testkit` 后 `kill -9` 子进程（`pgrep` 立即为空），紧接 32 路并发 `ping` RPC → **32/32 200**、`error` 0 条、无 `Connection refused`；journal「插件已拉起」只 +1（崩溃后单次重建），最终单进程 pid `147607`；`skiff.service` `active` / `NRestarts=0`，journal 无 panic/SIGSEGV/core。`8acc83f` 轮同法首测：32/32 200，delta=2（预热 1 + 重建 1），亦单进程。
- **下载端点六种输入**（`GET /{p}/files/api/download`，`8acc83f` 为修复前、`9a656d5` 为修复后）：
  - 缺参：**400** `files.bad_request`「缺少 path 参数」（两版一致）；
  - 目录 `/www`：**200**（打包 zip 流，159 MB）；
  - 受保护 `/etc/skiff/config.toml`：**403** `fs.denied`（两版一致）；
  - `/www/../etc/passwd` 与 `/www/%2e%2e/etc/passwd`：**403** `fs.denied`「路径含非法片段」（两版一致）；
  - 1 GiB 文件：**200**，1,073,741,824 B 完整输出、1.05 s，无中断（两版一致）；
  - 不存在文件 `/www/no-such-1862702.txt`：修复前 **403**「无法解析目标路径（父目录不存在或不可访问）」→ 修复后 **404**「/www/no-such-1862702.txt 不存在」；父目录不存在时仍为 **403**（硬拒口径不回退）。
- **主链路**：`/sites`、`/tasks`、`/jobs`、`/files`、`/files/api/list?path=/www`、`/containers`、`/plugins`、`/settings` 全 **200**；`/healthz` **200**；`NRestarts=0`。
- **WAF 口径**：WAF 未实现，已移出范围（配置与规则目录已删除），不再作为台账未闭环项。

**`ef7058b` 升级路径复验（Release 资产，2026-11-09 05:57–06:07）**

- **造包方式**：官方发布私钥只在 CI secrets、本机不可得，改用 `skiff-plugin keygen` 现生成测试钥（指纹 `3478875f…43bdf5e`）；在测试机上把官方 testkit 0.1.0 包解包、替换为 0.2.0 manifest 后 `stamp` + `pack`，产出 **327,489 B / md5 `54a3d53a577ff212943fdc40df3c33bb`** 的签名包（10 文件 / 891,907 B）。0.2.0 与 0.1.0 的差异：版本号、`busy-probe` 调度（`expr` `*/2 * * * *`、采样 1800 s、容量 8192 B、超时 15 s）、`fs` 新增敏感动词 `write`。
- **换钥闸门（本轮唯一人工越权）**：直接安装被拒「拒绝安装: 升级包签名公钥与已装版本不一致：换密钥视为新信任，需重新确认指纹」（审计 `id=18787 result=fail`）。为在不卸载的前提下走真实升级，临时把 `allow_unsigned` 置 `true`（备份 `/root/iter/config.before-allowunsigned.toml`），装完立即复原（sha 回到 `ebcccae6…`）。**不是同版本重装**：版本、manifest、签名、任务 spec 全部来自 0.2.0 包。
- **升级结果（0.1.0 → 0.2.0，CLI 安装 + 重启种入）**：
  - spec 已刷新：`expr` `* * * * *` → `*/2 * * * *`、`sample_interval_secs` 3600 → 1800、`capacity_bytes` 4096 → 8192、`timeout_secs` 10 → 15，`updated_at=1794175550`；`kind` 与 `payload_json` 保持不变。
  - 用户状态保留：`enabled=1`（升级前人工置 1）、`created_by=system:manifest`、`created_at=1794172700` 均未变；`task_runs` 2 条历史保留（`last_started=1794157634`）。
  - 权限补授：grants 7 → **8**，新增 `fs:write`；`object:path:/proc` 与 `scheduler:manage` 保留。
  - 审计：`plugin.grant_upgrade`（`id=18793, actor=system, result=ok`）detail = `升级新增授权: fs:write；版本 0.1.0 → 0.2.0（敏感: fs:write）`；另有 `plugin.install`（`signed=true grants=8`）与版本 0.2.0 的安装审计。
- **卸载回收**：`skiff plugin remove com.skiff.testkit` 输出「已卸载（delete_data=false，回收任务 1 条）」，`scheduled_tasks` 中 `source=plugin:com.skiff.testkit` 行立即消失、重启不再出现；`task_runs` 历史按设计保留。重装官方 0.1.0 后按 manifest 重新种入（`enabled=0`、`created_by=system:manifest`、新 `created_at=1794175576`），grants 回到 7（未再声明的 `fs:write` 被过滤）。
- **收尾**：官方 0.1.0 在装；`trusted_keys` 仅剩 `official`；`/root/iter/upgrade`（含测试私钥）与 0.2.0 包已删；`allow_unsigned` 还原为注释默认。
- **回归**：登录 303；`/sites` `/tasks` `/jobs` `/files` `/containers` `/plugins` `/settings` 全 **200**；裸 `/` **404**（AUTH-02 预期）、`/healthz` **200**；`NRestarts=0`；近 30 分钟 journal `ERROR` **0**。

**`0c99464` monitor 与 TASK-06 复验（Release 资产，2026-11-09 05:10–05:28）**

- **monitor 出站速率（全速窗口）**：`scp` 下载 200 MiB 文件耗时 9.04 s，采样窗口 10 s。宿主 `/proc/net/dev` 非 lo 接口 tx 差 **210,657,428 B** → 期望 **21,065,742.8 B/s**；插件 `net_tx_rate` = **21,065,742.8 B/s**，与宿主**逐字节一致、偏差 0.00%**（`net_tx` 字段同样为 210,657,428 B；before/after 之间 396 B 的偏移只来自两次读取时刻不同，不计入窗口差值）。
- **拉长窗口不被内核采样截断**：`scp -l 16384` 下载同文件 106 s，整窗均值 **1,985,505 B/s**。期间内核 60 s 采样只推进 `prev.json`（3060→3120），`prev_ui.json` 未被内核改写——两份基线独立，内核采样不截断 UI 快照窗口。面板 UI 自身约 60 s 一次的 `snapshot` 轮询把末段窗口缩到 **54 s**（`prev_ui` t=3051），插件报 **1,943,259.9 B/s**：对同 54 s 窗口期望 **0.00%**，对整窗 106 s 均值 **2.13%**。结论：窗口误差只来自 UI 自轮询覆盖 `prev_ui.json`（已知行为，非内核采样截断）。
- **TASK-06 缓冲探针**（testkit 重装取得 `scheduler:manage`、删行重种任务后）：
  - 基线 `buffer_probe` → `points=0`、`capacity_bytes=4096`。
  - `buffer_push{count:100}` → `accepted=100`、`rejected=0`、`dropped_points=17`、`dropped_bytes=833`、`signals=[high_watermark, overflow]`、`last.seq=99`。
  - 随后 `buffer_probe` → `points=83`、`bytes=4067`、`capacity_bytes=4096`、`seq=100`、`head_seq=17`、`tail_seq=99`、`dropped_points=17`、`dropped_bytes=833`、`rejected_points=0`、`rejected_bytes=0`、`high_watermark=true`（drop-oldest：100 点约 4,900 B 超 4,096 B 容量，保留 83 点 / 4,067 B、丢 17 点 / 833 B）。
  - 限速（两条都验）：同秒再推 1 点 → **HTTP 400**「buffer.push 超过限速（每任务 100 点/秒）」；`count=101` → 本地 **-32602**「count 需在 1..=100（内核限速每任务 100 点/秒）」。
  - 超限单点：约 70 KB 的单点 → `accepted=false`、`bytes=70043`、`reason=point_too_large`（插件汇总 `rejected=1`；内核 `rejected_points` 仍为 0，因在入缓冲前即被拒）。
  - 权限：无 `scheduler:manage` 的旧包调 `buffer_push` → **403**「缺少 scheduler:manage」。
  - **构造不出的断言**：`rejected_points` / `rejected_bytes` 的累计——拒绝发生在入缓冲前，且在 drop-oldest 策略下不累计；如实记为未覆盖。
- **主链路回归**：`/` **307**（AUTH-01 预期，跳登录页）、`/sites` `/tasks` `/jobs` `/files` `/containers` `/plugins` `/settings` 全 **200**；`/healthz` **200**；`systemctl show skiff -p ActiveState,NRestarts` = `active` / **0**；journal 近 30 分钟 `ERROR` 计数 **0**，无 panic/SIGSEGV。
- **同版本升级两项已知缺陷（该资产未含修复）**：①`seed_manifest_tasks` 对已存在任务直接 `continue`、不刷新 spec，且 CLI `plugin remove` 不删 `scheduled_tasks` 行，故同版本覆盖升级后 busy-probe 仍无缓冲声明，需 `systemctl stop skiff` → 删 `plugin:com.skiff.testkit:busy-probe` 行 → 启动重种；②同版本升级保留旧 grants、不自动授予 manifest 新增权限，testkit 需 `plugin remove` + `plugin install --yes` 才拿到 `scheduler:manage`。工作区已修复（`docs/changes/2026-11-09.md`），台账保留「待上机复验」。

**未闭环（唯一来源为 [TEST_LEDGER.md](TEST_LEDGER.md)）**
- aarch64 未实机装机、Release 下载端点容错、插件 UI 窄屏/跨浏览器、反代与 WS 残余等未闭环项的当前状态与下一步，统一见台账；本节只记已通过的断言。
- 2026-11-10 `483f898` 轮登记的「FTP→firewall 对象授权在升级路径缺失」已随 `f0c9872`/`c7a06b4` 修复，并在本轮以 `491da40` 真实旧包路径复验通过（见下方「`c7a06b4` 升级授权补齐复验」），台账该行已删除。
- TASK-06 缓冲、monitor 出站速率两条已随 `0c99464` 复验通过并从台账删除；「同版本升级不刷新任务 spec / 不补授新权限」两项已随 `ef7058b` 修复并上机复验通过，台账对应行已删除。
- 升级复验的两条限制：①官方发布私钥不可得，0.2.0 包用自签钥产出（换钥闸门由临时 `allow_unsigned` 越过），「官方钥签名包跨版本升级」本身未验，本轮验的是版本升级逻辑（spec 刷新 / 补授 / 审计 / 回收）；②「用户未勾选项不被翻案」在 CLI 无拒绝交互，构造不出，只由本机单测覆盖。
- 受限形态「文件管理」入口与直达已按能力面处理，`607d09b` 装机复验通过，该行已从台账删除。

**收尾状态（历史：2026-11-09 `ef7058b` 复验后）**
- 面板：形态三明文 `http://10.10.15.4:18081/udkwxz96` 可用（admin，口令文件 `/root/iter/admin-pw.txt` 第 3 行）；`skiff.service` active、`NRestarts=0`、`skiff-http-fwd.service` active；`/usr/local/bin/skiff` 13,063,056 B、sha256 `5af48e15…f9c786260`（与 `ef7058b` 包内一致）。
- 资产：x86_64 包 8,823,281 B（`dbd65a15…246103e15`）、aarch64 包 8,415,416 B（`1dabb995…d1a0eb4867`，包内二进制 `94cac88d…0cd951bf`，10,723,496 B）。
- 数据：库保持 `user_version=15`；6 个官方插件已装（grants：cron 6 / files 7 / ftp 11 / guard 6 / monitor 3 / testkit **7**）；站点仍为 1（`iter-rust-1862702.test`，`status=running`）；`site_runtimes` 2（php / rust）；testkit `busy-probe` 任务 `enabled=0`（默认停用）。
- 配置：`/etc/skiff/config.toml` sha256 `ebcccae6…` 覆盖安装前后一致。
- 清理：本轮无新增站点/容器/账号；升级复验的临时物（自签测试钥、0.2.0 包、`/root/iter/upgrade/`）已删，`trusted_keys` 仅剩 `official`；缓冲为内核内存态（随停服清空），monitor 只读 `/proc`。
- 容器：`skiff-web-nginx` / `skiff-php-iter-php-1862702` / `skiff-rust-iter-rust-1862702` 全部 running；宿主面板 `18080` 与转发 `18081` 的 `/healthz` 均 200。
- 备份：`/root/iter/config-dev-20261108-fresh.toml`、`/root/iter/config.before-allowunsigned.toml`、`/root/iter/admin-pw.txt`、`/root/skiff.before-d80661b`、`/root/iter/aef7058b/`、`/root/iter/a9a656d5/`、`/root/iter/a8acc83f/`、`/root/iter/a607d09b/`、`/root/iter/ad80661b/`、`/root/iter/a2f63aeb/` 保留。

**`491da40` 全新安装轮（2026-11-09 21:20–21:55）**

前置：机器还原后 `/etc/skiff` 不存在、`skiff.service` 与 `skiff-http-fwd.service` 均不存在、`uptime` 78 min；Docker 已装且 `docker info` 通（`DOCKER_READY`），`skiff doctor` 全 ok。

安装：Release 资产 x86_64 `6d66d6ed…2fc7154ff9` → `scp` → 测试机 `sha256sum` 复核 → 解压 → `sh install.sh --no-docker`（退出 0，脚本提示已追加 `allow_insecure_http = true`）→ 服务 active。入口前缀 `w3qz4vc2`；`/www/skiff/initial-admin.txt`（0600）第 3 行 `口令: …` 提取为 `/root/iter/admin-pw.txt`（0600，17 B）；`/etc/skiff/config.toml` 追加 `allow_reduced_sandbox = true` 后重启；重建 `/root/iter/skiff-http-fwd.py` 与 `skiff-http-fwd.service`（`0.0.0.0:18081 → 127.0.0.1:18080`）。6 个官方插件 `skiff plugin install <pkg> --yes` 全装成。

首轮验收（35 项 PASS / 0 FAIL）：

- AUTH：`/$P/` 307 → `/$P`；`/$P` 303 → `/$P/login?next=…`；`/healthz` 200；前缀 `w3qz4vc2` 符合 8 位字符集；裸 `/` 与 `/static/skiff.js` 均 404；登录页带 `csrf`；不带 `csrf` 的 POST 403；带 `csrf` 303；`Set-Cookie: skiff_dev_session=…; HttpOnly; SameSite=Lax` 且无 `Secure`；会话内 `GET /$P` 200。
- AUTH-05 限流：前 5 次错误口令返回错误页 200，**第 6 次起 503**（页面含「剩余 299 秒」），封禁期内正确口令仍 503；静置 345 s 窗口自然过期后正确口令 303，随后 1 次错误口令返回错误页 200（非 503），计数已清零。全分支通过。
- FS（14 项）：`mkdir` / `write` / `read` / `list` / `archive(zip)` / `extract` / 解压内容一致 / `remove` → `/files/api/trash` 含条目 / `trash/restore` 后可读 / 二次删除 → `trash/purge` / `quotas` 200 全部 PASS；根外写 `/etc/acc-denied.txt` 返回 **403**（审计 `fs.write` 被拒，`fs.read_only`）。
- PLUGIN 能力面：`com.skiff.files` RPC `list /www` 与 `write /www/acc-*/plugin.txt` 均有 `result`；`write /etc/acc-plugin-denied.txt` 被拒（4xx，审计落 `plugin.rpc` 失败）。
- TASK：`/tasks` 200 且含可运行任务；`POST /tasks/builtin:db-vacuum/run` 303；`/jobs` 200（内置任务同步执行，不落 `jobs` 行，符合设计）。
- 审计落库：`skiff audit 12` 可见 `fs.write` 拒绝、`plugin.rpc` 失败、`task.run`、回收站 purge、调度器触发等条目，写库正常。
- SITE + nginx：`POST /sites/env/web/action/install` 拉取 OpenResty 并起容器 `skiff-web-nginx`（running）；建静态站 `matrix.local` 303、站点页可见；vhost 落 `/www/sites/matrix.local/conf/matrix.local.conf`；容器内 `nginx -t` 通过；写入 `index/index.html` 后 `curl -H 'Host: matrix.local' http://127.0.0.1/` 200 且返回 `matrix-ok`。
- 限制：aarch64 未实机验证；未做 TLS 形态切换与浏览器视觉项。测试目录 `/www/acc-*` 已删，`matrix.local` 站点保留；`trash_entries` 余 1 行为 restore 历史行。

TASK-06 缓冲能力面补验（同日 22:06 之后，经 `com.skiff.testkit` RPC `buffer_probe` / `buffer_push`，登录会话 + `x-csrf-token`）：

- 目标任务 `plugin:com.skiff.testkit:busy-probe`（`capacity_bytes=4096`、`sample_interval_secs=3600`、`enabled=false`、source=`com.skiff.testkit`）；初始 `stat` 为全 0（`points=0 / seq=0 / head_seq=null / tail_seq=null`）。
- `buffer_push count=100`：`accepted=100 / rejected=0`，`stat` 变为 `points=83 / bytes=4067 / seq=100 / head_seq=17 / tail_seq=99 / dropped_points=17 / dropped_bytes=833 / high_watermark=true`，回执 `signals=["high_watermark","overflow"]` —— drop-oldest 生效，单点约 49 B（序列化 + 32 B 开销）。
- 限速：两路 `count=100` 并发时，其中一路在第 51 点被内核以 400「buffer.push 超过限速（每任务 100 点/秒）」拒绝（两路交错后同秒内合计越 100 点）。
- 参数与越权：`count=101` 由 testkit 本地校验 400 拒（1..=100）；`buffer_probe` 对非本插件任务 `builtin:db-vacuum` 返回 **403**（内核 `ensure_owned`）。
- 内核启动日志确认按 manifest 种入 `plugin:com.skiff.guard:reconcile`、`plugin:com.skiff.monitor:sample`、`plugin:com.skiff.testkit:busy-probe` 三条任务；`rejected_*` 统计仍无法在本轮构造（拒绝发生在入缓冲前）。


**`483f898` 防火墙 + Node 运行时轮（2026-11-10 08:05–08:40，Release 资产覆盖安装）**

- 安装：Release x86_64 9,395,086 B `dae3157b…c97641` → `scp` → 测试机 `sha256sum -c --ignore-missing checksums.txt` OK → `sh install.sh --no-docker`（退出 0）。库保持 v15，入口前缀 `w3qz4vc2` 不变，`/etc/skiff/config.toml` sha `3133d44b…` 覆盖前后一致；`/usr/local/bin/skiff` 13,528,720 B `255846ea…07baee` 与包内一致；`skiff.service` active、`NRestarts=0`、`skiff-http-fwd` active，18080/18081 `/healthz` 均 200。
- 插件：`/www/skiff/official/` 7 包齐备；6 个既有插件随包重装，`com.skiff.firewall-0.1.0` 由 CLI 新装（grants 12，验签通过），共 7 个已装。
- AUTH（5 项 PASS）：`/w3qz4vc2/` 307、`/w3qz4vc2` 303、`/healthz` 200；无 csrf 的 POST 403、带 csrf 303；`Set-Cookie: skiff_dev_session=…; HttpOnly; SameSite=Lax` 且无 `Secure`；`POST /api/v1/session` 取 Bearer 后 `/api/v1/docker/containers` 正常、无 Bearer 401；X-Content-Type-Options / X-Frame-Options / CSP 齐备。
- FS（8 项 PASS）：`/files/api/list|read|write|mkdir|rename` 正常；列 `/www/skiff/skiff.db`、`/www/skiff/plugins` 与读 `skiff.db` 均 **403**（审计 `fs.read` → 「路径不在已授权的 path: 根内」）；300 KiB 文件读回 `text` 恰 262,144 B 且 `truncated=true`（>256 KiB 只读）；`/files/trash`、`/files/quotas` 200；`/$P/plugins/com.skiff.files/main` → **301** `/$P/files`；`/www/skiff` 0711、`plugins` 0711、`skiff.db` 0600。
- PLUGIN 能力面（4 项 PASS）：testkit `ping` 200；`fs_read /proc/version` 200、`/etc/passwd` **403**；未授权方法 `fs_write` 400；失败落 `plugin.rpc` 审计。
- 监控（PASS）：`processes` / `network` RPC 均 200 带数据（network 首次调用 `cpu_baseline=false`）；进程、网络两个视图页 200。
- TASK（PASS）：`/tasks` 200；`POST /tasks/builtin:db-vacuum/run` 303，`task_runs` 落 `result=ok`（含备份路径）；guard / monitor 周期任务同轮 `ok`。
- 审计（PASS）：可查 `firewall.status|rules.list|rule.add|rule.remove|fail2ban.*`、`interop.call`（ok 与 deny）、`sites.env.add|create|start|default|delete`、`plugin.rpc` 失败、`fs.read` deny。
- firewall（FW-01～06 PASS）：`status` 报 `backend=nftables`、`fail2ban.installed=false`；`rule.add` 建 `table inet skiff`，input 链 `policy accept` 未改、宿主其它规则不动；`rule.remove` 后表内无残留规则；`port=0` / `70000` / `"9-3"` / `source=300.1.1.1` / 未知 id 全部 400；`fail2ban.install` 首次安装并 running、二次调用幂等（jails 仍为 `["sshd"]`，写 `jail.d/skiff-defaults.conf`，SSH 未封）；`jail.ensure(pure-ftpd, filter=pure-ftpd, port="30000-30100,21", backend=systemd)` 写 `jail.d/skiff-pure-ftpd.conf` 并 restart；`ban` / `banned` / `unban`（203.0.113.9）全通；`jail.remove` 删 conf 后 fail2ban 仍 active。缺 `filter` 的 `jail.ensure` 400（参数必填校验正常）。
- Node 运行时（5 项 PASS）：`POST /sites/env/add kind=node version=24` → 303，`site_runtimes` 由 `installing` 转 `running`，容器 host 网络监听 `127.0.0.1:9200`；建 `node-acc.test` / `node-acc2.test` / `acc-a.test` / `acc-b.test` 均为 `site_type=runtime` 且 303，`curl -H 'Host: …'` 各域名 **200** 并返回运行时页面（同一进程一个端口服务多站）；站点详情「共享影响」显示「该环境已被 2 个站点使用，重启/升级/改端口会影响全部」，环境页显示「一个环境只有一个应用进程与一个端口：停止、重启、重建、改端口都会影响全部站点」；`GET /sites/env/<id>/health` 在线时 **303**（结果经 flash 呈现），运行时删除后同路径 400；有站点绑定时删除运行时被拒并列出域名（命令行带 `confirm=1` 亦拒），解绑后删除成功、容器移除。
- SITE（PASS）：新建静态站 `static-acc.test` 303，`curl -H 'Host: static-acc.test'` 200，既有 `matrix.local` 仍 200，`/sites/web` 200；测后测试站已删。
- **FAIL（1）FTP→firewall 对象授权在升级路径缺失**：先装 ftp、后装 firewall 时，`com.skiff.ftp` 的 `protection.enable` 返回 403「内核返回 HTTP 403: 缺少对象授权 plugin:com.skiff.firewall（安装时未勾选）」，审计落 `interop.call` deny；`skiff plugin install … --yes` 重装 ftp 不补授（`crates/core/src/commands.rs` 升级分支沿用既有 grants，不解析新增 `resource_hints`）；`plugin remove` + `install` 后才得到 13 条 grants，此时 `protection.enable` 200（写 `port = 21` 的 jail）、`protection.disable` 200 并移除 jail。复现：升级前装的插件在新 manifest 里新增对象提示时不会被补授。→ 已由 `f0c9872`/`c7a06b4` 修复，本轮以真实旧包路径复验通过（见下节）。
- 清理：测试站、两个测试运行时与容器、`/root/iter/acc-dir` 均已删除；`fail2ban` 包保留已装状态（FW-04 安装物），`skiff-pure-ftpd` jail 已移除，`jail.d` 仅剩 `defaults-debian.conf` + `skiff-defaults.conf`；`/root/iter/rel-483f898/`、`/root/skiff.before-483f898`、`/root/iter/config.before-483f898.toml` 保留。

**`c7a06b4` 升级授权补齐复验（2026-11-10 09:03–09:15，Release 资产覆盖安装）**

- 目标：验证「先装 ftp、后装 firewall → 升级 ftp 到新包 → `protection.enable`」路径下，升级分支按新 manifest 补齐对象授权，且只增不减、审计可查。
- 前置（真实旧包，不改库）：`systemctl stop skiff` → `skiff plugin remove com.skiff.ftp`（`delete_data=false`）→ 安装同机保留的 tag 基线包 `/root/iter/rel-491da40/skiff-0.1.0-x86_64-unknown-linux-musl/official/com.skiff.ftp-0.1.0.wbpkg`（manifest `resource_hints=[]`、无 `[interop]`）→ `plugin list` grants **11**（仅 `ftp:*` 动词，无 `interop:call`、无 `object:plugin:com.skiff.firewall`）；`com.skiff.firewall` 已在此前轮次装好。库备份 `/root/iter/skiff.db.before-ftp-recheck`。
- 升级：`skiff plugin install /root/iter/rel-c7a06b4/x/skiff-0.1.0-x86_64-unknown-linux-musl/official/com.skiff.ftp-0.1.0.wbpkg --yes` → grants **13**：新增 `perm:interop:call` 与 `object:plugin:com.skiff.firewall`，原 11 项完整保留（只增不减），无旧授权被清空。
- 审计：`plugin.install` `grants=13`（`signed=true`）；`plugin.grant_upgrade` `actor=cli result=ok`，detail「升级补授授权: interop:call,plugin:com.skiff.firewall；来源: manifest 新声明 + resource_hints/[interop]；版本 0.1.0 → 0.1.0；敏感: interop:call」。
- 功能：`systemctl start skiff`、`/healthz` 200；重登后 `protection` → `available:true, enabled:true, installed:true, jail:"pure-ftpd", jails:["pure-ftpd"], running:true`；`protection.enable` **200**。审计 `interop.call` 自升级时刻（09:06:57）起 **6 ok / 0 deny / 0 fail**（`fail2ban.jail.ensure`、`fail2ban.status`）；升级前那条 403 deny 作为历史保留。
- 口径：补齐只针对「相对已装 manifest 新增」的 hint；旧 manifest 已声明但当初未勾选的 hint 不翻案（`c7a06b4` 的显式设计）。后一分支 CLI 无拒绝入口、构造不出，由 `crates/plugin/src/install.rs` 单测覆盖。
- 回归（同资产）：AUTH 5 项 PASS（`/w3qz4vc2/` 307、`/w3qz4vc2` 303、`/healthz` 200、无 csrf POST 403、带 csrf 303、`skiff_dev_session` 属性与安全头、Bearer 200/401、七页 200）；FS PASS（list/write/read/mkdir/rename/remove 正常，`remove` 返回 `trash_id` 并落回收站，受保护路径 403，300 KiB 只读截断，`/files/trash|quotas` 200，`/www/skiff` 与 `plugins` 0711）；SITE PASS（`matrix.local` static running、`curl -H 'Host: matrix.local' http://127.0.0.1/` **200**、`/sites` 200、`skiff-web-nginx` Up）；TASK PASS（`POST /tasks/builtin:db-vacuum/run` 303，`task_runs result=ok` 含备份路径）；firewall PASS（`status` nftables + fail2ban running、`rule.add` 200 后 `nft list table inet skiff` 出现 `tcp dport 18099 accept`、`rule.remove` 200 后无残留、非法输入 400）。`skiff.service` active、`NRestarts=0`。
- 终态：7 个插件（grants：cron 6 / files 7 / firewall 12 / ftp 13 / guard 6 / monitor 3 / testkit 7，合计 54）；站点仍 `matrix.local`；回收站 1 条（本轮 FS remove 产生）；`/root/iter/rel-c7a06b4/`、`/root/skiff.before-c7a06b4`、`/root/iter/skiff.db.before-ftp-recheck` 保留。

**`2defdc7` 内核健壮性轮（2026-11-10 13:25 起，Release 资产覆盖升级，保留旧库验迁移）**

- 出包：CI run `42760437806`（`2defdc7`）success；Release run `42761174445` 由 workflow_dispatch（`ref=main`、`version=0.1.0`、`publish=true`、`release_tag=v0.1.0`、`skip_plugins=false`）触发，**双架构 `build` 与 `publish` 成功、`docker` job 失败**（`COPY .../tools/skiff-server` 找不到源，新增 workspace 成员未进 `deploy/Dockerfile`；tarball 资产不受影响，见台账）。tag `v0.1.0` 未新建、未移动。
- 升级路径（**保留旧库，不重置**）：停 `skiff` → 备份二进制 `/root/skiff.before-2defdc7`（`c7a06b4` 的 `e94c5c88…1d64e9a`）与库 `/root/iter/skiff.db.before-2defdc7` → `sh install.sh --no-docker`（退出 0）→ 启动。`PRAGMA user_version` 由 **15 升至 16**，`fs_quotas` 新增 `truncated` 列（`pragma table_info(fs_quotas)` 可见），迁移失败未发生、无回滚、服务一次启动成功（`NRestarts=0`）。旧数据全部保留：users 1 / plugins 7 / sites 1 / site_runtimes 1 / scheduled_tasks 6（新增 `builtin:audit-prune`）/ plugin_grants 54 / jobs 4 / trash_entries 3。
- 迁移断言：`/healthz` 200（18080 与 18081 均 200）、登录 303 换会话成功、`/w3qz4vc2` 与各资源页 200；配置 sha `3133d44b…` 前后一致，入口前缀 `w3qz4vc2` 未变。
- `builtin:audit-prune`（PASS）：预置 2 条 100 天前审计行，`POST /tasks/builtin:audit-prune/run` → 303，`task_runs.detail = deleted=2 cutoff=…`；立即重复执行 `deleted=0`（幂等），过龄行已删、近期行保留。
- 配额截断（后端 PASS / 页面 FAIL）：构造扫描条目超 `max_dir_entries=1_000_000` 场景，`GET /files/api/quotas` 返回行 `truncated=true`，该状态下写入 **200**（不因截断拒写）；`truncated=0` 且超限时写入 **507** `fs.quota_exceeded`。**页面 FAIL**：`fs_quotas.js` 的 `apiUrl` 拼接缺 `/`（`/{p}/files/apiquotas`），配额页所有请求 404，页面报「加载配额失败：请求失败：404」，无法呈现「不完整（仅下界）」。
- Compose 部分失败（PASS）：预置同名容器制造 create 冲突 → `POST /compose/up` 返回 **HTTP 500**，文案含「1 个服务未成功启动」，审计 `result=partial`。
- 调度重入拦截（PASS）：`SIGSTOP` 插件使其不回执 → run1 阻塞 10 s 结算 `result=timeout`；立即 run2 被拒，`error_class` 提示 `sched.reentry_blocked: 上一次运行 …:N 已超时但插件尚未回执`；`SIGCONT` 回执后 run3 可正常执行（结果为 `executor_failed`，非重入拦截）。验后任务复位 `enabled=0`。
- 回归（PASS）：AUTH（healthz / login 303 / 会话 / 安全头）；FS（list / write / read / remove / trash / 受保护路径 403；`/etc` 可浏览属设计）；SITE+nginx（`/sites`、`/sites/web`、`/sites/env` 200、容器 `skiff-web-nginx` 内 `nginx -t` 通过、`Host: matrix.local` → 200）；TASK（`builtin:trash-prune` ok）；firewall（`table inet skiff` 存在、插件页 200）；Node 运行时（自 `node:24-alpine` 建 `node-24` 实例、容器内实际 `node -v` = 24.22.2、配置保存触发重建生效、`action/delete`+`confirm=1` 删除干净）。
- 清理：删除 `/www/iter-quota`、`/www/iter-fs`、compose 测试容器与 `fs_quotas` 测试行；终态 users 1 / plugins 7 / sites 1 / site_runtimes 1（`php-8.5` stopped）/ scheduled_tasks 6 / plugin_grants 54 / jobs 4 / trash_entries 3 / fs_quotas 0；`/root/iter/rel-2defdc7/`、`/root/skiff.before-2defdc7`、`/root/iter/skiff.db.before-2defdc7` 保留。

**`38b0bb9` 两处 FAIL 修复复验（2026-11-10 14:09 起，Release 资产覆盖升级）**

- 出包：修复提交 `d0e967e`（`deploy/Dockerfile` 补 `tools/skiff-server`）、`38b0bb9`（配额页/回收站页 `apiUrl` 补路径分隔符，覆盖 8 个端点，并容忍无前导 `/`）。CI run `42764151249` success；Release run `42765656035` **四个 job 全绿**（x86_64 / aarch64 / docker / publish），`docker` job 自 `2defdc7` 轮的 COPY 失败恢复。tag `v0.1.0` 未新建未移动。
- 部署：同版本覆盖升级（库保持 **v16**，无迁移），二进制 13,692,688 B `5cf0eadc…81b6`、配置 sha `3133d44b…` 未变、前缀 `w3qz4vc2`、`/healthz` 200（18080/18081）、`NRestarts=0`。
- 配额页（PASS，浏览器实测）：`/{p}/files/api/quotas` 返回 **200** 且行内 `truncated:true`；页面配额表两列（已用容量 / 已用文件数）均渲染「**不完整（仅下界）**」，无「加载配额失败」横幅，控制台无报错。同状态下写入 `/www/iter-q-38b0bb9/b.txt` **200**（截断不拒写）。API 侧 `set` / `scan` / `list` / `cancel` 均 200。
- 回收站页（PASS，浏览器 + API 实测）：页面 GET `/{p}/files/api/trash` **200**，空态文案「共 0 项 · 保留 30 天，到期自动清理」正常，无 404 横幅；四动作 `restore`（内容还原一致）/`purge`/`clear`（`{"removed":3,"bytes":25}`，清后列表空）全 **200**。
- 回归（PASS）：AUTH（`/healthz` 200、`/{p}/` 307、`/{p}` 303、`login` 200、dashboard/files/trash/quotas/sites/tasks/plugins/settings 全 200）；FS（`list /www` 200、write 200、`read` 回显 `reg-ok`、受保护路径 remove **403** `fs.denied`）；SITE（`/sites`、`/sites/web` 200、`Host: matrix.local` → 200、`skiff-web-nginx` 内 `nginx -t` 通过）；TASK（`/tasks` 200、手动 `builtin:trash-prune` → 303，`task_runs.result=ok`、`error_detail=released_bytes=0`）。
- 清理：删 `/www/iter-q-38b0bb9`、`/www/iter-fs-38b0bb9` 与 `fs_quotas` 验证行；终态 users 1 / plugins 7 / sites 1 / site_runtimes 1（`php-8.5` stopped）/ scheduled_tasks 6 / plugin_grants 54 / jobs 4 / fs_quotas 0；`/root/iter/rel-38b0bb9/`、`/root/skiff.before-38b0bb9`、`/root/iter/skiff.db.before-38b0bb9` 保留。
- 备注：该轮 CI 首次红在 Windows 单测 `crates/fs/tests/ops.rs::atomic_replacement_has_no_torn_file`（路径锁等待超时，`Busy(...)`），仅 rerun 失败 job 后转绿，已记入台账。

## 9. 工具链验收（`tools/skiff-server` publish / sync，2026-11-10 `b094375`）

> 该提交只改 `tools/skiff-server`（新增 `publish` / `sync`），不动内核包，**本轮未重新部署面板**；面板仍为 §5 的 `c8bcb6e`。

- 执行位置：**测试机没有 Rust 工具链**（`cargo` / `rustc` / `skiff-server` 均不在 PATH，`rustc --version` 不存在），按约定未为其安装；**开发机没有 C 编译器**（`cl` / `clang` / `gcc` 均无，`ring` 构建脚本无法通过），未新编 Linux 产物。实际使用开发机既有 `target\debug\skiff-server.exe`（6,375,936 B，sha256 `b86b7fa77143fcfc40aa26bcb3453b94ae4135a5a851e6e271e3bd03dc043d4bb`，2026-11-10 20:07:25）；`cargo build -p skiff-server` 报 up-to-date，源码 = `b094375`（`2026-11-10T20:07:43+08:00`）。
- CI（同提交）：12 个 check-run = **11 success + 1 skipped**，其中 **`商店清单（skiff-server check）` success**；`单测（Linux）` / `单测（Windows）` success（两个平台都跑过 `tools/skiff-server/tests/p0.rs` 12 项与 `tests/sync.rs` 6 项）；skipped 项为 `编译（x86_64 musl 静态）`（本提交不触发内核打包路径）。
- publish（PASS，Windows 本地 CLI 实测）：
  - 缺私钥：不设 `SKIFF_SIGNING_KEY` → exit **1**「缺少环境变量 SKIFF_SIGNING_KEY…」，输出目录**不存在**，无任何清单；
  - 公私钥不匹配：seed A + `--public-key <pubB>` → exit **1**「公钥与私钥不一致」，输出目录不存在；
  - 正常发布：`manifest.seq=1`、`index` 声明的 sha256/size 与实测逐字节一致、3 个 blob 文件名 = 各自 sha256；`check --public-key <pubA>` 校验通过，`check --public-key <pubB>` 被拒；用 openssl（`pkeyutl -verify -rawin`，消息 = `SKIFF-SERVER-MANIFEST-v1\0` + manifest 原始字节）独立验签 **exit 0**；
  - seq 回退：catalog `seq=1` + `--prev <seq=2 产物>` → exit **1**「seq 未递增」，无输出。
- sync（PASS，Windows 本地 CLI 实测；源为本地目录，绕开 TLS）：
  - 空镜像：复用 0 / 新增 3；重复同步：复用 3 / 新增 0，清单未变化；
  - 预置 1 个 blob 的镜像：复用 1 / 新增 2（只补缺失，不重复写）；
  - seq 回退：`--from <seq=1>` 同步到 `seq=2` 镜像 → exit **1**「步骤 **3/6** seq 回退被拒绝：主站 1，镜像 2」，镜像清单保持 seq=2；
  - 源 blob 同长度改 1 字节 → exit **1**「步骤 **4/6** blob 摘要不匹配」，镜像无 `manifest.json`（只落了校验通过的 1 个 blob）；
  - 同步后人为篡改镜像内 blob → `check --dist <mirror> --public-key` 报 blob 摘要不匹配；再跑一次 sync 只补该 blob 并恢复自检通过；
  - 错误公钥 → 步骤 **1/6** 拒绝，镜像无输出。
- **FAIL（新发现，已于 `2bb5c90` 修复，见下）**：`--require-signature` / `--unsigned` 旗标解析失效。`parse()` 在 `tools/skiff-server/src/main.rs:213-216` 用 `strip_prefix("--")` 后的名字比较 `flags`，调用方传的却是带前缀的 `&["--unsigned"]`（L71）/ `&["--require-signature"]`（L121），永不匹配 → 旗标被当成「带值选项」：`check … --require-signature` 末尾 exit 1「参数 --require-signature 缺少取值」；`check … --require-signature --public-key <hex>` 把 `--public-key` 当值吞掉、`<hex>` 报「无法识别的参数」；`gen --unsigned --out X` 吞掉 `--out`。`publish` 内部仍强制验签（不受影响），但发布/同步门禁无法用严格模式。该缺陷已在 `2bb5c90` 修复并复验，已从 [TEST_LEDGER.md](TEST_LEDGER.md) 出账。
- **`2bb5c90` 旗标修复复验（PASS，2026-11-10）**：用重新构建的 `target\debug\skiff-server.exe`（6,375,936 B，sha256 `03d5fea20db02760540416a0ebcac5c557ec5786ef43077fa4ed17cdbb69231b`，2026-11-10 20:19:52，源码 = `2bb5c90`），执行位置与 §9 上文一致（测试机无工具链、开发机无 C 编译器，未改环境）：
  1. `gen --unsigned --out U1`（无密钥）→ exit 0，14 个文件；`manifest.key_id` = 64 个 `0`、`manifest.json.sig` 的 `sig` 为空串；`--out` 正常识别，不再被吞。
  2. 设置合法 `SKIFF_SIGNING_KEY` 后再跑 `gen --unsigned` → exit 0，产物仍 `key_id` 全 0 / `sig` 空（`--unsigned` 压过环境密钥）。
  3. `gen` 不带 `--unsigned` 且无密钥 → exit 1「缺少环境变量 SKIFF_SIGNING_KEY…」，输出目录不存在。
  4. 未签名产物 U1：`check --dist U1` → exit 0（仅警告「未签名占位」）；`--require-signature` → exit 1「渠道 stable 未签名：P0 占位产物不得发布」；`--require-signature --public-key <pubA>` 同样 exit 1「未签名」，`<hex>` 被正常识别（无「无法识别/缺少取值」）；`--public-key <pubA>` 单独使用 → exit 1「签名 key_id 与固定公钥不一致」。反向证明 `--public-key` 真正生效：已签名 dist 加 `--require-signature --public-key <pubA>` → exit 0，换 `<pubB>` → exit 1 key_id 不一致。
  5. 回归（与上一轮逐步对齐）：publish 缺私钥 / 公私钥不匹配 / 正常发布（seq=1、index 摘要一致、blob 全匹配）/ `--prev` seq 未递增均一致；sync 首次（复用 0 / 新增 3）、重复（复用 3 / 新增 0）、预置 1 个 blob（复用 1 / 新增 2）、seq 回退（步骤 3/6）、源 blob 篡改（步骤 4/6，镜像无 `manifest.json`）均一致；`check mirror1 --require-signature --public-key` 通过。
  - 该提交同时新增 `tools/skiff-server/tests/cli.rs` 4 条旗标用例（真实子进程断言）；同提交 CI **11 success + 1 skipped**（跳过项为 `编译（x86_64 musl 静态）`，不触发内核打包路径），`商店清单（skiff-server check）` 与 `单测（Linux/Windows）` 均 success。
- **无法验证**：测试机上的 CLI 实测（无 `rustc` / `cargo` / `skiff-server`，且约定不为测试机装工具链）；Linux 目标二进制行为（未交叉编译；本轮以 Windows 产物验证同一套库与 CLI 逻辑）。

## 10. 数据、备份与清理

| 项 | 路径 / 约定 |
| --- | --- |
| 数据根 | `/www/skiff`（0711）、`/www/sites`、`/www/php`、`/www/runtime` |
| 配置 | `/etc/skiff/config.toml`（0644） |
| 数据库 | `/www/skiff/skiff.db`（0600） |
| 运维备份 | `/root/iter/backups/`、`/root/backups/`；保留 `builtin:db-vacuum` 与运维备份 |
| 清理 | 测试完成后清理临时站点、容器、账号与压测文件，机器恢复可用状态 |
| GitHub 出网 | 内网代理与 git 全局配置见 [ITERATION.md](ITERATION.md) §7.1（不在本文重复） |
