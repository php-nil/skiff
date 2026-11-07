# Skiff 测试矩阵（按内核功能域）

> 定位：**回归清单**。改一处代码后，按章首「重跑触发」挑一章，按稳定编号逐项执行即可。
> 本文只写当前实现应通过的验收项，不记历史、不记未闭环问题；未闭环问题归 [TEST_LEDGER.md](TEST_LEDGER.md)。
> 所有预期值均按当前路由、模板与契约整理；无法从代码确认的项标「待核对」并写明原因。

## 0. 使用约定

- `$B`：测试机基址（当前地址、端口与 TLS 见 [TEST_ENV.md](TEST_ENV.md) §2，例如 `https://10.10.15.4:18080`）；`$P`：安全入口前缀，从 `journalctl -u skiff | grep 安全入口`、首次安装输出或 TEST_ENV §2 取当前值。
- `$J`：已登录管理员的 curl CookieJar；除 AUTH 章外，默认已登录。
- `[本机]`：开发机即可执行（`cargo test`、`xtask`、模板/路由单测）。
- `[本机+上机]`：先跑单测，再到测试机补一条手工断言（通常用于路径/权限类）。
- `[上机]`：必须有 Linux 测试机、systemd 或 Docker 才能执行。
- 判定栏统一记 `□通过 / □失败`；失败时把实际状态码/响应片段补在备注。
- 测试数据用 `matrix.local`、`matrix-static.local`、`matrix-proxy.local` 等专用域名，避免污染已有站点。

登录一次的命令：

```bash
export B='https://10.10.15.4' P='<entry-prefix>' J=/tmp/skiff.jar
C=$(curl -ks -c "$J" "$B/$P/login" | grep -o 'name="csrf" value="[^"]*"' | head -1 | cut -d'"' -f4)
curl -ks -b "$J" -c "$J" -d "csrf=$C&username=admin&password=<password>" -o /dev/null -w '%{http_code}\n' "$B/$P/login"
```

## 总览

| 域 | 项数 | 自动化判断 | 主要执行位置 |
| --- | --- | --- | --- |
| A. 身份与会话 | 10 | 1 项本机可验（AUTH-06），9 项必须上机 | `cargo test -p skiff-identity`；测试机 curl 登录链路 |
| B. 菜单与导航 | 8 | 1 项本机可验（MENU-01 常量单测），7 项必须上机 | 路由/模板断言；浏览器手风琴与布局 |
| C. 插件体系 | 10 | 1 项本机可验（PLUGIN-02），9 项必须上机 | `cargo test -p skiff-plugin`；`.wbpkg` 安装与 iframe |
| D. 网站与运行环境 | 13 | 1 项本机可验（SITE-13 模板/路由断言），12 项必须上机（crate 单测兜底） | `cargo test -p skiff-web site_detail`；测试机建站与容器 |
| E. Web 服务 | 8 | 0 项本机可验，8 项必须上机（crate 单测兜底） | `cargo test -p skiff-app nginx`；测试机 nginx -t 与日志 |
| F. 文件系统原语 | 8 | 4 项本机可验（FS-02～05），4 项必须上机 | `cargo test -p skiff-fs -p skiff-plugin`；插件能力面 |
| G. 调度器 | 9 | 3 项本机可验（TASK-05～07），6 项需上机（TASK-09 另有 `skiff-cron` 单测兜底） | `cargo test -p skiff-sched -p skiff-app -p skiff-cron`；任务页与 cron 插件页 |
| H. Docker | 10 | 2 项本机可验（DOCKER-09/10，本轮本机 OOM 未跑、CI 兜底），8 项必须上机 | `cargo test -p skiff-docker -p skiff-policy`；Docker 页面/API |
| I. 部署与升级 | 11 | 1 项本机可验（DEPLOY-01），10 项必须上机；DEPLOY-09/10/11 因环境/复现条件阻塞 | `deploy/*.sh`、systemd、安装包 |
| J. 站点压测与资源开销 | 8 | 0 项本机可验，8 项必须上机（测试机两站并发 + `/proc` 采样） | `tools/perf/*`；nginx / PHP-FPM / 反代三路径；`skiff` 进程与插件子进程 |
| K. FTP 与宿主机文件服务 | 5 | 1 项本机可验（FTP-02 账号/密码/端口校验单测），4 项必须上机 | `cargo test -p skiff --bin skiff kernel::ftp`；测试机 pure-ftpd + PHP 容器 |

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

**本轮实测（2026-11-06，测试机 10.10.15.4）**

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

> 重跑触发：改了建站表单、vhost 生成、域名/别名、站点目录、运行环境安装/启停/解绑、扩展、socket/端口、SSL 或删除保护。

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

最小回归命令：

```bash
curl -ks -o /dev/null -w 'sites=%{http_code}\n' -b "$J" "$B/$P/sites"
curl -ks -o /dev/null -w 'site_php=%{http_code}\n' -H 'Host: matrix.local' 'http://10.10.15.4/'
curl -ks -o /dev/null -w 'site_static=%{http_code}\n' -H 'Host: matrix-static.local' 'http://10.10.15.4/'
curl -ks -o /dev/null -w 'site_proxy=%{http_code}\n' -H 'Host: matrix-proxy.local' 'http://10.10.15.4/'
curl -ks -o /dev/null -w 'traversal=%{http_code}\n' -b "$J" "$B/$P/sites/matrix.local?tab=dir&path=../log"
curl -ks -o /dev/null -w 'site_manage=%{http_code}\n' -b "$J" "$B/$P/sites/matrix.local?tab=manage"
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
| FS-04 | 回收站移动/恢复/清理 | `[本机]` 单测；`[已核对]` 无上机入口（能力面仅 read/write/list/mkdir/remove/rename，无 trash 动词） | `cargo test -p skiff-fs --locked` | 删除进回收站、可恢复、过期清理；恢复后原路径内容一致 | □通过 / □失败 | `crates/fs/src/trash.rs` |
| FS-05 | 全局与插件配额 | `[本机]` 单测；`[已核对]` 无上机入口（配额在 fs 内部生效，能力面无 quota 动词、无配置项） | `cargo test -p skiff-fs --locked` | 超全局/插件配额被拒；回收站占用计入配额；配额释放后可再写 | □通过 / □失败 | `crates/fs/src/quota.rs` |
| FS-06 | 审计分级与目视筛选 | `[上机]` 已产生 fs 操作 | 打开 `/$P/audit`（页面只展示最近 200 条，无服务端筛选参数），在列表中找出 `fs.*` 与 result=deny/fail 行 | 每条含 actor、channel、action、target、result；能按文本找到 `fs.*` 与 deny/fail 条目 | □通过 / □失败 | `crates/plugin/src/capability.rs`、`handlers/settings.rs`、`templates/audit.html` |
| FS-07 | 内核文件管理页基本操作（第一步） | `[上机]` 管理员会话，`/$P/files` 可打开 | 浏览目录；打开文本并修改保存；新建目录/文件；重命名；删除；再打开 >256 KiB 文件 | 操作成功；大小/时间正确；>256 KiB 只读且保存禁用；删除二次确认；保存失败时编辑内容保留 | □通过 / □失败 | `crates/web/src/handlers/files.rs`、`crates/app/src/admin_fs.rs`、`static/files.js` |
| FS-08 | 数据根硬拒与权限位 | `[上机]` 已安装 | 用内核文件页尝试列/读/写 `/www/skiff/skiff.db`、`/www/skiff/plugins`、`/www/skiff/official`；检查数据根权限 | 即使管理员也返回 403 `fs.protected`；绕过软链仍拒绝；数据根与 plugins 为 0711；DB 文件非全局可写 | □通过 / □失败 | `crates/app/src/admin_fs.rs`、`crates/plugin/src/fsgrant.rs`、`deploy/install.sh`、`docs/DEPLOY_TEST.md` §1.1 |
| FS-09 | 旧深链 301 与插件菜单撤销 | `[上机]` `com.skiff.files` 已装 | 访问 `/$P/plugins/com.skiff.files/main`；查看「工具」菜单与全站导航 | 返回 301 且 `Location: /$P/files`；菜单中只有内核「文件管理」入口，无插件文件管理入口 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`、`crates/web/src/menu.rs`、`crates/web/src/handlers/mod.rs` |

最小回归命令：

```bash
cargo test -p skiff-fs --locked
cargo test -p skiff-plugin capability --locked
curl -ks -o /dev/null -w 'audit=%{http_code}\n' -b "$J" "$B/$P/audit"
curl -ks -o /dev/null -w 'files=%{http_code}\n' -b "$J" "$B/$P/plugins/com.skiff.files/main"
```

**本轮实测（2026-11-06，测试机 10.10.15.4）**

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
| TASK-06 | 缓冲回执与限速 | `[本机]` app 单测；`[已核对]` 无上机入口（testkit 无 scheduler_call，cron 只代理 task.*） | `cargo test -p skiff-app buffer_push --locked` | accepted/rejected 回执字段正确；每任务 100 点/秒限速生效 | □通过 / □失败 | `crates/app/src/tasks.rs`、`crates/sched/src/buffer.rs` |
| TASK-07 | 插件任务唤醒 | `[本机]` sched 单测；`[已核对]` 有入口 | `cargo test -p skiff-sched plugin --locked`；cron 新建生成 `plugin:com.skiff.cron:<key>`、`source=plugin:<target>` | 插件任务按调度唤醒、超时/失败回执；插件不可用时记 blocked 而非 failed | □通过 / □失败 | `crates/core/src/server.rs`、`crates/sched`、`crates/app/src/tasks.rs` |
| TASK-08 | 任务页权限与 CSRF | `[上机]` viewer 与 admin 各一 | viewer 打开任务页；尝试启停/立即执行/删除；admin 不带 CSRF 执行 | viewer 只读；写操作 403；admin 无 CSRF 403，带 CSRF 303 | □通过 / □失败 | `crates/web/src/handlers/tasks.rs`、`crates/model/src/rbac.rs` |
| TASK-09 | 调度预设选择与编辑反解 | `[本机]` `skiff-cron` 单测；`[上机]` 已装 cron 插件 | 本机跑 `cargo test -p skiff-cron --locked`；新建页逐个选预设并核对表达式/人话预览；保存后在列表「改调度」逐项确认回显；自定义表达式填非法值 | 6 类预设各自生成标准 5 段表达式且预览为人话；编辑时预设能回显成对应表单；匹配不到预设落「自定义」并保留原表达式；非法表达式有明确报错且拒绝保存；界面标注本地时区、不支持秒级与 `@reboot` | □通过 / □失败 | `crates/cron/src/preset.rs`、`plugins/cron/src/main.rs`、`plugins/cron/ui/app.js` |

最小回归命令：

```bash
cargo test -p skiff-sched --locked
cargo test -p skiff-app tasks --locked
curl -ks -o /dev/null -w 'tasks=%{http_code}\n' -b "$J" "$B/$P/tasks"
curl -ks -o /dev/null -w 'task_detail=%{http_code}\n' -b "$J" "$B/$P/tasks/builtin:db-vacuum"
```

**本轮实测（2026-11-06，测试机 10.10.15.4）**

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| TASK-01 | ☑通过 | 列表 200，`builtin:db-vacuum` 显示 kind/source/调度/状态/下次运行；详情历史可见 |
| TASK-02 | ☑通过（待发版） | 列表标「内置」；模板不渲染删除按钮；内核已补服务端硬拦（`Scheduler::delete` 拒 `builtin:`，能力面 `task.delete` 同拒），发版后按台账复验 RPC 删除 → 403 |
| TASK-03 | ☑通过 | 开关 303；`scheduled_tasks` 行 `id=builtin:site-backup:matrix.local`、`kind=builtin:site-backup`、`source=site:matrix.local`、`schedule_json={"type":"cron","expr":"0 3 * * *","tz":...}`、`payload={"domain":"matrix.local"}`、`enabled=1`；关闭后 `enabled=0` 且 8 条历史保留；重开恢复 `enabled=1`。环境无 OpenResty，站点为库内夹具，未触发真实 vhost |
| TASK-04 | ☑通过 | 手动触发 303；`task_runs` 落 `trigger=manual`/`result=ok`（站点备份产出 tar）；插件任务手动触发同样落 `trigger=manual` |
| TASK-05 | ☑通过（本机） | `cargo test -p skiff-sched --locked` 45 项通过：超时取消、退避重试、单飞、`queue_one`、冷却期、错过策略、`retain_across_ticks=false` |
| TASK-06 | ☑通过（本机） | `cargo test -p skiff-app tasks --locked` 7 项通过：`buffer.push`/`buffer.stat` 回执、仅本人任务、每任务 100 点/秒限速、无权限拒绝；上机入口缺口记台账 |
| TASK-07 | ☑部分通过 | `cron` `jobs.list/toggle/run` 200；`jobs.remove` 200 且行删除（`{"removed":true}`）；内核任务详情/历史页 200；插件任务按调度唤醒、失败回执 `result=fail`/`error_class=executor_failed`、手动触发均通过；`cron` 代他人注册被 `source != owner` 拒，修复（`source=owner`）已在本机待发版，见台账 |
| TASK-08 | ☑通过 | viewer 只读；写操作 403；admin 无 CSRF 403、带 CSRF 303 |
| TASK-09 | ☑通过（2026-11-07） | `skiff-cron` 14 项单测（预设→表达式、越界拒绝、往返反解、等价写法、自定义回退、描述文案）全绿；测试机 10.10.15.4 真浏览器 41 项断言全过：6 类预设分别落 `*/5 * * * *`、`0 */2 * * *`、`0 3 * * *`、`30 9 * * 1,3,5`、`0 4 1 * *`、`0 9 * * 1-5` 且预览为人话；编辑「改调度」逐项回显对应表单；自定义 `0,30 8-18 * * 1-5` 原样保留；非法 `0 25 * * *` 在新建与编辑两处均报错并拒绝保存；验收任务已清理。截图 `dist/iter/shots/{list,edit-card}.png` |

**调度器补充实测（本轮）**

| 项 | 结果 | 证据 |
| --- | --- | --- |
| 插件门三态 | ☑通过 | 未安装 `NotInstalled`、禁用 `Disabled`、`api=1.1` 且 `plugins.api=1.1` 时 `ApiMismatch`；均只写 `task.blocked` 审计，不产生 failed 运行 |
| 执行期回调 scheduler | ☐未上机 | 现有 testkit 无「执行期回调 scheduler」方法，无法从 RPC 构造；代码核对 `crates/core/src/kernel/capability.rs:134-141`：`is_interop_busy || tasks.is_executing` → 403 + `scheduler.deny`。缺口记台账 |
| 超时晚到结果 | ☑通过（本机） | `timeout_enters_sixty_second_cooldown` 新增断言：`timeout` 结算后 `finish(同一 run_id)` 返回 `Protocol`，任务保持 `Failed`，终态不被覆盖；上机未单独构造晚到回执 |
| 缓冲能力面 | ☑部分通过 | 本机回执/权限/限速全绿；测试机 testkit 未暴露 `buffer.*` 入口，无法上机复验（台账记缺口） |
| monitor / guard 过渡期 | ☑通过（偏差已记） | 两者 manifest 仍声明 `cron`，安装/加载记 `plugin.cron_deprecated`；运行依赖内核 legacy `run_cron`，源码无自建 `sleep`/`interval`/线程计时器；guard 61 次、monitor 11 次 `plugin.cron` 审计 |

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
| DOCKER-10 | Compose 子集静态校验 | `[本机]` 仓库根 | `cargo run -p xtask -- check`（`check-1panel` + `check-compose`）；再对一份含不支持字段的 compose 跑 `check-compose` | 样例全部通过；含不支持字段的 compose 被拒；`[已核对]` 无运行时入口，仅 `xtask check` 静态校验（本轮因本机 OOM 未跑） | □通过 / □失败 | `xtask`、`crates/compose`、`deploy/compose` |

最小回归命令：

```bash
curl -ks -o /dev/null -w 'containers=%{http_code}\n' -b "$J" "$B/$P/containers"
curl -ks -o /dev/null -w 'containers_error=%{http_code}\n' -b "$J" "$B/$P/containers?filter=error"
curl -ks -o /dev/null -w 'images=%{http_code}\n' -b "$J" "$B/$P/images"
curl -ks -o /dev/null -w 'networks=%{http_code}\n' -b "$J" "$B/$P/networks"
cargo test -p skiff-policy container_remove --locked
cargo test -p skiff-docker stale_kernel --locked
cargo test -p skiff-app container_busy_reason --locked
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
| DOCKER-09 | ⏸阻塞（本机 OOM） | `cargo test -p skiff-policy` 在 `skiff-model` 编译期 `memory allocation failed`；需充足内存或 CI 兜底 |
| DOCKER-10 | ⏸阻塞（本机 OOM） | `xtask check` 同样需本地编译；且无运行时 Compose 入口（仅静态校验） |

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
| DEPLOY-07 | 升级覆盖安装与数据保留 | `[上机]` 已有库与站点 | 备份旧二进制与 DB；覆盖安装新 release；重启；登录看站点/任务/插件 | 新版本启动；`user_version` 升到当前 schema（11）；站点、任务、插件数据保留；已装官方插件的 `versions/<ver>` 与随包内容一致（版本或内容不同则更新，`data/` 与既有授权不变）。`[已复验 2026-11-07]` 同包覆盖安装，配置 sha 不变、业务计数保留 | □通过 / □失败 | `deploy/install.sh`、`crates/store/src/db.rs`（迁移） |
| DEPLOY-08 | 卸载与 purge 边界 | `[上机]` 可丢弃环境 | `deploy/uninstall.sh`；再加 `--purge`；检查数据目录 | 默认卸载保留 `/www/skiff` 数据；`--purge` 明确删除数据与配置并提示。`[已复验 2026-11-07]` 默认卸载删程序保数据（`/www/skiff`、`/etc/skiff`、`/www/sites`），重装恢复一致 | □通过 / □失败 | `deploy/uninstall.sh` |
| DEPLOY-09 | aarch64 装机 | `[上机]` arm64 Linux | 安装 release arm64 包；起 systemd；建站访问 | 安装/启动/建站均成功；`uname -m` 为 aarch64。`[阻塞]` 无 arm64 实机：Release 已有 aarch64 包（7,675,401 B，`sha256:a780dd7f…`），仅核对资产存在，未实装（摘要见 [TEST_ENV.md](TEST_ENV.md) §6） | □通过 / □失败 | `deploy/pack-release.sh`、`.github/workflows/release.yml` |
| DEPLOY-10 | 1Panel Docker 受限形态 | `[上机]` 1Panel 环境 | 按商店包安装；尝试安装 fs/net 插件；打开三核心 | 面板可用；fs/net 插件因能力面收缩而拒装；docker/identity 正常。`[阻塞]` 无 1Panel 环境；商店包与 `init.sh` 能力面收缩声明已在仓库，未实装 | □通过 / □失败 | `deploy/1panel/`、`crates/plugin` |
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
| DEPLOY-09 | ⏸阻塞（无环境） | Release 已上架 `aarch64-unknown-linux-musl` 包（7,675,401 B，`sha256:a780dd7f…`）；复现条件=arm64 Linux + systemd |
| DEPLOY-10 | ⏸阻塞（无环境） | `deploy/1panel/apps/skiff/0.1.0` 已在仓库；复现条件=1Panel 主机装商店包，验 fs/net 拒装、docker/identity 可用 |
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
| PERF-07 | 无意义消耗定量：日志轮转/日志页/采样写放大 | `[上机]` PERF-02/PERF-03 通过 | 在最高并发档读 `skiff` 进程写字节与磁盘写；比对站点 `access.log` 尺寸；打开站点「日志」页一次；观察 `plugins/monitor/data/samples.json` 的 mtime 与尺寸 | 能给出「每请求额外写多少字节」「一次日志页读入多少字节」「监控每 5 分钟重写多少字节」三项实测值，并判定是否与请求量线性放大 | □通过 / □失败 | `crates/app/src/sites.rs::rotate_if_large`（copytruncate 整文件复制）、`crates/app/src/sites.rs::read_log`（全量 `read_to_string`）、`plugins/monitor/src/main.rs`（`samples.json` 全量重写） |
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
| PERF-07 | ☑通过（三项定量） | ①PHP 每请求 stdout 日志经 containerd-shim→dockerd，c32 合计 13.78% 单核；②30 s copytruncate 轮转使 `skiff` 写 1/215/464 KB/s（c4/c16/c32，≈161 B/请求），磁盘写 45/291/779 KB/s；③`monitor` `samples.json` 36,549 B 每 300 s 整文件重写（≈10.4 MB/天）；④`read_log` 整文件读入，`audit-env.local/access.log` 轮转前达 40 MB 级 |
| PERF-08 | ☑通过 | 临时文件与新增 `access.log.1` 已删；三容器 Up、两站 200；插件/授权/任务计数与压测前一致；`integrity=ok`、`user_version=11`；`NRestarts=0`；`MemAvailable` 约 3301 MB、`load1` 回落 |
| PERF-01 备注 | — | 首轮 `collect` 采样脚本变量被覆盖导致无数据，已修复；`load.py` 曾把 nginx `keepalive_requests` 到期断连计为 error，已改计 `reconnects` |

## K. FTP 与宿主机文件服务

> 重跑触发：改了 `ftp` 能力面（`crates/core/src/kernel/ftp.rs`）、`com.skiff.ftp` 插件、pure-ftpd 配置键、账号身份探测/`MinUID`/`Umask`、站点目录对齐或 fail2ban 策略。
> 执行前提：内核二进制与 `official/com.skiff.ftp-*.wbpkg` 均来自 GitHub Release 资产（下载后先 `sha256sum -c` 再安装）；本地/交叉编译产物只作编译级验证，**不作为部署来源，也不作为验收依据**。契约见 [KERNEL_FTP_CONTRACT.md](KERNEL_FTP_CONTRACT.md)。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| FTP-01 | 首次安装、幂等重入与卸载回收 | `[上机]` Release 安装后未装 pure-ftpd；能 `apt` | 插件「运行状态」点安装；再次点安装；检查 `pure-ftpd -v`、`systemctl status pure-ftpd`、`/etc/pure-ftpd/conf/*`、`iptables`/`nft` 无新规则；再点回收并确认 | 首次安装成功且自动启动；二次调用返回同样配置不重复装包；配置含 `ChrootEveryone`/`NoAnonymous`/`PureDB`/`PassivePortRange`/`Umask 002 002`/`MaxClientsPerIP`，且 `/etc/pure-ftpd/auth` 只保留 `50pure -> ../conf/PureDB`（`65unix`/`70pam` 已清理）；回收后服务停止、包可移除或至少禁用，站点账号全部失效 | △部分通过（2026-11-07 测试机：安装、二次重入、配置项与认证链通过；卸载回收未执行） | `crates/core/src/kernel/ftp.rs::install/uninstall/status`、`plugins/ftp/src/main.rs`、`plugins/ftp/ui/runtime.html` |
| FTP-02 | 账号名/密码/端口范围校验（纯函数） | `[本机]` | `cargo test -p skiff --bin skiff kernel::ftp --locked` | 非法账号名、过短密码、控制字符、非法端口区间全部被拒；登录名 `skiff_<slug16>_<fnv4>_<account>` 稳定且不同域名不碰撞 | 通过（2026-11-07 本机 `kernel::ftp` 7 项单测） | `crates/core/src/kernel/ftp.rs`（`valid_account_name`/`valid_password`/`valid_port_range`/`ftp_login`） |
| FTP-03 | 建号身份探测与 `MinUID` 只降不升 | `[上机]` PHP 站点 running；容器内 `www-data` 非 0 | 建账号；`docker exec -u www-data <php容器> id -u`；读 `/etc/pure-ftpd/conf/MinUID`；再对另一个 uid 不同的运行环境建号 | 账号 uid/gid 与容器内 `www-data` 一致；`MinUID` 取 `min(当前值, 探测 uid)`，不被上调；站点 `index/` 属主/属组改为该身份，目录 2775、文件 0664/0775 | 通过（2026-11-07 测试机：uid/gid 82 与容器 `www-data` 一致，`MinUID=82`，`index/` 目录 `drwxrwsr-x`、上传文件 `-rw-rw-r--`） | `crates/core/src/kernel/ftp.rs::resolve_identity/probe_id/ensure_min_uid/align_tree` |
| FTP-04 | FTP 上传的文件 PHP 容器内可读写 | `[上机]` FTP-03 通过；客户端可用（`curl`/`lftp`） | 用账号登录（显式 TLS 可选），上传 `probe.txt` 与 `probe.php`；在容器内 `cat`/`stat` 并写入同名文件；浏览器访问 `probe.php` | 上传后容器内可读且属主/属组与 `www-data` 一致；容器内写入成功（非只读）；HTTP 返回上传内容，无 403/500 | 通过（2026-11-07 测试机：`STOR` 226，宿主侧 `uid=82 gid=82 mode=775`，容器内 `www-data` 读得上传内容、写入 `php-write-ok`；浏览器访问 `probe.php` 与显式 TLS 未跑） | `crates/core/src/kernel/ftp.rs::align_tree`、`write_conf("Umask")`；站点 `index/` |
| FTP-05 | 越界、跨站与暴力登录被拒；删号回收与审计 | `[上机]` 两站各一账号；已装 fail2ban（`login_guard=true`） | 用 A 站账号 `cd ..`/绝对路径/软链尝试进 `log`、`conf`、B 站；连续错密码 3 次；删除 A 站账号后再登录；查 `/$P/audit` 中 `ftp.*` 行 | chroot 生效，越界与跨站全部 530/550；`MaxClientsPerIP=3`、fail2ban 10 分钟 3 次封禁生效；删号后登录失败且 `pureftpd.passwd` 无该行；审计含 `ftp.account.create/delete`、`ftp.deny.*`，无密码内容 | △部分通过（2026-11-07 测试机：删号后重新登录 530、`pureftpd.passwd` 无该行、审计有 `ftp.account.create/delete` 且无密码；越界/跨站、fail2ban 封禁、`ftp.deny.*` 未跑） | `crates/core/src/kernel/ftp.rs::ftp_call/dispatch/account_delete/ensure_login_guard`、`crates/core` 审计表 |

最小回归命令：

```bash
# 本机（编译级 + 纯函数单测）
cargo test -p skiff --bin skiff kernel::ftp --locked

# 测试机（安装后；$P 为安全入口前缀）
curl -ks -b "$J" -o /dev/null -w 'ftp_plugin=%{http_code}\n' "$B/$P/plugins/com.skiff.ftp"
curl -ks -b "$J" -H 'content-type: application/json' \
  -d '{"method":"list","params":{}}' "$B/$P/plugins/com.skiff.ftp/rpc"
docker exec -u www-data skiff-php-batch1 id
grep -H . /etc/pure-ftpd/conf/{MinUID,Umask,PureDB}
ls -l /etc/pure-ftpd/auth
systemctl is-active pure-ftpd fail2ban
```

## 附：待核对项结论（2026-11-07 已全部定性）

| 编号 | 结论 | 依据 |
| --- | --- | --- |
| FS-04 | 只有后端没有入口 | 内核 fs 能力面仅 read/write/list/mkdir/remove/rename；无 `fs.trash` 动词，也无 UI |
| FS-05 | 只有后端没有入口 | 配额在 `crates/fs` 内部生效，能力面与配置/UI 均未暴露 |
| TASK-06 | 只有后端没有入口 | `buffer.push`/`buffer.stat` 在能力面实现并有限速单测；testkit 无 `scheduler_call`，cron 只代理 `task.*` |
| TASK-07 | 有入口 | cron 新建任务落库为 `plugin:com.skiff.cron:<key>`、`source=plugin:<target>`，本轮实测创建/删除均 200 |
| DOCKER-10 | 只有静态校验，无运行时入口 | Web 路由无 Compose；仓库仅 `xtask check` 字段子集校验 |
| DEPLOY-09 | 因无 arm64 实机阻塞 | Release 有 aarch64 包（7,675,401 B，`sha256:a780dd7f…`）与 `.sha256`；复现条件=arm64 Linux + systemd，装包后验启动/建站 |
| DEPLOY-10 | 因无 1Panel 环境阻塞 | `deploy/1panel/apps/skiff/0.1.0` 已在仓库并声明 fs/net 收缩；复现条件=1Panel 主机装商店包后验 fs/net 拒装、docker/identity 可用 |
| DEPLOY-11 | 因无可复现超时/断网条件阻塞 | 正常下载+摘要校验通过（c39fe7d2…）；代码核对 `tools/iter.sh::cmd_fetch` 单次 `curl --max-time 600`，无重试/结构化错误输出；复现条件=对下载域名注入超时或丢包 |
