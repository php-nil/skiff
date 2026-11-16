# A 身份与会话 / B 菜单与导航 / C 插件体系

> 上级索引：[TEST_MATRIX.md](../TEST_MATRIX.md) ｜ 拆自 `docs/test/TEST_MATRIX.md` 原 A–C（2026-11-16 整理，正文未改）。

---

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
| PLUGIN-12 | 内置任务删除的 RPC 状态与文案 | `[上机]` 已装 cron | 调 `cron/rpc`，`{"method":"remove","params":{"id":"builtin:db-vacuum"}}` | HTTP 400（命令层与插件 RPC 统一按业务/参数错误返回）；正文保留「内置任务不可删除（只能停用）」；不得出现「请确认计划任务持有 scheduler 权限」 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`、`plugins/cron/src/main.rs` |

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
| PLUGIN-12 | ☑关闭（2026-11-15 复验，状态口径修正为 400） | `remove builtin:db-vacuum` 经插件 RPC 返回 **400** `shape.invalid`「内置任务不可删除（只能停用）」；同一动作走 `POST /ui/v1/tasks/builtin:db-vacuum/delete` 也是 **400** 与同一正文——命令层把 `BuiltinProtected` 归为业务错误（`TaskCommandError::Failed → BadRequest`），插件 RPC 层同一口径，**不再是 403**；正文未追加 scheduler 授权提示。`scheduler` 能力面路由自身的 403 仅用于「执行期回调 / 缺授权」，不经过这两个入口。「未装官方包不被自动安装」由 `crates/plugin/src/official.rs` 单测覆盖 |
