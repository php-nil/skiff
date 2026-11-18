# Docker 受限形态、本地开发与配置速查（§3–5）

> 上级索引：[DEPLOY_TEST.md](../DEPLOY_TEST.md) ｜ 拆自 `docs/test/DEPLOY_TEST.md` 原 §3–5（2026-11-16 整理，正文未改）。

---

## 3. 形态 B：Docker 受限形态（1Panel 商店包，可选）

**先读能力面收缩表，不能接受就别用这个形态。**

| 能力 | 容器形态 | 后果 |
|---|---|---|
| docker | 可用 | 仍走动词/对象/参数三层策略 |
| identity | 可用 | 登录/会话/TOTP/RBAC/审计 |
| fs | 不可用 | 需要 fs 的插件装前拒装；内核「工具 → 文件管理 / 回收站 / 配额管理」入口隐藏，`/{p}/files*` 直达一律 403「当前部署形态不提供文件管理能力」 |
| net | 不可用 | 需要 net 的插件装前拒装；出网收口无法兑付 |
| 宿主绑定挂载 | 全部 403 | 只能用 named volume |
| 沙箱 | reduced（独立 UID 有，cgroups/landlock 无） | UI 标 reduced sandbox |

安装要点：

1. 表单里 `SKIFF_ALLOW_INSECURE_HTTP` 与 `SKIFF_TRUSTED_PROXIES` 必须成对配置（默认 `1` + `172.16.0.0/12`）。只开明文不给白名单 → 内核拒绝启动，容器反复重启，日志里能看到明确的拒绝原因。
2. 访问入口请用 1Panel 的网站反向代理并开启 HTTPS；直接 `http://IP:18080` 在公网等于把管理面裸奔。
3. 初始口令：`docker exec <容器名> cat /data/initial-admin.txt`。
4. 升级：1Panel 升级流程会先跑 `scripts/upgrade.sh`，在停机状态下把 `skiff.db` 快照到 `data/backup/`（保留 3 份）。这是**升级回滚用**的快照，不替代备份恢复演练。

与主形态的差异（容器形态做不到，别照着主形态文档找）：

| 主形态有 | 容器形态 |
|---|---|
| 宿主 bind 挂载（授权根内） | 全拒 |
| 插件 fs 能力 | 拒装 |
| 内核文件管理（`/{p}/files*`） | 入口隐藏 + 直达 403 |
| 插件 net 能力（端口规则） | 拒装 |
| 完整沙箱（landlock/cgroups） | reduced |

## 4. 本地开发与冒烟

```sh
# loopback 明文（Cookie 降级为 skiff_dev_session，页面持续标红）
cargo run -p skiff --bin skiff -- --dev serve

# 或指定独立数据目录，避免污染开发机
SKIFF_DATA_DIR=./tmp-data SKIFF_RUN_DIR=./tmp-run \
  cargo run -p skiff --bin skiff -- --dev --config ./tmp-config.toml serve
```

开发机限制（Windows/macOS）：

| 项 | 状态 |
|---|---|
| 编译与单测 | 可用 |
| Docker 管理 | 不可用（无 Linux socket），面板显示“Docker 不可用”，其余功能正常 |
| 插件进程隔离（setuid/landlock/cgroups） | 不可用，走 `Unsupported` 分支 |
| kernel.sock（Unix socket） | 不可用 |
| 平台相关代码的类型检查 | **部分**：`cargo check -p skiff-model -p skiff-policy -p skiff-wire --target x86_64-unknown-linux-gnu` 可跑（纯 Rust）；其余 crate 有 C 依赖，Windows 上无法为 Linux 目标编译 |

## 5. 配置速查

优先级：**环境变量 > `/etc/skiff/config.toml` > 内置默认**。未知字段会导致启动失败（`deny_unknown_fields`），这是刻意的——拼错不该静默忽略。

| 环境变量 | 配置文件键 | 默认 | 作用 |
|---|---|---|---|
| `SKIFF_LISTEN` | `listen` | `127.0.0.1:18080` | 监听地址 |
| `SKIFF_DATA_DIR` | `data_dir` | `/www/skiff` | 唯一数据根 |
| `SKIFF_RUN_DIR` | `run_dir` | `/run/skiff` | 运行目录（含 kernel.sock） |
| `SKIFF_DOCKER_SOCKET` | `docker_socket` | `/var/run/docker.sock` | Engine socket |
| `SKIFF_FORM` | `form` | `native` | `native` / `docker`（受限） |
| `SKIFF_DOCKER_TIER` | `docker_tier` | `lifecycle` | `basic`(M2) / `lifecycle`(M3) |
| `SKIFF_TLS_CERT` / `SKIFF_TLS_KEY` | `tls_cert` / `tls_key` | 无 | 内置 TLS |
| `SKIFF_ALLOW_INSECURE_HTTP` | `allow_insecure_http` | `false` | 明文开关（见 §2.4 矩阵） |
| `SKIFF_TRUSTED_PROXIES` | `trusted_proxies` | 空 | 可信反代 CIDR，逗号分隔 |
| `SKIFF_AUTHORIZED_ROOTS` | `authorized_roots` | 空 | 宿主 bind 授权根（空 = 不许 bind） |
| `SKIFF_ALLOW_UNSIGNED` | `allow_unsigned` | `false` | 允许未签名插件（仅开发） |
| `SKIFF_ALLOW_REDUCED_SANDBOX` | `allow_reduced_sandbox` | `false` | 允许沙箱降级装插件 |
| `SKIFF_EXT_EXEC_TIMEOUT_SECS` | `ext_exec_timeout_secs` | `900` | 单扩展一次 `docker exec` 硬超时（秒；0 拒绝，上限 86400；超时作业 `failed` + `error_class=timeout`） |
| `SKIFF_EXT_BATCH_TIMEOUT_SECS` | `ext_batch_timeout_secs` | `1800` | 批量扩展脚本硬超时（秒；口径同上） |
| `SKIFF_EXTERNAL_URL` | `external_url` | 无 | 对外地址（绝对链接兜底） |
| `SKIFF_LOG_LEVEL` | — | `info` | tracing 过滤；排障用 `RUST_LOG` 风格，如 `debug,skiff_plugin=trace` |
| `SKIFF_CONFIG` | — | `/etc/skiff/config.toml` | 配置文件路径 |

> `login_concurrency` 只能写在配置文件里，**没有**对应环境变量。
>
> 商店目录（`[catalog]` 段 / `SKIFF_CATALOG_*`）的键与语义归 [SKIFF_SERVER.md](../../plugin/SKIFF_SERVER.md) §11.1。
> 默认镜像为内网 `http://10.66.52.7:8080`（一期只 HTTP，签名 + `seq` 保证不可伪造/不可回滚）；允许自定义 `http://` / `https://` 镜像。若内网自签镜像走 HTTPS，把签发证书的 CA 存成 PEM，配
> `ca_bundle`（或 `SKIFF_CATALOG_CA_BUNDLE`）即可，校验不放松，CA 文件非法则启动失败。
