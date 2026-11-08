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
| URL | `http://10.10.15.4:18081/udkwxz96/login`（**当前为形态三：直接明文 HTTP，仅限内网**） |
| 入口前缀 | 随机 8 位，**每次重装可能变化**；最近记录 `udkwxz96`（2026-11-08 19:59 全新库生成） |
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
| tag / 构建提交 | `v0.1.0` = `1862702`（Release 由该提交构建）；Release run `42471149737` success，CI run `42471115305` success |
| 部署时间 | 2026-11-08 19:59（Asia/Shanghai；**机器已还原，全新库安装，不是升级**） |
| 部署方式 | `v0.1.0`（`1862702`）Release 资产 → `sha256sum -c checksums.txt`（双架构均 OK）→ 解压 → `sh install.sh --no-docker`；未传本地二进制；安装后仅追加 `allow_reduced_sandbox=true` 再重启 |
| 数据库 | 空库直建 **`PRAGMA user_version=15`**；`jobs` / `job_logs` / `trash_entries` / `fs_quotas` 均建出（`jobs` 含 `cancel_requested` / `log_seq` / `retry_of`）；users 1 / plugins 6 / sites 0 / site_runtimes 1 |
| 插件 | 6 个官方插件 CLI 安装成功（grants 数：cron 6 / files 7 / ftp 11 / guard 6 / monitor 3 / testkit 5）；`files` 授权 `object:path:/www`，`monitor`/`testkit` 授权 `object:path:/proc` |
| 服务 | `skiff.service` active、`/healthz` 200；`skiff-http-fwd.service` active（形态三对外转发，脚本 `/root/iter/skiff-http-fwd.py`）；`skiff-tls-proxy.service` 未部署 |
| 配置 | `/etc/skiff/config.toml`：`listen=127.0.0.1:18080` + `allow_insecure_http=true` + `tls_enabled=false`（形态三明文 dev HTTP；sha256 `ebcccae69288b36063d2545d89cb86f91df51f277ef2ca9c3b0a079a9cf8816c`）；`allow_reduced_sandbox=true`（landlock/cgroups 未实现，测试机显式降级）。备份 `/root/iter/config-dev-20261108-fresh.toml` |

## 6. Release 资产（v0.1.0 = `1862702`，2026-11-08）

构建来源：Release workflow run `42471149737`（`push` tag `v0.1.0`，`head_sha=1862702`；同一时刻只有该 Release 在跑，已 success）。
`git merge-base --is-ancestor` 校验：`92aa580` / `aa292b7` / `8716e51` / `48f57dc` / `5498f28`（任务中心）/ `d0e656d`（文件管理阶段二）/ `1862702`（站点与运行环境）对 `v0.1.0` 退出码均为 0；构建时 tag、`origin/main` 与 HEAD 同为 `1862702`。资产摘要经 GitHub API 复核，并下载后本地重算。

| 资产 | 大小 | sha256 |
| --- | --- | --- |
| `checksums.txt` | 223 B | `ae5fe49646cc30eecc177c323a0c0c6b88333918f0acef1d3f6fdda6fa3ecd60` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 8,785,237 B | `640603cadf74f40b9101b4a2187cf4b27dd773b59225bd0b1785fe9a8d120918` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 8,378,566 B | `4c1232c9942d9292ed73d0814e05e45f34ac78ddcb72475cff4309dbbedb1b07` |

- 下载端点：`https://github.com/php-nil/skiff/releases/download/v0.1.0/<资产名>`；Windows 直连常被重置时改用 `https://api.github.com/repos/php-nil/skiff/releases/assets/<id>` + `Accept: application/octet-stream` 经内网代理下载；本轮缓存于 `d:\panel\.workbuddy\release-1862702\`，`scp` 到测试机后 `sha256sum -c` 双架构 OK。
- 包内 `official/` 含 6 个 `.wbpkg`（guard / monitor / files / cron / ftp / testkit）与 `public-key.txt`；签名指纹 `ed73de3e…cd47`，首启自动信任。
- 测试期只保留 `v0.1.0` 一个 tag，后续发版一律 force 移动同一 tag、覆盖同一 Release，**不新建版本号**（违规的 `v0.1.1` 已于本轮前清除）。

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

## 8. 最近一次全链路验收结论（2026-11-08 `1862702` 全新安装轮）

**安装链路（`1862702` 全新安装轮）**
- 机器已还原：无 `/etc/skiff`、无 `admin` 用户、无旧数据根；按 Release 资产安装，不传本地二进制。
- 资产校验：`checksums.txt` 两个架构 `sha256sum -c` 均 OK；解压后 `sh install.sh --no-docker` 一次成功。
- 空库直建：`user_version=15`；`jobs`（含 `cancel_requested`/`log_seq`/`retry_of`）、`job_logs`、`trash_entries`、`fs_quotas` 全部建出；users 1 / plugins 0（安装前）/ site_runtimes 1。
- 初始管理员口令生成于 `/www/skiff/initial-admin.txt`（已转存 `/root/iter/admin-pw.txt`，0600）；安全入口随机生成 `udkwxz96`。
- 插件：为降级沙箱在 `config.toml` 追加 `allow_reduced_sandbox=true` 后重启，6 个官方插件全部 `已验签` 安装成功，grants 与 manifest 一致（`plugin list` 已核对）。
- 访问：形态三 `http://10.10.15.4:18081`（`skiff-http-fwd.service`），`/healthz` 200、登录页 200、`POST` 登录 303 并下发 `skiff_dev_session`。

**分组验收**
- 任务中心 / 文件管理 / Web 五条 / 删站确认 / FS-14 五组结论随本轮验收逐组回填。

**沿用通过（`8716e51` 小批收口轮，本轮未复跑全部断言）**
- 装机/升级：Release 资产双架构 `sha256sum -c` OK；`sh install.sh --no-docker` 完成；库保持 **v14**，站点 2 / settings 7 / users 1 / plugins 6 全部保留；配置 sha256 保持 `0103e75a…f053e1`，`/healthz` 200。
- 配置保护组：删 / 改名 / 覆盖写 `/etc/skiff/config.toml` 全部 **403**「内核运行配置或数据目录受保护…（命中 /etc/skiff/config.toml）」；对 `/etc/skiff` 设配额 **403**（命中 `/etc/skiff`）；`/etc/hosts` 读 200；保护区外 `/www/iter-protect-probe.txt` 写/改名/读/删均 200（`/etc` 内其它路径不撞保护判定，写入被 systemd 只读挡回 500）；重启后服务 active、`/healthz` 与登录 200、配置 sha 前后一致。
- 插件日志组：journal 可见 monitor 启动横幅（`WARN … stream="stderr"`）；`kill -9` 子进程后 RPC **55 ms** 返回 200 并拉起新 pid，journal 先「上一次运行的 stderr 尾部」后新横幅，新日志不覆盖旧尾巴；初始化失败探针（临时替换二进制为 `exit 9`，已恢复、md5 `6c5ea70b…`）RPC **133 ms** 返回 400，正文、journal `ERROR` 与审计 `plugin.spawn` fail 的 `detail` 均带 `stderr 尾部: BOOT-8716e51 init failed probe`；恢复后 `status=running`、`status_detail` 清空。
- 上一批回归：回收站 删除→列表→恢复（内容仍为 `hello-a`）→彻底删除，回收站清空归零；配额 100B → 300B 写入 **507**「目录容量配额超限：… 307 字节 > 上限 100 字节」→ 重扫/取消后写回 200；Docker `GET /containers`、`GET /containers/new` 200，创建 `iter-lim-8716e51` 后 `docker inspect` 为 `mem=128MB / cpu=512 / pids=64`，限额更新后即时变 `256MB / 1024 / 128`，删除后容器消失；`GET /compose` 200，合法 YAML「校验通过」、非法 YAML 报错。

**沿用通过（`48f57dc` 收口轮，本轮未复跑全部断言）**
- FS-10～12 全链路（含同名冲突 409、父目录缺失 403、审计 `fs.recycle.*`/`fs.quota.*`）与 DOCKER-12～14 全链路（坏镜像回显、cgroup `pids.max`、Compose `up/down` 保护标签与卷策略）。
- monitor/guard 内核调度器（`driver=kernel`、停用任务不采样、卸载清任务、重装重 seed）与 WEB-02 `worker_connections` 无告警。
- TLS 三形态、430/860px 视觉、工作台标签与吸顶、插件 iframe 等前轮结论继续有效。

**未闭环（详见 [TEST_LEDGER.md](TEST_LEDGER.md)）**
- WAF 拦截能力不存在（仅字段/目录预留），需产品定稿后实现。
- aarch64 仅核对 sha256，无 arm64 实机装机。
- FTP fail2ban 三次封禁、1Panel 受限形态、BusyGuard/buffer 上机入口、并发 spawn 复验等无环境项。
- 主机监控历史页「出站」仍用累计字节（`net_tx_rate` 未产生），待补采样字段后对齐。
- Release 下载端点直连 `github.com`/`objects.githubusercontent.com` 偶发重置，需走 API 资产端点或代理。
- 原两条观察项已在 `8716e51` 关闭：插件 stderr 已转出 journal；`/etc/skiff` 与配置/TLS 私钥已纳入受保护路径。

**收尾状态（2026-11-08 `1862702` 全新安装轮后）**
- 面板：形态三明文 `http://10.10.15.4:18081/udkwxz96` 可用；`skiff.service` active；`skiff-http-fwd.service` active；`skiff-tls-proxy.service` 未部署。
- 数据：全新库（`user_version=15`），6 个官方插件已装；验收产生的站点/文件/任务随分组验收清理，回收站归零。
- 备份：`/root/iter/config-dev-20261108-fresh.toml`、`/root/iter/admin-pw.txt`、`/root/iter/cookies.txt`（会话）、`/root/skiff-pkg/`（解压目录）保留；安装包原件在 `/root/*.tar.gz`。


## 9. 数据、备份与清理

| 项 | 路径 / 约定 |
| --- | --- |
| 数据根 | `/www/skiff`（0711）、`/www/sites`、`/www/php`、`/www/runtime` |
| 配置 | `/etc/skiff/config.toml`（0644） |
| 数据库 | `/www/skiff/skiff.db`（0600） |
| 运维备份 | `/root/iter/backups/`、`/root/backups/`；保留 `builtin:db-vacuum` 与运维备份 |
| 清理 | 测试完成后清理临时站点、容器、账号与压测文件，机器恢复可用状态 |
| GitHub 出网 | 内网代理与 git 全局配置见 [ITERATION.md](ITERATION.md) §7.1（不在本文重复） |
