# 测试台账（只记未闭环项）

> 口径：**只保留「正在处理」「待处理」两栏**。验证通过（PASS）的条目当轮删除，不留历史流水账。
> 每轮结束就地收敛；某条只有部分子断言通过时留在「正在处理」，并写清还差哪条。
> 每条必须能直接落地：问题描述、涉及模块/文件、当前状态（未开始 / 进行中 / 卡在什么条件）、下一步动作。
> 验收方法与分组见 [ITERATION.md](ITERATION.md) §5、[DEPLOY_TEST.md](DEPLOY_TEST.md) §6；用例的预期结果与代码位置归 [TEST_MATRIX.md](TEST_MATRIX.md)，测试机访问与发布资产归 [TEST_ENV.md](TEST_ENV.md)。本表只保留未闭环状态、最小复现与下一步，不再抄写 PASS 证据。

## 正在处理

| 事项 | 涉及模块/文件 | 当前状态 | 下一步 |
| --- | --- | --- | --- |
| 回滚协同与基线校验 | `tools/iter.sh`、`/etc/skiff-baseline`、`deploy/docker-bootstrap.sh` | **部分实测（2026-11-09）**：`deploy/docker-bootstrap.sh --check-only` 在 Docker 就绪的测试机 exit **0** 并正确识别「已就绪」；`/etc/skiff-baseline` 在本机不存在，基线名校验未构造；`tools/iter.sh rollback-wait` 需真实快照回滚（外部 hypervisor）+ `.workbuddy/server.env`，本机无法构造 | 在具备快照回滚的环境（双机/CI）先写最小 `server.env` 跑一次 `rollback-wait`；另手工创建 `/etc/skiff-baseline` 覆盖基线名校验 |
| 缓冲能力面（`buffer.push` / `buffer.stat`）缺上机入口（TASK-06） | `plugins/testkit`、`crates/app/src/tasks.rs`、`crates/sched/src/buffer.rs` | 本机单测已覆盖回执字段、仅本人任务、`scheduler:manage`/`scheduler:read` 权限门、每任务 100 点/秒限速，全部通过；2026-11-07 复核确认 testkit 仍无 `scheduler_call`/`buffer.*` 方法，cron 只代理 `task.*` | 给 testkit 增加 `buffer_push`/`buffer_stat` 探针方法（或在插件测试里构造最小调用），上机复验权限拒绝与限速；不改产品语义 |
| 内核进程崩溃修复（P0）后的并发 spawn 上机复验 | `crates/plugin/src/install.rs::shared_gid`、`crates/plugin/src/supervisor.rs::spawn/ensure`、`crates/core/src/server.rs` | **修复已随 `v0.1.0` 发版**；根因＝musl `getgrnam` 非线程安全（共享 static 双重 free）导致 GP fault/BUS，修复为 `getgrnam_r` + 调用方缓冲区，已于 2026-11-07 累计 60 轮、0 崩溃、`NRestarts=0`；其后新增「`ensure` 锁外 spawn / 孤儿回收」改动三关本地通过，但发版后并发 toggle/RPC 下同插件只剩一个进程未上机复验 | 测试机并发 toggle/RPC 打到同一插件，断言进程数恒为 1、无新 core、`NRestarts=0` |
| files 编辑器弹窗关闭路径（保留插件 UI 入口时） | `plugins/files/ui/main.html`、`plugins/files/ui/app.js`、`crates/web/tests/main_flow.rs` | 修复已随 `v0.1.0` 发版（`.editor-dialog` 作者样式 `display:flex` 盖掉 `dialog:not([open])`，关闭/ESC/遮罩三条路径失效）；本地复现台同 CSP + mock RPC 真浏览器全过，测试机面板内未复验；`com.skiff.files` 菜单入口已撤销，主路径改由内核 `/{p}/files` 承担 | 若保留插件 UI 入口，测试机浏览器复验三条关闭路径、未保存确认、关闭后焦点回列表；否则本项随入口一并关闭 |

## 待处理

| 事项 | 涉及模块/文件 | 当前状态 | 下一步 |
| --- | --- | --- |
| 站点 WAF 拦截未实现（产品待定项） | `crates/app/src/sites.rs`、`crates/app/src/nginx.rs`、`crates/web/src/handlers/` | **2026-11-09：占位配置已删除**。`KEY_WAF_MODE`（`sites.waf_mode`）、`SiteSettings::waf_rules_dir()`、`SiteEnvStatus` 两个字段、`bootstrap.rs` 与 `install.sh` 的 `/www/skiff/waf/rules` 目录预留均移除；无 UI、无规则来源、无加载/拦截逻辑 | 产品先定规则来源、开关位置与回滚口径，再实现并补「命中规则被拦」与「关闭后放行」用例 |
| 菜单偏好按用户隔离（MENU-04 第二账号部分） | `crates/web/src/menu.rs`、`handlers/menu.rs`、`user_prefs` | **已定性（2026-11-07）**：仍无建号入口——面板只有 `/{p}/password`，无用户 CRUD 路由/页面；`user_prefs` 按 user id 存偏好，数据层可隔离，但无法建第二账号实测 | 定稿是否恢复多账号能力；若有，则补建号入口并复验偏好隔离 |
| aarch64 仅 qemu-user 冒烟，未装机实测 | `deploy/pack-release.sh`、`.github/workflows/release.yml`、`docs/DEPLOY_TEST.md` | **部分实测（2026-11-09）**：包内二进制 `file` = `ELF 64-bit LSB executable, ARM aarch64, … statically linked, stripped`，`ldd` = `not a dynamic executable`；`qemu-aarch64-static` 10.0.13 下 `skiff version` → `skiff 0.1.0 / arch=arm64 os=linux`、`--help` 正常、`doctor` 全绿。仍无 arm64 实机，安装/ systemd/Docker/站点 200 全链路未验 | 借/租 aarch64 机器跑 `install.sh`、systemd、Docker、站点 200 全链路 |
| 真实 Release 下载偶发超时回退 API 端点 | `tools/iter.sh`、`.github/workflows/release.yml` | **阻塞（2026-11-08 复现）**：Windows 直连 `github.com` 与 `objects.githubusercontent.com` 均被重置，测试机 `curl` 直连超时；改走 `https://api.github.com/repos/<repo>/releases/assets/<id>` + `Accept: application/octet-stream`，经内网代理 `http://10.66.10.22:1010` 下载后 `scp` 传输，双架构 `sha256sum -c` OK。代码核对 `tools/iter.sh::cmd_fetch` 仍为单次 `curl --max-time 600`，无重试、无代理、无结构化错误输出 | 给 `cmd_fetch` 加有限重试（`--retry`/`--retry-all-errors`）、API 资产端点回退与代理开关，记录失败 URL/状态码/耗时；必要时换镜像域名 |
| 反代与 WS 残余：`trusted_proxies` 采信 `X-Forwarded-*` / 异源 WS 拒绝 / 反代站 WS 升级头 | `crates/web/src/handlers/`、`crates/core` | TLS（自签同端口、`Secure` 回传、CSRF 不 403）已于 2026-11-03 实测；反代站 HTTP 200 与上游 `Host` 透传已过；仍缺 peer 命中 `trusted_proxies` 时采信 `X-Forwarded-Proto/For`、异源 WS 被拒、反代站 WS 升级头的实测环境。HSTS 代码未实现，不计验收 | 搭反代实测环境，补三项用例 |
| 插件 UI 浏览器内渲染与交互（430px / 跨浏览器 / 快捷键） | `plugins/*/ui/`、`crates/web/src/handlers/plugins*` | 2026-11-05 Playwright + Chromium 实测 files 0.1.2：目录浏览、Ctrl 多选、右键菜单、行内重命名、新建目录、编辑器修改保存、256 KiB 超限只读、主题跟随 light↔dark 均通过，并已修 iframe `sandbox` 缺 `allow-forms`；2026-11-08 已验 cron/FTP 菜单页渲染。仍差：插件 UI 430px 窄屏、Firefox/Safari、方向键/F2/Delete 快捷键矩阵 | 补 430px、Firefox/Safari 与快捷键矩阵 |
| systemd unit 对「写 `/etc`」插件的支持 | `deploy/systemd/skiff.service`、`deploy/` | `ProtectSystem=full` 让 `/etc` 只读；测试机已用 `ReadWritePaths=/www` drop-in 解决（站点 vhost 落 `/www/sites/<域名>/conf`）。仓库 unit 已带 `ReadWritePaths=/www /run/skiff` | 决定是否给插件默认放行更多目录，并同步 unit 与部署文档 |
| 受限形态「文件管理」入口未按能力面处理（DEPLOY-10 附项） | `crates/web/src/menu.rs`、`crates/web/src/lib.rs`、`crates/web/src/middleware.rs`、`crates/web/src/handlers/menu.rs` | **2026-11-09 已修（待装机复验）**：入口按 `Form::DockerRestricted` 隐藏；`/{p}/files*` 单独成组挂 `fs_capability` 门禁，`/{p}/m/core/files*` 深链复用同一 `check_fs_capability`，直达均 403「当前部署形态不提供文件管理能力」；回归 `restricted_form_hides_and_rejects_files_management` 本地通过 | 发版后在 `form=docker` 探针容器复验入口消失、直达 403 文案可读；通过即删本行 |
| G 组：闲置 RSS / 并发登录 / 与 1Panel 共存 | `deploy/`、`crates/core` | 长时观测，未开始 | 单独排长时观测轮次，不进每轮 |
| 主机监控历史页「出站」把累计字节当速率（原 `AUDIT_UI_2026-11-06.md` C6） | `plugins/monitor/ui/app.js`、`plugins/monitor/src/main.rs` | **2026-11-08 代码核对仍在**：`app.js:159-160` 入站用 `rate(s.net_rx_rate)`、出站用 `bytes(s.net_tx)`，而 `net_tx` 是 `/proc/net/dev` 累计字节；概览卡片也只用 `net_rx_rate`，`net_tx_rate` 从未产生 | 采样补 `net_tx_rate`（差值/秒）并在历史表改用它；概览同时展示入/出站速率；补单测与上机对齐 |
