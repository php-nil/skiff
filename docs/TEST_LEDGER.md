# 测试台账（只记未闭环项）

> 口径：**只保留「正在处理」「待处理」两栏**。验证通过（PASS）的条目当轮删除，不留历史流水账。
> 每轮结束就地收敛；某条只有部分子断言通过时留在「正在处理」，并写清还差哪条。
> 每条必须能直接落地：问题描述、涉及模块/文件、当前状态（未开始 / 进行中 / 卡在什么条件）、下一步动作。
> 验收方法与分组见 [ITERATION.md](ITERATION.md) §5、[DEPLOY_TEST.md](DEPLOY_TEST.md) §6；用例的预期结果与代码位置归 [TEST_MATRIX.md](TEST_MATRIX.md)，测试机访问与发布资产归 [TEST_ENV.md](TEST_ENV.md)。本表只保留未闭环状态、最小复现与下一步，不再抄写 PASS 证据。

## 正在处理

| 事项 | 涉及模块/文件 | 当前状态 | 下一步 |
| --- | --- | --- | --- |
| 回滚协同与基线校验 | `tools/iter.sh`、`/etc/skiff-baseline`、`deploy/docker-bootstrap.sh` | **部分实测（2026-11-09）**：`deploy/docker-bootstrap.sh --check-only` 在 Docker 就绪的测试机 exit **0** 并正确识别「已就绪」；`/etc/skiff-baseline` 在本机不存在，基线名校验未构造；`tools/iter.sh rollback-wait` 需真实快照回滚（外部 hypervisor）+ `.workbuddy/server.env`，本机无法构造 | 在具备快照回滚的环境（双机/CI）先写最小 `server.env` 跑一次 `rollback-wait`；另手工创建 `/etc/skiff-baseline` 覆盖基线名校验 |
| files 编辑器弹窗关闭路径（保留插件 UI 入口时） | `plugins/files/ui/main.html`、`plugins/files/ui/app.js`、`crates/web/tests/main_flow.rs` | 修复已随 `v0.1.0` 发版（`.editor-dialog` 作者样式 `display:flex` 盖掉 `dialog:not([open])`，关闭/ESC/遮罩三条路径失效）；本地复现台同 CSP + mock RPC 真浏览器全过，测试机面板内未复验（入口已撤销，默认随入口关闭）；`com.skiff.files` 菜单入口已撤销，主路径改由内核 `/{p}/files` 承担 | 若保留插件 UI 入口，测试机浏览器复验三条关闭路径、未保存确认、关闭后焦点回列表；否则本项随入口一并关闭 |

## 待处理

| 事项 | 涉及模块/文件 | 当前状态 | 下一步 |
| --- | --- | --- |
| 菜单偏好按用户隔离（MENU-04 第二账号部分） | `crates/app/src/menu.rs`、`handlers/menu.rs`、`user_prefs` | **部分验证（2026-11-11 `e2d6240` 轮）**：仍无建号入口，本轮用 SQL 造临时 viewer 验证了角色过滤（受限项不出现、受限路由 403），但偏好隔离本身未测（`down`/`hide`/`reset` 只在 admin 与 admin 第二会话上验过） | 定稿是否恢复多账号能力；若有，则补建号入口并复验两账号各自偏好互不影响 |
| aarch64 仅 qemu-user 冒烟，未装机实测 | `deploy/pack-release.sh`、`.github/workflows/release.yml`、`docs/DEPLOY_TEST.md` | **部分实测（2026-11-09）**：包内二进制 `file` = `ELF 64-bit LSB executable, ARM aarch64, … statically linked, stripped`，`ldd` = `not a dynamic executable`；`qemu-aarch64-static` 10.0.13 下 `skiff version` → `skiff 0.1.0 / arch=arm64 os=linux`、`--help` 正常、`doctor` 全绿。仍无 arm64 实机，安装/ systemd/Docker/站点 200 全链路未验 | 借/租 aarch64 机器跑 `install.sh`、systemd、Docker、站点 200 全链路 |
| 真实 Release 下载偶发超时回退 API 端点 | `tools/iter.sh`、`.github/workflows/release.yml` | **未修复 / 阻塞（2026-11-08 复现）**：Windows 直连 `github.com` 与 `objects.githubusercontent.com` 均被重置，测试机 `curl` 直连超时；改走 `https://api.github.com/repos/<repo>/releases/assets/<id>` + `Accept: application/octet-stream`，经内网代理 `http://10.66.10.22:1010` 下载后 `scp` 传输，双架构 `sha256sum -c` OK。代码核对 `tools/iter.sh::cmd_fetch` 仍为单次 `curl --max-time 600`，无重试、无代理、无结构化错误输出 | 给 `cmd_fetch` 加有限重试（`--retry`/`--retry-all-errors`）、API 资产端点回退与代理开关，记录失败 URL/状态码/耗时；必要时换镜像域名 |
| 反代与 WS 残余：`trusted_proxies` 采信 `X-Forwarded-*` / 异源 WS 拒绝 / 反代站 WS 升级头 | `crates/web/src/handlers/`、`crates/core` | **部分通过 / 待外部条件**：TLS（自签同端口、`Secure` 回传、CSRF 不 403）已于 2026-11-03 实测；反代站 HTTP 200 与上游 `Host` 透传已过；仍缺 peer 命中 `trusted_proxies` 时采信 `X-Forwarded-Proto/For`、异源 WS 被拒、反代站 WS 升级头的实测环境。HSTS 代码未实现，不计验收 | 搭反代实测环境，补三项用例 |
| 插件 UI 浏览器内渲染与交互（430px / 跨浏览器 / 快捷键） | `plugins/*/ui/`、`crates/web/src/handlers/plugins*` | **部分通过**：2026-11-05 Playwright + Chromium 实测 files 0.1.2：目录浏览、Ctrl 多选、右键菜单、行内重命名、新建目录、编辑器修改保存、256 KiB 超限只读、主题跟随 light↔dark 均通过，并已修 iframe `sandbox` 缺 `allow-forms`；2026-11-08 已验 cron/FTP 菜单页渲染。仍差：插件 UI 430px 窄屏、Firefox/Safari、方向键/F2/Delete 快捷键矩阵 | 补 430px、Firefox/Safari 与快捷键矩阵 |
| systemd unit 对「写 `/etc`」插件的支持 | `deploy/systemd/skiff.service`、`deploy/` | **待决定（未开始）**：`ProtectSystem=full` 让 `/etc` 只读；测试机已用 `ReadWritePaths=/www` drop-in 解决（站点 vhost 落 `/www/sites/<域名>/conf`）。仓库 unit 已带 `ReadWritePaths=/www /run/skiff` | 决定是否给插件默认放行更多目录，并同步 unit 与部署文档 |
| G 组：闲置 RSS / 并发登录 / 与 1Panel 共存 | `deploy/`、`crates/core` | 长时观测，未开始 | 单独排长时观测轮次，不进每轮 |
| 多仓库单镜像：同一 image id 挂两个 RepoDigest 的 E2E 现场无法单机构造 | `crates/docker/src/client.rs`、`crates/app/src/sites.rs` | **部分实测（2026-11-10 `5c15b6b` 轮）**：`primary_digest_for` 按仓库名匹配的分支有单测 `client::tests::primary_digest_matches_repo_not_first_entry`（本地 gate 通过）；测试机上用本地 `registry:2` 造两个仓库（`php` / `php2`）分别推送同一镜像得到两条不同 digest，按 `sites.php_image` 切换后运行时登记的 digest 与仓库匹配。本地 registry 拉取后 image ID 与 RepoDigest 各自独立，无法造出「一个本地镜像同时挂两条不同仓库 digest」的现场 | 在真实仓库（同一镜像推两个 repo 标签后 `docker pull` 该两个引用）或 CI 中构造该现场，补一条 E2E |
| `skiff-server serve` 无逐请求访问日志，304 条件请求无法从服务端日志取证 | `tools/skiff-server/src/serve.rs` | **已实测（2026-11-11 `f863e84` 轮）**：serve 只打启动/错误日志（`[serve] …`），无 per-request access log；测试机无 `strace`/`socat`/`tcpdump`，HTTPS 下无法旁证请求头。304 目前由客户端状态文案「目录未更新（304）」+ curl 直测 304/无 body 双证 | 若需服务端侧证据：给 `serve` 加可选 `--access-log`（方法/路径/状态/条件头摘要，不回显完整路径）；或测试机装 `socat`/`tcpdump` 后用 `--http` 明文捕获 |
