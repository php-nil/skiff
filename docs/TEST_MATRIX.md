# Skiff 测试矩阵（按内核功能域）

> 定位：**回归清单**。改一处代码后，按章首「重跑触发」挑一章，按稳定编号逐项执行即可。
> 本文只写当前实现应通过的验收项，不记历史、不记未闭环问题；未闭环问题归 [TEST_LEDGER.md](TEST_LEDGER.md)。
> 所有预期值均按当前路由、模板与契约整理；无法从代码确认的项标「待核对」并写明原因。

## 0. 使用约定

- `$B`：测试机基址（当前地址、端口与 TLS 见 [TEST_ENV.md](TEST_ENV.md) §2，例如 `http://10.10.15.4:18081`）；`$P`：安全入口前缀，从 `journalctl -u skiff | grep 安全入口`、首次安装输出或 TEST_ENV §2 取当前值。
- `$J`：已登录管理员的 curl CookieJar；除 AUTH 章外，默认已登录。
- `[本机]`：开发机即可执行（`cargo test`、`xtask`、模板/路由单测）。
- `[本机+上机]`：先跑单测，再到测试机补一条手工断言（通常用于路径/权限类）。
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

## A. 身份与会话

> 重跑触发：改了入口前缀、登录/登出、会话 Cookie 属性、CSRF、登录限速、改密策略、TOTP、API 登录或安全头。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| AUTH-01 | 未登录访问被引导到登录页，健康检查免鉴权 | `[上机]` 服务已启动，`$P` 已知 | `curl -ks -o /dev/null -w '%{http_code} %{redirect_url}\n' "$B/$P/"`；再取 `$P` 与 `$P/login`；`curl -ks -o /dev/null -w '%{http_code}\n' "$B/healthz"` | `/$P/` 307 到 `/$P`（保留方法的临时重定向），`/$P` 303 到 `/$P/login?next=...`；`/healthz` 200 | □通过 / □失败 | `crates/web/src/middleware.rs`、`crates/web/src/handlers/meta.rs` |
| AUTH-02 | 安全入口前缀形态正确，裸路径不暴露 | `[上机]` 能读到启动日志或安装输出 | 从日志取 `$P`；`curl -ks -o /dev/null -w '%{http_code}\n' "$B/"`；`curl -ks -o /dev/null -w '%{http_code}\n' "$B/static/skiff.js"` | `$P` 为 8 位小写字母/数字组合（字母表去掉 `0/1/l/o`，数字仅 `2-9`）；裸 `/` 与根 `/static/skiff.js` 均 404 | □通过 / □失败 | `crates/core/src/bootstrap.rs`（`random_prefix`）、`crates/web/src/lib.rs`（静态资源前缀） |
| AUTH-03 | 登录成功下发安全 Cookie | `[上机]` admin 口令已知；HTTPS 直连 | `curl -ks -D - -c "$J" -d "csrf=$C&username=admin&password=<password>" "$B/$P/login" -o /dev/null` | 303；`Set-Cookie: __Host-skiff_session=...; Max-Age=43200; Path=/; Secure; HttpOnly; SameSite=Lax`（dev HTTP 下为 `skiff_dev_session` 且无 `Secure`） | □通过 / □失败 | `crates/web/src/handlers/auth.rs`、`handlers/mod.rs`（`simple_cookie`）、`crates/identity/src/session.rs` |
| AUTH-04 | 登录 CSRF 不能绕过 | `[上机]` 未登录 | GET `/$P/login` 拿 `skiff_login` Cookie；POST 不带 `csrf`；再带正确 `csrf` 重试 | 第一次 403；第二次 303 到 `/$P`（无尾斜杠） | □通过 / □失败 | `crates/web/src/handlers/auth.rs`、`templates/login.html` |
| AUTH-05 | 登录失败限速生效且成功登录清零 | `[上机]` 用错误口令 | 5 分钟内连续 5 次错误口令；第 6 次（含正确口令）；封禁窗口过后用正确口令登录，再输 1 次错误口令 | 累计第 5 次起返回 503 并提示剩余秒数，封禁期内正确口令同样 503；窗口过后正确口令 303 且计数清零，随后 1 次错误口令返回错误页（非 503） | □通过 / □失败 | `crates/app/src/lib.rs`（`LOGIN_MAX_FAILURES` 等）、`crates/app/src/auth.rs` |
| AUTH-06 | 会话空闲与绝对过期 | `[本机]` 身份/存储 crate 单测 | `cargo test -p skiff-identity session --locked`（Cookie 属性）；`cargo test -p skiff-store session --locked`（空闲续期与过期吊销）；上机可把 `sessions.idle_expires` 改到过去再请求 | 空闲 30 分钟过期；绝对 12 小时过期；过期后请求 303 到登录页 | □通过 / □失败 | `crates/identity/src/session.rs`、`crates/store/src/db/sessions.rs` |
| AUTH-07 | 改密策略与强制改密开关 | `[上机]` 已登录 admin | POST `/$P/password`，`new_password` 少于 8 位；再用 8 位以上改密；另一浏览器会话随后访问 | 短口令 400；合规口令 303；其他会话失效；未显式开启 `--require-initial-password-change` 时不强制跳转 | □通过 / □失败 | `crates/web/src/handlers/auth.rs`、`crates/core/src/cli.rs`、`crates/core/src/bootstrap.rs` |
| AUTH-08 | TOTP 绑定、启用、恢复码 | `[上机]` 已登录 admin，手机 TOTP 可用 | 打开 `/$P/settings`，在安全区生成二维码（POST `/$P/totp`）；输入当前码启用（POST `/$P/totp/enable`）；登出后用 TOTP 登录；再用一枚恢复码登录 | 启用返回 200 并渲染恢复码页（10 个一次性码，非 303）；错误码/无码被拒（错误页）；正确码登录成功；恢复码一次性使用后失效 | □通过 / □失败 | `crates/identity/src/totp.rs`、`crates/web/src/handlers/auth.rs` |
| AUTH-09 | JSON 登录与 Bearer 鉴权 | `[上机]` admin 口令已知 | `curl -ks -H 'Content-Type: application/json' -d '{"username":"admin","password":"..."}' "$B/api/v1/session"`；用返回 token 请求 `/api/v1/docker/containers`；去掉 Authorization 再请求 | JSON 登录 200 且含 token；带 Bearer 200；不带 401 | □通过 / □失败 | `crates/web/src/handlers/api.rs`、`crates/web/src/middleware.rs` |
| AUTH-10 | 登出即吊销，安全头存在 | `[上机]` 已登录 | POST `/$P/logout`（带 CSRF）；用旧 Cookie 再访问；检查首页响应头 | 303；旧 Cookie 再访问 303 到登录页；响应含 `X-Content-Type-Options: nosniff`、`X-Frame-Options: DENY` 或 CSP `frame-ancestors 'none'` | □通过 / □失败 | `crates/web/src/handlers/auth.rs`、`crates/web/src/middleware.rs` |

最小回归命令：

```bash
curl -ks -o /dev/null -w 'health=%{http_code}\n' "$B/healthz"
curl -ks -o /dev/null -w 'login=%{http_code}\n' "$B/$P/login"
curl -ks -o /dev/null -w 'root=%{http_code}\n' "$B/$P/"
curl -ks -o /dev/null -w 'bad_csrf=%{http_code}\n' -d 'username=admin&password=x' "$B/$P/login"
curl -ks -o /dev/null -w 'api_noauth=%{http_code}\n' "$B/api/v1/docker/containers"
```

## B. 菜单与导航

> 重跑触发：改了一级菜单、manifest 菜单声明、深链解析、布局/宽度偏好、菜单设置或手风琴交互。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| MENU-01 | 一级菜单固定六项且顺序稳定 | `[本机]` 菜单常量单测；`[上机]` 已登录 | `cargo test -p skiff-web menu --locked`；或 `curl -ks -b "$J" "$B/$P/"` 统计一级导航项 | 概览、网站、Docker、插件、工具、设置六项，顺序固定；`overview` 不可隐藏 | □通过 / □失败 | `crates/web/src/menu.rs`（L1 常量）、`templates/base.html` |
| MENU-02 | 二级/三级由 manifest 声明，停用即消失 | `[上机]` 装有 cron、guard、monitor 等插件 | 打开首页记二级项；停用 monitor；刷新；再启用 | monitor 的二级项消失，其余不变；重新启用后恢复 | □通过 / □失败 | `crates/web/src/menu.rs`、`crates/plugin`、`plugins/*/manifest.toml` |
| MENU-03 | 深链与信息页入口均可直达 | `[上机]` 已登录，插件已启用 | 访问 `/$P/m/overview`、`/$P/m/plugin/com.skiff.monitor/monitor`、`/$P/m/plugin/com.skiff.monitor/monitor/overview`、`/$P/plugins/com.skiff.monitor`、`/$P/m/plugin/com.skiff.monitor/nope`、`/$P/m/not-exist` | 前四条 200 且 active 菜单一致（`/plugins/<id>` 为信息页，不是插件工作台）；`monitor` 的 L2 是 `monitor`、L3 是 `overview/history`（`.../overview` 直挂 L2 下为 404）；未知 L2/L3 与未知 `m/*` 均 404 | □通过 / □失败 | `crates/web/src/menu.rs`、`crates/web/src/handlers/plugins.rs` |
| MENU-04 | 布局 left/top 按用户持久化 | `[上机]` admin 与第二个账号 | 菜单设置分别选 left、top，各刷新两次；第二账号打开首页 | 两种布局均保持；第二账号看到默认布局，不受 admin 影响 | □通过 / □失败 | `crates/web/src/menu.rs`、`handlers/menu.rs`、`user_prefs`。`[待补入口]` 无建号入口：面板只有 `/{p}/password`，无用户 CRUD 路由与页面；需补「用户管理」后再验多用户隔离 |
| MENU-05 | 内容宽度 auto/fixed 生效且越界拒绝 | `[上机]` 已登录 | 设置 `content_width=auto`；再设 `fixed=1600`；再提交 `800` 与 `5000` | auto 无 `max-width`；1600 落库并生成 `--content-max`；800/5000 被拒并保留旧值 | □通过 / □失败 | `crates/web/src/menu.rs`、`templates/base.html` |
| MENU-06 | 菜单设置：上移、隐藏、固定、恢复默认 | `[上机]` 已登录 | 在 `/$P/settings/menu` 依次上移、隐藏、固定、取消固定、恢复默认，每次刷新 | 顺序变化持久；隐藏项不再出现；固定项固定在顶部；恢复默认回到初始顺序 | □通过 / □失败 | `crates/web/src/handlers/menu.rs`、`menu.rs` |
| MENU-07 | 停用全部插件后内核菜单不受影响、概览给出行动入口 | `[上机]` 已登录 | 停用全部插件，打开概览、工具、网站；再全部启用 | 工具仍固定显示内核「计划任务」，网站仍显示内核站点项，二者都不出现空态；概览出现「N 个插件已停用」提示与「去插件中心」链接且链接 200；重新启用后提示消失、插件二级项恢复 | □通过 / □失败 | `crates/web/src/menu.rs`（L1 常量与空态）、`templates/dashboard.html`、`templates/base.html` |
| MENU-08 | 手风琴展开/收起/单开与键盘回焦 | `[上机]` 真实浏览器 | 点击一级菜单展开二级；点另一个一级；按 Escape | 同一时间只开一个一级；`.subnav.open` 高度 48px；Escape 收起并把焦点回一级按钮 | □通过 / □失败 | `crates/web/static/skiff.css`、`crates/web/static/skiff.js`、`templates/base.html` |

最小回归命令：

```bash
for u in "" "m/overview" "m/plugin/com.skiff.monitor/overview" "plugins/com.skiff.monitor" "settings/menu"; do
  printf '%s -> ' "$u"; curl -ks -b "$J" -o /dev/null -w '%{http_code}\n' "$B/$P/$u"
done
curl -ks -b "$J" "$B/$P/" | grep -c 'class="subnav'   # 至少 1 个二级容器
curl -ks -b "$J" "$B/$P/" | grep -o 'data-layout="[^"]*"' | head -1
```

## C. 插件体系

> 重跑触发：改了 `.wbpkg` 格式、manifest 校验、安装/确认授权、启停/卸载、视图路由、iframe 沙箱/CSP、槽位、RPC 或菜单声明。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| PLUGIN-01 | 签名包安装与未签名包策略 | `[上机]` 有 official 签名包与一个未签名包 | 上传签名包；在 `config.toml` 设 `allow_unsigned = true` 后再传未签名包；改回 `false` 再传 | 签名包安装成功；开关开启时未签名包成功且详情标「未验签」；关闭时明确拒绝 | □通过 / □失败 | `crates/plugin`、`crates/web/src/handlers/plugins.rs`、`deploy/config.toml.example` |
| PLUGIN-02 | manifest/包结构与摘要校验 | `[本机]` 有 `.wbpkg` 文件 | `cargo run -p xtask -- verify-pkg path/to/pkg.wbpkg --pubkey <hex>`（未签名包加 `--allow-unsigned`）；再改一个文件字节并重新打包后重跑 | 原始包校验通过；篡改包摘要/签名失败；结构不符被拒 | □通过 / □失败 | `xtask`、`crates/pkgfmt` |
| PLUGIN-03 | 安装时逐条确认授权，拒绝即中止 | `[上机]` 有请求 `fs:read` 等权限的包 | 安装到确认页；先取消一项权限；再全选确认 | 取消时安装中止；全选后安装成功，详情页列出授权项 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`、`templates/plugin_confirm.html` |
| PLUGIN-04 | 启用/停用联动菜单、槽位与深链 | `[上机]` 已装 monitor | 停用 monitor；访问 `/$P/m/plugin/com.skiff.monitor/overview`；启用后重试 | 停用后菜单、槽位消失且深链 404；启用后均恢复 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`、`menu.rs`、`templates/dashboard.html` |
| PLUGIN-05 | 信息页与多视图路由 | `[上机]` 已装 cron、guard、monitor、files、testkit | 访问 `/$P/plugins/<id>` 及 `list/new`、`containers/events`、`overview/history`、`main/check/group`、`single`、未知视图 | 信息页 200 且版本/指纹/验签/授权/启停/卸载齐全；已知视图 200 且标签数正确；`files/main` 无标签；未知视图 404 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`、`templates/plugin_info.html`、`templates/plugin_view.html` |
| PLUGIN-06 | iframe 静态资源与 CSP/沙箱 | `[上机]` 已装 files | 访问 `/$P/plugins/<id>/ui/index.html`；`curl -I` 看头；浏览器打开工作台 | HTML 200；CSP 为 `default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'; frame-ancestors 'self'`；iframe `sandbox="allow-scripts allow-modals allow-forms"`（无 `allow-same-origin`） | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`（`PLUGIN_UI_CSP`）、`templates/plugin_view.html` |
| PLUGIN-07 | 标题归属：插件不能覆盖内核标题 | `[上机]` 已装任意插件 | 打开插件 UI 页，检查 `<title>` 与工作台标题；在插件脚本里尝试改 `document.title` 后刷新 | 页面标题由内核注入；插件脚本的 `document.title` 赋值被标题守卫覆盖回内核标题 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`（`inject_title_guard`） |
| PLUGIN-08 | 概览槽位与固定标题 | `[上机]` monitor 启用 | 打开概览；停用 monitor；再启用 | monitor 启用时出现 `overview.health` 槽位且标题由内核固定；停用回落原生 `health-grid` | □通过 / □失败 | `crates/web/src/templates.rs`、`templates/dashboard.html`、`templates/mount_card.html`、`crates/plugin` |
| PLUGIN-09 | RPC 成功、业务错误与授权拒绝 | `[上机]` 已装 testkit | 调 `/$P/plugins/com.skiff.testkit/rpc` 的 `ping`、`fail`、`fs_read` 越界路径与无授权方法 | `ping` 200；`fail` 400 且含 `-32602`；`fs_read /etc/passwd` 403；无授权方法 400 且审计 result=fail | □通过 / □失败 | `crates/plugin/src/rpc.rs`、`capability.rs`、`plugins/testkit`、`crates/web/src/handlers/plugins.rs`（内核 403/404/409 透传，其余 400） |
| PLUGIN-10 | 卸载清理菜单/路由但保留 data、清空授权 | `[上机]` 已装 files 且有数据文件与授权 | 卸载 files；看 `/www/skiff/plugins/com.skiff.files/data`；重装 | 菜单与 `/$P/plugins/<id>` 消失；data 文件原样保留；授权清空，重装需重新确认 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`、`crates/app`、`plugins/README.md` |
| PLUGIN-11 | 重装/升级保留既有授权，新增声明不自动授予 | `[上机]` 已装 files，装时只勾选部分权限并手授路径 | 记下详情页授权项；上传同版本包覆盖重装；再上传更高版本；查看授权与确认页 | 重装/升级均直接安装、不再弹确认页；授权项与原来一致；manifest 新增声明不被自动授予 | □通过 / □失败 | `crates/plugin/src/install.rs`、`crates/web/src/handlers/plugins.rs`、`crates/core/src/commands.rs` |
| PLUGIN-12 | 内置任务删除的 RPC 状态与文案 | `[上机]` 已装 cron | 调 `cron/rpc`，`{"method":"remove","params":{"id":"builtin:db-vacuum"}}` | HTTP 403；正文保留「内置任务不可删除（只能停用）」；不得出现「请确认计划任务持有 scheduler 权限」 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`、`plugins/cron/src/main.rs` |

最小回归命令：

```bash
for id in com.skiff.guard com.skiff.monitor com.skiff.files com.skiff.cron; do
  printf '%s ' "$id"; curl -ks -b "$J" -o /dev/null -w '%{http_code}\n' "$B/$P/plugins/$id"
done
curl -ks -b "$J" -o /dev/null -w 'ui=%{http_code}\n' "$B/$P/plugins/com.skiff.files/ui/index.html"
curl -ksI -b "$J" "$B/$P/plugins/com.skiff.files/ui/index.html" | grep -i 'content-security-policy'
curl -ks -b "$J" -o /dev/null -w 'unknown_view=%{http_code}\n' "$B/$P/plugins/com.skiff.monitor/nope"
```

**2026-11-06 实测（测试机 10.10.15.4）**

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| PLUGIN-01 | ☑通过 | 签名包安装成功；`allow_unsigned=true` 放行 key_id 不在信任库的未签名包，确认页与详情标「未验签」；改回 `false` 后同名上传 400「未导入该包的公钥」；恢复签名包后详情「已验签」。注：删 `sig` 但保留已信任 `key_id` 的包即使开关打开也应拒装（防伪造），属预期 |
| PLUGIN-02 | ☑通过 | `skiff-plugin verify` 原包「已验签」；改动 `ui/app.js` 重打包报「哈希清单不一致: 文件哈希不匹配: ui/app.js」 |
| PLUGIN-03 | ☑通过 | 确认页可取消、未确认提交 404；全选确认 303；详情列出授权项 |
| PLUGIN-04 | ☑通过 | 停用 monitor 后 `m/plugin/com.skiff.monitor/monitor/overview` 404、概览槽位消失；启用后均恢复 200 |
| PLUGIN-05 | ☑通过 | cron/files/guard/monitor/testkit 信息页 200；`list/new`、`containers/events`、`overview/history`、`main/check/group` 标签数正确；`files/main` 无标签；未知视图 404 |
| PLUGIN-06 | ☑通过 | `ui/index.html` 200；CSP 实测 `default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'; frame-ancestors 'self'`；iframe `sandbox="allow-scripts allow-modals allow-forms"`（无 `allow-same-origin`） |
| PLUGIN-07 | ☑通过 | 未知视图页 200 且注入标题守卫脚本，`document.title` 被覆盖回内核标题 |
| PLUGIN-08 | ☑通过 | 启用出 `overview.health` 槽位与固定标题；停用回落 `health-grid`；与 PLUGIN-04 同轮复验 |
| PLUGIN-09 | ☑通过 | `ping` 200；`fail` 400 含 `-32602`；越界 `fs_read` 400；无授权方法 400 且审计 `result=fail` |
| PLUGIN-10 | ☑通过 | 卸载后菜单、信息页、路由消失；`/www/skiff/plugins/com.skiff.files/data` 原样保留；重装后数据仍在 |
| PLUGIN-12 | ☑关闭（2026-11-07 追补） | RPC 403 透传已实测（`remove builtin:db-vacuum` → 403，审计 `scheduler.deny`，正文「内置任务不可删除（只能停用）」）；「未装官方包不被自动安装」由 `crates/plugin/src/official.rs` 单测覆盖（本轮未做上机卸载样本，按已定论关闭） |

## D. 网站与运行环境

> 重跑触发：改了建站表单、vhost 生成、域名/别名、站点目录、运行环境安装/启停/解绑、扩展、socket/端口、SSL、删除保护、PHP 容器身份初始化脚本或池模板、PHP 扩展安装脚本的包管理器分支。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| SITE-01 | 站点列表与状态列 | `[上机]` 至少 1 个站点 | 打开 `/$P/sites`；检查表格、默认标记、状态列 | 只有一个 `table.grid.compact.sites-grid`；默认站带「默认」徽标；状态列同时显示状态徽标与 HTTPS 徽标（四种文案：已启用 HTTPS / 证书缺失 / 证书已上传 / 未配置 HTTPS） | □通过 / □失败 | `crates/web/src/handlers/sites.rs`、`templates/sites.html` |
| SITE-02 | 运行环境建站（PHP） | `[上机]` nginx 与 php-8.5 已安装 | 新建 `matrix.local`，类型「运行环境 / PHP」；访问 HTTP；打开详情概览 | 建站 303；HTTP 200 且含 `PHP Version 8.5`；四子目录 `index/log/ssl/conf` 生成；HTTPS 需完成 SITE-12 后才 200 | □通过 / □失败 | `crates/app/src/sites.rs`、`runtime_catalog.rs`、`templates/site_new_dialog.html` |
| SITE-03 | 静态站建站 | `[上机]` nginx 就绪 | 新建 `matrix-static.local`，类型「静态」；在 `index/` 放自定义 `index.html` | 建站 303；访问返回自定义内容；无 PHP 解析 | □通过 / □失败 | `crates/app/src/sites.rs`、`templates/site_new_dialog.html` |
| SITE-04 | 反向代理建站 | `[上机]` 上游 HTTP 服务已起（含 18081） | 新建 `matrix-proxy.local` 指向 `127.0.0.1:18081`；访问；查上游日志 | 站点 200；上游自定义响应头（如测试服务的 `X-Upstream`）原样透传；上游收到 `Host: matrix-proxy.local` | □通过 / □失败 | `crates/app/src/sites.rs`、`templates/site_new_dialog.html` |
| SITE-05 | Rust 运行环境建站 | `[上机]` rust 运行环境已安装、镜像可拉取 | 新建 `matrix-rust.local`，类型「运行环境 / Rust」；启动运行环境；访问站点 | vhost 生成 303；容器 running；站点返回 Rust 服务响应 | □通过 / □失败 | `crates/app/src/sites.rs`、`runtime_catalog.rs` |
| SITE-06 | 域名与别名校验 | `[上机]` 已登录 | 依次提交重复域名、非法域名、IP、通配符；再为已有站点加别名 | 重复/非法/IP/通配符被拒并给内联错误；合法别名保存后同一 vhost 生效 | □通过 / □失败 | `crates/app/src/sites.rs`、`templates/site_detail.html` |
| SITE-07 | 站点目录与越界保护 | `[上机]` 已建站 | 打开 `/$P/sites/matrix.local?tab=dir`；再请求 `?tab=dir&path=../log`、`../ssl`；检查目录权限 | 目录页可列 `index/`；`../log`、`../ssl`、绝对路径 400；`/www/sites` 为 0755，数据根 `/www/skiff` 与 `plugins` 为 0711 | □通过 / □失败 | `crates/web/src/handlers/sites.rs`（`detail` 的 `tab=dir`）、`crates/app/src/sites.rs`、`deploy/install.sh` |
| SITE-08 | 运行环境安装/启停/重建 | `[上机]` 已登录，镜像源可用 | 安装 php-8.5；停、启、重建；每步看状态与 `docker ps` | 安装各步骤全部成功；停/启状态 303 且容器状态一致；重建后站点仍 200 | □通过 / □失败 | `crates/app/src/sites.rs`、`templates/runtime_detail.html` |
| SITE-09 | PHP 扩展安装/卸载 | `[上机]` php-8.5 已安装 | 在运行环境详情安装一个扩展；重建；卸载；再重建 | 安装/卸载 303；`php -m` 或 `phpinfo` 出现/消失对应扩展；站点不 500 | □通过 / □失败 | `crates/app/src/runtime_catalog.rs`、`sites.rs`、`templates/runtime_detail.html` |
| SITE-10 | 解绑与删除保护 | `[上机]` 有绑定站点的运行环境 | 运行环境详情尝试删除；先解绑一个站点再删；删除绑定过的站点 | 有绑定站点时删除被拒并列出域名；解绑后列表更新；删站不影响其他站与运行环境 | □通过 / □失败 | `crates/app/src/sites.rs`（`unbind_runtime_site`、`delete_site_runtime`） |
| SITE-11 | socket、端口与网络模式 | `[上机]` PHP 与 nginx 运行中 | 检查 `/www/runtime/php/<name>/php-fpm.sock` 权限与属主；`docker inspect skiff-web-nginx` 与 PHP 容器；`ss -ltnp` 看 80 | socket 0660 `0:65534`（root:gid 65534）；PHP 容器挂载 `web_root`（读写）与 pool 配置（只读）；nginx `NetworkMode=host` 且监听 80；面板不占用 80 | □通过 / □失败 | `crates/app/src/sites.rs`（`SOCKET_MODE`/`SOCKET_GID`、`nginx_spec`、`php_spec_for`）、`deploy/install.sh`、`docs/DEPLOY_TEST.md` §1.1 |
| SITE-12 | 证书与强制 HTTPS | `[上机]` 站点已建 | 生成自签证书；上传证书；开启强制跳转；访问 HTTP 与 HTTPS | 证书落 `/www/sites/<domain>/ssl/`；HTTPS 200；HTTP 301/308 到 HTTPS；`docker exec skiff-web-nginx nginx -t` 通过 | □通过 / □失败 | `crates/app/src/sites.rs`、`templates/site_detail.html` |
| SITE-13 | 高风险动作只在「管理」页签 | `[本机]` 模板断言；`[上机]` 已登录 admin 且至少 1 个站点 | 打开站点详情概览，检查顶部操作区；再切到「管理」页签启停一次并删除 | 顶部只有访问/日志/默认与返回，无「停止/启动」「删除站点」；管理页签内两种动作均可执行；启停后 303 回 `?tab=manage` 并显示「站点已启动/已停止」；非管理员看不到「管理」页签 | □通过 / □失败 | `crates/web/templates/site_detail.html`、`crates/web/src/handlers/sites.rs`（`render_detail` 的 tab 白名单、`action` 回跳）、`crates/web/tests/main_flow.rs`（`site_detail_risk_actions_only_in_manage_tab`） |
| SITE-14 | PHP 池 `nobody` 组别名（Debian/Alpine 两家族） | `[本机]` 单测；`[上机]` 两家族镜像各一个运行环境 | 分别用 `php:8.5-fpm`（Debian）与 `php:8.5-fpm-alpine`（Alpine）建实例并启动；`docker logs` 查 FPM 启动；`docker exec … getent group nobody`、`ls -l /www/runtime/php-*/php-fpm.sock` | 两家族都 running；`getent group nobody` = 65534；socket `root:nobody 0660`；Debian 系无 `cannot get gid for group 'nobody'`；自建镜像同样成立（脚本不按家族分支） | □通过 / □失败 | `crates/app/src/sites.rs`（`PHP_ACCOUNT_INIT_SCRIPT`，单测 `php_init_script_resolves_nobody_from_gid_not_family_name`）、`crates/app/src/sites_vhost.rs` |
| SITE-15 | PHP 扩展安装在 Debian/Alpine 两家族可用 | `[本机]` 单测；`[上机]` 两家族 PHP 运行环境各一个，均 running 且**已按新环境变量重建** | 在 `php:8.5-fpm`（Debian）与 `php:8.5-fpm-alpine`（Alpine）环境分别安装一个需编译的扩展（如 `imagick` / `redis` / PECL `msgpack`）；看安装任务日志；`docker exec … php --ini`、`php -m`；访问绑定站点 | 两家族都 `succeeded`、主路径 exit 0；日志无 `cannot create /etc/apk/config`；`php --ini` 中 `conf.d` 与 `conf.d-skiff` **各出现一次**，无 `Module already loaded` 告警；`/etc/apk/config`、`apk add/del` 只出现在 Alpine 分支；Debian 走 `apt-get install --no-install-recommends` 装 `-dev` 包；`php -m` 出现扩展且站点仍 200；容器内无 `apk` 也无 `apt-get` 时明确报错退出码非 0 | □通过 / □失败 | `crates/app/src/runtime_catalog.rs`（`installer_prelude`、`legacy_deps_branch`、`debian_build_package`）、`crates/app/src/sites.rs`（`PHP_INI_SCAN_DIR_VALUE`，单测 `php_ini_scan_dir_lists_each_directory_once`、`extension_scripts_branch_on_container_package_manager`、`installer_prelude_enables_apk_package_cache`） |
| SITE-16 | 默认站点双向切换与失败恢复 | `[本机]` 单测；`[上机]` 至少 2 个运行中站点（先建的 rowid 更小） | 先把默认站设为后建的 b，再切回先建的 a；再制造 reload 失败（停掉 nginx 容器或断开 Docker）后切一次 | 两个方向都 303，库、`00-skiff-default.conf`、nginx 行为三者一致；reload 失败时返回错误，库仍指向切换前站点，conf 按旧默认重写且不含失败方向 | □通过 / □失败 | `crates/store/src/db/sites.rs`（`default_site_switches_in_both_rowid_directions`）、`crates/app/src/sites/crud.rs::set_default_site`、`crates/app/src/sites/tests.rs`（`set_default_site_rolls_back_db_and_conf_when_reload_fails`） |
| SITE-17 | 保存 vhost 走 reload 优先、证书成对写入不产生 mismatch | `[本机]` 单测；`[上机]` nginx 运行中、站点已启用 HTTPS | 4 会话并发保存同一站点 vhost 若干轮，确认无「配置已回滚：nginx 未能保持运行」且 `docker ps` 显示容器未重启（`StartedAt` / 容器 ID 不变）；反复上传/生成证书并同时保存 vhost，`nginx -t` 与站点访问无 `key values mismatch`；再提交一份坏配置确认仍被回滚 | 并发保存不触发容器重启、无窗口回滚；日志出现「nginx 已平滑 reload（worker PID 换代）」，`StartedAt` 不变即证明走的是 reload 而非 restart；worker 探测读 `/proc/1/task/1/children`（`-r` + 读后判空，不用恒为 0 size 的 `-s`），退化扫描同时认 `nginx` 与 OpenResty 的 `openresty`；证书与私钥始终配对，私钥 rename 失败时回滚成替换前的成对文件（不得删掉新证书留下悬空引用），首次上传失败时库 `https=0` 且 vhost 不再含 `ssl_certificate`，`nginx -t` 通过、站点 200；坏配置仍返回 `Conflict`（校验失败）或 `Unavailable`（起不来并回滚） | □通过 / □失败 | `crates/app/src/sites/crud.rs`（`activate_with`、`reload_nginx` 回退链、`nginx_worker_generation` / `parse_worker_probe` / `is_nginx_comm`、`replace_cert_pair`、`repair_no_cert_state`、`nginx_write_lock`）、`crates/app/src/sites/tests.rs`（`nginx_activation::openresty_comm_reaches_reload_branch`、`cert_pair_*`、`failed_first_upload_repairs_dangling_ssl_state` 等）、`crates/core/src/server.rs`（巡检 reopen 同锁） |

最小回归命令：

```bash
curl -ks -o /dev/null -w 'sites=%{http_code}\n' -b "$J" "$B/$P/sites"
curl -ks -o /dev/null -w 'site_php=%{http_code}\n' -H 'Host: matrix.local' 'http://10.10.15.4/'
curl -ks -o /dev/null -w 'site_static=%{http_code}\n' -H 'Host: matrix-static.local' 'http://10.10.15.4/'
curl -ks -o /dev/null -w 'site_proxy=%{http_code}\n' -H 'Host: matrix-proxy.local' 'http://10.10.15.4/'
curl -ks -o /dev/null -w 'traversal=%{http_code}\n' -b "$J" "$B/$P/sites/matrix.local?tab=dir&path=../log"
curl -ks -o /dev/null -w 'site_manage=%{http_code}\n' -b "$J" "$B/$P/sites/matrix.local?tab=manage"
```

本机脚本断言（扩展安装两家族分支、`nobody` 组别名、ini 扫描目录不重复）：

```bash
cargo test -p skiff-app runtime_catalog
cargo test -p skiff-app php_init_script
cargo test -p skiff-app php_ini_scan_dir
```

## E. Web 服务

> 重跑触发：改了 nginx 主配置/性能参数、`nginx -t` 校验与回滚、日志读取、模块页或 Web 服务容器操作。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| WEB-01 | Web 服务状态与最近 reload | `[上机]` nginx 已安装 | 打开 `/$P/sites/web` | 200；显示容器状态、版本、配置路径、最近一次 reload 结果 | □通过 / □失败 | `crates/web/src/handlers/nginx.rs`、`templates/nginx.html` |
| WEB-02 | 性能参数保存并生效 | `[上机]` nginx 运行中 | 保存 worker、gzip、keepalive、client_max_body_size；看生成配置；`nginx -t` | 303 且页面提示成功；生成配置含新值；`nginx -t` 成功 | □通过 / □失败 | `crates/app/src/nginx.rs`、`handlers/nginx.rs` |
| WEB-03 | 非法参数拒绝且不落库 | `[上机]` 已登录 | 提交越界值（例如 worker=0、body=0、keepalive 超范围） | 303 回页面并显示内联错误；旧配置与旧值保持不变 | □通过 / □失败 | `crates/app/src/nginx.rs`（校验）、`handlers/nginx.rs` |
| WEB-04 | `nginx -t` 失败时配置回滚 | `[上机]` 有站点 vhost | 在站点配置页写入非法指令并保存；访问站点；检查 vhost 文件 | 303 回配置页并显示 `nginx -t` 错误；旧 vhost 保留；站点仍 200 | □通过 / □失败 | `crates/app/src/sites.rs`（配置保存）、`crates/app/src/nginx.rs` |
| WEB-05 | 日志按需加载与过滤 | `[上机]` 有访问日志 | 打开 `/$P/sites/web?tab=logs` 不带 `load`；再带 `load=1`；按 `q`、`status`、`lines` 过滤 | 不加载时只显示入口；加载后返回尾部日志；过滤只保留匹配行 | □通过 / □失败 | `crates/web/src/handlers/nginx.rs`、`templates/nginx.html` |
| WEB-06 | 模块列表 | `[上机]` nginx 运行中 | 打开 `/$P/sites/web?tab=modules` | 200；列出已编译模块；与 `nginx -V` 输出一致 | □通过 / □失败 | `crates/app/src/nginx.rs`、`handlers/nginx.rs` |
| WEB-07 | 站点日志增长与面板可读 | `[上机]` 已建站 | 连续访问站点 10 次；看 `/www/sites/<domain>/log/` 与 `/$P/sites/<domain>/logs` | access log 增长；面板站点日志页可读同一文件；无越权读取其他站日志 | □通过 / □失败 | `crates/app/src/sites.rs`、`handlers/sites.rs`、`/www/sites` |
| WEB-08 | 合法配置保存后 reload 生效 | `[上机]` nginx 运行中 | 在主配置页改一处合法指令（如 `server_tokens off;`）保存；刷新状态页；访问站点 | 303；状态页「最近 reload」时间更新；站点仍 200；`nginx -t` 输出变化 | □通过 / □失败 | `crates/app/src/nginx.rs`（`save_nginx_main`、`NGINX_LAST_RELOAD_KEY`）、`templates/nginx.html` |

最小回归命令：

```bash
curl -ks -o /dev/null -w 'web=%{http_code}\n' -b "$J" "$B/$P/sites/web"
curl -ks -o /dev/null -w 'logs_lazy=%{http_code}\n' -b "$J" "$B/$P/sites/web?tab=logs"
curl -ks -o /dev/null -w 'logs_load=%{http_code}\n' -b "$J" "$B/$P/sites/web?tab=logs&load=1&lines=50"
curl -ks -o /dev/null -w 'modules=%{http_code}\n' -b "$J" "$B/$P/sites/web?tab=modules"
```

## F. 文件系统原语

> 重跑触发：改了路径授权代数、原子写/路径锁、回收站、配额、审计分级、插件 `fs:*` 能力或管理员文件页。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| FS-01 | 插件只能读授权根内文件 | `[上机]` testkit 已装且授 `fs:read` + `path:` 根 | 调 testkit `fs_read` 读授权根内文件；读根外文件 | 根内 200 且返回内容；根外 400 并审计 `fs.read deny` | □通过 / □失败 | `crates/plugin/src/capability.rs`、`crates/model/src/path.rs` |
| FS-02 | 路径越界与符号链接逃逸 | `[本机+上机]` 单测 + 临时目录 | `cargo test -p skiff-fs --locked`；上机用 `../`、绝对路径、指向根外的软链测试 | 全部拒绝；规范化后仍在根内的路径才放行 | □通过 / □失败 | `crates/fs`、`crates/model/src/path.rs` |
| FS-03 | 原子写与并发路径锁 | `[本机]` 单测 | `cargo test -p skiff-fs --locked` | 写入走临时文件+rename；并发写同一路径不出现半文件；失败不留 `.tmp` | □通过 / □失败 | `crates/fs/src/atomic.rs`、`crates/fs/src/lock.rs` |
| FS-04 | 回收站移动/恢复/清理 | `[本机]` 单测；`[上机]` 管理员会话可打开 `/$P/files/trash` | `pwsh tools/gate.ps1 -Task test -Pkg skiff-fs`；上机删除文件后进回收站，恢复、彻底删除、过期清理 | 删除进回收站、可恢复、过期清理；恢复后原路径内容一致；父目录缺失/同名冲突有可读提示 | □通过 / □失败 | `crates/fs/src/trash.rs`、`crates/app/src/admin_fs.rs`、`crates/web/handlers/files.rs` |
| FS-05 | 全局与插件配额 | `[本机]` 单测；`[上机]` 管理员会话可打开 `/$P/files/quotas` | `pwsh tools/gate.ps1 -Task test -Pkg skiff-fs`；设置目录树字节/文件数上限后写入 | 超全局/插件配额被拒；目录树超限返回 `fs.quota_exceeded` 且消息含配额根与用量；回收站对象不计入目录树用量（恢复回补） | □通过 / □失败 | `crates/fs/src/quota.rs`、`crates/fs/src/ops/mod.rs`、`crates/app/src/admin_fs.rs` |
| FS-06 | 审计分级与目视筛选 | `[上机]` 已产生 fs 操作 | 打开 `/$P/audit`（页面只展示最近 200 条，无服务端筛选参数），在列表中找出 `fs.*` 与 result=deny/fail 行 | 每条含 actor、channel、action、target、result；能按文本找到 `fs.*` 与 deny/fail 条目 | □通过 / □失败 | `crates/plugin/src/capability.rs`、`handlers/settings.rs`、`templates/audit.html` |
| FS-07 | 内核文件管理页基本操作（第一步） | `[上机]` 管理员会话，`/$P/files` 可打开 | 浏览目录；打开文本并修改保存；新建目录/文件；重命名；删除；再打开 >256 KiB 文件 | 操作成功；大小/时间正确；>256 KiB 只读且保存禁用；删除二次确认；保存失败时编辑内容保留 | □通过 / □失败 | `crates/web/src/handlers/files.rs`、`crates/app/src/admin_fs.rs`、`static/files.js` |
| FS-08 | 数据根硬拒与权限位 | `[上机]` 已安装 | 用内核文件页尝试列/读/写 `/www/skiff/skiff.db`、`/www/skiff/plugins`、`/www/skiff/official`；检查数据根权限 | 即使管理员也返回 403 `fs.protected`；绕过软链仍拒绝；数据根与 plugins 为 0711；DB 文件非全局可写 | □通过 / □失败 | `crates/app/src/admin_fs.rs`、`crates/plugin/src/fsgrant.rs`、`deploy/install.sh`、`docs/DEPLOY_TEST.md` §1.1 |
| FS-09 | 旧深链 301 与插件菜单撤销 | `[上机]` `com.skiff.files` 已装 | 访问 `/$P/plugins/com.skiff.files/main`；查看「工具」菜单与全站导航 | 返回 301 且 `Location: /$P/files`；菜单中只有内核「文件管理」入口，无插件文件管理入口 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`、`crates/web/src/menu.rs`、`crates/web/src/handlers/mod.rs` |
| FS-10 | 回收站页面主链路（第二步） | `[上机]` 管理员会话，`/$P/files/trash` 可打开 | 在文件页删除文件/目录 → 打开回收站；恢复、故意制造同名冲突后覆盖恢复、彻底删除、清空 | 删除后进回收站且原路径消失；恢复后内容一致；冲突提示可读且需二次确认覆盖；清空二次确认并说明不可恢复；列表清空 | □通过 / □失败 | `crates/web/src/handlers/files.rs`、`crates/web/templates/fs_trash.html`、`static/fs_trash.js` |
| FS-11 | 配额页面与执行（第二步） | `[上机]` 管理员会话，`/$P/files/quotas` 可打开 | 设置目录树字节/文件数上限 → 在文件页写入超限；修改上限；取消上限；点「重新统计」 | 超限写入返回 `fs.quota_exceeded` 且含原因；页面显示已用/上限；取消后不再拦截且列表隐藏；重扫耗时提示出现 | □通过 / □失败 | `crates/web/src/handlers/files.rs`、`crates/web/templates/fs_quotas.html`、`static/fs_quotas.js`、`crates/app/src/admin_fs.rs` |
| FS-12 | 回收站/配额审计与受保护路径 | `[上机]` 已产生回收站/配额操作 | 删除、恢复、彻底删除、清空、设置/取消配额后打开 `/$P/audit`；再尝试把配额根设为数据根/`plugins/` | 审计出现 `fs.recycle.*` / `fs.quota.*` 且不含文件内容；受保护根被拒（403 `fs.denied/protected`） | □通过 / □失败 | `crates/fs/src/audit.rs`、`crates/app/src/admin_fs.rs`、`handlers/settings.rs` |
| FS-13 | 内核运行配置保护（加固） | `[本机]` 单测；`[上机]` 管理员会话，文件页可浏览 `/etc` | 在文件页对 `/etc/skiff/config.toml` 执行删除、改名、覆盖保存；再对 `/etc/nginx/nginx.conf` 执行同样操作；尝试把配额根设为 `/etc/skiff` | 配置/TLS 路径三类操作均 403 且提示「内核运行配置或数据目录受保护」；`/etc/nginx/nginx.conf` 仍可操作；配额设置/扫描对 `/etc/skiff` 均 403 | □通过 / □失败 | `crates/fs/src/path.rs`、`crates/fs/src/ops/mutate.rs`、`crates/app/src/admin_fs.rs` |
| FS-14 | 宿主只读/容量类 I/O 错误映射 | `[本机]` 单测；`[上机]` systemd `ProtectSystem` 生效，管理员会话可浏览 `/etc` | 本机 `cargo test -p skiff-fs -p skiff-plugin`；上机在文件页写保护区外只读路径（如 `/etc/skiff-iter-probe.txt`），有只读挂载时再写该挂载点；ENOSPC/EDQUOT 构造不出则如实标未实测 | `EROFS` → 403 `fs.read_only` 且文案含「只读」，不再 500；`EACCES`/`EPERM` → 403 `fs.denied`；`ENOSPC`/`EDQUOT` → 507 `fs.no_space`；未归类 I/O 仍 500；插件能力面同一口径 | □通过 / □失败 | `crates/fs/src/error.rs`、`crates/plugin/src/fsops.rs`、`crates/web/src/handlers/files.rs`、`static/files.js` |

最小回归命令：

```bash
cargo test -p skiff-fs --locked
cargo test -p skiff-plugin capability --locked
curl -ks -o /dev/null -w 'audit=%{http_code}\n' -b "$J" "$B/$P/audit"
curl -ks -o /dev/null -w 'files=%{http_code}\n' -b "$J" "$B/$P/plugins/com.skiff.files/main"
```

**2026-11-06 实测（测试机 10.10.15.4）**

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| FS-01 | ☑通过 | testkit `fs_read /proc/version` 200（bytes=213）；`fs_read /etc/passwd` 400，内核 403「路径不在已授权的 path: 根内」；审计行 `plugin:com.skiff.testkit | uds | fs.read | /etc/passwd | deny | 路径不在已授权的 path: 根内` |
| FS-02 | ☐部分通过（仅上机） | `/www/../etc`、`/proc/../etc/passwd`、根内指向 `/etc` 的目录/文件软链、经软链写穿透均 400/403；运行态判定实际在 `crates/plugin/src/fsgrant.rs`（canonicalize + 授权根），`skiff-fs` 未被任何 crate 依赖；本机 `cargo test -p skiff-fs` 未执行（开发机无 Rust 工具链） |
| FS-03 | ☐未执行 | 仅本机单测；开发机无 `cargo`/`rustup`；运行态写路径是 `crates/plugin/src/fsops.rs::write_chunk` 直接 `set_len(0)`+写，无临时文件/rename/路径锁 |
| FS-04 | ☐未执行 | 仅本机单测；运行时无回收站存储/入口（`/www/skiff` 下无 trash 目录），files 删除为永久删除 |
| FS-05 | ☐未执行 | 仅本机单测；运行时无配额配置/入口（`config.toml` 无 trash/quota 键） |
| FS-06 | ☑通过 | `/$P/audit` 200；列为 时间/操作者/第三列（管理员 IP 或插件 `uds`）/动作/对象/结果/详情；同一列表含 `fs.read` ok 与 deny 行、`plugin.rpc` 行 |
| FS-07 | ☑本机集成通过（2026-11-07） | `files_page_browses_edits_and_redirects_legacy_link` 覆盖页面渲染、目录列表、读文本、原子保存、数据根 403 与旧深链 301；`crates/app/src/admin_fs.rs` 三个单测覆盖往返读写/数据根硬拒/超限只读。真实浏览器与上机复核待发版 | 发版后按上表 FS-07/08/09 上机复验；重点看保存失败提示与编辑内容保留 |
| FS-08 | ☑本机集成通过（2026-11-07，内核文件页） | 旧结论（files 插件按 `path:/www` 可写数据根）已被内核接管取代：`AdminFs::guard` 对 `data/`、`data/plugins`、`data/official` 一律 403，集成测试断言列 `data_dir/plugins` 返回 403；Unix 软链绕回单测同样拒绝。上机权限位复核仍待发版 | 发版后按 FS-08 上机复验：管理员在 `/files` 打开数据根必须 403；软链逃逸仍拒绝；数据根 0711/DB 0600 不变 |

> 稳定性：开跑前后 6 次探活（`/$P/` 307、`/api/v1/docker/containers` 401）一致，`NRestarts=0`，未遇到重启/502；测试期间 `monitor` 被并行会话停用，不影响 F 章判定。

> 2026-11-07 补充（FS-07 编辑器关闭回归）：`plugins/files` 编辑器弹窗的关闭按钮 / Esc / 点遮罩三条路径全部失效。根因是 `.editor-dialog` 上的作者样式 `display:flex` 盖掉了 UA 的 `dialog:not([open]) { display:none }`，`close()` 只移除 `open` 属性，窗口整块留在页面上。修复为 `display` 只挂 `.editor-dialog[open]`、补 `dialog:not([open]) { display:none !important }`、遮罩点击接入 `requestCloseEditor`（未保存内容先确认）、关闭后焦点回列表、确认框防重复 `showModal`。本地复现台（同 CSP + mock RPC，真浏览器）已验证：关闭按钮 / Esc / 遮罩均能收起、未保存内容先确认、保存后重开内容持久、>256 KiB 只读且禁用保存；测试机面板内复验待发版（浏览器工具无法越过自签证书）。

## G. 调度器

> 重跑触发：改了任务模型、内置任务、站点备份开关、手动触发、超时/重试/并发、内存缓冲/回执或插件唤醒链路。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| TASK-01 | 任务列表与详情状态 | `[上机]` 已登录 | 打开 `/$P/tasks`；点任一任务详情 | 列表 200 且显示 kind/source/调度/状态/下次运行；详情 200 且历史可见 | □通过 / □失败 | `crates/web/src/handlers/tasks.rs`、`templates/tasks.html` |
| TASK-02 | 内置任务标记与删除入口 | `[上机]` 新库或已有内置任务 | 打开列表找 `builtin:db-vacuum`；检查是否有删除按钮；如需可再直接 POST `/$P/tasks/builtin:db-vacuum/delete` 观察 | 列表可见并标「内置」；模板不为内置任务渲染删除按钮。注：服务端 `delete_task` 当前未拦截 builtin，直接 POST 仍会删除（如要硬保护需另开事项） | □通过 / □失败 | `crates/app/src/tasks.rs`、`crates/web/src/handlers/tasks.rs`、`templates/tasks.html` |
| TASK-03 | 站点备份开关 | `[上机]` 有站点 | 站点详情打开备份开关；关闭；刷新站点页与任务页 | 开关 303；启用后出现 `builtin:site-backup:<domain>`（每日 03:00），关闭后任务保留但 `enabled=false`；站点页开关与任务页状态一致；无「保留天数」参数 | □通过 / □失败 | `crates/app/src/tasks.rs`（`set_site_backup`）、`templates/site_detail.html` |
| TASK-04 | 手动触发并写历史 | `[上机]` 任一启用任务 | 点「立即执行」；刷新详情；看 `task_runs` | 303；出现 running 记录；完成后 success/failed 与耗时、输出摘要可见 | □通过 / □失败 | `crates/app/src/tasks.rs`、`crates/sched` |
| TASK-05 | 超时、重试与退避 | `[本机]` 调度器单测 | `cargo test -p skiff-sched timeout --locked`；`cargo test -p skiff-sched backoff --locked` | 超时被取消并记 timeout；按重试次数与退避间隔重跑；最终状态正确 | □通过 / □失败 | `crates/sched/src/scheduler.rs`、`docs/KERNEL_SCHEDULER_CONTRACT.md` |
| TASK-06 | 缓冲回执、统计与限速 | `[本机]` app 单测；`[上机]` testkit 已装并授 `scheduler:read` + `scheduler:manage` | 调 testkit `buffer_probe {"id":"plugin:com.skiff.testkit:busy-probe"}`；`buffer_push {"id":"…","count":N}`（N≤100，溢出用大 `point` 或超容量）；再 `buffer_probe` 对比；`cargo test -p skiff-app buffer --locked` | `buffer_probe` 原样回报 `points/bytes/capacity_bytes/seq/dropped_*/rejected_*/high_watermark`；`buffer_push` 汇总 `accepted/rejected/dropped_*/signals`，回执字段与内核一致；每任务 100 点/秒限速生效；`count>100` 本地 -32602；无 `manage` 时 push 被拒。注：内核只为声明 `sample_method` 的任务建缓冲，`busy-probe` 已声明（容量 4096 B、默认停用不自动采样） | □通过 / □失败 | `plugins/testkit`、`crates/app/src/tasks.rs`、`crates/sched/src/buffer.rs` |
| TASK-07 | 插件任务唤醒 | `[本机]` sched 单测；`[已核对]` 有入口 | `cargo test -p skiff-sched plugin --locked`；cron 新建生成 `plugin:com.skiff.cron:<key>`、`source=plugin:<target>` | 插件任务按调度唤醒、超时/失败回执；插件不可用时记 blocked 而非 failed | □通过 / □失败 | `crates/core/src/server.rs`、`crates/sched`、`crates/app/src/tasks.rs` |
| TASK-08 | 任务页权限与 CSRF | `[上机]` viewer 与 admin 各一 | viewer 打开任务页；尝试启停/立即执行/删除；admin 不带 CSRF 执行 | viewer 只读；写操作 403；admin 无 CSRF 403，带 CSRF 303 | □通过 / □失败 | `crates/web/src/handlers/tasks.rs`、`crates/model/src/rbac.rs` |
| TASK-09 | 调度预设选择与编辑反解 | `[本机]` `skiff-cron` 单测；`[上机]` 已装 cron 插件 | 本机跑 `cargo test -p skiff-cron --locked`；列表页右上角「新建任务」弹窗逐个选预设并核对表达式/人话预览；保存后在列表「编辑」逐项确认回显；自定义表达式填非法值 | 6 类预设各自生成标准 5 段表达式且预览为人话；编辑时预设能回显成对应表单；匹配不到预设落「自定义」并保留原表达式；非法表达式有明确报错且拒绝保存；界面标注本地时区、不支持秒级与 `@reboot` | □通过 / □失败 | `crates/cron/src/preset.rs`、`plugins/cron/src/main.rs`、`plugins/cron/ui/app.js` |

最小回归命令：

```bash
cargo test -p skiff-sched --locked
cargo test -p skiff-app tasks --locked
curl -ks -o /dev/null -w 'tasks=%{http_code}\n' -b "$J" "$B/$P/tasks"
curl -ks -o /dev/null -w 'task_detail=%{http_code}\n' -b "$J" "$B/$P/tasks/builtin:db-vacuum"
```

**2026-11-06 实测（测试机 10.10.15.4）**

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| TASK-01 | ☑通过 | 列表 200，`builtin:db-vacuum` 显示 kind/source/调度/状态/下次运行；详情历史可见 |
| TASK-02 | ☑通过（待发版） | 列表标「内置」；模板不渲染删除按钮；内核已补服务端硬拦（`Scheduler::delete` 拒 `builtin:`，能力面 `task.delete` 同拒），发版后按台账复验 RPC 删除 → 403 |
| TASK-03 | ☑通过 | 开关 303；`scheduled_tasks` 行 `id=builtin:site-backup:matrix.local`、`kind=builtin:site-backup`、`source=site:matrix.local`、`schedule_json={"type":"cron","expr":"0 3 * * *","tz":...}`、`payload={"domain":"matrix.local"}`、`enabled=1`；关闭后 `enabled=0` 且 8 条历史保留；重开恢复 `enabled=1`。环境无 OpenResty，站点为库内夹具，未触发真实 vhost |
| TASK-04 | ☑通过 | 手动触发 303；`task_runs` 落 `trigger=manual`/`result=ok`（站点备份产出 tar）；插件任务手动触发同样落 `trigger=manual` |
| TASK-05 | ☑通过（本机） | `cargo test -p skiff-sched --locked` 45 项通过：超时取消、退避重试、单飞、`queue_one`、冷却期、错过策略、`retain_across_ticks=false` |
| TASK-06 | ☑通过（上机，2026-11-09） | 官方包 `0c99464` 上机：`buffer_probe` 基线 `points=0/capacity_bytes=4096`；`buffer_push count=100` → `accepted=100, dropped_points=17, dropped_bytes=833, signals=[high_watermark,overflow]`；再 probe `points=83, bytes=4067, seq=100, head_seq=17, tail_seq=99, high_watermark=true`；同秒再推 1 点被 400 拒（每任务 100 点/秒限速）；`count=101` 本地 `-32602`；70KB 大点 `accepted=false, bytes=70043, reason=point_too_large`。`rejected_points/rejected_bytes` 未能构造：超限点与限速在入缓冲前即被拒，统计恒 0。旧包（无 `manage`）`buffer_push` 403「缺少 scheduler:manage」已验 |
| TASK-07 | ☑部分通过 | `cron` `jobs.list/toggle/run` 200；`jobs.remove` 200 且行删除（`{"removed":true}`）；内核任务详情/历史页 200；插件任务按调度唤醒、失败回执 `result=fail`/`error_class=executor_failed`、手动触发均通过；`cron` 代他人注册被 `source != owner` 拒，修复（`source=owner`）已在本机待发版，见台账 |
| TASK-08 | ☑通过 | viewer 只读；写操作 403；admin 无 CSRF 403、带 CSRF 303 |
| TASK-09 | ☑通过（2026-11-07） | `skiff-cron` 14 项单测（预设→表达式、越界拒绝、往返反解、等价写法、自定义回退、描述文案）全绿；测试机 10.10.15.4 真浏览器 41 项断言全过：6 类预设分别落 `*/5 * * * *`、`0 */2 * * *`、`0 3 * * *`、`30 9 * * 1,3,5`、`0 4 1 * *`、`0 9 * * 1-5` 且预览为人话；编辑「改调度」逐项回显对应表单；自定义 `0,30 8-18 * * 1-5` 原样保留；非法 `0 25 * * *` 在新建与编辑两处均报错并拒绝保存；验收任务已清理。截图 `dist/iter/shots/{list,edit-card}.png` |

**调度器补充实测（本轮）**

| 项 | 结果 | 证据 |
| --- | --- | --- |
| 插件门三态 | ☑通过 | 未安装 `NotInstalled`、禁用 `Disabled`、`api=1.1` 且 `plugins.api=1.1` 时 `ApiMismatch`；均只写 `task.blocked` 审计，不产生 failed 运行 |
| 执行期回调 scheduler | ☐未上机 | testkit 已提供 `busy_probe`（默认停用的 `busy-probe` 任务，启用后「立即执行」即构造执行期回调），待上机复验；代码核对 `crates/core/src/kernel/capability.rs:134-141`：`is_interop_busy || tasks.is_executing` → 403 + `scheduler.deny` |
| 超时晚到结果 | ☑通过（本机） | `timeout_enters_sixty_second_cooldown` 新增断言：`timeout` 结算后 `finish(同一 run_id)` 返回 `Protocol`，任务保持 `Failed`，终态不被覆盖；上机未单独构造晚到回执 |
| 缓冲能力面 | ☑通过（上机，2026-11-09） | 容量/条数/字节/丢弃/高水位/seq 全对上：4096 B 容量下 100 点推入后保留 83 点/4067 B、丢 17 点/833 B（drop-oldest，单点约 49 B）；限速与本地 `count` 上限、无 `manage` 403 均通过。`rejected_*` 统计无法构造（拒绝发生在入缓冲前） |
| monitor / guard 迁移 | ☐未上机（代码完成） | manifest 改 `[[task]]`，内核启动/安装种入 `plugin:com.skiff.monitor:sample` 与 `plugin:com.skiff.guard:reconcile`；`run_cron` 已删，插件无 `tick`。上机断言：`scheduled_tasks` 两条、source 前缀正确、无 `plugin.cron_deprecated` 审计、重装不重复、卸载任务消失 |
| monitor 采样/落盘 | ☐未上机 | 内核每 60s 调 `sample`（只更新 `prev.json`），每 5 分钟调 `execute` 追加 `samples.jsonl`（一行一点，超 480 行压缩回 240 行）并回 `persist`；写盘失败时缓冲保留、下一轮重试；`snapshot` 返回 `scheduler.driver=kernel`，基线独立写 `prev_ui.json`，不截断采样窗口 |
| monitor 网络速率口径 | ☑通过（2026-11-09，官方包 `0c99464`） | 全速窗口：`/proc/net/dev` tx 差 210,657,428 B / 10 s → 期望 21,065,742.8 B/s，插件 `net_tx_rate=21,065,742.8 B/s`（差 0.00%，`net_tx` 与宿主逐字节一致）。限速长窗口 106 s（200 MB，`scp -l 16384`）：整窗均值 1,985,505 B/s；面板 UI 每约 60 s 自身的 `snapshot` 轮询把末段窗口截为 54 s，插件报 1,943,259.9 B/s，对同 54 s 期望差 0.00%、对整窗均值差 2.13%。同窗内内核 60 s 采样推进 `prev.json`（3060→3120），`prev_ui.json` 未被内核改写——两份基线独立，内核采样不截断 UI 窗口 |
| guard 单路径与语义 | ☐未上机 | 每分钟 `reconcile`；异常容器重启/冷却/小时预算/未授权只报不动与迁移前一致；UI 无「立即巡检」入口；`status.scheduler.driver=kernel` |
| manifest 任务升级刷新 | ☑通过（本机 + 上机，2026-11-09） | 本机 `cargo test -p skiff-app manifest_task --locked`：`*/5`→`*/10`、`execute`→`run`、采样 60→120、超时 30→45 后任务行同步更新；管理员停用的 `enabled=0`、`created_by=system:manifest`、`created_at` 与 `task_runs` 历史全部保留；撤掉声明后任务行被回收。上机（`ef7058b`，真实 0.1.0→0.2.0 签名包）：`expr` `* * * * *`→`*/2 * * * *`、采样 3600→1800、容量 4096→8192、超时 10→15 全部落库；`enabled=1`、`created_by`、`created_at=1794172700` 与 2 条运行历史保留 |
| 升级补齐新增授权 | ☑通过（本机 + 上机，2026-11-09） | 本机 `cargo test -p skiff-plugin upgrade_ --locked`：0.1.0→0.2.0 新声明 `docker:restart`/`docker:inspect` 后 grants 补齐，敏感档写审计 `plugin.grant_upgrade`（detail 含动词与 `0.1.0 → 0.2.0`）；只读新增不写该审计；未再声明的 perm 被过滤；同版本重装保留既有授权。上机：grants 7→8 补 `fs:write`，审计 detail = `升级新增授权: fs:write；版本 0.1.0 → 0.2.0（敏感: fs:write）`；卸载重装 0.1.0 后回到 7（`fs:write` 被过滤）。「用户未勾选项不翻案」无 CLI 拒绝交互，构造不出，仅本机单测覆盖 |
| 卸载回收插件任务 | ☑通过（上机，2026-11-09） | `skiff plugin remove com.skiff.testkit` 输出「已卸载（delete_data=false，回收任务 1 条）」，`scheduled_tasks` 中 `source=plugin:com.skiff.testkit` 行立即消失、重启不再出现；`task_runs` 历史按设计保留；重装 0.1.0 后按 manifest 重新种入（`enabled=0`、`created_by=system:manifest`、新 `created_at`） |

## H. Docker

> 重跑触发：改了容器页/API、日志 SSE、exec/WS、镜像、网络/卷、受保护容器、能力面策略或 Compose 子集。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| DOCKER-01 | 容器列表与异常过滤 | `[上机]` Docker 可用 | 打开 `/$P/containers`；打开 `?filter=error`；对比 `docker ps` | 列表 200 且数量一致；异常过滤只返回异常容器；无异常时显示空态 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`、`crates/docker` |
| DOCKER-02 | 容器详情与不存在/非法参数 | `[上机]` 有容器 | 打开详情；请求不存在 id；请求非法动词 | 详情 200；不存在 404；非法动词 400 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`、`crates/docker/src` |
| DOCKER-03 | 启停/重启/删除 | `[上机]` 一个可丢弃容器 | 依次 start、stop、restart；再 POST remove；对受保护容器 POST remove；对执行中任务的运行环境容器 POST remove；每步 `docker ps` 对照 | start/stop/restart/remove 均 303 且状态与 `docker ps` 一致；remove 后容器消失；受保护容器 remove 403；任务占用 remove 409；未知动作仍 400。`[已复验 2026-11-07，f9a0b30；该资产已被同日最终版覆盖，当前资产见 TEST_ENV §6]` 见下方「Docker 复验」表 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`、`crates/app/src/sites.rs`、`crates/docker` |
| DOCKER-04 | 日志 SSE 与断连 | `[上机]` 有持续输出的容器 | 打开日志页；`curl -N` 订阅 SSE；关闭客户端 | SSE 持续返回 `data:` 行；断开后服务端停止 follow，无泄漏 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`、`crates/docker` |
| DOCKER-05 | exec 一次性票据与 Origin | `[上机]` 容器可执行 sh | 申请 exec 票据；用票据连 WS；重复用同一票据；异源 Origin/无 Origin 再连；过期票据再连；浏览器打开详情页在终端输入命令 | 首次可连；重复票据被拒；异源/无 Origin 403；过期票据被拒；终端可输入并有回显。`[已复验 2026-11-07]` 详情页加载 xterm；`echo ok` 回显；票据一次性/异源/无 Origin/过期均 403 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`、`crates/docker/src/client.rs`、`templates/container_detail.html` |
| DOCKER-06 | 镜像列表/拉取/删除 | `[上机]` 可访问镜像源 | 拉取一个小镜像；列表出现；删除；再拉 | 拉取返回 200 且结果/错误可见；列表与 `docker images` 一致；删除后消失；再拉成功 | □通过 / □失败 | `crates/web/src/handlers/resources.rs`、`crates/docker` |
| DOCKER-07 | 网络与卷创建/删除 | `[上机]` Docker 可用 | 创建 bridge 网络、volume；挂载到临时容器；删除 | 创建 200（页面刷新，非 303）；列表出现；挂载可用；删除后列表消失；使用中的卷删除被拒 | □通过 / □失败 | `crates/web/src/handlers/resources.rs`、`crates/docker` |
| DOCKER-08 | 受保护容器不可被面板操作 | `[上机]` 新版本重装/重建后的内核容器（`skiff-web-nginx`、`skiff-php-*`、`skiff-rust-*`） | 查看三个容器 Labels；对每个执行 stop/restart/remove；再看一个旧版未打标签的内核容器列表标签 | 新容器均带 `io.skiff.project=sites` 与 `io.skiff.protected=true`；stop/restart/remove 一律 403 且容器仍运行；存量未打标签容器在页面显示「存量未受保护」，重建后标签生效。`[已复验 2026-11-07，f9a0b30；该资产已被同日最终版覆盖，当前资产见 TEST_ENV §6]` 见下方「Docker 复验」表 | □通过 / □失败 | `crates/app/src/sites.rs`（`skiff_labels`）、`crates/policy/src/policy.rs`（`Target::is_protected`）、`crates/docker/src/client.rs`（`unprotected_kernel`） |
| DOCKER-09 | 危险字段无法表达 | `[本机]` 模型/策略单测 | `cargo test -p skiff-policy --locked`；构造含 `privileged`/`cap_add` 的 JSON 请求 | 反序列化拒绝或校验拒绝；容器规格只允许契约字段 | □通过 / □失败 | `crates/model/src/container.rs`、`crates/policy` |
| DOCKER-10 | Compose 子集解析校验 | `[本机]` 仓库根 | `cargo run -p xtask -- check`（`check-1panel` + `check-compose`）；再对一份含不支持字段的 compose 跑 `check-compose`；`cargo test -p skiff-compose` | 样例全部通过；含不支持字段的 compose 被拒；解析器单测覆盖拓扑序、卷/网络前缀与标签不注入 | □通过 / □失败 | `xtask`、`crates/compose`、`deploy/compose` |
| DOCKER-11 | 网络页显示子网/网关/IPAM/内外部/容器挂载 | `[上机]` Docker 可用 | 打开 `/$P/networks`；对照 `docker network inspect <name>`；打开一个网络的容器明细；停掉 Docker 或让它返回错误再打开页面 | 列表显示每个网络的 Subnet、Gateway、IPAM 驱动、internal/attachable、容器数量；展开 `details` 能看到容器名/IPv4/IPv6/MAC；取不到的数据显示「未知」，**不显示 0 或空白**；Docker 不可用时页面给出错误而不是空列表 | □通过 / □失败 | `crates/docker/src/client.rs`（`list_networks` / `network_attachments`）、`crates/web/src/handlers/resources.rs`、`templates/networks.html` |
| DOCKER-12 | 创建容器表单与受限形态拒绝 | `[本机]` 表单解析单测；`[上机]` 一个可丢弃镜像 | 本机：`cargo test -p skiff-web spec_from_form --locked`；上机：打开 `/$P/containers/new` 填镜像/名称/端口/环境变量/卷/网络/重启策略提交；再以 viewer 打开该页；在 1Panel Docker 受限形态（`Tier::Basic`）用 admin 打开该页 | 页面 200 且各字段齐全；提交 303 跳详情且 `docker ps` 出现该容器、端口/环境/挂载一致；viewer 403；Basic 档 admin 也 403（能力面收缩）；镜像为空/端口越界/环境变量无 `=` 一律 400 并保留表单 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`（`new_page`/`create`/`parse_ports`/`parse_env`/`parse_mounts`/`spec_from_form`）、`templates/container_new.html`、`crates/policy/src/policy.rs` |
| DOCKER-13 | 资源限制热更新与生效值 | `[本机]` 策略/模型单测；`[上机]` 一个可丢弃容器 | 本机：`cargo test -p skiff-policy resource_limits --locked`；上机：详情页改 CPU 份额/配额/内存/内存+交换/`pids`，提交后 `docker inspect` 对照；填非法值（内存 <6MB、交换 < 内存、端口越界等）；把字段清空再提交 | 更新 303 且 `docker inspect` 的 `HostConfig` 与表单一致；详情页「当前生效值」同步刷新；非法值 400 并保留表单；清空按 0 = 清除上限；镜像/命令等结构项在页面上注明「需重建」，无输入入口；`pids` 生效范围标「待实测」 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`（`update_limits`/`limits_from_form`）、`crates/docker/src/client.rs`（`update_container`）、`crates/policy/src/policy.rs`（`validate_limits`）、`templates/container_detail.html` |
| DOCKER-14 | Compose 主链路：校验→部署→状态/日志→down | `[本机]` handler/解析单测；`[上机]` Docker 可用 | 本机：`cargo test -p skiff-compose --locked`、`cargo test -p skiff-web compose --locked`；上机：`/$P/compose` 粘贴一份两服务 compose（含 `depends_on` 与命名卷）→ 仅校验 → 部署；观察服务表与日志；打开容器列表看「compose · 项目」归属；停止/重启项目；`down` 先不勾删除卷，再勾删除卷 | 校验通过并回显计划（项目名/服务/容器名）；部署按拓扑序创建容器并启动，报告逐服务状态；服务表显示状态与最近日志；列表归属来自标签（内核 `io.skiff.project` 优先，回落 `com.docker.compose.project`）；`down` 二次确认后容器全部删除、网络前缀清理，未勾删除卷时卷保留并在报告列出，勾选后卷删除；部分失败不判整体成功；`sites`/`skiff` 项目名被拒；viewer 看不到写操作按钮 | □通过 / □失败 | `crates/compose/src/lib.rs`、`crates/docker/src/compose_run.rs`、`crates/web/src/handlers/compose.rs`、`crates/web/src/menu.rs`（`core/docker/compose`）、`templates/compose.html` |

最小回归命令：

```bash
curl -ks -o /dev/null -w 'containers=%{http_code}\n' -b "$J" "$B/$P/containers"
curl -ks -o /dev/null -w 'containers_error=%{http_code}\n' -b "$J" "$B/$P/containers?filter=error"
curl -ks -o /dev/null -w 'images=%{http_code}\n' -b "$J" "$B/$P/images"
curl -ks -o /dev/null -w 'networks=%{http_code}\n' -b "$J" "$B/$P/networks"
cargo test -p skiff-policy container_remove --locked
cargo test -p skiff-docker stale_kernel --locked
cargo test -p skiff-app container_busy_reason --locked
cargo test -p skiff-compose --locked
cargo test -p skiff-web docker_create_and_compose --locked
cargo run -p xtask -- check
```

**Docker 补充实测（2026-11-07 历史轮，包 `v0.1.0` / `3e98b5e`，x86_64 sha256 `c39fe7d2…`；最终资产见 [TEST_ENV.md](TEST_ENV.md) §6）**

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| DOCKER-01 | ☑通过 | `/containers` 200，3 个容器名全部命中；`?filter=error` 200 且空态「没有处于异常状态的容器。」 |
| DOCKER-02 | ☑通过 | 详情 200；不存在 404；非法动词 400「未知动作: nope」 |
| DOCKER-03 | ☑通过（remove 缺口） | stop/start/restart 303 且 `docker inspect` 状态随动；remove 400「未知动作: remove」 |
| DOCKER-04 | ☑通过 | `curl -N` 收到 76 行 `data:`；断开后无残留连接 |
| DOCKER-05 | ☑通过 | 合法票据 101 且 `echo ok` 回显帧含 `ok\r\n`；无票据/坏票据/复用/异源/无 Origin/过期均 403，文案分别为「缺少 exec 票据」「票据校验失败」「WebSocket Origin 校验失败」「票据已过期」；详情页已加载 xterm |
| DOCKER-06 | ☑通过 | `busybox:latest` 拉取 200「已拉取」，列表出现；删除后面板与 `docker images` 均消失 |
| DOCKER-07 | ☑通过（实际 200） | 创建网络/卷返回 200 并实际落库 Docker（原写 303 为页面刷新语义）；挂载临时容器可用；使用中卷删除被拒（`in use - [id]`），清理后删除成功 |
| DOCKER-08 | ☑通过（发现新缺口） | 合成 `io.skiff.protected=true` 容器 stop/restart 403「受保护对象 … 拒绝操作」且保持 running；remove 仍 400（DOCKER-03 缺口）。**新缺口**：运行时 `skiff-web-nginx`/`skiff-php-batch1`/`skiff-rust-rust-1.83` 未带该标签、名称也不匹配 `skiff`/`skiff_`，实测可被面板停止 |
| DOCKER-09 | ☑通过（本机） | `cargo test -p skiff-policy` 在闸门内串行通过：危险字段反序列化拒绝、规格白名单校验均绿 |
| DOCKER-10 | ☑通过（本机解析） | `cargo test -p skiff-compose` 通过；运行时入口已从无到有（`/$P/compose`），`xtask check` 的字段子集校验仍保留 |

**Docker 复验（2026-11-07 发版轮，包 `v0.1.0` / `f9a0b30`，x86_64 sha256 `87427aa6…`，测试机 10.10.15.4；该资产已被同日最终版覆盖，当前资产见 [TEST_ENV.md](TEST_ENV.md) §6）**

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| DOCKER-08（存量标注） | ☑通过 | 部署后未重建时：列表页「存量未受保护」3 处（三容器各一），详情页各 1 处；`docker inspect` 三容器均无 `io.skiff.protected` |
| DOCKER-08（重建后标签） | ☑通过 | Web 服务 + `batch1` + `rust-rust-1.83` 重建后 `protected=true` ×3；列表「受保护」3 处、「存量未受保护」0；详情各 1 处且无存量字样；`audit-env.local` / `audit-rust.local` 均 200 |
| DOCKER-08（面板拒操作） | ☑通过 | 三容器 stop、restart 共 6 次请求全部 403，`docker ps` 保持 `Up` |
| DOCKER-03（删除入口） | ☑通过 | 可丢弃容器 `remove` 303（回列表）且 `docker ps -a` 消失；三受保护容器 `remove` 均 403；临时运行环境在 `imagick` 安装任务 running 时 `remove` 409 且容器保持运行；未知动作 400（前轮已验） |
| DOCKER-03/08 备注 | — | 409 分支只在「存量未受保护容器 + 进行中任务」可达：受保护容器先被策略判 403。本轮用临时 PHP 运行环境容器（带 `io.skiff.project=sites`、无 `protected`）复现该路径，面板列表亦标注「存量未受保护」；测后已删运行时记录与容器 |

## I. 部署与升级

> 重跑触发：改了安装/卸载脚本、systemd unit、配置解析、TLS/反代信任、数据目录权限、数据库迁移、发行包或升级流程。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| DEPLOY-01 | 发行包结构与摘要/签名 | `[本机]` 有 release 包 | 解包检查 `skiff`、`skiff-plugin`、`install.sh`、`uninstall.sh`、`docker-bootstrap.sh`、`skiff.service`、`config.toml.example`、`vendor/install-php-extensions`、`official/*.wbpkg` 与 `public-key.txt`；`cargo run -p xtask -- verify-pkg …`；`sha256sum -c` | 结构完整；单包 `.sha256` 校验通过；缺失或篡改被拒 | □通过 / □失败 | `deploy/pack-release.sh`、`xtask`、`.github/workflows/release.yml` |
| DEPLOY-02 | systemd 直装与自启 | `[上机]` 干净 Linux | `deploy/install.sh`；`systemctl status skiff`；重启机器 | 安装成功；服务 active；重启后自动拉起；面板 200。`[已复验 2026-11-07]` 重启后自启、三容器 Up、面板/两站 200 | □通过 / □失败 | `deploy/install.sh`、`deploy/systemd/skiff.service` |
| DEPLOY-03 | 数据目录与权限口径 | `[上机]` 已安装 | 查看 `/etc/skiff/config.toml`、`/www/skiff`、`/www/skiff/plugins`、`/www/skiff/skiff.db` | 数据根与 plugins 0711；DB 0600；面板自身配置 `/etc/skiff/config.toml` 由安装脚本落为 0644（非 0600）；目录属主正确 | □通过 / □失败 | `deploy/install.sh`、`crates/core/src/bootstrap.rs`、`docs/DEPLOY_TEST.md` §1.1 |
| DEPLOY-04 | 配置严格校验与非回环无 TLS 拒绝 | `[上机]` 可改配置 | 写未知字段启动；监听 `0.0.0.0` 且不配 TLS 启动；恢复合法配置 | 未知字段启动失败并指出字段；非回环无 TLS 拒绝启动；合法配置恢复 200 | □通过 / □失败 | `crates/core/src/config.rs`、`deploy/config.toml.example` |
| DEPLOY-05 | 直连 TLS | `[上机]` 有证书 | 配 `tls_cert`、`tls_key` 指向证书；重启；访问 HTTPS；用 http 访问同一端口 | HTTPS 200 且证书正确；明文 HTTP 无法完成 TLS 握手（不会降级为明文）；面板 Cookie 带 Secure | □通过 / □失败 | `crates/core/src/config.rs`、`crates/core/src/server.rs`（TLS 监听）、`deploy/install.sh` |
| DEPLOY-06 | 反代信任边界 | `[上机]` 前置反代 | 配 `trusted_proxies` 为反代 CIDR；从可信/不可信来源分别带 `X-Forwarded-Proto`、`X-Forwarded-For` 访问，看审计里的来源 IP 与 scheme | 可信来源的头被采信，来源 IP 为最左真实客户端；不可信来源的头一律忽略，来源 IP 为直连 peer；伪造头不改变客户端识别 | □通过 / □失败 | `crates/core/src/config.rs`、`crates/web/src/middleware.rs` |
| DEPLOY-07 | 升级覆盖安装与数据保留 | `[上机]` 已有库与站点 | 备份旧二进制与 DB；覆盖安装新 release；重启；登录看站点/任务/插件 | 新版本启动；`user_version` 升到当前 schema（15）；站点、任务、插件数据保留；已装官方插件的 `versions/<ver>` 与随包内容一致（版本或内容不同则更新，`data/` 与既有授权不变）。`[已复验 2026-11-07]` 同包覆盖安装，配置 sha 不变、业务计数保留 | □通过 / □失败 | `deploy/install.sh`、`crates/store/src/db.rs`（迁移） |
| DEPLOY-08 | 卸载与 purge 边界 | `[上机]` 可丢弃环境 | `deploy/uninstall.sh`；再加 `--purge`；检查数据目录 | 默认卸载保留 `/www/skiff` 数据；`--purge` 明确删除数据与配置并提示。`[已复验 2026-11-07]` 默认卸载删程序保数据（`/www/skiff`、`/etc/skiff`、`/www/sites`），重装恢复一致 | □通过 / □失败 | `deploy/uninstall.sh` |
| DEPLOY-09 | aarch64 装机 | `[上机]` arm64 Linux | 安装 release arm64 包；起 systemd；建站访问 | 安装/启动/建站均成功；`uname -m` 为 aarch64。`[部分]` 无 arm64 实机，仅 qemu-user 冒烟：`file` = ARM aarch64 静态链接、`ldd` = `not a dynamic executable`、`skiff version/--help/doctor` 正常（2026-11-09，`2f63aeb`）；装机未验 | □通过 / □失败 | `deploy/pack-release.sh`、`.github/workflows/release.yml` |
| DEPLOY-10 | 1Panel Docker 受限形态 | `[上机]` 1Panel 环境 | 按商店包安装；尝试安装 fs/net 插件；打开三核心 | 面板可用；fs/net 插件因能力面收缩而拒装；docker/identity 正常。`[部分通过]` 2026-11-09 用等价镜像（`alpine:3.23` + 发布包 `skiff`，`SKIFF_FORM=docker`）实测：`/healthz` 200、登录 303、`files/monitor/guard/testkit` 400「需要能力 fs」、`ftp` 400「需要能力 ftp」、`cron` 400「要求完整沙箱」、`/containers/rows` 200 + `stop/start` 303；未在真实 1Panel 商店流程内装。**文件管理已按能力面收口（`607d09b` 复验）**：`core/files`、`core/files/trash`、`core/files/quotas` 入口不渲染，`/{p}/files*`、`/{p}/m/core/files*` 直达均 403「当前部署形态不提供文件管理能力」；回归 `crates/web/tests/main_flow.rs::restricted_form_hides_and_rejects_files_management` | □通过 / □失败 | `deploy/1panel/`、`crates/plugin`、`crates/web/src/middleware.rs`（`check_fs_capability`） |
| DEPLOY-11 | Release 下载失败可诊断 | `[上机]` 外网下载 | 正常下载一次；模拟超时/断网再下载 | 正常成功且校验摘要；失败时保留明确 URL/状态码/原因并有限重试。`[阻塞]` 无可复现超时/断网条件；代码核对 `tools/iter.sh::cmd_fetch` 为单次 `curl --max-time 600`，无重试与结构化错误输出 | □通过 / □失败 | `tools/iter.sh`、`.github/workflows/release.yml` |

最小回归命令：

```bash
sudo systemctl is-active skiff
curl -ks -o /dev/null -w 'panel=%{http_code}\n' "$B/$P/login"
stat -c '%a %U:%G %n' /www/skiff /www/skiff/plugins /www/skiff/skiff.db /etc/skiff/config.toml
sqlite3 /www/skiff/skiff.db 'PRAGMA user_version;'
sudo journalctl -u skiff -n 50 --no-pager | grep -E '安全入口|listen|error' | tail -5
```

**部署补充实测（本轮 2026-11-07）**

| 编号 | 结果 | 证据 / 阻塞条件 |
| --- | --- | --- |
| DEPLOY-02 | ☑通过 | 重启后 `skiff.service` enabled+active、`NRestarts=0`，三容器 Up，面板与两站 200；开机后约 4s 服务就绪、约 10s 面板可用 |
| DEPLOY-07 | ☑通过 | 同包覆盖安装不清库：配置 sha 不变，users/sites/grants/plugins/tasks 计数全保留，`integrity=ok` |
| DEPLOY-08 | ☑通过 | 默认卸载删除二进制/unit/`/run/skiff`，保留 `/www/skiff`、`/etc/skiff`、`/www/sites` 与运行组；重装恢复后站点 `diff -r` 无差异 |
| DEPLOY-09 | △部分（qemu-user 冒烟） | 包内 ARM aarch64 静态链接（`file`/`ldd` 正常）、`version`/`--help`/`doctor` 通过（2026-11-09 `2f63aeb`）；复现条件=arm64 Linux + systemd 装包后验启动/建站 |
| DEPLOY-10 | △部分通过（等价镜像） | 2026-11-09 用 `alpine:3.23` + 发布包二进制、`SKIFF_FORM=docker` 实测：healthz 200、登录 303、fs/net/ftp 插件拒装、容器 stop/start 303；`607d09b` 复核文件管理入口已隐藏、直达 403 文案可读；未在真实 1Panel 商店流程内装 |
| DEPLOY-11 | ⏸阻塞（无复现条件） | 正常下载+`sha256sum -c` 通过；`tools/iter.sh::cmd_fetch` 无重试；复现条件=对下载域名注入超时/丢包 |

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

**压测补充实测（2026-11-07 首轮，测试机 10.10.15.4，4 vCPU / 3.9 GB；规格当前值见 [TEST_ENV.md](TEST_ENV.md) §1）**

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
> 执行前提：内核二进制与 `official/com.skiff.ftp-*.wbpkg` 均来自 GitHub Release 资产（下载后先 `sha256sum -c` 再安装）；本地/交叉编译产物只作编译级验证，**不作为部署来源，也不作为验收依据**。契约见 [KERNEL_FTP_CONTRACT.md](KERNEL_FTP_CONTRACT.md)。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| FTP-01 | 首次安装、幂等重入与卸载回收 | `[上机]` Release 安装后未装 pure-ftpd；能 `apt` | 插件「运行状态」点安装；再次点安装；检查 `pure-ftpd -v`、`systemctl status pure-ftpd`、`/etc/pure-ftpd/conf/*`、`iptables`/`nft` 无新规则；再点回收并确认 | 首次安装成功且自动启动；二次调用返回同样配置不重复装包；配置含 `ChrootEveryone`/`NoAnonymous`/`PureDB`/`PassivePortRange`/`Umask 113 002`/`MaxClientsPerIP`，且 `/etc/pure-ftpd/auth` 只保留 `50pure -> ../conf/PureDB`（`65unix`/`70pam` 已清理）；回收后服务停止、包可移除或至少禁用，站点账号全部失效 | △部分通过（2026-11-07 测试机：安装、二次重入、配置项与认证链通过；卸载回收未执行） | `crates/plugin/src/ftp.rs::install/uninstall/status`、`crates/core/src/kernel/ftp.rs::ftp_call`、`plugins/ftp/src/main.rs`、`plugins/ftp/ui/runtime.html` |
| FTP-02 | 账号名/密码/端口范围校验（纯函数） | `[本机]` | `cargo test -p skiff-plugin ftp --locked` | 非法账号名、过短密码、控制字符、非法端口区间全部被拒；登录名 `skiff_<slug16>_<fnv4>_<account>` 稳定且不同域名不碰撞 | 通过（2026-11-07 本机 7 项单测；2026-11-11 迁至 `skiff-plugin`，14 项） | `crates/plugin/src/ftp.rs`（`valid_account_name`/`valid_password`/`valid_port_range`/`ftp_login`） |
| FTP-03 | 统一身份与 `MinUID` 只降不升 | `[上机]` 已安装并建站点；宿主存在 `skiff-web` | 对 PHP 站与静态站各建一个账号；`getent passwd skiff-web`；读 `/etc/pure-ftpd/conf/MinUID`；建号后 `ls -ln /www/sites/<域名>/index` | 两类站点账号 uid/gid 均为 201000:201000；`MinUID` 不会被上调（默认仍为 1000，历史下调值保留）；`index/` 目录 `2775`、文件 `0664`/`0775`，属主统一为 `skiff-web` | 待上机（统一身份后需重跑） | `crates/plugin/src/ftp.rs::resolve_identity/ensure_min_uid/align_tree`、`crates/model/src/site.rs` |
| FTP-04 | FTP 上传的文件 PHP 容器内可读写 | `[上机]` FTP-03 通过；客户端可用（`curl`/`lftp`） | 用账号登录（显式 TLS 可选），上传 `probe.txt` 与 `probe.php`；在容器内 `id skiff-web`、`cat`/`stat` 并写入同名文件；浏览器访问 `probe.php` | 上传后容器内可读且属主/属组为 `skiff-web`(201000)；上传文件 `0664`、新建目录 `2775`（不依赖 `align_tree`，与 FTP-03 归一结果一致）；容器内写入成功（非只读）；HTTP 返回上传内容，无 403/500 | 待上机（统一身份后需重跑） | `crates/plugin/src/ftp.rs::align_tree`、`write_conf("Umask")`；`crates/app/src/sites.rs::PHP_ACCOUNT_INIT_SCRIPT`；站点 `index/` |
| FTP-05 | 越界、跨站与暴力登录被拒；删号回收与审计 | `[上机]` 两站各一账号；已装 fail2ban（`login_guard=true`） | 用 A 站账号 `cd ..`/绝对路径/软链尝试进 `log`、`conf`、B 站；连续错密码 3 次；删除 A 站账号后再登录；查 `/$P/audit` 中 `ftp.*` 行 | chroot 生效，越界与跨站全部 530/550；`MaxClientsPerIP=3`、fail2ban 10 分钟 3 次封禁生效；删号后登录失败且 `pureftpd.passwd` 无该行；审计含 `ftp.account.create/delete`、`ftp.deny.*`，无密码内容 | △部分通过（2026-11-09 `2f63aeb`：删号后重新登录 530、`pureftpd.passwd` 无该行、审计有 `ftp.account.create/delete` 且无密码；**三次封禁已实测**——容器客户端 172.31.0.2 连续 3 次错误密码后 `Currently banned:1`、`banip --with-time` 到 `01:54:49 + 600s`、正确口令 `ConnectionRefusedError`、解封恢复；同时修复并复验非 21 端口漏封；越界/跨站与 `ftp.deny.*` 未跑） | `crates/plugin/src/ftp.rs::account_delete/ensure_login_guard`、`crates/core/src/kernel/ftp.rs::ftp_call`、`crates/core` 审计表 |
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

> 重跑触发：改了 `Config` 的 `listen` / `entry_prefix` / `tls_enabled` / `tls_cert` / `tls_key`、设置页的保存逻辑、旧前缀重定向、Cookie 命名或 `install.sh` 的 TLS 分支。契约见 [DEPLOY_TEST.md](DEPLOY_TEST.md) §2.3 / §2.4。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| PANEL-01 | 设置页读取端口/前缀/TLS 与 `config.toml` 一致 | `[本机+上机]` admin 已登录 | 打开 `/$P/settings`；对照 `/etc/skiff/config.toml` 的 `listen`、`entry_prefix`、`tls_enabled`、`tls_cert`、`tls_key` | 页面字段值与配置文件一致；未配置的项显示「自动 / 未设置」而不是伪造默认值；非管理员看不到该表单 | □通过 / □失败 | `crates/web/src/handlers/settings.rs`、`templates/settings.html`、`crates/app/src/panel_settings.rs` |
| PANEL-02 | 端口范围与占用检测 | `[本机]` 单测 + `[上机]` 改端口 | `[本机]` `cargo test -p skiff-app panel_settings`；`[上机]` 设置页填 `0`、`70000`、一个已被 `ss -lntp` 占用的端口、当前监听端口 | `0`/`70000` 被拒；被占用端口被拒并提示占用；当前端口视为自己占用、允许保存；被拒时 `config.toml` 不变 | □通过 / □失败 | `crates/app/src/panel_settings.rs::validate_port/check_port_free`、`crates/web/src/handlers/settings.rs` |
| PANEL-03 | 入口前缀字符集/长度/保留段 | `[本机]` 单测 | `cargo test -p skiff-app panel_settings`；再在设置页填 `abc`（过短）、`/`、含空格/中文、`api`、`healthz`、32 位与 33 位 | 6–32 位 ASCII 字母数字 `-` `_` 通过；过短/含非法字符/`/`/保留段被拒；首字符必须字母或数字；被拒时不落盘 | □通过 / □失败 | `crates/app/src/panel_settings.rs::validate_entry_prefix`、`crates/core/src/config.rs::validate` |
| PANEL-04 | 保存写回 `config.toml`、需重启提示 | `[上机]` 可改配置并重启 | 改端口与前缀后保存；不重启先访问；`systemctl restart skiff` 后再访问 | 保存后页面明确提示「需重启生效」，不是静默成功；`config.toml` 原位更新且保留注释与其他字段、无重复键；不重启仍是旧监听；重启后新端口/新前缀可用 | □通过 / □失败 | `crates/app/src/panel_settings.rs::write_config_updates`、`crates/web/src/handlers/settings.rs`、`crates/web/src/templates.rs` |
| PANEL-05 | 旧前缀 24h 302 防锁死 | `[上机]` 刚改过前缀 | 用旧前缀 GET 首页与子路径；等窗口外（或手改 `..._previous_until` 为过去时间）再访问旧前缀 | 窗口内旧前缀 GET 302 到新前缀且保留路径；新前缀始终 200；窗口外旧前缀 404；非 GET（POST）不走旧前缀重定向 | □通过 / □失败 | `crates/web/src/handlers/settings.rs::legacy_prefix_root/legacy_prefix_redirect`、`crates/web/src/middleware.rs::prefix_ok`、`crates/core/src/bootstrap.rs` |
| PANEL-06 | TLS 三态与三种 Cookie 策略 | `[本机]` 单测 + `[上机]` 三种部署 | `[本机]` `cargo test -p skiff-core tls_`；`[上机]` 分别用「TLS 开」「反代终止 TLS」「`tls_enabled=false` 直接明文」启动，用 cookie-jar 登录一次 | TLS 开：HTTPS 200，Cookie `__Host-skiff_session` + `Secure`；反代：面板侧明文但 Cookie 仍是 `__Host-` + `Secure`，只有 `trusted_proxies` 能直连；直接明文：日志有「明文 HTTP」警告、页面有告警，Cookie 为 `skiff_dev_session`（无 `Secure`、保留 HttpOnly/SameSite），**浏览器 HTTP 下登录/改密能走通**；`tls_enabled=true` 缺证书拒绝启动且报错可读 | □通过 / □失败 | `crates/core/src/config.rs`（`validate` / `tls_enabled_*` 测试）、`crates/core/src/server.rs`、`crates/identity`（Cookie 命名）、`deploy/install.sh`（`apply_tls_config`） |
| PANEL-07 | 设置写入按段归属且重启可解析 | `[本机]` 单测 + `[上机]` 文件末尾含 `[catalog]` | `[本机]` `cargo test -p skiff-app panel_settings`、`cargo test -p skiff panel_write_then_restart_loads_roundtrip`；`[上机]` 在 `config.toml` 末尾保留 `[catalog]` 段，改端口/前缀/TLS 后保存并 `systemctl restart skiff` | 顶层键写在第一个表头之前，`[catalog]` 内容与顺序不变；落盘前用真实 `FileConfig` 语义复验，非法结果不写且原文件字节不变；重启后服务正常（`NRestarts` 不增长、`/healthz` 200），不出现「语法合法但解析失败」的 crash-loop | □通过 / □失败 | `crates/app/src/panel_settings.rs::write_config_updates/apply_update`、`crates/core/src/config.rs::Config::load`（`panel_write_then_restart_loads_roundtrip`） |

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

## M. 防火墙 / fail2ban / 进程与网络（2026-11-09 新增，未经编译验证）

> 重跑触发：改了 `crates/plugin/src/firewall.rs`、`crates/core/src/kernel/firewall.rs`、
> `plugins/firewall`、`plugins/ftp` 的 protection 分支、`plugins/monitor` 的 `processes` / `network`。
> 本轮**只写用例，未编译、未上机**；契约见 [KERNEL_FIREWALL_CONTRACT.md](KERNEL_FIREWALL_CONTRACT.md)。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| FW-01 | 能力面授权拒绝 | `[本机]` 单测 | 以无 `firewall:*` 授权的插件调 `firewall_call("status")` | 403，审计 `firewall.deny.status`；不执行任何宿主命令 | □通过 / □失败 | `crates/core/src/kernel/firewall.rs`、`crates/plugin/src/firewall.rs` |
| FW-02 | 规则增删只动自有作用域 | `[上机]` 有 nft 或 iptables | `rule.add` 一条 tcp/端口规则；`rules.list` 核对；`rule.remove` 自家 id | 规则出现在 `inet skiff`/`SKIFF` 链且带 `skiff-` 标识；删除后消失；宿主其它规则不变 | □通过 / □失败 | `crates/plugin/src/firewall.rs` |
| FW-03 | 非法参数拒绝 | `[本机]` 单测 | 依次传非法端口 `0`/`70000`/`9-3`、非法 CIDR、非法 jail 名、非法 backend、非法 id | 全部 400，且不写规则/不写文件 | □通过 / □失败 | `crates/plugin/src/firewall.rs`（`valid_*`、`RuleSpec::parse`） |
| FW-04 | fail2ban 安装与复用 | `[本机]` 单测 + `[上机]` 未装 fail2ban | `fail2ban.install`；装完立即 `fail2ban-client status`；再调一次 | 首次安装写 `skiff-defaults.conf`、`enable` 后显式 `restart`；`status` 的 jail 列表不含 `sshd`；第二次复用不重复安装且不动宿主既有 jail | □通过 / □失败 | `crates/plugin/src/firewall.rs::fail2ban_install/restart_and_verify` |
| FW-05 | jail 生命周期 | `[上机]` fail2ban 已装 | `fail2ban.jail.ensure` 建 `pure-ftpd`；`fail2ban.banned`；`fail2ban.jail.remove` | 生成 `/etc/fail2ban/jail.d/skiff-pure-ftpd.conf` 并重启；封禁列表可读；移除后文件消失，其它 `skiff-*` 不受影响 | □通过 / □失败 | `crates/plugin/src/firewall.rs` |
| FW-06 | 非 Native 形态拒绝 | `[本机]` 受限形态配置 | 调 `firewall_call("status")` | 503，审计 result=fail | □通过 / □失败 | `crates/core/src/kernel/firewall.rs` |
| FW-15 | fail2ban 三态与可读原因 | `[本机]` 单测 + `[上机]` | 分别构造未安装、已安装未运行、运行中三种起点，打开 fail2ban 页 | `state` 分别为 `missing` / `stopped` / `running`；未运行时 `reason` 非空且可读；不停留在旧按钮状态 | □通过 / □失败 | `crates/plugin/src/firewall.rs::fail2ban_status/state_reason` |
| FW-16 | fail2ban 显式启停 | `[本机]` 单测 + `[上机]` | 点「启动」/「停止」；未安装时直接调 `fail2ban.start` | `start` = `systemctl enable` + `restart`，重启后复核 jail（非预期 jail 被 stop/disable）；`disabled_jails` 只含复核后确实消失的 jail，仍在的写 `residual_jails`，与 `jails` 不交叉；`stop` = `disable --now`；未安装 503 且 message 可读 | □通过 / □失败 | `crates/plugin/src/firewall.rs::fail2ban_start/fail2ban_stop/restart_and_verify` |
| FW-19 | 宿主 jail 覆盖导致停用不生效时返回体自洽 | `[本机]` 单测 + `[上机]` 宿主存在 `jail.d/zz-sshd.conf` | 构造 `skiff-defaults.conf` 关闭 `sshd`、同时放置排序更晚的 `zz-sshd.conf` 启用 `sshd`；调 `fail2ban.start`；核对返回体与 `fail2ban-client status` | 返回 `jails` 含 `sshd`；`disabled_jails` 不含 `sshd`；`residual_jails` 含 `sshd`；两数组互斥且 `disabled_jails ∩ jails = ∅` | □通过 / □失败 | `crates/plugin/src/firewall.rs::restart_and_verify`（单测 `fail2ban_reports_residual_jail_when_host_override_wins`） |
| FW-18 | 插件 RPC 状态码与语义一致 | `[本机]` 单测 | `cargo test -p skiff-web --lib rpc_error`；`[上机]` 未安装 fail2ban 时经面板调 `fail2ban.start`/`stop`/`uninstall` | 内核 503 原样返回 503（正文仍为内核原文）；401/403/404/409/500/504/507 各自透传；仅未识别状态与业务错误归 400 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs::rpc_error/kernel_http_status`（`rpc_error_passes_kernel_status_through`） |
| FW-17 | fail2ban 显式卸载 | `[本机]` 单测 + `[上机]` | 点「卸载」并确认；查审计与 `dpkg -l fail2ban` | 停服务、删除 `skiff-defaults.conf`、`apt-get purge`；返回 `state=missing`；审计 `firewall.fail2ban.uninstall`；其它 `skiff-<jail>.conf` 保留 | □通过 / □失败 | `crates/plugin/src/firewall.rs::fail2ban_uninstall`、`plugins/firewall/ui/app.js` |
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
cargo test -p skiff-web --test main_flow job_retry
cargo test -p skiff-app php_init_script
```

## N. 任务中心作业（2026-11-10 新增）

> 重跑触发：改了 `crates/web/src/handlers/jobs.rs`、`crates/app/src/sites.rs` 的作业提交/重试分支、
> `crates/store/src/db/jobs.rs` 的唯一索引或执行器 spawn。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| JOB-01 | 重试复用正常执行器 | `[本机]` 集成测试 + `[上机]` 一条失败任务 | 对失败任务点「重试」；观察任务中心状态与日志 | 新任务 `retry_of` 指向原任务，离开 `queued` 并落 `exit_code`/`error_class`；不再永久占索引 | □通过 / □失败 | `crates/app/src/sites.rs::retry_job`、`crates/web/src/handlers/jobs.rs::retry`、`crates/web/tests/main_flow.rs::job_retry_reuses_executor_and_rejects_unsupported_kind` |
| JOB-02 | 同目标冲突可解释 | `[本机]` 集成测试 + `[上机]` | 重试进行中再对同目标提交/重试 | 明确 409，文案含「该对象已有任务在执行…」，不排队、不插新行 | □通过 / □失败 | `crates/store/src/db/jobs.rs`（`idx_jobs_active_target`）、`crates/web/src/handlers/jobs.rs` |
| JOB-03 | 无执行器类型拒绝 | `[本机]` 集成测试 | 对 `site.create` 等无执行器 kind 的历史任务点「重试」 | 400 + 「不支持重试」，作业列表不新增行 | □通过 / □失败 | `crates/app/src/sites.rs::retry_job`（`RetryPlan` 解析） |

## O. 商店目录（Skiff-server）

> 重跑触发：改了 `crates/app/src/catalog/fetch.rs` 的传输层/TLS 配置、`crates/core/src/config.rs`
> 的 `[catalog]` 校验，或 `docs/SKIFF_SERVER.md` §4.5 的内网口径。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| STORE-01 | 自签 CA 内网镜像可接入且仍强制 https | `[本机]` crate 单测；`[上机]` 内网静态镜像 + 该 CA 签发的服务器证书 | 不配 `ca_bundle` 访问自签镜像；再把 CA 导出 PEM 配到 `[catalog].ca_bundle` 后重启刷新 | 未配置时连接被拒（证书不受信）；配置后刷新成功、`catalog.refresh` 落库；镜像地址仍只接受 `https://`，无 insecure 开关 | □通过 / □失败 | `crates/app/src/catalog/fetch.rs::{UreqTransport::load_ca_bundle,root_store}`、`catalog::fetch::tls_tests`、`crates/core/src/config.rs::build_catalog_settings` |
| STORE-02 | `ca_bundle` 空值/非法值的启动语义 | `[本机]` 单测 | 不配置、配置不存在路径、配置非 PEM 内容三种启动 | 空值行为与历史一致（`extra_roots` 为空）；缺失/非法明确报「`catalog.ca_bundle` 非法」并拒绝启动，不静默回退内置根 | □通过 / □失败 | `crates/core/src/config.rs::catalog_ca_bundle_is_optional_and_strict`、`skiff_app::panel_settings::CatalogFile::ca_bundle` |
| STORE-03 | `skiff-server serve` 符合静态服务契约 | `[本机]` 集成测试；`[上机]` 产物目录 + 自签证书 + Release 资产 `skiff-server-<target>`（测试机无 Rust 工具链） | 从 Release 取 `skiff-server-<target>` 并 `sha256sum -c checksums.txt` 校验，再启动 `serve --dist`（默认 https），分别验证条件请求、Range、HEAD、404/405、路径穿越与 `/api/`；再用 `--http` 试一次；最后加 `--access-log <path>` 重启，重放一次 `If-None-Match` 后读该文件 | 资产校验通过且可直接执行；304/206/416 语义、`HEAD` 无 body、非法 blob 名与穿越 404、`/api/**` 404、普通方法 405；默认无证书拒绝启动，`--http` 打印「仅测试用」；客户端以 `ca_bundle` 导入 CA 后能刷新；访问日志出现 `status=304 … cache=inm`，未开启时不产生文件或输出 | □通过 / □失败 | `tools/skiff-server/src/serve.rs`、`tools/skiff-server/tests/serve.rs`、`.github/workflows/release.yml`（`skiff-server-<target>`）、`docs/SKIFF_SERVER.md` §16 |

最小回归命令：

```bash
cargo test -p skiff-app catalog::fetch::tls_tests
cargo test -p skiff catalog
cargo test -p skiff-server
```

## P. 发布面 API（Skiff-server R0/R1/R2a/R2b/R2c/R2d，2026-11-12 更新）

> 重跑触发：改了 `tools/skiff-server/src/api/**`、`src/events.rs`、`src/notify.rs`、`src/http.rs`、`src/build.rs`、`src/main.rs` 的
> `api`/`notify`/`release` 子命令，或 `docs/SKIFF_SERVER.md` §17 的签名/状态机/事件/产物口径。
> R0 默认关闭、默认只绑回环；`serve` 与 `api`/`notify` 是不同进程，`serve` 对 `/api/**` 仍一律 404。
> R1 只填充 v1 索引既有字段（`yanked` / `revoked`），不升 schema；R2b 的 `kind` 对插件记录不落盘。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| API-01 | 请求签名与重放防护 | `[本机]` 集成测试；作者/审核两把 Ed25519 钥 | 正例提交；再把 body 改一个字节、改用清单签名域、把时间戳拨到 −301s、复用同一 nonce 重放 | 正例 201；其余全部 401，且不产生提交记录 | □通过 / □失败 | `tools/skiff-server/src/api/auth.rs`、`tests/api.rs` |
| API-02 | 权限分离 | 作者的 `id_prefixes` 限定 `com.example` | 作者钥调 approve；同一把钥同时登记为审核钥后自我审核；提交前缀外 id | 均 403 且响应体统一 `forbidden`；审计不记签名与包内容 | □通过 / □失败 | `api/auth.rs::Authenticated`、`api/store.rs::{approve,reject}` |
| API-03 | 幂等与状态机 | 同一份签名 `.wbpkg` | 同摘要重提；approved 后作者 withdraw；已发布版本再提交 | 重提 200 且 `sid` 相同；非法迁移 412；已发布同版本 409 | □通过 / □失败 | `api/store.rs::{sid_of,approve,withdraw}` |
| API-04 | 传输严格性 | `[本机]` 集成测试 | POST 不带 `Content-Length`；带 `Transfer-Encoding: chunked`；声明 999999999 字节 | 分别 411 / 400 / 413；413 不读取 body | □通过 / □失败 | `tools/skiff-server/src/api/mod.rs::read_body`、`api/multipart.rs` |
| API-05 | 产物生成与失败隔离 | `[本机]` 集成测试；`SKIFF_SIGNING_KEY` + `SKIFF_SERVER_PUBLIC_KEY` | 提交→approve→`release --staging-out`；再构造 schema 缺失让 `release --dist --swap` 失败 | staging 的 `manifest` seq=1 且验签通过、`blobs/sha256/<hex>` 存在；失败时 `--dist` 的 marker 原样保留、提交仍为 `approved` | □通过 / □失败 | `api/job.rs`、`tests/api.rs::failed_release_does_not_touch_dist` |
| API-06 | `serve` 零改动 | `[本机]` 集成测试 | 抽取 `http.rs` 后重跑既有 serve 契约用例（304 / 206 / 405 / 404、HEAD、穿越、访问日志） | 行为与抽取前一致，`/api/**` 仍 404 | □通过 / □失败 | `tools/skiff-server/src/http.rs`、`tests/serve.rs` |
| API-07 | yank 撤包 API 与发布 | 已发布 `com.example.demo 1.0.0` | 作者调 yank；审核钥撤未知版本；审核钥撤真实版本并重复一次；`release` 到 seq+1 | 作者 403、未知版本 404、首次 202、重复 200 且同一记录；索引该条 `yanked=true`、`seq` +1、提交记录出现 `takedown` | □通过 / □失败 | `api/takedown.rs::yank`、`api/job.rs`、`tests/api.rs::yank_*` |
| API-08 | 密钥吊销与历史处置 | 已发布版本 + 待审提交 | 审核钥吊销作者钥并重复；作者再提交；审核该 key 的历史/待审提交；`release` | 首次 202、重复 200、未登记 key 404；该 key 提交/审核均 403；历史版本 `yanked=true` 且进入 `revoked[]`；待审提交 approve 409 | □通过 / □失败 | `api/takedown.rs::revoke_key`、`api/auth.rs::authenticate`、`api/store.rs::approve` |
| API-09 | 客户端离线读取撤包信息 | 已生成的旧索引（seq=2，`yanked=false`、`revoked=[]`）与新索引 | 用当前客户端解析旧索引进安装；再用填充 `yanked`/`revoked` 的新索引安装 | 旧索引解析成功、`yanked=false`、`revoked` 为空，安装链路与 R1 前一致；新索引不报解析失败，安装返回 `revoked` 原因；已安装卡片提示来自本地缓存，不触网 | □通过 / □失败 | `crates/app/src/catalog/{model,mod}.rs`、`commands/store.rs::check_compatibility`、`crates/web/src/handlers/plugins.rs` |
| API-10 | 审计查询 | 已有 submit/approve/yank/revoke 记录 | 按 `sid`、`key_id`、`action`、`result`、`since/until`、`limit/offset` 查询；作者查他人 `key_id` | 审核者可见全部；作者默认只看自己、显式查他人 403；分页返回 `total`/`next_offset`；字段只有时间/主体/动作/对象/结果，不含签名与包内容 | □通过 / □失败 | `api/audit.rs`、`tests/api.rs::audit_query_is_scoped_and_paginated_without_sensitive_fields` |
| API-11 | `revoked.json` 纳入签名覆盖 | 临时源数据（一条插件） | 缺 `revoked.json` 构建一次；写入 `[]` 再构建；写入非法 JSON；写入一条吊销记录 | 缺文件与空数组产物逐字节一致；非法 JSON 构建失败；有效记录进入 `index.revoked` 且 `manifest.index.sha256` 覆盖索引字节 | □通过 / □失败 | `src/build.rs`、`tests/p0.rs::revoked_json_*` |
| API-12 | 撤包原因端到端可见 | 源条目 `yanked=true` + `yank_reason` | 构建索引；客户端解析并取 `plugin_package`/`cached_takedown`；再尝试安装该版本 | 索引含 `yank_reason`；插件页提示带原因；安装被拒文案带原因；无原因时仍是通用文案；旧索引无该字段时解析与安装不变 | □通过 / □失败 | `src/model.rs`、`src/api/job.rs`、`crates/app/tests/takedown_reason.rs`、`commands/store.rs::check_compatibility` |
| API-13 | 审计按 sid 串起发布 | 提交→审核→release，并执行 yank | `GET /api/v1/audit?sid=<sid>` | 同一次查询返回 submit / approve / release 三类动作；release 行含 `submission_ids[]` 且命中该 sid；仍不含签名与包内容 | □通过 / □失败 | `src/api/audit.rs`、`tests/api.rs::audit_sid_chains_submit_approve_and_release` |
| API-14 | webhook 事件生成与去重 | 已配置订阅；执行 submit/approve/release/yank | 观察 `state/outbox/pending/` | 生成 created/approved/published/yanked 事件；同一业务动作重复 emit 只有一条；载荷无包体/签名 | □通过 / □失败 | `src/events.rs`、`tests/api.rs::api_actions_emit_outbox_events` |
| API-15 | webhook 投递与签名 | 本地假接收端 | 500 后 200；400；超时；校验签名 | 5xx/超时重试，4xx 直接 dead；成功移入 delivered；Ed25519 签名可用独立域验证 | □通过 / □失败 | `src/notify.rs` 单测 |
| API-16 | 通知失败隔离 | `state/outbox` 被替换为普通文件 | 正常提交 | 提交仍 201，业务状态不受影响 | □通过 / □失败 | `tests/api.rs::submit_succeeds_when_outbox_cannot_be_written` |
| API-17 | `release.failed` 事件 | 已 approve 提交 + 不存在的 schema 目录 | 执行 release | 发布失败并留下 `release.failed` 事件；不影响线上目录 | □通过 / □失败 | `src/api/job.rs`、`tests/api.rs::release_failure_emits_event` |
| API-18 | 提交记录 `kind` 与旧记录兼容 | 旧格式 `state/submissions/*.json`（无 `kind`） | 提交插件并检查落盘/响应；再 approve→release 检查源条目与索引 | 插件记录不出现 `kind`；无 `kind` 的旧记录解析为 `plugin`；源条目与 `index.json` 字节不变 | □通过 / □失败 | `src/api/store.rs`（`EntryKind`）、`tests/api.rs::plugin_records_omit_kind_and_legacy_records_still_load` |
| API-19 | 应用模板提交→发布全链路 | 作者钥已登记 `com.example` 前缀 | `POST /api/v1/apps`（`entry`+`compose`）→ approve → release | 201/202；`index.apps[]` 带 `compose_sha256`/`compose_url`，`blobs/sha256/<sha>` 与提交原文逐字节一致，源条目 `asset` 指向 `assets/<sha>.yaml` | □通过 / □失败 | `src/api/store.rs::create_app`、`src/api/job.rs`、`tests/api.rs::app_submit_approve_release_writes_compose_blob_and_index` |
| API-20 | compose 严格子集校验 | 含 `build:` 或非 UTF-8 的 compose | 提交应用模板 | 422，不落任何提交状态；不另写第二套解析器（复用 `skiff-compose`） | □通过 / □失败 | `src/api/store.rs::create_app`、`tests/api.rs::app_compose_outside_supported_subset_is_rejected` |
| API-21 | 应用版本不可变与幂等 | 同 id/version 的 compose A、B | 先提 A，再提 A，再提 B | 201；重复 A → 200 同一 `sid`；B → 409 | □通过 / □失败 | `src/api/store.rs::create_app`、`tests/api.rs::same_app_version_with_different_compose_conflicts` |
| API-22 | 插件与应用共享单次 `seq` | 同一渠道各一份 approved 提交 | 一次 release | `seq = 1`、`released.len() == 2`，索引 `plugins[]`/`apps[]` 各 1 条 | □通过 / □失败 | `src/api/job.rs`、`tests/api.rs::plugin_and_app_release_share_one_seq` |
| API-23 | 运行环境提交→发布全链路 | 作者/审核者钥已登记 | `POST /api/v1/runtimes` → approve → release | 201/202；`index.runtimes[]` 的 `kind`/`version`/`image`/`image_digest`/`api_min`/`default` 正确；源条目落 `catalog/<ch>/runtimes/<kind>-<version>.json` | □通过 / □失败 | `src/api/store.rs::create_runtime`、`src/api/job.rs`、`tests/api.rs::runtime_submit_approve_release_writes_index_entry` |
| API-24 | 运行环境 `default` 提交约束 | `entry.default = true` | 提交运行环境 | 422；`default` 只能在发布后走元数据修订 | □通过 / □失败 | `src/api/store.rs::create_runtime`、`tests/api.rs::runtime_default_must_be_false_at_submission` |
| API-25 | 运行环境元数据修订 | 已发布运行环境 | `POST /api/v1/submissions/{sid}/revise`（`eol`/`default`）→ release | 202；下一次 release 应用到源数据并递增 `seq`；`audit.jsonl` 有 `action=revise`；带 `image`/`image_digest` 的修订 422；未发布 412、作者 403 | □通过 / □失败 | `src/api/store.rs::revise_runtime`、`src/api/job.rs::apply_runtime_revisions`、`tests/api.rs::runtime_revision_changes_eol_and_default_with_audit`、`runtime_revision_requires_reviewer_and_published_state` |
| API-26 | 同 kind 多个 `default` 被 release 拒绝 | 同 kind 两个已发布运行环境均修订为 `default=true` | 再次 release | 发布失败并留 `release.failed`；不生成矛盾索引 | □通过 / □失败 | `src/build.rs`（default 唯一性）、`tests/api.rs::release_rejects_multiple_defaults_per_kind` |

最小回归命令：

```bash
cargo test -p skiff-server --test api
cargo test -p skiff-server --test serve
cargo test -p skiff-server
```

上机（R0 全链路，仅测试姿态）：先起 `skiff-server api --state <0700> --authors <0600> --reviewers <0600>`，
用作者钥提交并让审核钥 approve，再在可持有 `SKIFF_SIGNING_KEY` 的环境执行
`skiff-server release --state <dir> --catalog <dir> --staging-out <dir>`；把 staging 交给分发主机后，
现有 Skiff 刷新目录应看到新版本。**生产不得把发布私钥放到分发主机上同机 `--swap`。**

## 附：待核对项结论（2026-11-07 已全部定性）

| 编号 | 结论 | 依据 |
| --- | --- | --- |
| FS-04 | 后端+管理员 UI 已接（第二步） | 回收站接 SQLite `trash_entries`，`/{p}/files/trash` 提供恢复/彻底删除/清空；能力面仍无 `fs.trash` 动词（插件不直接操作回收站） |
| FS-05 | 后端+管理员 UI 已接（第二步） | 目录树配额接 `fs_quotas`，`/{p}/files/quotas` 提供设置/修改/取消/重扫；能力面无 quota 动词 |
| TASK-06 | ☑通过（上机，2026-11-09） | `buffer.push`/`buffer.stat` 在能力面实现并有限速单测；testkit `buffer_probe`/`buffer_stat`/`buffer_push` 上机实测覆盖容量/丢弃/高水位/限速/权限，数字见 G 章「调度器补充实测」 |
| TASK-07 | 有入口 | cron 新建任务落库为 `plugin:com.skiff.cron:<key>`、`source=plugin:<target>`，2026-11-06 实测创建/删除均 200 |
| DOCKER-10 | 运行时入口已接（单文件） | `/$P/compose` 走 `skiff-compose` 解析 + `Docker::execute` 的 `compose.up/down`；不支持 `-f` 多文件叠加，源文件不落盘；`xtask check` 字段子集校验保留 |
| DEPLOY-09 | 部分通过：qemu-user 冒烟，未实机装机 | `file`/`ldd` 显示 ARM aarch64 静态链接，`version`/`--help`/`doctor` 通过（2026-11-09 `2f63aeb`）；复现条件=arm64 Linux + systemd，装包后验启动/建站 |
| DEPLOY-10 | 部分通过：等价镜像实测，未走真实 1Panel 商店流程 | `SKIFF_FORM=docker` 下 healthz 200、登录 303、fs/net/ftp 插件 400 拒装、容器 stop/start 303；文件管理入口已在 `607d09b` 按形态隐藏、页面/深链/API 直达 403 且文案可读 |
| DEPLOY-11 | 因无可复现超时/断网条件阻塞 | 正常下载+摘要校验通过（c39fe7d2…）；代码核对 `tools/iter.sh::cmd_fetch` 单次 `curl --max-time 600`，无重试/结构化错误输出；复现条件=对下载域名注入超时或丢包 |
