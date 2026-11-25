# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-25 晚：换到含 11-25 源码的新包，就地升级两跳 + 补完 MIRROR 上机验收（当前；包 `a0f27c8`，`sha256=2d0e29b7…`）**
> - 背景：上一条记录用的是 11-24 的包，面板侧「三个版本清单」根本不存在（`/ui/v1/runtimes/versions` 被 `/runtimes/{id}` 吃掉）。本轮把 11-24 后半到 11-25 的积压改动提交、CI 转绿、出包，再就地升级沿用现场（不清库、不换前缀）。
> - **提交与门禁**：`7918bac`（代码）/ `7e3146f`（文档）/ `f194751`（`eol` 按日期比对 + `--help`）/ `a0f27c8`（修 11-24 遗留的 9 项 `main_flow` 断言）。CI run `45616188710`（提交 `a0f27c8`）**全绿**；同一提交的 Release run `45616962452` 出包。
> - **两跳升级**（都是 `sh install.sh --no-start < /dev/null`，不传 `--listen/--tls`）：`7e3146f` 包 12,538,394 B / `2c8297f0…` → `a0f27c8` 包 **12,538,544 B / `2d0e29b7…`**；两跳都 exit 0、`/etc/skiff/config.toml` 与升级前**逐字节相同**、`/www/skiff/official/` 收敛为 7 个包（手工清掉上一轮遗留的 `com.skiff.files-0.1.3.wbpkg`）、前缀仍是 `8au3xewy`、`/healthz` 200。
> - **升级副作用（已修）**：第一跳后版本下拉把 13 个 PHP 版本全标「已 EOL」——清单 `eol` 是日期，面板按「非空即 EOL」解析，日期字段当布尔用会全量命中；`f194751` 改成与今天比对，第二跳后 `eol: []`、0 处「已 EOL」。注意面板下拉读本地缓存，升包后要点一次「刷新版本清单」才重算。
> - **MIRROR 验收**（详见 [08-SELFHOST_MIRROR.md](../../TEST_MATRIX/08-SELFHOST_MIRROR.md) §2.1）：01~09 全部通过，含篡改摘要拒绝、异钥 `key_id` 拒绝、断源降级与新包的三类清单（PHP 13 / Node 9 / Rust 1，每分支 3 条）。

> **2026-11-25 干净重装：本机当服务端（`skiff-server serve`）+ 测试机当客户端的安装对接轮（历史；包同 `c841074`，未出新包）**
> - 背景：为验证「先装、再连镜像」这条链路，测试机现场（11-23 首装 + 两轮就地升级，前缀 `gk5y33tq`，库 `user_version=22`）整体备份后 **干净重装**；本轮**换包不换代**——仍用 11-24 的 amd64 包，只为验安装与目录对接，不带 2026-11-25 的源码改动（`runtime_index` 版本匹配修复、`/ui/v1/runtimes/versions` 端点均**不在**该包内）。
> - **备份**（重装前，测试机 `/root/iter/backup-20261125/`）：`config.toml.bak`（7,048 B）、`skiff-data.tgz`（4,122,767 B，含库与插件数据）、`sites-old/`、`php-old/`、`runtime-old/`。
> - **卸载**：`printf 'YES\n' | sh uninstall.sh --purge` → 退出 0；`/etc/skiff`、`/www/skiff`、`/usr/local/bin/skiff*`、unit 全部消失，`/www` 仅剩 `log`/`nginx`。
> - **安装**（2026-11-25 19:18 HKT = 11:18 UTC）：`sha256sum -c` 通过（`c905859d…9451b2`）→ `sh install.sh --listen 0.0.0.0:18080 --tls off --no-start` exit **0** → 追加 `allow_insecure_http = true` 与 `[catalog]` 段 → `systemctl restart skiff`。`skiff.service` enabled+active；`/healthz` 200；日志：初始 admin 口令生成、官方公钥 `ed73de3e…cd47` 自动信任。
> - **坐标变化**：入口前缀 **`gk5y33tq` → `8au3xewy`**；口令重置为 `/www/skiff/initial-admin.txt` 生成值（已同步 `/root/iter/admin-pw.txt`，0600，未改密）。坐标见 [01-ACCESS.md](01-ACCESS.md) §2/§3。
> - **形态三配置**（同上一轮口径）：`listen = "0.0.0.0:18080"`、`tls_enabled = false`、`allow_insecure_http = true`（人工追加）。
> - **新增 `[catalog]` 段**（人工追加，验对接用）：`enabled = true`、`channel = "stable"`、`mirrors = ["http://10.66.52.7:8080"]`、`public_key = "7a9cb5d3…da2b"`、`timeout_secs = 10`。出厂默认 `enabled = false`，不写这段就没有商店入口。
> - **自检**：`skiff doctor` 六项全 ok（数据根 / 站点根 / 库 / 配置 / TLS 成对 / Docker socket），另加一条「未配置宿主挂载授权根」的安全提示；`/www/skiff/official/` 8 个官方包 + `public-key.txt` 在位；启动后自动建出 `php-8.5` 运行环境（Docker 29.7.2 在位）。
> - **对接结果**：商店页 `message` = 「已从 http://10.66.52.7:8080 更新到 seq 1」，`key_configured=true`，`sections` 三类齐全（插件 1 / 应用模板 1 / 运行环境 23）；再刷新命中 304（「展示本地缓存」）；停掉镜像服务后为「镜像源不可达…Connection refused」，恢复后回到 304。逐项判定见 [04-ACCEPTANCE.md](04-ACCEPTANCE.md)，用例见 [TEST_MATRIX/08-SELFHOST_MIRROR.md](../TEST_MATRIX/08-SELFHOST_MIRROR.md)。
> - **未覆盖**：`accept.sh` 全量（本轮只跑 `doctor` + 手工断言，未跑 B 组改密与 A6 权限位全表）；浏览器巡检；运行环境清单的三类来源（该包无 `versions` 端点，面板侧 404 —— 见 [TEST_LEDGER.md](../TEST_LEDGER.md)）。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此「某轮的资产 sha」只在该轮有效；对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表。

- **当前资产来源**：Release run `45317927914`（dispatch `ref=main`，head `c841074`，`publish=true`、`release_tag=v0.1.0`），success；资产覆盖于 2026-11-24 03:17 UTC。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`c841074`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,923,388 B | `c905859dd6bdb72a48b0e85b7061f21f7476ffbcac194b49804fed2d459451b2` | （本轮） |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | — | （侧车，内容同上摘要） | （本轮） |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 12,388,716 B | （本轮仅 amd64 实测） | （本轮） |
| `skiff-server-x86_64-unknown-linux-musl` | 3,941,088 B | （本轮未下载） | （本轮） |
| `checksums.txt` | 434 B | （本轮未下载） | （本轮） |

- **下载与核对**：本轮走「测试机 curl 直连 GitHub Release → 侧车 `sha256sum -c` → 包内 `checksums.txt` 复核」。
- **Release 触发方式**：开发机用 git credential manager 取 token 后 `Invoke-RestMethod -Uri .../actions/workflows/release.yml/dispatches -Method Post` 直接 dispatch（`git push` 已通，网络恢复）。

### 6.1 镜像侧产物（开发机 `serve` 托管，2026-11-25）

这套产物是**发布侧**，不属于 Release 资产；由开发机 `publish` 生成，测试机从它取数。

| 项 | 值 |
| --- | --- |
| 产物目录 | 开发机 `target/skiff-server/dist`（18 个文件：`channels/stable/*` 4 个 + `runtime-index/*` 3 个 + `blobs/sha256/*` 3 个 + `schema/*` 8 个） |
| 发布命令 | `SKIFF_SIGNING_KEY=<seed> SKIFF_SERVER_PUBLIC_KEY=<pub> skiff-server publish --catalog deploy/skiff-server/catalog --schema deploy/skiff-server/schema --out target/skiff-server/dist` |
| 渠道清单 | `channel=stable`、`seq=1`、`generated_at=2026-11-25T10:07:00Z`（与 `catalog/stable/channel.json` 一致） |
| 签名 | `manifest.key_id=707cede37fad5c348298ac466c726e248dc7e416dd697e20fbf75de04c441ac2`（= sha256(公钥 `7a9cb5d3…da2b`)）；Ed25519 detached，`manifest.json.sig` 244 B |
| 索引 | `index.json` 8,303 B、`sha256=2921ccf4b45e3e6db210922c5135a244d077277ca39f2fdc03e1076e0f10df61`；条目 plugins 1 / apps 1 / runtimes 23 / upgrades 1 |
| 版本清单（不签名） | `runtime-index/php.json` 1,030 B `5871e1cd…`；`node.json` 777 B `60af05e0…`；`rust.json` 214 B `19a13fd7…`（每份都是产物同一份字节，`check --builtin-index` 对 php 报「内置副本一致：1 份」） |
| 发布二进制 | 开发机 `target/debug/skiff-server.exe` 12,793,856 B `sha256=f8f9d2acc20da88d3d5afe89b7f2d6613cbb1cc196c8f425568b2f273167983b`（Windows 构建，仅本机演练用；正式分发用 Release 里的 musl 静态二进制） |
