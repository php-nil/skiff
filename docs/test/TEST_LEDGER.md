# 测试台账（只记未闭环项）

> 口径：**只保留「正在处理」「待处理」两栏**。验证通过（PASS）的条目当轮删除，不留历史流水账。
> 每轮结束就地收敛；某条只有部分子断言通过时留在「正在处理」，并写清还差哪条。
> 每条必须能直接落地：问题描述、涉及模块/文件、当前状态（未开始 / 进行中 / 卡在什么条件）、下一步动作。
> 验收方法与分组见 [ITERATION/01-ROUND.md](ITERATION/01-ROUND.md) §5、[DEPLOY_TEST/06-CASES.md](DEPLOY_TEST/06-CASES.md) §6；用例的预期结果与代码位置归 [TEST_MATRIX.md](TEST_MATRIX.md)，测试机访问与发布资产归 [TEST_ENV.md](TEST_ENV.md)。本表只保留未闭环状态、最小复现与下一步，不再抄写 PASS 证据。

## 正在处理

| 事项 | 涉及模块/文件 | 当前状态 | 下一步 |
| --- | --- | --- | --- |
| 回滚协同与基线校验 | `tools/iter.sh`、`/etc/skiff-baseline`、`deploy/docker-bootstrap.sh` | **部分实测（2026-11-09）**：`deploy/docker-bootstrap.sh --check-only` 在 Docker 就绪的测试机 exit **0** 并正确识别「已就绪」；`/etc/skiff-baseline` 在本机不存在，基线名校验未构造；`tools/iter.sh rollback-wait` 需真实快照回滚（外部 hypervisor）+ `.workbuddy/server.env`，本机无法构造 | 在具备快照回滚的环境（双机/CI）先写最小 `server.env` 跑一次 `rollback-wait`；另手工创建 `/etc/skiff-baseline` 覆盖基线名校验 |

## 待处理

| 事项 | 涉及模块/文件 | 当前状态 | 下一步 |
| --- | --- | --- |
| 维护锁争用 1.5s 超时路径不可构造（已知限制） | `crates/store/src/db/maintenance.rs`、`crates/core/src/commands.rs` | **实测（2026-11-12 `13a054c`）**：CLI `skiff backup` 与内置备份均为第二连接只读 `VACUUM INTO`，WAL 下读写不互斥；18 次交错面板侧 17–53 ms 全 `ok`、CLI 10/10 rc=0；持 `BEGIN IMMEDIATE` 3.5 s 与 `BEGIN EXCLUSIVE` 9.5 s 时内置备份仍 23 / 82 ms 成功；`locking_mode=EXCLUSIVE` 在本环境未真正持锁（面板 30–54 ms 正常）。同期 `/{p}/sites` 曾等待 5,046 ms（主连接短事务等 `busy_timeout=5s`） | 若要验 1.5 s 失败路径，需真正挡住第二连接读快照的条件（非 WAL/独占文件锁/专用测试库）；不修改 CLI 备份语义，当前按 PASS + 已知限制收口 |
| aarch64 仅 qemu-user 冒烟，未装机实测 | `deploy/pack-release.sh`、`.github/workflows/release.yml`、`docs/test/DEPLOY_TEST.md` | **部分实测（2026-11-09）**：包内二进制 `file` = `ELF 64-bit LSB executable, ARM aarch64, … statically linked, stripped`，`ldd` = `not a dynamic executable`；`qemu-aarch64-static` 10.0.13 下 `skiff version` → `skiff 0.1.0 / arch=arm64 os=linux`、`--help` 正常、`doctor` 全绿。仍无 arm64 实机，安装/ systemd/Docker/站点 200 全链路未验 | 借/租 aarch64 机器跑 `install.sh`、systemd、Docker、站点 200 全链路 |
| 真实 Release 下载偶发超时回退 API 端点 | `tools/iter.sh`（`cmd_fetch`） | **已加固，待实测（2026-11-21）**：2026-11-08 复现 Windows 直连 `github.com` / `objects.githubusercontent.com` 被重置、测试机 `curl` 直连超时，当时靠 API 资产端点 + 内网代理 `http://10.66.10.22:1010` 手工绕过。现改为：每轮**重新取签名地址**再下（redirect 几分钟过期，复用必然失败）、`--retry 2`（不依赖 curl 7.71 的 `--retry-all-errors`）、`ART_PROXY`（默认沿用 `GH_PROXY`）代理开关、诊断打印 host / HTTP 码 / 字节数 / 耗时，三次失败给三条排查指引。本机只做 `sh -n` 语法校验。**2026-11-21 23:10 轮旁证**：测试机经 `api.github.com` 资产端点一次下载成功（12,725,968 B，侧车摘要一致），未走 `iter.sh`，故本项不关闭 | 在有网环境跑一次 `tools/iter.sh fetch`，覆盖「直连成功 / 经代理成功 / artifact 过期」三条路径 |
| 反代与 WS 残余：`trusted_proxies` 采信 `X-Forwarded-*` / 异源 WS 拒绝 / 反代站 WS 升级头 | `crates/web/src/handlers/`、`crates/core` | **部分通过 / 待外部条件**：TLS（自签同端口、`Secure` 回传、CSRF 不 403）已于 2026-11-03 实测；反代站 HTTP 200 与上游 `Host` 透传已过；仍缺 peer 命中 `trusted_proxies` 时采信 `X-Forwarded-Proto/For`、异源 WS 被拒、反代站 WS 升级头的实测环境。HSTS 代码未实现，不计验收 | 搭反代实测环境，补三项用例 |
| 插件 UI 浏览器内渲染与交互（430px / 跨浏览器 / 快捷键） | `plugins/*/ui/`、`crates/web/src/handlers/plugins*` | **部分通过**：2026-11-08 已验 cron/FTP 菜单页渲染；iframe `sandbox` 缺 `allow-forms` 的问题是历史缺陷（已修）。仍差：插件 UI 430px 窄屏、Firefox/Safari、方向键/F2/Delete 快捷键矩阵 | 补 430px、Firefox/Safari 与快捷键矩阵 |
| systemd unit 对「写 `/etc`」插件的支持 | `deploy/systemd/skiff.service`、`deploy/` | **待决定（未开始）**：`ProtectSystem=full` 让 `/etc` 只读；测试机已用 `ReadWritePaths=/www` drop-in 解决（站点 vhost 落 `/www/sites/<域名>/conf`）。仓库 unit 已带 `ReadWritePaths=/www /run/skiff` | 决定是否给插件默认放行更多目录，并同步 unit 与部署文档 |
| G 组：闲置 RSS / 并发登录 / 与 1Panel 共存 | `deploy/`、`crates/core` | 长时观测，未开始 | 单独排长时观测轮次，不进每轮 |
| 多仓库单镜像：同一 image id 挂两个 RepoDigest 的 E2E 现场无法单机构造 | `crates/docker/src/client.rs`、`crates/app/src/sites.rs` | **部分实测（2026-11-10 `5c15b6b` 轮）**：`primary_digest_for` 按仓库名匹配的分支有单测 `client::tests::primary_digest_matches_repo_not_first_entry`（本地 gate 通过）；测试机上用本地 `registry:2` 造两个仓库（`php` / `php2`）分别推送同一镜像得到两条不同 digest，按 `sites.php_image` 切换后运行时登记的 digest 与仓库匹配。本地 registry 拉取后 image ID 与 RepoDigest 各自独立，无法造出「一个本地镜像同时挂两条不同仓库 digest」的现场 | 在真实仓库（同一镜像推两个 repo 标签后 `docker pull` 该两个引用）或 CI 中构造该现场，补一条 E2E |
| fail2ban 首次 `jail.ensure` 后 socket 未就绪竞态 | `crates/plugin/src/firewall.rs`（`restart_and_verify` / `fail2ban-client` 调用） | **观察项（2026-11-15 上机）**：全新 `fail2ban.install` 后立即 `jail.ensure`（jail 落盘并 `restarted:true`），紧接着 `fail2ban.banned` 返回 **500**（`fail2ban-client` 连不上 socket）；1–2 秒后重试或直接 `ban` 均成功，`jail.remove`/`uninstall` 正常。仅首次安装重启后命中一次，未复现第二次 | 若要消除：`restart_and_verify` 成功后轮询 `/var/run/fail2ban/fail2ban.sock`（有限退避）再返回；或让 `banned/ban/unban` 对 socket 未就绪做短重试。需与内核一起出包后上机复验 |
| PHP 扩展首装的软件包源（已定稿，只剩耗时复验） | `crates/app/src/runtime_catalog.rs`（`installer_prelude`、`APK_MIRRORS`）、`crates/app/src/sites/crud.rs`（`save_apk_mirror`）、`crates/web/static/budding/budding-boot.js`（新建环境表单） | **已实现（2026-11-19 `29eb9f2`）**：新建运行环境可选软件包源（`auto` = 容器内实测择快 / 固定源 / 自填源根），固定选择写 `<runtime>/cache/apk/.skiff-mirror` 且首行 `# skiff-fixed` 永不被探测覆盖；空值保持镜像默认。**产品决定（2026-11-21）不再做安装器回落**——源连不通由 `com.skiff.sources` 插件测速与用户自行判断，安装器不替用户改回默认源 | 上机复验：清 `<runtime>/cache/apk` 后固定一个源装 `php-8.5` + 一个扩展，确认 prelude 打印「用户固定」且耗时下降；然后把本行降级为 PASS 删除 |
| 运行环境版本清单未签名（可被伪造） | `tools/skiff-server/src/build.rs`、`crates/app/src/runtime_index.rs`、`docs/plugin/SKIFF_SERVER/01-DESIGN.md` §2.2 | **设计边界（2026-11-24）**：清单走内网 HTTP、不签名，只能靠 `generated_at` 挡陈旧镜像回滚，挡不住伪造（能改文件的人也能改时间戳）。已写进契约 §4.5 与 `07-SERVE.md` §16.5 | 二期：把 `runtime-index/<kind>.json` 的 sha256 写进已签名 `channels/<ch>/index.json`，面板启用 catalog 时校验摘要；在此之前不声称清单可信 |
