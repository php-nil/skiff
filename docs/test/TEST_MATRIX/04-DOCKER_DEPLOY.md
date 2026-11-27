# H Docker / I 部署与升级

> 上级索引：[TEST_MATRIX.md](../TEST_MATRIX.md) ｜ 拆自 `docs/test/TEST_MATRIX.md` 原 H–I（2026-11-16 整理，正文未改）。

---

## H. Docker

> 重跑触发：改了容器页/API、日志 SSE、exec/WS、镜像、网络/卷、受保护容器、能力面策略或 Compose 子集。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| DOCKER-01 | 容器列表与异常过滤 | `[上机]` Docker 可用 | 打开 `/$P/docker/containers`；打开 `?filter=error`；对比 `docker ps` | 列表 200 且数量一致；异常过滤只返回异常容器；无异常时显示空态 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`、`crates/docker` |
| DOCKER-02 | 容器详情与不存在/非法参数 | `[上机]` 有容器 | 打开详情；请求不存在 id；请求非法动词 | 详情 200；不存在 404；非法动词 400 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`、`crates/docker/src` |
| DOCKER-03 | 启停/重启/删除 | `[上机]` 一个可丢弃容器 | 依次 start、stop、restart；再 POST remove；对受保护容器 POST remove；对执行中任务的运行环境容器 POST remove；每步 `docker ps` 对照 | start/stop/restart/remove 均 303 且状态与 `docker ps` 一致；remove 后容器消失；受保护容器 remove 403；任务占用 remove 409；未知动作仍 400。`[已复验 2026-11-07，f9a0b30；该资产已被同日最终版覆盖，当前资产见 TEST_ENV/02-DEPLOY_STATE.md §6]` 见下方「Docker 复验」表 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`、`crates/app/src/sites.rs`、`crates/docker` |
| DOCKER-04 | 日志 SSE 与断连 | `[上机]` 有持续输出的容器 | 打开日志页；`curl -N` 订阅 SSE；关闭客户端 | SSE 持续返回 `data:` 行；断开后服务端停止 follow，无泄漏 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`、`crates/docker` |
| DOCKER-05 | exec 一次性票据与 Origin | `[上机]` 容器可执行 sh | 申请 exec 票据；用票据连 WS；重复用同一票据；异源 Origin/无 Origin 再连；过期票据再连；浏览器打开详情页在终端输入命令 | 首次可连；重复票据被拒；异源/无 Origin 403；过期票据被拒；终端可输入并有回显。`[已复验 2026-11-07]` 详情页加载 xterm；`echo ok` 回显；票据一次性/异源/无 Origin/过期均 403 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`、`crates/docker/src/client.rs`、`crates/web/static/budding/budding-boot.js`（容器详情） |
| DOCKER-06 | 镜像列表/拉取/删除 | `[上机]` 可访问镜像源 | 拉取一个小镜像；列表出现；删除；再拉 | 拉取返回 200 且结果/错误可见；列表与 `docker images` 一致；删除后消失；再拉成功 | □通过 / □失败 | `crates/web/src/handlers/resources.rs`、`crates/docker` |
| DOCKER-07 | 网络与卷创建/删除 | `[上机]` Docker 可用 | 创建 bridge 网络、volume；挂载到临时容器；删除 | 创建 200（页面刷新，非 303）；列表出现；挂载可用；删除后列表消失；使用中的卷删除被拒 | □通过 / □失败 | `crates/web/src/handlers/resources.rs`、`crates/docker` |
| DOCKER-08 | 受保护容器不可被面板操作 | `[上机]` 新版本重装/重建后的内核容器（`skiff-web-nginx`、`skiff-php-*`、`skiff-rust-*`） | 查看三个容器 Labels；对每个执行 stop/restart/remove；再看一个旧版未打标签的内核容器列表标签 | 新容器均带 `io.skiff.project=sites` 与 `io.skiff.protected=true`；stop/restart/remove 一律 403 且容器仍运行；存量未打标签容器在页面显示「存量未受保护」，重建后标签生效。`[已复验 2026-11-07，f9a0b30；该资产已被同日最终版覆盖，当前资产见 TEST_ENV/02-DEPLOY_STATE.md §6]` 见下方「Docker 复验」表 | □通过 / □失败 | `crates/app/src/sites.rs`（`skiff_labels`）、`crates/policy/src/policy.rs`（`Target::is_protected`）、`crates/docker/src/client.rs`（`unprotected_kernel`） |
| DOCKER-09 | 危险字段无法表达 | `[本机]` 模型/策略单测 | `cargo test -p skiff-policy --locked`；构造含 `privileged`/`cap_add` 的 JSON 请求 | 反序列化拒绝或校验拒绝；容器规格只允许契约字段 | □通过 / □失败 | `crates/model/src/container.rs`、`crates/policy` |
| DOCKER-10 | Compose 子集解析校验 | `[本机]` 仓库根 | `cargo run -p xtask -- check`（`check-1panel` + `check-compose`）；再对一份含不支持字段的 compose 跑 `check-compose`；`cargo test -p skiff-compose` | 样例全部通过；含不支持字段的 compose 被拒；解析器单测覆盖拓扑序、卷/网络前缀与标签不注入 | □通过 / □失败 | `xtask`、`crates/compose`、`deploy/compose` |
| DOCKER-11 | 网络页显示子网/网关/IPAM/内外部/容器挂载 | `[上机]` Docker 可用 | 打开 `/$P/docker/networks`；对照 `docker network inspect <name>`；打开一个网络的容器明细；停掉 Docker 或让它返回错误再打开页面 | 列表显示每个网络的 Subnet、Gateway、IPAM 驱动、internal/attachable、容器数量；展开 `details` 能看到容器名/IPv4/IPv6/MAC；取不到的数据显示「未知」，**不显示 0 或空白**；Docker 不可用时页面给出错误而不是空列表 | □通过 / □失败 | `crates/docker/src/client.rs`（`list_networks` / `network_attachments`）、`crates/web/src/handlers/ui/resources.rs`、`crates/web/static/budding/budding-boot.js`（networks 页） |
| DOCKER-12 | 创建容器表单与受限形态拒绝 | `[本机]` 表单解析单测；`[上机]` 一个可丢弃镜像 | 本机：`cargo test -p skiff-web spec_from_form --locked`；上机：打开 `/$P/docker/containers/new` 填镜像/名称/端口/环境变量/卷/网络/重启策略提交；再以 viewer 打开该页；在 1Panel Docker 受限形态（`Tier::Basic`）用 admin 打开该页 | 页面 200 且各字段齐全；提交 303 跳详情且 `docker ps` 出现该容器、端口/环境/挂载一致；viewer 403；Basic 档 admin 也 403（能力面收缩）；镜像为空/端口越界/环境变量无 `=` 一律 400 并保留表单 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`（`new_page`/`create`/`parse_ports`/`parse_env`/`parse_mounts`/`spec_from_form`）、`templates/container_new.html`、`crates/policy/src/policy.rs` |
| DOCKER-13 | 资源限制热更新与生效值 | `[本机]` 策略/模型单测；`[上机]` 一个可丢弃容器 | 本机：`cargo test -p skiff-policy resource_limits --locked`；上机：详情页改 CPU 份额/配额/内存/内存+交换/`pids`，提交后 `docker inspect` 对照；填非法值（内存 <6MB、交换 < 内存、端口越界等）；把字段清空再提交 | 更新 303 且 `docker inspect` 的 `HostConfig` 与表单一致；详情页「当前生效值」同步刷新；非法值 400 并保留表单；清空按 0 = 清除上限；镜像/命令等结构项在页面上注明「需重建」，无输入入口；`pids` 生效范围标「待实测」 | □通过 / □失败 | `crates/web/src/handlers/containers.rs`（`update_limits`/`limits_from_form`）、`crates/docker/src/client.rs`（`update_container`）、`crates/policy/src/policy.rs`（`validate_limits`）、`crates/web/static/budding/budding-boot.js`（容器详情） |
| DOCKER-14 | Compose 主链路：校验→部署→状态/日志→down | `[本机]` handler/解析单测；`[上机]` Docker 可用 | 本机：`cargo test -p skiff-compose --locked`、`cargo test -p skiff-web compose --locked`；上机：`/$P/docker/compose` 粘贴一份两服务 compose（含 `depends_on` 与命名卷）→ 仅校验 → 部署；观察服务表与日志；打开容器列表看「compose · 项目」归属；停止/重启项目；`down` 先不勾删除卷，再勾删除卷 | 校验通过并回显计划（项目名/服务/容器名）；部署按拓扑序创建容器并启动，报告逐服务状态；服务表显示状态与最近日志；列表归属来自标签（内核 `io.skiff.project` 优先，回落 `com.docker.compose.project`）；`down` 二次确认后容器全部删除、网络前缀清理，未勾删除卷时卷保留并在报告列出，勾选后卷删除；部分失败不判整体成功；`sites`/`skiff` 项目名被拒；viewer 看不到写操作按钮 | □通过 / □失败 | `crates/compose/src/lib.rs`、`crates/docker/src/compose_run.rs`、`crates/web/src/handlers/compose.rs`、`crates/app/src/menu.rs`（`core/docker/compose`）、`crates/web/static/budding/budding-boot.js`（compose 页） |

最小回归命令：

```bash
curl -ks -o /dev/null -w 'containers=%{http_code}\n' -b "$J" "$B/$P/docker/containers"
curl -ks -o /dev/null -w 'containers_error=%{http_code}\n' -b "$J" "$B/$P/docker/containers?filter=error"
curl -ks -o /dev/null -w 'images=%{http_code}\n' -b "$J" "$B/$P/docker/images"
curl -ks -o /dev/null -w 'networks=%{http_code}\n' -b "$J" "$B/$P/docker/networks"
cargo test -p skiff-policy container_remove --locked
cargo test -p skiff-docker stale_kernel --locked
cargo test -p skiff-app container_busy_reason --locked
cargo test -p skiff-compose --locked
cargo test -p skiff-web docker_create_and_compose --locked
cargo run -p xtask -- check
```

**Docker 补充实测（2026-11-07 历史轮，包 `v0.1.0` / `3e98b5e`，x86_64 sha256 `c39fe7d2…`；最终资产见 [TEST_ENV/02-DEPLOY_STATE.md](../TEST_ENV/02-DEPLOY_STATE.md) §6）**

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| DOCKER-01 | ☑通过 | `/containers` 200，3 个容器名全部命中；`?filter=error` 200 且空态「没有处于异常状态的容器。」 |
| DOCKER-02 | ☑通过 | 详情 200；不存在 404；非法动词 400「未知动作: nope」 |
| DOCKER-03 | ☑通过（remove 缺口） | stop/start/restart 303 且 `docker inspect` 状态随动；remove 400「未知动作: remove」 |
| DOCKER-04 | ☑通过 | `curl -N` 收到 76 行 `data:`；断开后无残留连接 |
| DOCKER-05 | ☑通过 | 合法票据 101 且 `echo ok` 回显帧含 `ok\r\n`；无票据/坏票据/复用/异源/无 Origin/过期均 403，文案分别为「缺少 exec 票据」「票据校验失败」「WebSocket Origin 校验失败」「票据已过期」；详情页已加载 xterm |
| DOCKER-06 | ☑通过 | `busybox:latest` 拉取 200「已拉取」，列表出现；删除后面板与 `docker images` 均消失 |
| DOCKER-07 | ☑通过（实际 200） | 创建网络/卷返回 200 并实际落库 Docker（原写 303 为页面刷新语义）；挂载临时容器可用；使用中卷删除被拒（`in use - [id]`），清理后删除成功 |
| DOCKER-08 | ☑通过（发现新缺口） | 合成 `io.skiff.protected=true` 容器 stop/restart 403「受保护对象 … 拒绝操作」且保持 running；remove 仍 400（DOCKER-03 缺口）。**新缺口**：运行时 `skiff-web-nginx`/`skiff-php-batch1`/`skiff-rust-rust-1.83` 未带该标签、名称也不匹配 `skiff`/`skiff_`，实测可被面板停止 |
| DOCKER-09 | ☑通过（本机） | `cargo test -p skiff-policy` 在闸门内串行通过：危险字段反序列化拒绝、规格白名单校验均绿 |
| DOCKER-10 | ☑通过（本机解析） | `cargo test -p skiff-compose` 通过；运行时入口已从无到有（`/$P/compose`），`xtask check` 的字段子集校验仍保留 |

**Docker 复验（2026-11-07 发版轮，包 `v0.1.0` / `f9a0b30`，x86_64 sha256 `87427aa6…`，测试机 10.10.15.4；该资产已被同日最终版覆盖，当前资产见 [TEST_ENV/02-DEPLOY_STATE.md](../TEST_ENV/02-DEPLOY_STATE.md) §6）**

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| DOCKER-08（存量标注） | ☑通过 | 部署后未重建时：列表页「存量未受保护」3 处（三容器各一），详情页各 1 处；`docker inspect` 三容器均无 `io.skiff.protected` |
| DOCKER-08（重建后标签） | ☑通过 | Web 服务 + `batch1` + `rust-rust-1.83` 重建后 `protected=true` ×3；列表「受保护」3 处、「存量未受保护」0；详情各 1 处且无存量字样；`audit-env.local` / `audit-rust.local` 均 200 |
| DOCKER-08（面板拒操作） | ☑通过 | 三容器 stop、restart 共 6 次请求全部 403，`docker ps` 保持 `Up` |
| DOCKER-03（删除入口） | ☑通过 | 可丢弃容器 `remove` 303（回列表）且 `docker ps -a` 消失；三受保护容器 `remove` 均 403；临时运行环境在 `imagick` 安装任务 running 时 `remove` 409 且容器保持运行；未知动作 400（前轮已验） |
| DOCKER-03/08 备注 | — | 409 分支只在「存量未受保护容器 + 进行中任务」可达：受保护容器先被策略判 403。本轮用临时 PHP 运行环境容器（带 `io.skiff.project=sites`、无 `protected`）复现该路径，面板列表亦标注「存量未受保护」；测后已删运行时记录与容器 |

## I. 部署与升级

> 重跑触发：改了安装/卸载脚本、systemd unit、配置解析、TLS/反代信任、数据目录权限、数据库迁移、发行包或升级流程。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| DEPLOY-01 | 发行包结构与摘要/签名 | `[本机]` 有 release 包 | 解包检查 `skiff`、`skiff-plugin`、`install.sh`、`uninstall.sh`、`docker-bootstrap.sh`、`skiff.service`、`config.toml.example`、`vendor/install-php-extensions`、`official/*.wbpkg` 与 `public-key.txt`；`cargo run -p xtask -- verify-pkg …`；`sha256sum -c` | 结构完整；单包 `.sha256` 校验通过；缺失或篡改被拒 | □通过 / □失败 | `deploy/pack-release.sh`、`xtask`、`.github/workflows/release.yml` |
| DEPLOY-02 | systemd 直装与自启 | `[上机]` 干净 Linux | `deploy/install.sh`；`systemctl status skiff`；重启机器 | 安装成功；服务 active；重启后自动拉起；面板 200。`[已复验 2026-11-07]` 重启后自启、三容器 Up、面板/两站 200 | □通过 / □失败 | `deploy/install.sh`、`deploy/systemd/skiff.service` |
| DEPLOY-03 | 数据目录与权限口径 | `[上机]` 已安装 | 查看 `/etc/skiff/config.toml`、`/www/skiff`、`/www/skiff/plugins`、`/www/skiff/skiff.db` | 数据根与 plugins 0711；DB 0600；面板自身配置 `/etc/skiff/config.toml` 由安装脚本落为 0644（非 0600）；目录属主正确 | □通过 / □失败 | `deploy/install.sh`、`crates/core/src/bootstrap.rs`、`docs/test/DEPLOY_TEST.md` §1.1 |
| DEPLOY-04 | 配置严格校验与非回环无 TLS 拒绝 | `[上机]` 可改配置 | 写未知字段启动；监听 `0.0.0.0` 且不配 TLS 启动；恢复合法配置 | 未知字段启动失败并指出字段；非回环无 TLS 拒绝启动；合法配置恢复 200 | □通过 / □失败 | `crates/core/src/config.rs`、`deploy/config.toml.example` |
| DEPLOY-05 | 直连 TLS | `[上机]` 有证书 | 配 `tls_cert`、`tls_key` 指向证书；重启；访问 HTTPS；用 http 访问同一端口 | HTTPS 200 且证书正确；明文 HTTP 无法完成 TLS 握手（不会降级为明文）；面板 Cookie 带 Secure | □通过 / □失败 | `crates/core/src/config.rs`、`crates/core/src/server.rs`（TLS 监听）、`deploy/install.sh` |
| DEPLOY-06 | 反代信任边界 | `[上机]` 前置反代 | 配 `trusted_proxies` 为反代 CIDR；从可信/不可信来源分别带 `X-Forwarded-Proto`、`X-Forwarded-For` 访问，看审计里的来源 IP 与 scheme | 可信来源的头被采信，来源 IP 为最左真实客户端；不可信来源的头一律忽略，来源 IP 为直连 peer；伪造头不改变客户端识别 | □通过 / □失败 | `crates/core/src/config.rs`、`crates/web/src/middleware.rs` |
| DEPLOY-07 | 升级覆盖安装与数据保留 | `[上机]` 已有库与站点 | 备份旧二进制与 DB；覆盖安装新 release；重启；登录看站点/任务/插件 | 新版本启动；`user_version` 升到当前 schema（15）；站点、任务、插件数据保留；已装官方插件的 `versions/<ver>` 与随包内容一致（版本或内容不同则更新，`data/` 与既有授权不变）。`[已复验 2026-11-07]` 同包覆盖安装，配置 sha 不变、业务计数保留 | □通过 / □失败 | `deploy/install.sh`、`crates/store/src/db.rs`（迁移） |
| DEPLOY-08 | 卸载与 purge 边界 | `[上机]` 可丢弃环境 | `deploy/uninstall.sh`；再加 `--purge`；检查数据目录 | 默认卸载保留 `/www/skiff` 数据；`--purge` 明确删除数据与配置并提示。`[已复验 2026-11-07]` 默认卸载删程序保数据（`/www/skiff`、`/etc/skiff`、`/www/sites`），重装恢复一致 | □通过 / □失败 | `deploy/uninstall.sh` |
| DEPLOY-09 | aarch64 装机 | `[上机]` arm64 Linux | 安装 release arm64 包；起 systemd；建站访问 | 安装/启动/建站均成功；`uname -m` 为 aarch64。`[部分]` 无 arm64 实机，仅 qemu-user 冒烟：`file` = ARM aarch64 静态链接、`ldd` = `not a dynamic executable`、`skiff version/--help/doctor` 正常（2026-11-09，`2f63aeb`）；装机未验 | □通过 / □失败 | `deploy/pack-release.sh`、`.github/workflows/release.yml` |
| DEPLOY-10 | 1Panel Docker 受限形态 | `[上机]` 1Panel 环境 | 按商店包安装；尝试安装 fs/net 插件；打开三核心 | 面板可用；fs/net 插件因能力面收缩而拒装；docker/identity 正常。`[部分通过]` 2026-11-09 用等价镜像（`alpine:3.23` + 发布包 `skiff`，`SKIFF_FORM=docker`）实测：`/healthz` 200、登录 303、`files/monitor/guard/testkit` 400「需要能力 fs」、`ftp` 400「需要能力 ftp」、`cron` 400「要求完整沙箱」、容器页壳 200 + `stop/start` 303；未在真实 1Panel 商店流程内装。**文件管理已按能力面收口（`607d09b` 复验）**：`core/files`、`core/files/trash`、`core/files/quotas` 入口不渲染，`/{p}/files*`、`/{p}/m/core/files*` 直达均 403「当前部署形态不提供文件管理能力」；回归 `crates/web/tests/main_flow/ops.rs::files_routes_are_forbidden_in_docker_restricted_form` | □通过 / □失败 | `deploy/1panel/`、`crates/plugin`、`crates/web/src/middleware.rs`（`check_fs_capability`） |
| DEPLOY-11 | Release 下载失败可诊断 | `[上机]` 外网下载 | 正常下载一次；模拟超时/断网再下载 | 正常成功且校验摘要；失败时保留明确 URL/状态码/原因并有限重试。`[阻塞]` 无可复现超时/断网条件；代码核对 `tools/iter.sh::cmd_fetch` 为单次 `curl --max-time 600`，无重试与结构化错误输出 | □通过 / □失败 | `tools/iter.sh`、`.github/workflows/release.yml` |

最小回归命令：

```bash
sudo systemctl is-active skiff
curl -ks -o /dev/null -w 'panel=%{http_code}\n' "$B/$P/login"
stat -c '%a %U:%G %n' /www/skiff /www/skiff/plugins /www/skiff/skiff.db /etc/skiff/config.toml
sqlite3 /www/skiff/skiff.db 'PRAGMA user_version;'
sudo journalctl -u skiff -n 50 --no-pager | grep -E '安全入口|listen|error' | tail -5
```

**部署补充实测（本轮 2026-11-07）**

| 编号 | 结果 | 证据 / 阻塞条件 |
| --- | --- | --- |
| DEPLOY-02 | ☑通过 | 重启后 `skiff.service` enabled+active、`NRestarts=0`，三容器 Up，面板与两站 200；开机后约 4s 服务就绪、约 10s 面板可用 |
| DEPLOY-07 | ☑通过 | 同包覆盖安装不清库：配置 sha 不变，users/sites/grants/plugins/tasks 计数全保留，`integrity=ok` |
| DEPLOY-08 | ☑通过 | 默认卸载删除二进制/unit/`/run/skiff`，保留 `/www/skiff`、`/etc/skiff`、`/www/sites` 与运行组；重装恢复后站点 `diff -r` 无差异 |
| DEPLOY-09 | △部分（qemu-user 冒烟） | 包内 ARM aarch64 静态链接（`file`/`ldd` 正常）、`version`/`--help`/`doctor` 通过（2026-11-09 `2f63aeb`）；复现条件=arm64 Linux + systemd 装包后验启动/建站 |
| DEPLOY-10 | △部分通过（等价镜像） | 2026-11-09 用 `alpine:3.23` + 发布包二进制、`SKIFF_FORM=docker` 实测：healthz 200、登录 303、fs/net/ftp 插件拒装、容器 stop/start 303；`607d09b` 复核文件管理入口已隐藏、直达 403 文案可读；未在真实 1Panel 商店流程内装 |
| DEPLOY-11 | ⏸阻塞（无复现条件） | 正常下载+`sha256sum -c` 通过；`tools/iter.sh::cmd_fetch` 无重试；复现条件=对下载域名注入超时/丢包 |
