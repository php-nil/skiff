# M 防火墙与进程网络 / N 任务中心作业

> 上级索引：[TEST_MATRIX.md](../TEST_MATRIX.md) ｜ 拆自 `docs/test/TEST_MATRIX.md` 原 M–N（2026-11-16 整理，正文未改）。

---

## M. 防火墙 / fail2ban / 进程与网络（2026-11-09 新增，未经编译验证）

> 重跑触发：改了 `crates/plugin/src/firewall.rs`、`crates/core/src/kernel/firewall.rs`、
> `plugins/firewall`、`plugins/ftp` 的 protection 分支、`plugins/monitor` 的 `processes` / `network`。
> 2026-11-26 已上机核对并复验 firewall / fail2ban 段：`a915b6e` 上 FW-04/15/16/17 全 FAIL
> （`149de92` 迁移时丢了托管默认值、复核与 purge，插件侧另有一处探测缓存不失效）；修复后在新包
> `64f64cc`（sha256 `3098f223…`）上四项 **全部 PASS**，FW-19 亦 PASS。
> 契约见 [KERNEL_FIREWALL_CONTRACT.md](../../contract/KERNEL_FIREWALL_CONTRACT.md)。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| FW-01 | 能力面授权拒绝 | `[本机]` 单测 | 以无 `firewall:*` 授权的插件调 `firewall_call("status")` | 403，审计 `firewall.deny.status`；不执行任何宿主命令 | □通过 / □失败 | `crates/core/src/kernel/firewall.rs`（冻结入口）、`crates/plugin/src/firewall.rs`（统一拒绝） |
| FW-02 | 规则增删只动自有作用域 | `[上机]` 有 nft 或 iptables | `rule.add` 一条 tcp/端口规则；`rules.list` 核对；`rule.remove` 自家 id | 规则出现在 `inet skiff`/`SKIFF` 链且带 `skiff-` 标识；删除后消失；宿主其它规则不变 | □通过 / □失败 | `plugins/firewall/src/main.rs`（`h_rule_add` / `h_rules_list` / `h_rule_remove`） |
| FW-03 | 非法参数拒绝 | `[本机]` 单测 | 依次传非法端口 `0`/`70000`/`9-3`、非法 CIDR、非法 jail 名、非法 backend、非法 id | 全部 400，且不写规则/不写文件 | □通过 / □失败 | `plugins/firewall/src/main.rs`（`h_rule_add` 的参数校验、`port_param`） |
| FW-04 | fail2ban 安装与复用 | `[本机]` 单测 + `[上机]` 未装 fail2ban | `fail2ban.install`；装完立即 `fail2ban-client status`；再调一次 | 首次安装写 `skiff-defaults.conf`（`[sshd] enabled = false`）、`enable` 后显式 `restart`；装完 `fail2ban-client status` **不含 `sshd`**（2026-11-26 实测：`Number of jail: 0`，返回体 `defaults_disabled: ["sshd"]`）；第二次复用不重复安装且不动宿主既有 jail | □通过 / □失败 | `plugins/firewall/src/main.rs::h_f2b_install` |
| FW-05 | jail 生命周期 | `[上机]` fail2ban 已装 | `fail2ban.jail.ensure` 建 `pure-ftpd`（`filter: pure-ftpd`、`backend: systemd`）；`fail2ban.banned`；`fail2ban.jail.remove` | 生成 `/etc/fail2ban/jail.d/skiff-pure-ftpd.conf`，**含 `filter = pure-ftpd` 行**，重启后该 jail 出现在 `fail2ban-client status`；封禁列表可读；移除后文件消失、jail 消失，其它 `skiff-*` 不受影响 | □通过 / □失败 | `plugins/firewall/src/main.rs`（`h_f2b_jail_ensure` / `h_f2b_banned` / `h_f2b_jail_remove`） |
| FW-06 | 非 Native 形态拒绝 | `[本机]` 受限形态配置 | 调 `firewall_call("status")` | 503，审计 result=fail | □通过 / □失败 | `crates/core/src/kernel/firewall.rs` |
| FW-15 | fail2ban 三态与可读原因 | `[本机]` 单测 + `[上机]` | 分别构造未安装、已安装未运行、运行中三种起点，打开 fail2ban 页 | RPC `fail2ban.status` 直接返回 `state` = `missing`/`stopped`/`running` 与可读 `reason`（2026-11-26 实测三态：`reason` 分别为空 / 「已安装但服务未运行」/「未安装 fail2ban」）（面板优先读 `state`，缺失才按 `installed`/`running` 折算）；`jails` 只列 `skiff-*` 受管 jail，宿主自有 jail 不出现在这里 | □通过 / □失败 | `plugins/firewall/src/main.rs::h_f2b_status` |
| FW-16 | fail2ban 显式启停 | `[本机]` 单测 + `[上机]` | 点「启动」/「停止」；未安装时直接调 `fail2ban.start` | `start` = `systemctl enable` + `restart`，随后用 `fail2ban-client status` 复核实际 jail；`residual_jails` 只含「请求停用但仍在跑」的 jail（如宿主 `jail.d` 排序更晚的 `zz-sshd.conf` 覆盖了托管默认值）；`stop` = `systemctl stop`；未安装时 `start` 返回可读错误（提示先安装） | □通过 / □失败 | `plugins/firewall/src/main.rs::h_f2b_start` / `h_f2b_stop` |
| FW-19 | 宿主 jail 覆盖导致停用不生效时返回体自洽 | `[本机]` 单测 + `[上机]` 宿主存在 `jail.d/zz-sshd.conf` | 构造 `skiff-defaults.conf` 关闭 `sshd`、同时放置排序更晚的 `zz-sshd.conf` 启用 `sshd`；调 `fail2ban.start`；核对返回体与 `fail2ban-client status` | 返回 `jails` 含 `sshd`、`residual_jails` 也含 `sshd`（请求停用但覆盖生效）；`skiff-*` 受管 jail 不受影响 | □通过 / □失败 | `plugins/firewall/src/main.rs::h_f2b_start`（单测 `residual_jails_only_reports_requested_but_still_running`） |
| FW-20 | jail 名与 filter 名不一致 / filter 或日志不存在 / 坏配置回滚 | `[本机]` 单测 + `[上机]` fail2ban 已装 | ① `jail.ensure` 传 `jail=skiffftptest`、`filter=pure-ftpd`、`backend=systemd`；② 传 `jail=nope`、`filter=no-such-filter`；③ 传 `jail=../etc/passwd`；④ 传 `jail=badlog`、`filter=pure-ftpd`、`backend=auto`（机器上无 pure-ftpd 日志） | ① 生成 `skiff-skiffftptest.conf` 且含 `filter = pure-ftpd`，jail 正常起来；② 直接报「找不到 filter」，**不写文件、不影响其它 jail**；③ 400（名字字符集校验）；④ 预检阶段即报 fail2ban 原文（`Have not found any log file`），文件被撤回，且 `systemctl is-active fail2ban` 仍是 `active`——四者都不允许出现「服务躺了但返回 `ensured: true`」 | □通过 / □失败 | `plugins/firewall/src/main.rs`（`h_f2b_jail_ensure`、`filter_exists`、`valid_f2b_name`、`f2b_config_test`、`rollback_jail`） |
| FW-18 | 插件 RPC 状态码与语义一致 | `[本机]` 单测 | `cargo test -p skiff-web --lib rpc_error`；`[上机]` 未安装 fail2ban 时经面板调 `fail2ban.start`/`stop`/`uninstall` | 内核 503 原样返回 503（正文仍为内核原文）；401/403/404/409/500/504/507 各自透传；仅未识别状态与业务错误归 400 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs::rpc_error/kernel_http_status`（`rpc_error_passes_kernel_status_through`） |
| FW-17 | fail2ban 显式卸载 | `[本机]` 单测 + `[上机]` | 点「卸载」并确认；查审计与 `dpkg -l fail2ban` | 停服务、`disable`、`apt-get purge`（`dpkg -l fail2ban` 不留 `rc` 行）、删除 `skiff-defaults.conf`，再 `rm -df` 清掉因此残留的空 `jail.d` / `/etc/fail2ban`（dpkg 只在目录空时删目录，purge 时托管文件还在）；返回体带 `state: missing`；其它 `skiff-<jail>.conf` 保留 | □通过 / □失败 | `plugins/firewall/src/main.rs::h_f2b_uninstall`、`plugins/firewall/ui/app.js` |
| MON-P01 | 进程列表字段与排序 | `[上机]` monitor 已装且授权 `path:/proc` | 打开「进程」页；点 CPU/内存/PID/启动时间表头 | PID/用户/CPU%/内存/启动时间/状态/命令行齐全；排序方向可切换；首刷 CPU% 为「—」 | □通过 / □失败 | `plugins/monitor/src/main.rs::processes`、`plugins/monitor/ui/process.html` |
| MON-P02 | 过滤与分页 | `[上机]` 同上 | 搜索 PID/用户/命令；输入用户过滤；切换每页条数；翻页 | 结果与筛选一致；分页总数/页码正确；`truncated` 时提示扫描上限 | □通过 / □失败 | `plugins/monitor/src/main.rs::processes`、`plugins/monitor/ui/app.js` |
| MON-P03 | 无 `/etc/passwd` 授权退化 | `[上机]` 只授权 `path:/proc` | 打开进程页 | 用户列显示数字 uid，不报错；`user_source=uid` | □通过 / □失败 | `plugins/monitor/src/main.rs::load_passwd` |
| MON-N01 | 网络连接解析 | `[本机]` 单测 + `[上机]` | `cargo test -p skiff-plugin-monitor`；打开「网络」页 | tcp/tcp6/udp/udp6 行解析为可读地址与端口；监听项标记；状态码正确 | □通过 / □失败 | `plugins/monitor/src/main.rs::parse_net_connections` |
| MON-N02 | 筛选与只读声明 | `[上机]` 同上 | 切协议/状态、搜索、翻页 | 结果与筛选一致；页面明确「所属进程以 uid 呈现」，无结束进程等写操作 | □通过 / □失败 | `plugins/monitor/ui/network.html`、`app.js` |
| MON-N03 | `/proc` 缺授权 | `[上机]` 不勾 `path:/proc` | 打开进程/网络页 | 返回 403 提示与采样一致；页面展示错误，不崩溃 | □通过 / □失败 | `plugins/monitor/src/main.rs::fs_err` |
| FTP-P01 | 未装防火墙插件时开启防护 | `[上机]` 只装 ftp | 运行状态页点「启用防护」 | `available=false` + 原因；FTP 安装/账号/配置不受影响 | □通过 / □失败 | `plugins/ftp/src/main.rs::protection_enable` |
| FTP-P02 | 已装防火墙插件时开启/关闭 | `[上机]` ftp + firewall | 开启防护；查封禁；关闭防护 | 生成 `pure-ftpd` jail（端口取 `listen_port`）；封禁列表可读可解封；关闭只删该 jail，不卸载 fail2ban | □通过 / □失败 | `plugins/ftp/src/main.rs`、`plugins/firewall/src/main.rs` |
| FTP-P03 | 不再代管 fail2ban | `[上机]` 只装 ftp | 安装 FTP；`grep login_guard /etc/skiff/ftp-service.json` | 安装不触发 fail2ban；持久化中 `login_guard=false` | □通过 / □失败 | `plugins/ftp/src/main.rs::install`、`save_config` |

最小回归命令（本机，需 Rust 工具链；本轮未执行）：

```bash
cargo test --manifest-path plugins/Cargo.toml -p skiff-plugin-monitor
cargo test --manifest-path plugins/Cargo.toml -p skiff-plugin-firewall
cargo test -p skiff-plugin firewall
cargo test -p skiff-web --test main_flow ui_task_and_job_writes
cargo test -p skiff-app php_init_script
```

## N. 任务中心作业（2026-11-10 新增）

> 重跑触发：改了 `crates/web/src/handlers/jobs.rs`、`crates/app/src/sites.rs` 的作业提交/重试分支、
> `crates/store/src/db/jobs.rs` 的唯一索引或执行器 spawn。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| JOB-01 | 重试复用正常执行器 | `[本机]` 集成测试 + `[上机]` 一条失败任务 | 对失败任务点「重试」；观察任务中心状态与日志 | 新任务 `retry_of` 指向原任务，离开 `queued` 并落 `exit_code`/`error_class`；不再永久占索引 | □通过 / □失败 | `crates/app/src/sites/runtime.rs::retry_job`、`crates/web/src/handlers/ui/jobs.rs::job_retry`、`crates/web/tests/main_flow/ops.rs::ui_task_and_job_writes_require_csrf_and_permission`（本机只覆盖重试入口的鉴权与目标不存在；`retry_of` / 执行器行为需上机） |
| JOB-02 | 同目标冲突可解释 | `[本机]` 集成测试 + `[上机]` | 重试进行中再对同目标提交/重试 | 明确 409，文案含「该对象已有任务在执行…」，不排队、不插新行 | □通过 / □失败 | `crates/store/src/db/jobs.rs`（`idx_jobs_active_target`）、`crates/web/src/handlers/jobs.rs` |
| JOB-03 | 无执行器类型拒绝 | `[本机]` 集成测试 | 对 `site.create` 等无执行器 kind 的历史任务点「重试」 | 400 + 「不支持重试」，作业列表不新增行 | □通过 / □失败 | `crates/app/src/sites/runtime.rs::retry_job`（`RetryPlan` 解析）、`crates/web/tests/main_flow/ops.rs::ui_task_and_job_writes_require_csrf_and_permission`（断言无执行器 kind 重试回 400） |
