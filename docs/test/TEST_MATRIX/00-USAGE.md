# 使用约定与总览（§0 + 总览）

> 上级索引：[TEST_MATRIX.md](../TEST_MATRIX.md) ｜ 拆自 `docs/test/TEST_MATRIX.md` 原 §0 + 总览（2026-11-16 整理，正文未改）。

---

> 定位：**回归清单**。改一处代码后，按章首「重跑触发」挑一章，按稳定编号逐项执行即可。
> 本文只写当前实现应通过的验收项，不记历史、不记未闭环问题；未闭环问题归 [TEST_LEDGER.md](../TEST_LEDGER.md)。
> 所有预期值均按当前路由、模板与契约整理；无法从代码确认的项标「待核对」并写明原因。

## 0. 使用约定

- `$B`：测试机基址（当前地址、端口与 TLS 见 [TEST_ENV/01-ACCESS.md](../TEST_ENV/01-ACCESS.md) §2，例如 `http://10.10.15.4:18081`）；`$P`：安全入口前缀，从 `journalctl -u skiff | grep 安全入口`、首次安装输出或 TEST_ENV/01-ACCESS.md §2 取当前值。
- `$J`：已登录管理员的 curl CookieJar；除 AUTH 章外，默认已登录。
- `[本机]`：由 crate 单测、`xtask` 或模板/路由断言覆盖的项。**开发机只编译不跑**（`cargo check` / `clippy --all-targets`），用例结果看 CI（`AGENTS.md` §4.3）。
- `[本机+上机]`：先由 CI 的对应 crate 用例兜底，再到测试机补一条手工断言（通常用于路径/权限类）。
- `[上机]`：必须有 Linux 测试机、systemd 或 Docker 才能执行。
- 判定栏统一记 `□通过 / □失败`；失败时把实际状态码/响应片段补在备注。
- 测试数据用 `matrix.local`、`matrix-static.local`、`matrix-proxy.local` 等专用域名，避免污染已有站点。

登录一次的命令：

```bash
export B='http://10.10.15.4:18081' P='<entry-prefix>' J=/tmp/skiff.jar
C=$(curl -ks -c "$J" "$B/$P/login" | grep -o 'name="csrf" value="[^"]*"' | head -1 | cut -d'"' -f4)
curl -ks -b "$J" -c "$J" -d "csrf=$C&username=admin&password=<password>" -o /dev/null -w '%{http_code}\n' "$B/$P/login"
```

## 总览

> 表中「本机可验」= 有 crate 级自动化用例覆盖（命令见最后一列）：**开发机只做编译级校验，这些命令在 CI 上执行**（`AGENTS.md` §4.3）。

| 域 | 项数 | 自动化判断 | 主要执行位置 |
| --- | --- | --- | --- |
| A. 身份与会话 | 10 | 1 项本机可验（AUTH-06），9 项必须上机 | `cargo test -p skiff-identity`；测试机 curl 登录链路 |
| B. 菜单与导航 | 8 | 1 项本机可验（MENU-01 常量单测），7 项必须上机 | 路由/模板断言；浏览器手风琴与布局 |
| C. 插件体系 | 10 | 1 项本机可验（PLUGIN-02），9 项必须上机 | `cargo test -p skiff-plugin`；`.wbpkg` 安装与 iframe |
| D. 网站与运行环境 | 15 | 3 项本机可验（SITE-13 模板/路由断言、SITE-14 初始化脚本断言、SITE-15 扩展安装脚本两家族分支断言），12 项必须上机（crate 单测兜底） | `cargo test -p skiff-web site_detail`、`cargo test -p skiff-app php_init_script`、`cargo test -p skiff-app runtime_catalog`；测试机建站与容器 |
| E. Web 服务 | 8 | 0 项本机可验，8 项必须上机（crate 单测兜底） | `cargo test -p skiff-app nginx`；测试机 nginx -t 与日志 |
| F. 文件系统原语 | 13 | 6 项本机可验（FS-02～05、FS-07、FS-13 集成），7 项必须上机（FS-10～12 为第二步新增） | `pwsh tools/gate.ps1 -Task test -Pkg skiff-fs`；插件能力面；管理员文件页 |
| G. 调度器 | 9 | 3 项本机可验（TASK-05～07），6 项需上机（TASK-09 另有 `skiff-cron` 单测兜底） | `cargo test -p skiff-sched -p skiff-app -p skiff-cron`；任务页与 cron 插件页 |
| H. Docker | 14 | 5 项本机可验（DOCKER-09/10/12/13/14），9 项必须上机 | `cargo test -p skiff-docker -p skiff-policy -p skiff-compose -p skiff-web`；Docker 页面/API |
| I. 部署与升级 | 11 | 1 项本机可验（DEPLOY-01），10 项必须上机；DEPLOY-09/10/11 因环境/复现条件阻塞 | `deploy/*.sh`、systemd、安装包 |
| J. 站点压测与资源开销 | 8 | 0 项本机可验，8 项必须上机（测试机两站并发 + `/proc` 采样） | `tools/perf/*`；nginx / PHP-FPM / 反代三路径；`skiff` 进程与插件子进程 |
| K. FTP 与宿主机文件服务 | 10 | 3 项本机可验（FTP-02、FTP-06 配置校验、迁移路径收集单测），7 项必须上机（FTP-06 另有持久化断言需上机） | `cargo test -p skiff-plugin ftp --locked`；测试机 pure-ftpd + PHP 容器 |
| L. 后台端口 / 入口前缀 / TLS | 6 | 2 项本机可验（PANEL-02/03 校验单测，PANEL-06 有 `skiff` 单测兜底），4 项必须上机 | `cargo test -p skiff-app -p skiff`；测试机改端口/前缀/TLS 三形态 |
| M. 防火墙 / fail2ban / 进程与网络 | 20 | 4 项本机单测（FW-03、FW-18、FW-19、MON-N01 解析），其余必须上机 | `cargo test -p skiff-plugin firewall`；`plugins/` 第二 workspace 单测；测试机 nft/iptables/fail2ban + `/proc` |
| N. 任务中心作业 | 3 | 3 项本机可验（`main_flow` 集成测试），需上机复验执行器真实行为 | `cargo test -p skiff-web --test main_flow job_retry`；测试机运行环境安装/扩展任务 |
| O. 商店目录（Skiff-server） | 3 | 3 项本机可验（自签 CA 端到端 + 配置拒绝 + serve 契约），建议上机复验内网自签镜像与 serve | `cargo test -p skiff-app catalog::fetch::tls_tests`、`cargo test -p skiff catalog`、`cargo test -p skiff-server`；测试机内网镜像 |
