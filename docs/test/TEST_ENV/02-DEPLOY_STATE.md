# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-24 测试机还原后干净首装 + budding 1.8.7 同步 + 页头主操作迁移 + SUBVIEWS 双源合一 + writePost 模板（当前；HEAD `c841074`）**
> - 背景：测试机还原为干净 T0（uptime 5 min，无 `/etc/skiff`、无 `/www/skiff`，18080 空闲，Docker 29.7.2 在位，时钟 UTC 已同步，磁盘 114 GB 可用）。L1 门禁 `cargo fmt --all --check` + `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` 均 exit 0。
> - **首装**（2026-11-24 11:24 HKT = 03:24 UTC）：`sh install.sh` exit **0**；`skiff.service` active；前缀 **`gk5y33tq`**；`/healthz` 200（`{"docker":"available","status":"ok","uptime_secs":10}`）。
> - **形态三配置**（install.sh 只写 loopback + `allow_insecure_http`，以下人工追加）：
>   - `listen = "0.0.0.0:18080"`（默认 loopback，改为外网直连）
>   - `tls_enabled = false`（直接明文监听，形态三必需；缺省启动会因非 loopback + 无 TLS 被拒）
>   - `allow_insecure_http = true`（让 accept.sh 判 dev 档、跑通 B 组 Cookie 断言）
>   - `allow_reduced_sandbox = true`（插件沙箱降级开关，landlock/cgroups 未实现）
>   - `entry_prefix = "gk5y33tq"`（硬编码，避免重装时随机变化）
> - **首装资产**：Release run `45317927914`（dispatch `ref=main`，head `c841074`，`publish=true`），amd64 包 **12,923,388 B**，`sha256=c905859dd6bdb72a48b0e85b7061f21f7476ffbcac194b49804fed2d459451b2`（侧车 + 包内 `checksums.txt` 双重校验）。
> - **accept.sh**：`--group all --password W8UdMRcTHTCbgKVb --pkg-dir /root/iter/skiff-night/skiff-0.1.0-x86_64-unknown-linux-musl` → **48 PASS / 0 FAIL / 4 SKIP**（PF3 未传 `--now` 时钟偏差、PF4 装后端口、A3 未传 `--install-exit`、B4 未开限流）。
> - **权限位**（A6 全对）：`/www/skiff` 711 root:root、`plugins` 711、`backup` 700、`skiff.db` 600 root:skiff-plugins、`/etc/skiff/config.toml` 644、`/run/skiff` 711 root:skiff-plugins、`kernel.sock` 660 root:skiff-plugins、`initial-admin.txt` 600 root:skiff-plugins。
> - **B 组认证**：CSRF→登录 200 + `redirect_to=/gk5y33tq`（默认不强制改密）→ 错误口令 401「用户名或口令不正确」→ 改密成功 + `initial-admin.txt` 删除 + 旧会话 303 → 新会话 200。
> - **D0 Docker**：Docker 29.7.2 已装可用 → `--check-only` exit 0 且不改动 daemon.json → 已装不重装。
> - **重启自恢复**（A9/A10）：`systemctl restart skiff` 后 3s 内 healthz 200；`kill -9` PID 后 2s systemd 拉起新进程，healthz 恢复 200。
> - **登录**：`admin`，初始口令见 `/www/skiff/initial-admin.txt`（`W8UdMRcTHTCbgKVb`）；`/root/iter/admin-pw.txt`（0600）已保存。

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
