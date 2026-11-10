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
| 菜单偏好按用户隔离（MENU-04 第二账号部分） | `crates/web/src/menu.rs`、`handlers/menu.rs`、`user_prefs` | **待外部条件（2026-11-07 已定性）**：仍无建号入口——面板只有 `/{p}/password`，无用户 CRUD 路由/页面；`user_prefs` 按 user id 存偏好，数据层可隔离，但无法建第二账号实测 | 定稿是否恢复多账号能力；若有，则补建号入口并复验偏好隔离 |
| aarch64 仅 qemu-user 冒烟，未装机实测 | `deploy/pack-release.sh`、`.github/workflows/release.yml`、`docs/DEPLOY_TEST.md` | **部分实测（2026-11-09）**：包内二进制 `file` = `ELF 64-bit LSB executable, ARM aarch64, … statically linked, stripped`，`ldd` = `not a dynamic executable`；`qemu-aarch64-static` 10.0.13 下 `skiff version` → `skiff 0.1.0 / arch=arm64 os=linux`、`--help` 正常、`doctor` 全绿。仍无 arm64 实机，安装/ systemd/Docker/站点 200 全链路未验 | 借/租 aarch64 机器跑 `install.sh`、systemd、Docker、站点 200 全链路 |
| 真实 Release 下载偶发超时回退 API 端点 | `tools/iter.sh`、`.github/workflows/release.yml` | **未修复 / 阻塞（2026-11-08 复现）**：Windows 直连 `github.com` 与 `objects.githubusercontent.com` 均被重置，测试机 `curl` 直连超时；改走 `https://api.github.com/repos/<repo>/releases/assets/<id>` + `Accept: application/octet-stream`，经内网代理 `http://10.66.10.22:1010` 下载后 `scp` 传输，双架构 `sha256sum -c` OK。代码核对 `tools/iter.sh::cmd_fetch` 仍为单次 `curl --max-time 600`，无重试、无代理、无结构化错误输出 | 给 `cmd_fetch` 加有限重试（`--retry`/`--retry-all-errors`）、API 资产端点回退与代理开关，记录失败 URL/状态码/耗时；必要时换镜像域名 |
| 反代与 WS 残余：`trusted_proxies` 采信 `X-Forwarded-*` / 异源 WS 拒绝 / 反代站 WS 升级头 | `crates/web/src/handlers/`、`crates/core` | **部分通过 / 待外部条件**：TLS（自签同端口、`Secure` 回传、CSRF 不 403）已于 2026-11-03 实测；反代站 HTTP 200 与上游 `Host` 透传已过；仍缺 peer 命中 `trusted_proxies` 时采信 `X-Forwarded-Proto/For`、异源 WS 被拒、反代站 WS 升级头的实测环境。HSTS 代码未实现，不计验收 | 搭反代实测环境，补三项用例 |
| 插件 UI 浏览器内渲染与交互（430px / 跨浏览器 / 快捷键） | `plugins/*/ui/`、`crates/web/src/handlers/plugins*` | **部分通过**：2026-11-05 Playwright + Chromium 实测 files 0.1.2：目录浏览、Ctrl 多选、右键菜单、行内重命名、新建目录、编辑器修改保存、256 KiB 超限只读、主题跟随 light↔dark 均通过，并已修 iframe `sandbox` 缺 `allow-forms`；2026-11-08 已验 cron/FTP 菜单页渲染。仍差：插件 UI 430px 窄屏、Firefox/Safari、方向键/F2/Delete 快捷键矩阵 | 补 430px、Firefox/Safari 与快捷键矩阵 |
| systemd unit 对「写 `/etc`」插件的支持 | `deploy/systemd/skiff.service`、`deploy/` | **待决定（未开始）**：`ProtectSystem=full` 让 `/etc` 只读；测试机已用 `ReadWritePaths=/www` drop-in 解决（站点 vhost 落 `/www/sites/<域名>/conf`）。仓库 unit 已带 `ReadWritePaths=/www /run/skiff` | 决定是否给插件默认放行更多目录，并同步 unit 与部署文档 |
| G 组：闲置 RSS / 并发登录 / 与 1Panel 共存 | `deploy/`、`crates/core` | 长时观测，未开始 | 单独排长时观测轮次，不进每轮 |
| 多仓库单镜像：同一 image id 挂两个 RepoDigest 的 E2E 现场无法单机构造 | `crates/docker/src/client.rs`、`crates/app/src/sites.rs` | **部分实测（2026-11-10 `5c15b6b` 轮）**：`primary_digest_for` 按仓库名匹配的分支有单测 `client::tests::primary_digest_matches_repo_not_first_entry`（本地 gate 通过）；测试机上用本地 `registry:2` 造两个仓库（`php` / `php2`）分别推送同一镜像得到两条不同 digest，按 `sites.php_image` 切换后运行时登记的 digest 与仓库匹配。本地 registry 拉取后 image ID 与 RepoDigest 各自独立，无法造出「一个本地镜像同时挂两条不同仓库 digest」的现场 | 在真实仓库（同一镜像推两个 repo 标签后 `docker pull` 该两个引用）或 CI 中构造该现场，补一条 E2E |
| 商店目录仅支持受信 https，内网/自签不可用（2026-11-10 `0ee9065` 轮，`dc44024` / `5c15b6b` / `c8bcb6e` 轮复核仍在） | `crates/app/src/catalog/fetch.rs`、`crates/core/src/config.rs` | **限制 / 待产品决定**：ureq 用 webpki 内置根、无 native certs、无 `danger_accept_invalid_certs`；`mirrors` 仅接受 `https://`（http 启动即拒），自签 https 实测 `invalid peer certificate: CaUsedAsEndEntity`。正向链路只能在受信 https（serveo 反向隧道）验证；免费隧道域名会过期且**连接本身不稳定**——`5c15b6b` 轮收尾时隧道对同一域名返回 502，`c8bcb6e` 轮升级链路验收中途再次 502，`/store` 按设计降级为「离线 + 上次成功快照」并落 `catalog.refresh fail reason=offline`，每次重开隧道换域名、同步 `mirrors` 后恢复 | 若需支持纯内网，评估「显式导入自签 CA」或「受控 insecure 开关」（默认关闭 + 强审计）；不得直接放宽为允许任意 `http://` |
| Debian 系 PHP 扩展安装不可用（2026-11-11 `db7e579` 轮发现） | `crates/app/src/sites.rs`（扩展安装脚本 `SKIFF_LOCAL_IPE` 段） | **上机 FAIL（新发现）**：Debian 系 PHP 运行环境（`php:8.5-fpm`）提交 `runtime.ext.install` 立即 `failed exit_code=2`，日志 `sh: 5: cannot create /etc/apk/config: Directory nonexistent`；安装脚本无条件把本地 ipe 映射写入 Alpine 的 `/etc/apk/config`，Debian 无 `/etc/apk` 目录，`set -e` 直接中止。Alpine（`php:8.5-fpm-alpine`）不受影响，扩展可正常编译/安装（本轮 `redis` 走通，最后因已加载而 `exit_code=1`） | 按镜像系分支：仅 Alpine 写 `/etc/apk/config`，Debian 走 apt 路径；补一条 Debian 扩展安装回归 |
| `disabled_jails` 与实际 jail 可能不一致（2026-11-11 `db7e579` 轮发现） | `crates/plugin/src/firewall.rs`（`restart_and_verify`） | **上机边界 CASE**：人为放入晚排序的 `/etc/fail2ban/jail.d/zz-sshd.conf`（`[sshd] enabled=true`）后调 `fail2ban.start`，返回 `disabled_jails:["sshd"]` 并已向 `skiff-defaults.conf` 追加 `[sshd] enabled=false`，但同一返回体的 `jails` 仍是 `["pure-ftpd","sshd"]`（覆盖文件排序在 `skiff-defaults.conf` 之后，故障仍生效）；移除覆盖文件并重启后 sshd 消失。正常安装路径（`defaults-debian.conf` 排在 `skiff-defaults.conf` 之前）不触发 | 决定是否以「重启后实际仍活跃」为准修正 `disabled_jails`（或同时返回 `still_active`），并在契约里写明 jail.d 排序约束；该 CASE 目前只在人工构造下可复现 |
