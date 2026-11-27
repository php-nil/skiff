# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-27 还原后干净首装 + 一次就地升级（当前；包 `3fbf8e2`，`sha256=baac26fb…`）**
> - 背景：测试机再次还原为干净系统，按「重新发版 → 干净首装 → 装插件/建环境/建站 → 全量测 → 修缺陷 → 重出包 → 就地升级 → 再测」跑完整一轮；本轮修掉一个站点生命周期缺陷（站点启停/别名/建站/默认站切换/删站误用重启容器），见 `docs/changes/2026-11-27.md` 四。
> - **包序列**（都是 amd64 `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz`，覆盖同一 Release 资产）：
>   干净首装 `cda4553` 12,551,605 B `6b6074ea…`（CI `45938609570` 绿 / Release `45939138922`）→ **`3fbf8e2` 12,552,580 B `baac26fb3ee8933c303eb02b763d012145c2a622808f46779ee2e2981eeba1ef`（CI `45941201464` 绿 / Release `45942009500`，当前）**。
> - **干净首装**：preflight 7/7 PASS（PF3 时钟需 `--now`、未传则 SKIP）→ `sh install.sh --listen 0.0.0.0:18080 --tls off --no-start` exit 0 → 追加 `allow_insecure_http`、`allow_reduced_sandbox`、`[catalog]` → `systemctl start skiff`。`skiff.service` active、`/healthz` 200、前缀 **`gu9uhtu3`**、`skiff doctor` 全 ok、`official/` 7 包 + `public-key.txt`。**`php-8.5` 不会自动创建**（代码里没有这条路径），本轮用 `POST /ui/v1/runtimes {"kind":"php","version":"8.5"}` 建实例（约 30 s 拉镜像 + 启动）。
> - **就地升级**（`sh install.sh --no-start < /dev/null`，不传 `--listen/--tls`）：exit 0、`/etc/skiff/config.toml` 与升级前**逐字节相同**（`sha256=cc50e3b2…`）、前缀不变、`/healthz` 200、7 插件与 2 容器全保留。
> - **能力面**：7 个官方插件（cron / firewall / ftp / guard / monitor / sources / testkit）全部已验签安装（沙箱降级：本机 landlock 检测未通过，需 `allow_reduced_sandbox = true`），各自以独立 UID 运行、RPC 可用；`php-8.5`（`php:8.5-fpm-alpine`，真实版本 `8.5.13`）running；OpenResty 已装；三站 `matrix.local`（PHP）/ `matrix-static.local`（静态）/ `matrix-proxy.local`（反代，上游 18081）均 200，PHP 页 `PHP Version 8.5.13`。
> - **插件任务**：CLI `skiff plugin install` 不通知运行中的服务，manifest `[[task]]` 要到下次启动才种入（契约只承诺「启动装配 + Web 安装/升级成功后」）；本轮装完插件重启一次，任务中心出现 `plugin:com.skiff.guard:reconcile` / `monitor:sample` / `testkit:busy-probe` 三条。
> - **自建镜像**：开发机 `target/debug/skiff-server.exe serve --dist target/skiff-server/dist --listen 0.0.0.0:8080 --http`；测试机三类清单 `source=registry`、`failed=false`。`serve` 是前台进程，随开发机会话退出（Windows 无 systemd）。
> - **本轮验收**：`accept.sh` 34 PASS / 0 FAIL / 3 SKIP（preflight+A+D0）+ B 组 15 PASS / 0 FAIL / 1 SKIP；`round.sh` 106、`round_ext.sh` 32、`round_ext2.sh` 20，全 0 FAIL；浏览器 44 路由 **44 PASS / 0 FAIL**。
> - **已知行为（非缺陷）**：reload 方式生效是异步收敛——加别名后首个请求仍 404、停站后首个请求仍 200，`0.2–0.5 s` 后变成预期值（旧 worker 排空期间按旧配置应答）；用例按「2 秒内收敛」判。保存 PHP 运行参数 = 重启 PHP 容器，同环境站点约 3.2 秒 503。
> - 更早的部署轮次（2026-11-26 及以前）见 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md)。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此「某轮的资产 sha」只在该轮有效；对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表。

- **当前资产来源**：Release run `45942009500`（dispatch `ref=main`，head `3fbf8e2`，`publish=true`、`release_tag=v0.1.0`），success；资产覆盖于 2026-11-27 04:57 UTC（本机 12:57）。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`3fbf8e2`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果。
- **本轮（11-27）同一 Release 覆盖两次**（`cda4553`→`3fbf8e2`），下表只列最后一次；`cda4553` 的坐标为 12,551,605 B `6b6074ea…`（Release `45939138922`）。历史轮次的覆盖记录见 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md)。

| 资产 | 大小 | sha256 |
| --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,552,580 B | `baac26fb3ee8933c303eb02b763d012145c2a622808f46779ee2e2981eeba1ef` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | 111 B | 侧车，内容即上一行摘要 |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 11,985,180 B | 本轮未下载（仅 amd64 实测） |
| `skiff-server-x86_64-unknown-linux-musl` | 4,031,200 B | `e8cde7ad42d427aa139dfe6a43d59f7e9e6612fe43d77143cd9aac4cc35a7fde`（本轮未下载） |
| `skiff-server-aarch64-unknown-linux-musl` | 3,256,464 B | `0974d954…`（本轮未下载） |
| `checksums.txt` | 434 B | `bfb03f6cf56565a8d2ab5ad70497a70d0ac80905da15a18296ffc470d56e1739` |

- **下载与核对**：测试机 `curl` 直连 GitHub Release → 侧车 `sha256sum -c` → 包内 `checksums.txt` 复核（本轮两次出包都做了双验，全 OK；GitHub 下载偶发 `OpenSSL ... unexpected eof`，用 `--retry 3 -C -` 续传即可）。侧车声明的文件名带版本号，落盘改名会让 `sha256sum -c` 报 `FAILED open or read`——先按原名存再校验。
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
