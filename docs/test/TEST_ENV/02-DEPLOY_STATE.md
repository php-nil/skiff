# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-22 22:47 测试机还原后干净首装 + budding 1.8.5 同步 / 弹窗页脚提交（当前；HEAD `76412df`，其后仅文档提交）**
> - 背景：测试机还原为 T1（无 skiff / 无 `/etc/skiff`，18080 空闲，Docker 29.7.2 在位）；同步上游 budding `a3a3c8d`（1.8.5），弹窗保存/重置统一放图层页脚，新建入口收敛为弹窗，并按测试矩阵完成首轮验收与修复后升级复验。
> - **首装**：`install.sh --listen 0.0.0.0:18080 --tls off` exit 0；前缀 **`dwkaf8si`**；`skiff.service` active；healthz `{"docker":"available","status":"ok","version":"0.1.0"}`；人工追加 `allow_insecure_http` / `allow_reduced_sandbox`。
> - **首装资产**：Release run `45028320777`（head `e3066e6`），amd64包 12,971,860 B，`sha256=38eb2d64…46096a`（测试机 GitHub download 直连 + 侧车校验一致）。
> - **首轮验收**：`accept.sh --group all` **49 PASS / 0 FAIL / 3 SKIP**（PF4 装后端口、A6h 改密机无 initial-admin、B4 未开限流）；浏览器：概览、新建站点弹窗、防火墙「放行端口」弹窗与提交（新增 18081/tcp）、文件新建目录均通过；插件 schema 确认页脚 reset/close/submit + 注入 no-op api。
> - **首轮修复（`5fa12e2`，SHELL_VERSION 42）**：新建目录与编排失败改走 `formError`，错误留在弹窗内并保留输入。复验同名目录返回「新建目录失败：目标已存在」，输入仍为 `ui-check-1122`，无 CompileError。
> - **升级复验资产（中间）**：Release run `45030832428`（head `5fa12e2`），amd64包 12,971,860 B id `779283820`，`sha256=dec1254e68e0c4efe0eeea3dfff4e72a1f7a2313437cb22819fc66f57c40a42e`；就地升级 exit 0，前缀/口令/插件数据保留。
> - **最终升级资产**：Release run `45033701573`（head `76412df`），amd64包 12,971,630 B id `779372007`，`sha256=ddd297af831bf7f2d41170209b9ed41c1858b926149e690481388c12c4210e18`（侧车 id `779371978`）；追加「站点附加域名 / 设置公钥导入」弹窗化后就地升级，exit 0。
> - **升级后 accept**：三次 `--group all --password '…'` 均 **49 PASS / 0 FAIL / 3 SKIP**（SKIP 同上）。
> - **插件**：官方包目录经并发操作后仅剩 firewall 可装；已装 `com.skiff.firewall` 并实测弹窗提交。cron 包缺失（`/www/skiff/official` 无 cron 资产），未能在本轮验证 cron 弹窗，代码与 schema 断言已覆盖。
> - **浏览器**：概览、`/sites/list` 新建站点弹窗、防火墙 rules 弹窗与提交、`/files` 新建目录成功/失败复验、`/settings/security` 导入信任公钥弹窗，全部 0 CompileError / 0 console error。
> - **登录**：`admin`，口令取测试机 `/root/iter/admin-pw.txt`（0600）。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此「某轮的资产 sha」只在该轮有效；对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表。

- **当前资产来源**：Release run `45033701573`（dispatch `ref=main`，head `76412df`，`publish=true`、`release_tag=v0.1.0`），success；资产覆盖于 2026-11-22 14:46 UTC。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`76412df`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,971,630 B | `ddd297af831bf7f2d41170209b9ed41c1858b926149e690481388c12c4210e18` | `779372007` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | — | （侧车，内容同上摘要） | `779371978` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 12,386,158 B | （本轮仅 amd64 实测） | `779372002` |
| `skiff-server-x86_64-unknown-linux-musl` | 3,941,088 B | （本轮未下载） | `779371976` |
| `checksums.txt` | 434 B | （本轮未下载） | `779371981` |

- **下载与核对**：本轮测试机用 `api.github.com/repos/.../releases/assets/<id>` + `Accept: application/octet-stream` 直接下载成功，`sha256sum` 与侧车一致。
- **Release 触发方式**：开发机无 `gh` CLI 和 `GITHUB_TOKEN` env var，用 git credential manager 取 token 后 `Invoke-RestMethod -Uri .../actions/workflows/release.yml/dispatches -Method Post` 直接 dispatch。
