# 工具链验收、数据备份与清理（§9–10）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §9–10，正文未改（2026-11-16 整理）。

---

## 9. 工具链验收（`tools/skiff-server` publish / sync，2026-11-10 `b094375`）


> 该提交只改 `tools/skiff-server`（新增 `publish` / `sync`），不动内核包，**本轮未重新部署面板**；面板仍为 §5 的 `c8bcb6e`。

- 执行位置：**测试机没有 Rust 工具链**（`cargo` / `rustc` / `skiff-server` 均不在 PATH，`rustc --version` 不存在），按约定未为其安装；**开发机没有 C 编译器**（`cl` / `clang` / `gcc` 均无，`ring` 构建脚本无法通过），未新编 Linux 产物。实际使用开发机既有 `target\debug\skiff-server.exe`（6,375,936 B，sha256 `b86b7fa77143fcfc40aa26bcb3453b94ae4135a5a851e6e271e3bd03dc043d4bb`，2026-11-10 20:07:25）；`cargo build -p skiff-server` 报 up-to-date，源码 = `b094375`（`2026-11-10T20:07:43+08:00`）。
- CI（同提交）：12 个 check-run = **11 success + 1 skipped**，其中 **`商店清单（skiff-server check）` success**；`单测（Linux）` / `单测（Windows）` success（两个平台都跑过 `tools/skiff-server/tests/p0.rs` 12 项与 `tests/sync.rs` 6 项）；skipped 项为 `编译（x86_64 musl 静态）`（本提交不触发内核打包路径）。
- publish（PASS，Windows 本地 CLI 实测）：
  - 缺私钥：不设 `SKIFF_SIGNING_KEY` → exit **1**「缺少环境变量 SKIFF_SIGNING_KEY…」，输出目录**不存在**，无任何清单；
  - 公私钥不匹配：seed A + `--public-key <pubB>` → exit **1**「公钥与私钥不一致」，输出目录不存在；
  - 正常发布：`manifest.seq=1`、`index` 声明的 sha256/size 与实测逐字节一致、3 个 blob 文件名 = 各自 sha256；`check --public-key <pubA>` 校验通过，`check --public-key <pubB>` 被拒；用 openssl（`pkeyutl -verify -rawin`，消息 = `SKIFF-SERVER-MANIFEST-v1\0` + manifest 原始字节）独立验签 **exit 0**；
  - seq 回退：catalog `seq=1` + `--prev <seq=2 产物>` → exit **1**「seq 未递增」，无输出。
- sync（PASS，Windows 本地 CLI 实测；源为本地目录，绕开 TLS）：
  - 空镜像：复用 0 / 新增 3；重复同步：复用 3 / 新增 0，清单未变化；
  - 预置 1 个 blob 的镜像：复用 1 / 新增 2（只补缺失，不重复写）；
  - seq 回退：`--from <seq=1>` 同步到 `seq=2` 镜像 → exit **1**「步骤 **3/6** seq 回退被拒绝：主站 1，镜像 2」，镜像清单保持 seq=2；
  - 源 blob 同长度改 1 字节 → exit **1**「步骤 **4/6** blob 摘要不匹配」，镜像无 `manifest.json`（只落了校验通过的 1 个 blob）；
  - 同步后人为篡改镜像内 blob → `check --dist <mirror> --public-key` 报 blob 摘要不匹配；再跑一次 sync 只补该 blob 并恢复自检通过；
  - 错误公钥 → 步骤 **1/6** 拒绝，镜像无输出。
- **FAIL（新发现，已于 `2bb5c90` 修复，见下）**：`--require-signature` / `--unsigned` 旗标解析失效。`parse()` 在 `tools/skiff-server/src/main.rs:213-216` 用 `strip_prefix("--")` 后的名字比较 `flags`，调用方传的却是带前缀的 `&["--unsigned"]`（L71）/ `&["--require-signature"]`（L121），永不匹配 → 旗标被当成「带值选项」：`check … --require-signature` 末尾 exit 1「参数 --require-signature 缺少取值」；`check … --require-signature --public-key <hex>` 把 `--public-key` 当值吞掉、`<hex>` 报「无法识别的参数」；`gen --unsigned --out X` 吞掉 `--out`。`publish` 内部仍强制验签（不受影响），但发布/同步门禁无法用严格模式。该缺陷已在 `2bb5c90` 修复并复验，已从 [TEST_LEDGER.md](../TEST_LEDGER.md) 出账。
- **`2bb5c90` 旗标修复复验（PASS，2026-11-10）**：用重新构建的 `target\debug\skiff-server.exe`（6,375,936 B，sha256 `03d5fea20db02760540416a0ebcac5c557ec5786ef43077fa4ed17cdbb69231b`，2026-11-10 20:19:52，源码 = `2bb5c90`），执行位置与 §9 上文一致（测试机无工具链、开发机无 C 编译器，未改环境）：
  1. `gen --unsigned --out U1`（无密钥）→ exit 0，14 个文件；`manifest.key_id` = 64 个 `0`、`manifest.json.sig` 的 `sig` 为空串；`--out` 正常识别，不再被吞。
  2. 设置合法 `SKIFF_SIGNING_KEY` 后再跑 `gen --unsigned` → exit 0，产物仍 `key_id` 全 0 / `sig` 空（`--unsigned` 压过环境密钥）。
  3. `gen` 不带 `--unsigned` 且无密钥 → exit 1「缺少环境变量 SKIFF_SIGNING_KEY…」，输出目录不存在。
  4. 未签名产物 U1：`check --dist U1` → exit 0（仅警告「未签名占位」）；`--require-signature` → exit 1「渠道 stable 未签名：P0 占位产物不得发布」；`--require-signature --public-key <pubA>` 同样 exit 1「未签名」，`<hex>` 被正常识别（无「无法识别/缺少取值」）；`--public-key <pubA>` 单独使用 → exit 1「签名 key_id 与固定公钥不一致」。反向证明 `--public-key` 真正生效：已签名 dist 加 `--require-signature --public-key <pubA>` → exit 0，换 `<pubB>` → exit 1 key_id 不一致。
  5. 回归（与上一轮逐步对齐）：publish 缺私钥 / 公私钥不匹配 / 正常发布（seq=1、index 摘要一致、blob 全匹配）/ `--prev` seq 未递增均一致；sync 首次（复用 0 / 新增 3）、重复（复用 3 / 新增 0）、预置 1 个 blob（复用 1 / 新增 2）、seq 回退（步骤 3/6）、源 blob 篡改（步骤 4/6，镜像无 `manifest.json`）均一致；`check mirror1 --require-signature --public-key` 通过。
  - 该提交同时新增 `tools/skiff-server/tests/cli.rs` 4 条旗标用例（真实子进程断言）；同提交 CI **11 success + 1 skipped**（跳过项为 `编译（x86_64 musl 静态）`，不触发内核打包路径），`商店清单（skiff-server check）` 与 `单测（Linux/Windows）` 均 success。
- **无法验证**：测试机上的 CLI 实测（无 `rustc` / `cargo` / `skiff-server`，且约定不为测试机装工具链）；Linux 目标二进制行为（未交叉编译；本轮以 Windows 产物验证同一套库与 CLI 逻辑）。

## 10. 数据、备份与清理

| 项 | 路径 / 约定 |
| --- | --- |
| 数据根 | `/www/skiff`（0711）、`/www/sites`、`/www/php`、`/www/runtime` |
| 配置 | `/etc/skiff/config.toml`（0644） |
| 数据库 | `/www/skiff/skiff.db`（0600） |
| 运维备份 | `/root/iter/backups/`、`/root/backups/`；保留 `builtin:db-vacuum` 与运维备份；本轮旧件与插件目录快照在 `/root/iter/m30/backup/`（上一轮在 `/root/iter/m29/backup/`），发布面测试 state/catalog 与 dist 快照在 `/root/iter/m25/`、`/root/iter/m29/dist25/`（seq 25；当前线上 seq 26），签名私钥在 `/root/iter/catalog-test/signing.pem`（仅测试机） |
| 清理 | 测试完成后清理临时站点、容器、账号与压测文件，机器恢复可用状态 |
| GitHub 出网 | 内网代理与 git 全局配置见 [ITERATION/02-ISSUES_ENV.md](../ITERATION/02-ISSUES_ENV.md) §7.1（不在本文重复） |
