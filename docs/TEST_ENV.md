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
| tag / 构建提交 | `v0.1.0` = `9daabd3`（Release 由该提交构建；含 `1862702` 及其后三个修复提交）；Release run `42480735047` success，CI run `42480722493` success |
| 部署时间 | 2026-11-08 20:47（Asia/Shanghai；在 19:59 全新库上按 Release 资产覆盖安装，库保持 v15） |
| 部署方式 | `v0.1.0`（`9daabd3`）Release 资产 → `sha256sum -c checksums.txt`（双架构均 OK）→ 解压 → `sh install.sh --no-docker`；未传本地二进制 |
| 数据库 | 沿用 19:59 空库直建结果 **`PRAGMA user_version=15`**；`jobs` / `job_logs` / `trash_entries` / `fs_quotas` 均在（`jobs` 含 `cancel_requested` / `log_seq` / `retry_of`）；users 1 / plugins 6 / sites 0 / site_runtimes 1 |
| 插件 | 6 个官方插件 CLI 安装成功（grants 数：cron 6 / files 7 / ftp 11 / guard 6 / monitor 3 / testkit 5）；`files` 授权 `object:path:/www`，`monitor`/`testkit` 授权 `object:path:/proc`；复验后 files 已恢复默认授权 |
| 服务 | `skiff.service` active、`/healthz` 200；`skiff-http-fwd.service` active（形态三对外转发，脚本 `/root/iter/skiff-http-fwd.py`）；`skiff-tls-proxy.service` 未部署 |
| 配置 | `/etc/skiff/config.toml`：`listen=127.0.0.1:18080` + `allow_insecure_http=true` + `tls_enabled=false`（形态三明文 dev HTTP；sha256 `ebcccae69288b36063d2545d89cb86f91df51f277ef2ca9c3b0a079a9cf8816c`）；`allow_reduced_sandbox=true`（landlock/cgroups 未实现，测试机显式降级）。备份 `/root/iter/config-dev-20261108-fresh.toml` |

## 6. Release 资产（v0.1.0 = `9daabd3`，2026-11-08）

构建来源：Release workflow run `42480735047`（`push` tag `v0.1.0`，`head_sha=9daabd3`；同一时刻只有该 Release 在跑，已 success）。
`git merge-base --is-ancestor` 校验：`92aa580` / `aa292b7` / `8716e51` / `48f57dc` / `5498f28`（任务中心）/ `d0e656d`（文件管理阶段二）/ `1862702`（站点与运行环境）/ `fd6bdf8`（PHP 配置校验）/ `128d698`（插件 403 文案）对 `v0.1.0` 退出码均为 0；构建时 tag、`origin/main` 与该提交一致。资产摘要经 GitHub API 复核，并下载后本地重算。

| 资产 | 大小 | sha256 |
| --- | --- | --- |
| `checksums.txt` | 223 B | `50d07d113aafd5f8c45b17a5b68a8dd553e02825e1ec1541120810b49c0d86e3` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 8,786,193 B | `ff71be79b73bf5eab7efeb1c60c21ac95aa0f44864e63b30a1fd19e7b821de4c` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 8,380,846 B | `a2945c66518b5464b32e94bf4f8a83af6b82ce273c150d0e0d3d7072471d9dbe` |

- 下载端点：`https://github.com/php-nil/skiff/releases/download/v0.1.0/<资产名>`；Windows 直连常被重置时改用 `https://api.github.com/repos/php-nil/skiff/releases/assets/<id>` + `Accept: application/octet-stream`，本轮经内网代理 `http://10.66.10.22:1010` 下载（缓存 `d:\panel\.workbuddy\release-9daabd3\`），`scp` 到测试机后 `sha256sum -c` 双架构 OK。
- 包内 `official/` 含 6 个 `.wbpkg`（guard / monitor / files / cron / ftp / testkit）与 `public-key.txt`；签名指纹 `ed73de3e…cd47`，首启自动信任。
- 测试期只保留 `v0.1.0` 一个 tag，后续发版一律 force 移动同一 tag、覆盖同一 Release，**不新建版本号**（违规的 `v0.1.1` 已于本轮前清除，`git ls-remote --tags origin` 现只有 `v0.1.0`）。

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

## 8. 最近一次全链路验收结论（2026-11-08 `1862702` 全新安装 + `9daabd3` 复核轮）

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
- **EDQUOT（507「文件系统配额已用尽」）**：8 MB loop ext4（`mkfs.ext4 -O quota` 内建配额）+ `setquota` KB 级硬限。**关键构造点**：skiff 以 uid 0 运行且 `CapEff=000001ffffffffff` 含 `CAP_SYS_RESOURCE`，配额对 root 直接豁免（前三次构造写 20 KB 都返回 200，`repquota` 显示已超限也没用）；用 `setpriv --reuid=65534` 写同一挂载点会失败，证明配额本身生效。临时 drop-in `[Service] CapabilityBoundingSet=~CAP_SYS_RESOURCE` + `daemon-reload` + restart（`CapEff` 降为 `000001fffeffffff`）后：内核管理员面写 20 KB → **507**，`{"error":{"code":"fs.no_space","message":"文件系统配额已用尽，无法写入: /www/iter-quota/.big.bin.tmp-75024-0"},"ok":false}`；审计 `code=fs.no_space result=fail`。插件能力面同一次写 → **HTTP 400**，正文「…协议错误: 内核返回 HTTP 507: 文件系统配额已用尽，无法写入: /www/iter-quota/big2.bin」——代理层 `rpc_error` 当时只透传 403/404/409。已补 `AppError::NoSpace`（507）+ `rpc_error` 透传 507 + 单测 `rpc_error_passes_kernel_status_through` 覆盖；**该改动未随测试机二进制部署，插件面 507 端到端待下一次发版复验**。验后移除 drop-in、`daemon-reload` + restart（`CapEff` 回 `000001ffffffffff`）、`quotaoff`+`umount` 并删除镜像与目录。

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

**收尾状态（2026-11-08 `9daabd3` 复核轮后）**
- 面板：形态三明文 `http://10.10.15.4:18081/udkwxz96` 可用；`skiff.service` active；`skiff-http-fwd.service` active；`skiff-tls-proxy.service` 未部署。
- 数据：库保持 `user_version=15`，6 个官方插件已装；站点已清空，`iter-php-1862702` / `iter-rust-rust-1862702` 运行环境与容器保留（php.ini 复核需要）。
- 备份：`/root/iter/config-dev-20261108-fresh.toml`、`/root/iter/admin-pw.txt`、`/root/iter/cookies.txt`（会话）、`/root/skiff-pkg-9daabd3/`（解压目录）保留；安装包原件在 `/root/*.tar.gz`。


## 9. 数据、备份与清理

| 项 | 路径 / 约定 |
| --- | --- |
| 数据根 | `/www/skiff`（0711）、`/www/sites`、`/www/php`、`/www/runtime` |
| 配置 | `/etc/skiff/config.toml`（0644） |
| 数据库 | `/www/skiff/skiff.db`（0600） |
| 运维备份 | `/root/iter/backups/`、`/root/backups/`；保留 `builtin:db-vacuum` 与运维备份 |
| 清理 | 测试完成后清理临时站点、容器、账号与压测文件，机器恢复可用状态 |
| GitHub 出网 | 内网代理与 git 全局配置见 [ITERATION.md](ITERATION.md) §7.1（不在本文重复） |
