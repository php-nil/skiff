# 测试台账（只记未闭环项）

> 口径：**只保留「正在处理」「待处理」两栏**。验证通过（PASS）的条目当轮删除，不留历史流水账。
> 每轮结束就地收敛；某条只有部分子断言通过时留在「正在处理」，并写清还差哪条。
> 验收方法与分组见 [ITERATION.md](ITERATION.md) §5、[DEPLOY_TEST.md](DEPLOY_TEST.md) §6。

## 正在处理

| 事项 | 已验证到的程度 | 差哪条 / 下一步 |
| --- | --- | --- |
| Docker 容器增删改查 / 启停 / 日志 / 资源限制 / Compose 子集运行时 | 非法动词 400、不存在容器 404、`/api/v1/docker/containers` 200 且与 `docker ps` 一致；`xtask check` 的 Compose 字段子集静态校验通过；本轮经网站运行环境实测 `images.pull` / `containers.create` / `start` / `remove` / `logs` | **差：通用容器的 create/remove 页面路由仍未开（本轮只在网站服务内走 `Docker::execute`）；资源限制与 Compose 运行时未重跑** |
| 网站内核化（运行环境 / 站点闭环 / 默认站点 / 概览块） | 10.10.15.4 实测：卸载 nginx 插件；配镜像加速后拉取 `nginx:alpine`+`php:8.5-fpm-alpine`；7 步安装全绿；建站后站点 200 且 `phpinfo` 为 8.5.12；停/启/删、日志、概览网站块（1 个/异常 1）均生效；库迁移 v3。默认站点：库迁移 v4 保留原站点；面板设 `demo.local` 为默认后 `00-skiff-default.conf` 由代码生成（裸访问 200 + 8.5.12），列表显示「默认」；取消后裸访问 404、恢复后 200；两站互切仅一个默认；删除默认站后 conf 与目录消失、裸访问 404；手工 conf 已由面板生成替代。站点寄宿迁 `/www`（数据根 `/www/skiff`、站点 `/www/sites`、vhost `/www/nginx/conf.d`、日志 `/www/logs/nginx`、socket `/www/php`），OpenResty 固定 digest + host 网络监听 80，PHP 仅 unix socket。迁移实测：面板 18080 与站点裸 80 均 200（`PHP Version 8.5.12`），`Host: demo.local` 200；两容器 `NetworkMode=host`、`LogConfig=local/10m`；`/www/php/php-fpm.sock` 660 root:nogroup（`setpriv --reuid=12345` 连接实测 Permission denied）；80 被 `python3 -m http.server` 占用时安装步骤 0 失败并指明 `pid … python3`，释放后重装恢复；`/www/logs/nginx` 随请求增长且面板日志页可读；旧 `/var/lib/skiff/skiff.db` mtime 停在迁移前。2026-11-04 闭环轮：站点 HTTPS（自签/上传/强制跳转/`nginx -t` 回滚）、`registry_mirror` 自动落盘 daemon.json、重启自恢复均实测通过（见 DEPLOY_TEST §2.13/§2.14/§9.7） | **差：真实浏览器视觉与 430px、WAF 仅预留不拦截** |
| 新界面（概览 / 应用 / 插件中心 / 详情 / 容器筛选）上机复核 | 测试机换新二进制后：概览、列表、发现/可更新、详情、`containers?filter=error`、设置全部 200；导航一级 5 项、应用 5 个二级项顺序正确；停用 files 后二级项消失且「应用」仍在，启用后恢复；详情无启停按钮、有异常容器入口；线上 CSS 含 `health-grid` / `overview-grid` / `plugin-cards` 与 3 处 860px 断点 | **差：430px 真实浏览器渲染/横向溢出未跑（本机无浏览器、Playwright Chromium 下载失败）；iframe 内交互未跑** |
| 菜单契约 P1（深链 / 菜单设置 / 布局 / 按用户偏好）上机 | schema v2 迁移到 `user_prefs` 成功；六一级与深链/旧 URL 全 200；5 个老包 `plugin/<id>/legacy` 均打开各自 UI；停用全部插件后「网站/工具」显示可行动空态且 CTA 200；上移/下移、隐藏/显示、固定/取消、恢复默认均生效；`overview` 与 `core/settings/menu` 锁定项无法隐藏；`layout=left` 两次刷新保持；临时第二用户看到默认菜单，管理员偏好不影响它，已删除该用户 | **差：860px 真实浏览器下两种布局的抽屉与横向溢出未跑（测试机与开发机均无浏览器，Playwright 下载失败）** |
| 菜单契约 P2（L3 标签栏 / `overview.health` 槽位 / 5 插件 `[[menu]]`）上机（unsigned） | 备份 `/root/backups/p2-pre-20261103-231558`（DB + 插件 data + 旧 official 包）；5 个旧 1.0 包 remove 后 data 6 个文件原样保留；5 个新包无 `sig` 条目、`key_id` 全 0，`verify --allow-unsigned` 5/5 通过，安装后详情显示「未验签」；nginx `sites/list|backup|settings`、cron `jobs/list|new`、guard `guard/containers|events`、monitor `monitor/overview|history`、files `files` 全 200，标签/active/iframe 一致；未知 L3 404；`/plugins/<id>` 与 `ui/index.html` 5/5 200；概览 `data-slot="overview.health"` + 真实 CPU/内存/负载/uptime；停用 monitor 回落原生 `health-grid`，启用恢复；Docker active/0 容器时 nginx/guard 二级存在；恢复 5 启用、偏好为空、临时会话已删、admin 口令未变 | **差：iframe 内真实点击/切标签视觉未跑（测试机无浏览器）；Docker 不可用 gate 未实测（未中断 Docker）；未签名包仅供本轮测试，正式签名/发布包未做** |
| 回滚协同与基线校验 | — | **差：`tools/iter.sh rollback-wait`、`/etc/skiff-baseline` 基线名校验、`docker-bootstrap.sh --check-only` 退出 2 三条从未实测** |

## 待处理

| 事项 | 为什么还没做 |
| --- | --- |
| C 组剩余：反代正向采信与 WS Origin | TLS 已于 2026-11-03 实测（自签同端口，`Secure` 回传、CSRF 不 403，见 DEPLOY_TEST §2.4.2）；HSTS 代码未实现，不计验收。仍缺「peer 命中 `trusted_proxies` 时采信 `X-Forwarded-Proto/For`」与「异源 WS 被拒」的实测环境 |
| 插件 UI 浏览器内渲染与交互 | 只验到 HTTP 200 与 CSP 头（`default-src 'none'; script-src 'self'`）、RPC 5/5 返回 200；iframe 内点击与 430px 真实渲染未跑；**深色主题下 iframe 内部换肤**只能由浏览器验证（curl 只能确认 `data-theme="light"` 与 `skiff.theme.request` 在 HTML/JS 中） |
| systemd unit 对「写 `/etc`」插件的支持 | `deploy/systemd/skiff.service` 的 `ProtectSystem=full` 让 `/etc` 只读；测试机已用 `ReadWritePaths=/www` drop-in 解决（站点 vhost 现落 `/www/nginx/conf.d`）。仓库 unit 已带 `ReadWritePaths=/www /run/skiff`，需另立一条决定是否给插件默认放行更多目录 |
| 1Panel Docker 受限形态 | 未上机；能力面收缩为 docker/identity，fs/net 插件应拒装 |
| G 组：闲置 RSS / 并发登录 / 与 1Panel 共存 | 长时观测，不进每轮 |
