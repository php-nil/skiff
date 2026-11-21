# J 站点压测 / K FTP / L 后台端口与 TLS

> 上级索引：[TEST_MATRIX.md](../TEST_MATRIX.md) ｜ 拆自 `docs/test/TEST_MATRIX.md` 原 J–L（2026-11-16 整理，正文未改）。

---

## J. 站点压测与资源开销

> 重跑触发：改了 nginx/PHP-FPM/反代站点链路、站点日志轮转或日志页读取、`skiff` 主循环（1 s tick / 30 s 巡检）、调度器 tick、插件唤醒/常驻策略、Docker 容器列表与 inspect 调用、监控采样落盘，或更换测试机规格。
> 执行前提：测试机已有 `audit-env.local`（PHP 运行环境站点）与 `audit-rust.local`（反代站点）且均 `running`；不在测试机上编译任何东西；压测只从本机回环发起，不引外部流量。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| PERF-01 | 压测可重复执行且自带清理，不污染站点目录 | `[上机]` 两站 running；`python3` 可用；站点 `index/` 可写 | `OUT=/root/perf-$(date +%Y%m%d) bash tools/perf/run.sh quick`；结束后检查 `index/` 与 `log/` | 临时文件 `perf-static-64k.bin`、`perf.php` 与本轮新增 `access.log.1` 被删除；站点日志截回压测前尺寸；`OUT/` 下产出 `samples.csv`、`procs.csv`、`threads.csv`、`conts.csv`、各阶段 `*.json`、`state-before/after.json` | □通过 / □失败 | `tools/perf/run.sh`、`tools/perf/load.py`、`tools/perf/observe.py` |
| PERF-02 | 静态路径基准：并发档位吞吐/延迟/错误率可测 | `[上机]` PERF-01 通过 | `bash tools/perf/run.sh full`；读 `OUT/summary` 中 `static-c4/c16/c32` | 三档全部 200、`errors=0`；rps 随并发上升；`reconnects` 只来自 nginx `keepalive_requests` 到期（≈每 1000 请求 1 次），不计错误 | □通过 / □失败 | `tools/perf/load.py`；nginx `keepalive_requests` |
| PERF-03 | PHP 动态路径：FPM 池与每请求日志成本可归因 | `[上机]` PERF-02 通过；`audit-env.local` 为 PHP 站点 | 跑 `php-c4/c16/c32`；采样 `skiff-php-batch1` cgroup `cpu.stat`、容器内 FPM 进程、`dockerd` 与容器 shim；对比压测前后 `docker logs skiff-php-batch1` 行数 | 全部 200；PHP 路径吞吐不低于同档静态的 95%；容器 `docker logs` 不随请求增长（池模板 `access.log = /dev/null`）；容器 CPU 与 `dockerd`/shim CPU 之和能算到「每请求微秒」 | □通过 / □失败 | `crates/app/src/sites.rs::render_php_pool_with`（显式 `access.log = /dev/null`）、镜像 `/usr/local/etc/php-fpm.d/docker.conf`（`access.log = /proc/self/fd/2`） |
| PERF-04 | 反代路径：与静态/PHP 路径可横向对比 | `[上机]` PERF-02 通过；`audit-rust.local` 反代上游 200 | 跑 `rust-c4/c16/c32`；采 nginx 容器与上游容器 CPU | 全部 200；反代路径额外开销只落在 nginx 与上游容器；`skiff` 进程 CPU 不随 rps 增长 | □通过 / □失败 | `crates/app/src/sites.rs`（反代 vhost）、`crates/docker/src/client.rs` |
| PERF-05 | 宿主机与容器资源归因：1 Hz 采样可复算 | `[上机]` PERF-01 通过 | 检查 `samples.csv` 各阶段 CPU/内存/磁盘写/网络、`conts.csv` 各容器 cgroup CPU/内存、`procs.csv` 目标进程 | 三阶段（idle/warmup/负载）分档齐全；宿主 4 核合计 CPU 与容器 CPU 之和同量级；内存无持续增长；磁盘写增长能对应到日志轮转 | □通过 / □失败 | `tools/perf/observe.py`；`/proc/stat`、`/proc/meminfo`、`/proc/diskstats`、cgroup v2 |
| PERF-06 | 内核与插件常驻成本：tick 与插件进程可忽略 | `[上机]` PERF-01 通过；guard/monitor 已装并启用 | 对比 idle 与最高并发档的 `skiff` 主进程 CPU、线程数/自愿切换、插件子进程 RSS/CPU、`NRestarts` | `skiff` 主进程 CPU 不随请求增长（≤0.2% 单核）；线程数稳定；tokio 线程每秒自愿切换为个位数（含 1 s tick）；guard+monitor 常驻 RSS < 5 MB、CPU ≤0.1%；`NRestarts=0` | □通过 / □失败 | `crates/core/src/server.rs`（1 s tick、30 s 巡检）、`crates/sched/src/scheduler.rs::tick`、`plugins/guard/manifest.toml`、`plugins/monitor/manifest.toml` |
| PERF-07 | 无意义消耗定量：日志轮转/日志页/采样写放大 | `[上机]` PERF-02/PERF-03 通过 | 在最高并发档读 `skiff` 进程写字节与磁盘写；比对站点 `access.log` 尺寸；打开站点「日志」页一次；观察 `plugins/monitor/data/samples.jsonl` 的 mtime 与尺寸 | 能给出「每请求额外写多少字节」「一次日志页读入多少字节」「监控每 5 分钟重写多少字节」三项实测值，并判定是否与请求量线性放大 | □通过 / □失败 | `crates/app/src/sites.rs::rotate_if_large`（`rename` + 巡检后 `nginx -s reopen`）、`crates/app/src/sites.rs::read_log`（尾部 2 MiB 窗口）、`plugins/monitor/src/main.rs`（`samples.jsonl` 追加 + 超 480 行压缩） |
| PERF-08 | 压测后机器恢复干净、数据一致 | `[上机]` 已完成任一档压测 | 核对容器与站点状态、插件启停与基线一致、`PRAGMA integrity_check`、`user_version`、`systemctl show skiff -p NRestarts`、`load1` 与 `MemAvailable` 回落 | 临时站点/容器/文件全部消失；三容器 Up；插件启停与压测前一致；`integrity_check=ok`；`user_version` 与压测前相同；`NRestarts=0`；两站仍 200 | □通过 / □失败 | `tools/perf/report.py`；`/www/skiff/skiff.db`；`/www/sites` |

最小回归命令：

```bash
# 测试机上执行（不在测试机编译）；$REPO 为仓库检出目录
export REPO=/path/to/skiff OUT=/root/perf-$(date +%Y%m%d)
OUT=$OUT bash "$REPO/tools/perf/run.sh" quick
sqlite3 /www/skiff/skiff.db 'PRAGMA integrity_check; PRAGMA user_version;'
systemctl show skiff -p NRestarts
docker ps --format '{{.Names}}\t{{.Status}}'
curl -s -o /dev/null -w 'php=%{http_code}\n' -H 'Host: audit-env.local' http://127.0.0.1/
curl -s -o /dev/null -w 'rust=%{http_code}\n' -H 'Host: audit-rust.local' http://127.0.0.1/
```

**压测补充实测（2026-11-07 首轮，测试机 10.10.15.4，4 vCPU / 3.9 GB；规格当前值见 [TEST_ENV/01-ACCESS.md](../TEST_ENV/01-ACCESS.md) §1）**

并发档位固定 4/16/32，每档 60 s，每连接限速 100 rps；`quick` 档为 4 并发 30 s。宿主 CPU 为 4 核合计占比，容器 CPU 为单核百分比。

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| PERF-02 | ☑通过 | 静态 358 / 1480 / 2947 rps；p99 1.51 / 3.30 / 3.79 ms；`errors=0`，`reconnects` 20/80/160 |
| PERF-03 | ☑通过（发现日志成本） | PHP 357 / 1466 / 2913 rps；p99 2.50 / 4.53 / 6.02 ms；PHP 容器 CPU 9.41 / 35.61 / 36.88%，同档 `dockerd`+容器 shim 在 c32 合计 13.78% 单核（≈47 µs/请求），静态/Rust 路径仅 0.25–0.28% |
| PERF-04 | ☑通过 | Rust 353 / 1462 / 2921 rps；p99 2.24 / 3.65 / 5.44 ms；nginx 容器 10.38 / 27.94 / 54.45%；上游容器 2.12 / 6.71 / 11.49% |
| PERF-05 | ☑通过 | 宿主 CPU：静态 3.56 / 8.82 / 14.02%，PHP 7.04 / 25.75 / 26.45%，Rust 4.88 / 11.70 / 20.49%；nginx 容器 5.29 / 14.96 / 23.23%（静态）；磁盘写 45 / 291 / 779 KB/s；全程内存无单调增长 |
| PERF-06 | ☑通过 | `skiff` 主进程 idle 0.05% 单核，负载下 0.03–0.10%，与 rps 无关；tokio 线程自愿切换约 2.4 次/s；guard+monitor 常驻 RSS ≈1.4 MB、CPU ≤0.02%；`NRestarts=0` |
| PERF-07 | ☑通过（三项 A/B 定量，2026-11-08） | 改前：①PHP 每请求 stdout 日志经 containerd-shim→dockerd，c32 合计 13.78% 单核；②30 s copytruncate 轮转使 `skiff` 写 1/215/464 KB/s（c4/c16/c32）；③`samples.json` 36,549 B 每 300 s 整文件重写；④`read_log` 整文件读入，3 GB 文件 4.53 s/写 2.97 GB/返回空页。改后复测：②`skiff` 写 3/3/3 KB/s、磁盘写 52/124/195 KB/s、p99 2.08/4.11/4.99 ms（CPU 与 p99 在噪声内）；④9.5 MB 峰值 RSS 22,984→12,160 KB，3 GB 4.5 ms/写 16 KB/返回最新 199 行；③一次 `execute` 写 77,824→1,059 B、inode 不变、旧 `samples.json` 不再被写 |
| PERF-08 | ☑通过 | 临时文件与新增 `access.log.1` 已删；三容器 Up、两站 200；插件/授权/任务计数与压测前一致；`integrity=ok`、`user_version` 与压测前一致（当轮 11，现 v15）；`NRestarts=0`；`MemAvailable` 约 3301 MB、`load1` 回落 |
| PERF-01 备注 | — | 首轮 `collect` 采样脚本变量被覆盖导致无数据，已修复；`load.py` 曾把 nginx `keepalive_requests` 到期断连计为 error，已改计 `reconnects` |

## K. FTP 与宿主机文件服务

> 重跑触发：改了 `ftp` 能力面（`crates/plugin/src/ftp.rs`，内核侧只剩 `crates/core/src/kernel/ftp.rs` 的 grants/审计/特权通道）、`com.skiff.ftp` 插件、pure-ftpd 配置键、站点统一身份（`skiff-web` 201000:201000）/`MinUID`/`Umask`、站点目录对齐或 fail2ban 策略。
> 执行前提：内核二进制与 `official/com.skiff.ftp-*.wbpkg` 均来自 GitHub Release 资产（下载后先 `sha256sum -c` 再安装）；本地/交叉编译产物只作编译级验证，**不作为部署来源，也不作为验收依据**。契约见 [KERNEL_FTP_CONTRACT.md](../../contract/KERNEL_FTP_CONTRACT.md)。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| FTP-01 | 首次安装、幂等重入与卸载回收 | `[上机]` Release 安装后未装 pure-ftpd；能 `apt` | 插件「运行状态」点安装；再次点安装；检查 `pure-ftpd -v`、`systemctl status pure-ftpd`、`/etc/pure-ftpd/conf/*`、`iptables`/`nft` 无新规则；再点回收并确认 | 首次安装成功且自动启动；二次调用返回同样配置不重复装包；配置含 `ChrootEveryone`/`NoAnonymous`/`PureDB`/`PassivePortRange`/`Umask 113 002`/`MaxClientsPerIP`，且 `/etc/pure-ftpd/auth` 只保留 `50pure -> ../conf/PureDB`（`65unix`/`70pam` 已清理）；回收后服务停止、包可移除或至少禁用，站点账号全部失效 | △部分通过（2026-11-07 测试机：安装、二次重入、配置项与认证链通过；卸载回收未执行） | `crates/plugin/src/ftp.rs::install/uninstall/status`、`crates/core/src/kernel/ftp.rs::ftp_call`、`plugins/ftp/src/main.rs`、`plugins/ftp/ui/runtime.html` |
| FTP-02 | 账号名/密码/端口范围校验（纯函数） | `[本机]` | `cargo test -p skiff-plugin ftp --locked` | 非法账号名、过短密码、控制字符、非法端口区间全部被拒；登录名 `skiff_<slug16>_<fnv4>_<account>` 稳定且不同域名不碰撞 | 通过（2026-11-07 本机 7 项单测；2026-11-11 迁至 `skiff-plugin`，14 项） | `crates/plugin/src/ftp.rs`（`valid_account_name`/`valid_password`/`valid_port_range`/`ftp_login`） |
| FTP-03 | 统一身份与 `MinUID` 只降不升 | `[上机]` 已安装并建站点；宿主存在 `skiff-web` | 对 PHP 站与静态站各建一个账号；`getent passwd skiff-web`；读 `/etc/pure-ftpd/conf/MinUID`；建号后 `ls -ln /www/sites/<域名>/index` | 两类站点账号 uid/gid 均为 201000:201000；`MinUID` 不会被上调（默认仍为 1000，历史下调值保留）；`index/` 目录 `2775`、文件 `0664`/`0775`，属主统一为 `skiff-web` | 待上机（统一身份后需重跑） | `crates/plugin/src/ftp.rs::resolve_identity/ensure_min_uid/align_tree`、`crates/model/src/site.rs` |
| FTP-04 | FTP 上传的文件 PHP 容器内可读写 | `[上机]` FTP-03 通过；客户端可用（`curl`/`lftp`） | 用账号登录（显式 TLS 可选），上传 `probe.txt` 与 `probe.php`；在容器内 `id skiff-web`、`cat`/`stat` 并写入同名文件；浏览器访问 `probe.php` | 上传后容器内可读且属主/属组为 `skiff-web`(201000)；上传文件 `0664`、新建目录 `2775`（不依赖 `align_tree`，与 FTP-03 归一结果一致）；容器内写入成功（非只读）；HTTP 返回上传内容，无 403/500 | 待上机（统一身份后需重跑） | `crates/plugin/src/ftp.rs::align_tree`、`write_conf("Umask")`；`crates/app/src/sites.rs::PHP_ACCOUNT_INIT_SCRIPT`；站点 `index/` |
| FTP-05 | 越界、跨站与暴力登录被拒；删号回收与审计 | `[上机]` 两站各一账号；已装 fail2ban（`login_guard=true`） | 用 A 站账号 `cd ..`/绝对路径/软链尝试进 `log`、`conf`、B 站；连续错密码 3 次；删除 A 站账号后再登录；查 `/$P/settings/audit` 中 `ftp.*` 行 | chroot 生效，越界与跨站全部 530/550；`MaxClientsPerIP=3`、fail2ban 10 分钟 3 次封禁生效；删号后登录失败且 `pureftpd.passwd` 无该行；审计含 `ftp.account.create/delete`、`ftp.deny.*`，无密码内容 | △部分通过（2026-11-09 `2f63aeb`：删号后重新登录 530、`pureftpd.passwd` 无该行、审计有 `ftp.account.create/delete` 且无密码；**三次封禁已实测**——容器客户端 172.31.0.2 连续 3 次错误密码后 `Currently banned:1`、`banip --with-time` 到 `01:54:49 + 600s`、正确口令 `ConnectionRefusedError`、解封恢复；同时修复并复验非 21 端口漏封；越界/跨站与 `ftp.deny.*` 未跑） | `crates/plugin/src/ftp.rs::account_delete/ensure_login_guard`、`crates/core/src/kernel/ftp.rs::ftp_call`、`crates/core` 审计表 |
| FTP-06 | 服务配置校验与持久化 | `[本机+上机]` | 本机：`cargo test -p skiff-plugin ftp --locked`；上机：未安装时保存配置 → 安装 → 重装/升级 → 读 `/etc/skiff/ftp-service.json` 与 `/etc/pure-ftpd/conf/{Bind,MaxClientsNumber,MaxClientsPerIP,PassivePortRange,TLS}` | 本机非法值（端口 0、每 IP > 总数、TLS 未知模式、单边证书路径）全部被拒；上机配置在未安装时只落盘，安装后生效，重装/升级沿用；插件本地不再出现 `service-config.json` | 待上机 | `crates/plugin/src/ftp.rs::validate_service_config/merge_service_config/load_service_config/save_service_config`、`plugins/ftp/src/main.rs` |
| FTP-07 | TLS 证书/私钥路径与显式 TLS | `[上机]` 准备一对合法 PEM 与一个缺失路径 | 配置 `tls_mode=optional` + 合法 cert/key → 重载；检查合并 PEM；改成缺失路径再保存；`tls_mode=off` 保存；用 FTPS 客户端连接 | 合法路径写 `/etc/ssl/private/pure-ftpd.pem`（0600）且显式 TLS 可握手；缺失/非法路径返回 400 可读错误且不静默回退自签；`off` 时忽略路径；`required` 时非 TLS 登录被拒 | 待上机 | `crates/plugin/src/ftp.rs::validate_tls_material/ensure_tls_pem` |
| FTP-08 | fail2ban 状态详情与开关 | `[上机]` 已装/未装 fail2ban 两种起点 | 打开配置弹窗看软件包/运行/jail/封禁数；关闭 `login_guard` 保存；再打开；制造 3 次失败登录；停 fail2ban 后刷新状态 | 关闭前 `jail_enabled=true` 且封禁数真实；关闭后仅本插件 jail 消失（`[sshd] enabled=false` 保留）、fail2ban 重启、软件包仍在；重新开启恢复；停 fail2ban 后状态字段为 `null`（UI 显示未知），不显示 0 | 待上机 | `crates/plugin/src/ftp.rs::fail2ban_status/ensure_login_guard/disable_login_guard`、`plugins/ftp/ui/app.js::renderFail2ban` |
| FTP-09 | 账号启停与最后登录口径 | `[上机]` 已有至少一个账号 | 点「禁用」→ 用该账号登录 → 点「启用」→ 再登录；对禁用账号点「重置密码」；查 `accounts.list` | 禁用后登录 530/530、passwd 行保留为 `#skiff_...`、目录不变；启用后登录恢复；禁用账号改密返回 400「先启用」；`last_login=null`、`last_login_source=unavailable`，UI 显示「不可得」 | 待上机 | `crates/plugin/src/ftp.rs::account_set_enabled/toggle_account_line/accounts_list`、`plugins/ftp/ui/accounts.html` |
| FTP-10 | 旧站点 `index/` 属主一次性迁移 | `[上机]` 存在旧站点且 `index/` 非 201000 | `skiff sites chown-index --dry-run`；再执行 `skiff sites chown-index`；重复执行一次；`getent passwd skiff-web`；查审计 | dry-run 只打印不落盘；执行后 `index/` 全部归 201000:201000；重复执行变更数为 0；跳过符号链接；每站一条 `sites.chown-index` 审计；单站失败不影响其它站，失败站保持原状 | 待上机 | `crates/core/src/commands.rs::sites_chown_index`、`crates/core/src/cli.rs` |

最小回归命令：

```bash
# 本机（编译级 + 纯函数单测）
cargo test -p skiff-plugin ftp --locked
cargo test -p skiff-model site::tests --locked

# 测试机（安装后；$P 为安全入口前缀）
curl -ks -b "$J" -o /dev/null -w 'ftp_plugin=%{http_code}\n' "$B/$P/plugins/com.skiff.ftp"
curl -ks -b "$J" -H 'content-type: application/json' \
  -d '{"method":"list","params":{}}' "$B/$P/plugins/com.skiff.ftp/rpc"
getent passwd skiff-web
docker exec skiff-php-batch1 id skiff-web
grep -H . /etc/pure-ftpd/conf/{MinUID,Umask,PureDB}
ls -ln /www/sites/*/index | head
skiff sites chown-index --dry-run
ls -l /etc/pure-ftpd/auth
systemctl is-active pure-ftpd fail2ban
```

## L. 后台端口 / 入口前缀 / TLS

> 重跑触发：改了 `Config` 的 `listen` / `entry_prefix` / `tls_enabled` / `tls_cert` / `tls_key`、设置页的保存逻辑、旧前缀重定向、Cookie 命名或 `install.sh` 的 TLS 分支。契约见 [DEPLOY_TEST.md](../DEPLOY_TEST.md) §2.3 / §2.4。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| PANEL-01 | 设置页读取端口/前缀/TLS 与 `config.toml` 一致 | `[本机+上机]` admin 已登录 | 打开 `/$P/settings`；对照 `/etc/skiff/config.toml` 的 `listen`、`entry_prefix`、`tls_enabled`、`tls_cert`、`tls_key` | 页面字段值与配置文件一致；未配置的项显示「自动 / 未设置」而不是伪造默认值；非管理员看不到该表单 | □通过 / □失败 | `crates/web/src/handlers/settings.rs`、`templates/settings.html`、`crates/app/src/panel_settings.rs` |
| PANEL-02 | 端口范围与占用检测 | `[本机]` 单测 + `[上机]` 改端口 | `[本机]` `cargo test -p skiff-app panel_settings`；`[上机]` 设置页填 `0`、`70000`、一个已被 `ss -lntp` 占用的端口、当前监听端口 | `0`/`70000` 被拒；被占用端口被拒并提示占用；当前端口视为自己占用、允许保存；被拒时 `config.toml` 不变 | □通过 / □失败 | `crates/app/src/panel_settings.rs::validate_port/check_port_free`、`crates/web/src/handlers/settings.rs` |
| PANEL-03 | 入口前缀字符集/长度/保留段 | `[本机]` 单测 | `cargo test -p skiff-app panel_settings`；再在设置页填 `abc`（过短）、`/`、含空格/中文、`api`、`healthz`、32 位与 33 位 | 6–32 位 ASCII 字母数字 `-` `_` 通过；过短/含非法字符/`/`/保留段被拒；首字符必须字母或数字；被拒时不落盘 | □通过 / □失败 | `crates/app/src/panel_settings.rs::validate_entry_prefix`、`crates/core/src/config.rs::validate` |
| PANEL-04 | 保存写回 `config.toml`、需重启提示 | `[上机]` 可改配置并重启 | 改端口与前缀后保存；不重启先访问；`systemctl restart skiff` 后再访问 | 保存后页面明确提示「需重启生效」，不是静默成功；`config.toml` 原位更新且保留注释与其他字段、无重复键；不重启仍是旧监听；重启后新端口/新前缀可用 | □通过 / □失败 | `crates/app/src/panel_settings.rs::write_config_updates_via`、`crates/web/src/handlers/settings.rs`、`crates/web/src/templates.rs` |
| PANEL-05 | 旧前缀 24h 302 防锁死 | `[上机]` 刚改过前缀 | 用旧前缀 GET 首页与子路径；等窗口外（或手改 `..._previous_until` 为过去时间）再访问旧前缀 | 窗口内旧前缀 GET 302 到新前缀且保留路径；新前缀始终 200；窗口外旧前缀 404；非 GET（POST）不走旧前缀重定向 | □通过 / □失败 | `crates/web/src/handlers/settings.rs::legacy_prefix_root/legacy_prefix_redirect`、`crates/web/src/middleware.rs::prefix_ok`、`crates/core/src/bootstrap.rs` |
| PANEL-06 | TLS 三态与三种 Cookie 策略 | `[本机]` 单测 + `[上机]` 三种部署 | `[本机]` `cargo test -p skiff-core tls_`；`[上机]` 分别用「TLS 开」「反代终止 TLS」「`tls_enabled=false` 直接明文」启动，用 cookie-jar 登录一次 | TLS 开：HTTPS 200，Cookie `__Host-skiff_session` + `Secure`；反代：面板侧明文但 Cookie 仍是 `__Host-` + `Secure`，只有 `trusted_proxies` 能直连；直接明文：日志有「明文 HTTP」警告、页面有告警，Cookie 为 `skiff_dev_session`（无 `Secure`、保留 HttpOnly/SameSite），**浏览器 HTTP 下登录/改密能走通**；`tls_enabled=true` 缺证书拒绝启动且报错可读 | □通过 / □失败 | `crates/core/src/config.rs`（`validate` / `tls_enabled_*` 测试）、`crates/core/src/server.rs`、`crates/identity`（Cookie 命名）、`deploy/install.sh`（`apply_tls_config`） |
| PANEL-07 | 设置写入按段归属且重启可解析 | `[本机]` 单测 + `[上机]` 文件末尾含 `[catalog]` | `[本机]` `cargo test -p skiff-app panel_settings`、`cargo test -p skiff panel_write_then_restart_loads_roundtrip`；`[上机]` 在 `config.toml` 末尾保留 `[catalog]` 段，改端口/前缀/TLS 后保存并 `systemctl restart skiff` | 顶层键写在第一个表头之前，`[catalog]` 内容与顺序不变；落盘前用真实 `FileConfig` 语义复验，非法结果不写且原文件字节不变；重启后服务正常（`NRestarts` 不增长、`/healthz` 200），不出现「语法合法但解析失败」的 crash-loop | □通过 / □失败 | `crates/app/src/panel_settings.rs::write_config_updates_via/apply_update`、`crates/core/src/config.rs::Config::load`（`panel_write_then_restart_loads_roundtrip`） |

最小回归命令：

```bash
# 本机
cargo test -p skiff-app panel_settings --locked
cargo test -p skiff tls_ --locked
cargo test -p skiff-web settings --locked

# 测试机（直接明文形态下；$B 为 http://10.10.15.4:18081）
curl -s -c /tmp/j "$B/$P/login" | grep -o 'name="csrf" value="[^"]*"'
curl -s -b /tmp/j -c /tmp/j -D- -d "csrf=...&username=admin&password=..." "$B/$P/login" | grep -i set-cookie
curl -s -o /dev/null -w '%{http_code} %{redirect_url}\n' "$B/<旧前缀>/"
```
