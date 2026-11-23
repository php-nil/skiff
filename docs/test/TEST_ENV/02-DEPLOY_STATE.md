# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-23 测试机还原后干净首装 + budding 1.8.6 同步（当前；HEAD `9c1be6c`）**
> - 背景：测试机还原为 T1（无 `/etc/skiff`、无 `/www/skiff`、无 `/root/iter`，18080 空闲，Docker 29.7.2 在位，时钟已同步，磁盘 112 G 可用）；同步上游 budding 1.8.6（表单改 `submitMode: "event"` 事件提交、`setErrors` 一等动作、`confirm` 统一 `color`、`testId` / `data-bud-role` / `inspectLayers()`），删除宿主 K3/K4 补丁后按测试规范跑首轮。
> - **首装**（2026-11-23 18:58）：`install.sh --listen 0.0.0.0:18080 --tls off` exit **0**；前缀 **`28bwmusa`**；`skiff.service` active；`/healthz` 200；库 `user_version=22`、`integrity_check=ok`。安装脚本只写 `listen` + `tls_enabled`，`allow_insecure_http` 与 `allow_reduced_sandbox` 按惯例人工追加（前者的作用是让 `accept.sh` 判定为 dev 档、跑通 B 组 Cookie 断言；后者是插件沙箱降级开关）。
> - **首装资产**：Release run `45158992891`（head `9f43aee`），amd64 包 12,962,955 B，`sha256=f11172ee55b580fd8f12d065061466d329694ef05436035729e1793a6106c937`（侧车校验一致）。
> - **首轮 accept**：`--group all` 首跑 **35 PASS / 0 FAIL / 2 SKIP**——B 组因缺 `allow_insecure_http` 被判 prod 档整组 SKIP；补上后 **50 PASS / 0 FAIL / 2 SKIP**（PF4 装后端口、B4 未开限流）。
> - **首轮查出的缺陷（两个，均已修复并复验）**：
>   - **插件拒装把源包删了**：8 个官方插件全部因「要求完整沙箱」被拒，`/www/skiff/official/*.wbpkg` 同时被清空。根因 `commands/plugin.rs::install` 无条件 `remove_kernel(package)`。修复为「装成功 + 路径在 `data_dir/plugins/.incoming` 内」才清理（`5a48b83`，CI `45161514099` 绿）。
>   - **弹窗提交失败静默**：`formError` 只认 `ctx.extras.form`，弹窗页脚提交按钮不在表单祖先链上 → 恒为空 → 只剩 2 秒 toast。修复为按 budding 目标解析规则再查 `extras['budding:layer-forms']`（`fe4baa4`，断言修正 `9c1be6c`，CI `45169444034` 绿）。
> - **最终资产**：Release run `45170361272`（head `9c1be6c`），amd64 包 12,964,003 B，`sha256=0d5069ef1072194d0426fcb0543bed0324845bed36a90dce17bbd7ecd7424d62`；两轮就地升级（`5a48b83` 中间包 12,962,844 B `sha256=4f03248673c17721eb331f4d9cc4936ee92fed14e7593fbe3d9388ea5aea55ed`）均 exit 0，配置逐字节未变（`sha256=fa53f26bc9ace8815ae959dae574a5ce2f647277a5e969398b65847363e4f0fc`）、前缀与库内数据全保留。
> - **最终 accept**：**49 PASS / 0 FAIL / 3 SKIP**（PF4 装后端口、A6h 已改密机无 `initial-admin.txt`、B4 未开限流），首装/中间包/最终包三份二进制均一致。
> - **插件**：8 个官方包全部 installed + enabled（`cron` / `files` 0.1.3 / `firewall` / `ftp` / `guard` / `monitor` / `sources` / `testkit`，API 1.3）；修复后装完 `/www/skiff/official` 仍保留 9 个文件（8 包 + 公钥），官方随包更新链路可继续用。
> - **Web 服务**：本轮额外装了 OpenResty 容器（`skiff-web-nginx` Up），安装作业「拉取 OpenResty 镜像 → 完成」100%，站点创建不再被 503 拦住。
> - **浏览器**：实路由 **50/50**（`/ui/v1/routes` 展开的 69 条里 19 条是 `:id` 模板路由，逐条命中「不存在 / 页面加载失败」属预期）0 CompileError / 0 JS 异常；关键交互 15 项全过（同名建目录失败原地显示「新建目录失败：目标已存在」且值保留、层不关；空提交给字段级错误；新建站点成功关层并刷新列表；站点详情 6 个子视图；6 个插件页；主题页缺省主色 `--v-theme-primary=23,169,142`）。
> - **测试副作用（按要求保留）**：目录 `/www/ui-check-1123`、`/www/ui-check-t8ckvq`，站点 `ui-check-8kbxb.local`（及其目录），8 个已装插件与 OpenResty 容器。
> - **登录**：`admin`，口令取测试机 `/root/iter/admin-pw.txt`（0600）；本轮 `reset-password` 后用接口改为已知口令（B0 走 `--password` 复跑）。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此「某轮的资产 sha」只在该轮有效；对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表。

- **当前资产来源**：Release run `45170361272`（dispatch `ref=main`，head `9c1be6c`，`publish=true`、`release_tag=v0.1.0`），success；资产覆盖于 2026-11-23 12:08 UTC。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`9c1be6c`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,964,003 B | `0d5069ef1072194d0426fcb0543bed0324845bed36a90dce17bbd7ecd7424d62` | `782794482` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | — | （侧车，内容同上摘要） | `782794488` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 12,388,716 B | （本轮仅 amd64 实测） | `782794510` |
| `skiff-server-x86_64-unknown-linux-musl` | 3,941,088 B | （本轮未下载） | `782794513` |
| `checksums.txt` | 434 B | （本轮未下载） | `782794479` |

- **下载与核对**：本轮走「开发机带 token 下载 → `scp` 上测试机 → `sha256sum` 与侧车比对」；三轮（首装 / 中间包 / 最终包）摘要均一致。
- **Release 触发方式**：开发机无 `gh` CLI 和 `GITHUB_TOKEN` env var，用 git credential manager 取 token 后 `Invoke-RestMethod -Uri .../actions/workflows/release.yml/dispatches -Method Post` 直接 dispatch。
