# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-26 还原后干净首装 + 九次就地升级（当前；包 `804140c`，`sha256=5e333f7f…`）**
> - 背景：测试机还原为干净系统，按「重新发版 → 干净装 → 装插件/建环境/建站点 → 全量测 → 修缺陷 → 重出包 → 就地升级 → 再测」跑完整一轮；三个产品缺陷见 `docs/changes/2026-11-26.md`。
> - **包序列**（都是 amd64 `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz`，覆盖同一 Release 资产）：
>   干净首装 `92949ea` 12,538,527 B `6c246450…`（Release `45673872060`）→ `070ff1d` 12,539,101 B `87a828e5…`（`45678981070`，修 `gzip_types`）→ `7810675` 12,541,372 B `d7fb85d8…`（`45683912296`，修回收站 CompileError）→ **`a915b6e` 12,542,260 B `c038e313…`（`45688148056`，修文案与错误落地页）** → `a9d8308` 12,547,045 B `6936edd6…`（`45822071089`，C5a：能力面 HTTP 形状上移 core + 防火墙 fail2ban 行为修复） → `64f64cc` 12,546,974 B `3098f223…`（`45825094092`，卸载后清空壳目录） → `563c0b2`（`45830698442`，jail.ensure 落盘 filter + exec 假成功） → `511388d`（`45834060083`，fail2ban-client -t 预检 + logpath） → `3073be1`（`45836672351`，backend 白名单 + 日志摘要） → **`804140c` 12,552,092 B `5e333f7f…`（`45839284735`，当前；日志提示区分三种情形）**。
> - **干净首装**：preflight 8/8 PASS → `sh install.sh --listen 0.0.0.0:18080 --tls off --no-start` exit **0** → 追加 `allow_insecure_http` 与 `[catalog]` → `systemctl restart skiff`。`skiff.service` enabled+active、`/healthz` 200、前缀 **`5e3yz2yr`**、`skiff doctor` 六项 ok、`official/` 7 包 + `public-key.txt`、自动建出 `php-8.5`（Docker 29.7.2）。
> - **九次就地升级**（都 `sh install.sh --no-start < /dev/null`，不传 `--listen/--tls`）：exit 0、`/etc/skiff/config.toml` 与升级前**逐字节相同**（`sha256=5c41e260…`，九次全一致）、`/healthz` 200、`official/` 保持 8 项（7 包 + 公钥）、已装插件与容器全保留。官方插件在服务启动时按随包内容自动换版：`plugins/com.skiff.firewall/versions/0.1.0` 的 mtime 与包时间一致，说明新版二进制已生效（不靠手写版本号）。
> - **能力面**：7 个官方插件（cron / firewall / ftp / guard / monitor / sources / testkit）全部已验签安装；`php-8.5`（`php:8.5.13-fpm-alpine`）running；OpenResty 已装；站点 `matrix.local`（PHP）/ `matrix-static.local`（静态）/ `matrix-proxy.local`（反代，上游 18081）三站 200，PHP 页 `PHP Version 8.5.13`。
> - **自建镜像**：开发机 `serve --dist target/skiff-server/dist --listen 0.0.0.0:8080 --http`；测试机三类清单 `source=registry`、`failed=false`（PHP 13 / Node 9 / Rust 1）。`serve` 是前台进程，随开发机会话退出（Windows 无 systemd）；断源时测试机按设计降级（PHP→`builtin`、Node/Rust→`catalog`），重启服务后 `GET /ui/v1/runtimes/versions?kind=…&refresh=true` 回到 `registry`。
> - **已知行为**：保存 PHP 运行参数 = 重启 PHP 容器，同环境站点约 3.2 秒 503（100 ms 采样 150 次、非 200 共 29 次）；`crates/docker/src/client.rs` 的自定义信号白名单决定了不做 `USR2` 热重载，文案已改为「重启 PHP 容器」并写明 503 窗口。
> - **本轮验收**：`accept.sh` 47 PASS / 0 FAIL / 5 SKIP（SKIP 均为本次调用不适用）；`round.sh` 106、`round_ext.sh` 30、`round_ext2.sh` 20，全 0 FAIL；浏览器 44 路由 **44 PASS / 0 FAIL**；回收站恢复 / 覆盖恢复 / 彻底删除三条路径实测通过。
> - **镜像侧用例**：MIRROR-08/09 本轮也跑了——篡改 `index.json`（等长换 `8.5.13`→`8.5.19`）被拒（声明 `2921ccf4…` vs 实得 `9c65de52…`），换另一把密钥签的产物被拒（清单 `key_id=7c30dc6c…` vs 公钥 `707cede3…`），两次都 `can_install=false` 且保留上次快照；`runtimes --channel stable` 4.3 秒幂等（PHP/Node/Rust 全「未变 13·9·1」，内置副本无变化，工作区干净）。临时 8081/8082 镜像与临时 dist 已删、客户端 `config.toml` 已按备份原样恢复（与升级前逐字节相同）。
> - 更早的部署轮次（2026-11-25 及以前）见 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md)。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此「某轮的资产 sha」只在该轮有效；对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表。

- **当前资产来源**：Release run `45839284735`（dispatch `ref=main`，head `804140c`，`publish=true`、`release_tag=v0.1.0`），success；资产覆盖于 2026-11-26 14:35 UTC（本机 22:35）。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`a915b6e`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果。
- **本轮同一 Release 覆盖了十次**（`92949ea`→`070ff1d`→`7810675`→`a915b6e`→`a9d8308`→`64f64cc`→`563c0b2`→`511388d`→`3073be1`→`804140c`），下表只列最后一次；前三次坐标见 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md)，中间几次的 run id 见上「包序列」。

| 资产 | 大小 | sha256 |
| --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,552,092 B | `5e333f7f26095416a8beabc338b5f6c350e597289a30c3379d1377072c1f2e0f` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | 111 B | 侧车，内容同上摘要（`d3d0f954…`） |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 11,974,674 B | `4fe4772dfc230c0fb3f983fcd3422a76f8da8e33c2e5d5b5d2efecaca306926f`（本轮仅 amd64 实测） |
| `skiff-server-x86_64-unknown-linux-musl` | 4,031,200 B | `e8cde7ad42d427aa139dfe6a43d59f7e9e6612fe43d77143cd9aac4cc35a7fde`（本轮未下载） |
| `skiff-server-aarch64-unknown-linux-musl` | 3,256,464 B | `0974d954…`（本轮未下载） |
| `checksums.txt` | 434 B | `bfb03f6cf56565a8d2ab5ad70497a70d0ac80905da15a18296ffc470d56e1739` |

- **下载与核对**：测试机 `curl` 直连 GitHub Release → 侧车 `sha256sum -c` → 包内 `checksums.txt` 复核（四次升级都做了双验，全 OK）。侧车声明的文件名带版本号，落盘改名会让 `sha256sum -c` 报 `FAILED open or read`——先按原名存再校验。
- **Release 触发方式**：开发机用 git credential manager 取 token 后 `POST .../actions/workflows/release.yml/dispatches`（`version=0.1.0`、`publish=true`、`release_tag=v0.1.0`、`skip_plugins=false`）。

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
