# 一页结论、能力现状核对、竞品对照与清单总表（§0–3）

> 上级索引：[PLUGIN_CATALOG.md](../PLUGIN_CATALOG.md) ｜ 拆自 `docs/plugin/PLUGIN_CATALOG.md` 原 §0–3（2026-11-16 整理，正文未改）。

---

> 状态：**首批已落地（2026-11-01）** ｜ 日期：2026-11-01
> 关联：[§1.4 分层边界](../../sections/01-requirements.md)、[§3.3 原子能力](../../sections/03-architecture.md)、[§4 扩展体系](../../sections/04-plugin-package.md)、[§9 里程碑](../../sections/09-milestones.md)
> 落地细节（目录约定、构建自检、装机授权方法）见 [`plugins/README.md`](../../../plugins/README.md)，此处不重复。
> 数字纪律：竞品能力只做定性对照，未核实的具体数量不写；本仓库现状结论均附代码位置，便于复核。

## 0. 一页结论

| 维度 | 结论 |
|---|---|
| 选型依据 | 竞品把面板做成「服务器操作系统」，Skiff 只做三核心，**官方插件集就是产品完整度的上限** |
| 当前 6 个 | **容器守护**（零内核改动，链路验证）→ **主机监控**（含进程/网络只读视图）→ **站点文件**（站点级浏览/编辑）→ **计划任务** → **FTP 管理** → **防火墙**（Nginx 站点已内核化，按落地难度排序） |
| 目录 | 顶层 `plugins/`，作为**第二个 cargo workspace**，与 `crates/` 平级（已完成） |
| 能力前置 | `fs` 宿主授权已补齐（`path:` 对象 grant + 分块读写 + 六动词）；`docker` 已支持 `containers.exec`，仍缺 create/remove/pull/compose；`net` 一律 501 |
| 开箱即用 | 官方 `.wbpkg` 随发布包携带到 `/www/skiff/official/` + 公钥文件（**不自动安装、权限确认不跳过**）；内置官方公钥仍未做 |
| 能力面扩张 | 新增两个内核原语：`ftp`（FTP 管理）与 `firewall`（防火墙 / fail2ban），均涉及 `wire`/`pkgfmt`/SDK/插件 manifest 同步；契约见 [KERNEL_FTP_CONTRACT.md](../../contract/KERNEL_FTP_CONTRACT.md) 与 [KERNEL_FIREWALL_CONTRACT.md](../../contract/KERNEL_FIREWALL_CONTRACT.md) |

**主结论**：竞品用户预期的是文件、网站、监控、防火墙、计划任务/备份、应用部署、数据库七个入口，而 Skiff 把这些全部放在插件层——所以「内核不内置」不能变成「用户装完只有容器页」。当前 6 个官方插件（连同 testkit 共 7 个签名包）已与内核能力补齐一起交付；目录上按独立发布物组织，不混进内核 workspace。计划任务走「调度**插件接口**」而非宿主 shell，因此不需要新增宿主命令执行能力面。


## 1. 能力现状核对（选型的硬约束）

| 能力 | 代码现状 | 位置 | 对官方插件集的含义 |
|---|---|---|---|
| `docker` | 白名单动词；Broker 执行 `containers.list/inspect/logs/exec`、`images.list`、`networks.list`、`volumes.list`、`info`、`version`、`containers.start/stop/restart/kill`；create、remove、pull、compose 一律返回 Unsupported | `crates/docker/src/client.rs` 的 `Docker::execute` | 一键部署、数据库管理、备份导出仍卡在 create/remove/compose |
| `fs` | 两条分支：相对路径落插件自身 `data/`；**绝对路径**须命中安装时确认的 `path:` 对象授权根（`fsops::resolve` + `skiff_plugin::fsgrant`），动词 `read/write/list/mkdir/remove/rename`；单次读写 256KiB 分页 | `crates/plugin/src/{capability.rs,fsops.rs,fsgrant.rs}` | 文件管理器、主机监控、Nginx 站点已可做；大文件靠分页而非流式 |
| `net` | 一律 501 + 审计 | `crates/core/src/kernel/capability.rs` 的 `net_unimplemented` | ACME 证书、出网告警仍不可做；防火墙改走新增的 `firewall` 能力面，不依赖 `net` |
| `firewall` | **新增能力面**（api 1.2）：`status/rules.list/rule.add/rule.remove/port.allow/fail2ban.status/fail2ban.install/fail2ban.jail.ensure/fail2ban.jail.remove/fail2ban.banned/fail2ban.ban/fail2ban.unban`；只动 `inet skiff` 表 / `SKIFF` 链与 `/etc/fail2ban/jail.d/skiff-*.conf`；仅 Native 形态，Linux-only；插件按 `perm:firewall:<verb>` 或 `firewall:*` 取授权 | `crates/plugin/src/firewall.rs`（判定与命令编排）、`crates/core/src/kernel/firewall.rs`（鉴权/审计/特权通道）、`crates/wire`（`CAP_FIREWALL_CALL`）、`crates/pkgfmt`（`Capability::Firewall`）、`crates/sdk`（`firewall_call`） | 防火墙插件只做 UI 与编排；规则与 jail 由能力面经 systemd-run 瞬态单元执行 |
| `identity` | `capability/identity/self` 返回 `id/version/api/granted/permissions/objects`（插件拿到的权限与 `path:`/`container:` 授权根就在这里） | `crates/core/src/kernel/capability.rs` 的 `identity_self` | 插件可自检授权，不必把根目录写死在代码里 |
| `ftp` | **新增能力面**（api 1.2）：`status/install/service.configure/uninstall/sites.list/accounts.list/account.create/account.password/account.delete/account.set_enabled/service.restart`；`status` 回传真实监听/连接数/fail2ban 细节，配置持久化到 `/etc/skiff/ftp-service.json`；仅 Native 形态，Linux-only；插件按 `perm:ftp:<verb>` 或 `ftp:*` 取授权 | `crates/plugin/src/ftp.rs`（判定与编排）、`crates/core/src/kernel/ftp.rs`（grants/审计/`systemd-run` 通道）、`crates/wire`（`CAP_FTP_CALL`）、`crates/pkgfmt`（`Capability::Ftp`）、`crates/sdk`（`ftp_call`） | FTP 管理插件只做 UI 与编排；能力面的特权动作（apt/systemd/pure-pw/chown）由插件编排、经内核的特权通道执行 |
| `interop` | 两个路由：`capability/interop/call`（跨插件接口调用，四道闸 + 15s 超时 + busy 防环）、`capability/interop/catalog`（只列**调得通**的接口）；转发时注入 `params._interop.caller` | `crates/core/src/kernel/{routes.rs,interop.rs}`、manifest `[interop]`（`crates/pkgfmt/src/manifest.rs`）、`crates/plugin/src/interop.rs` | 计划任务（调度插件接口）、站点页「一键新增备份任务」这类**插件间协作**可做；不需要给插件宿主 shell |
| 插件 UI | 默认由壳页加载随包 schema 原生渲染（`api = 1.3`）；旧沙箱 iframe 工作台（无 same-origin + postMessage 桥，单帧上限 1MiB）只在显式 `?legacy=1` 时使用 | `crates/web/src/handlers/plugins.rs`、`crates/web/static/budding/budding-boot.js`、`crates/pkgfmt/src/schema.rs` | 插件 UI 不能直连内核，所有数据与写操作走 `POST /{p}/plugins/{id}/rpc`（CSRF + 权限 + 审计）；**大文件上传下载仍受帧上限限制**，文件管理器需新增传输契约 |
| 信任 | DB 信任库 + TOFU 三段式；**没有内置官方公钥**（§4.3 承诺「内置官方公钥」尚未落地） | `crates/plugin/src/trust.rs` 的 `resolve` / `decide` | 官方插件「开箱即用」的第一道门未开：现在仍要手动导入公钥 |
| 分发 | 渠道未决策（§1.4），安装全程离线可用 | [§4.4](../../sections/04-plugin-publish.md) | 开箱即用只能靠「随包携带 + 本地官方源」，不能依赖在线商店 |

## 2. 竞品对照

| 能力入口 | 1Panel | 宝塔 | Coolify / Dokploy | CasaOS | Portainer | YunoHost | Skiff 现状 | Skiff 目标 |
|---|---|---|---|---|---|---|---|---|
| 容器管理 | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ 内核 | ✅ 内核 |
| 容器终端 | ✅ | ✅ | ✅ | ✅ | ✅ | ⭕ | ✅ 内核 | ✅ 内核 |
| 文件管理 | ✅ | ✅ | ❌ | ✅ | ❌ | ❌ | ❌ | 官方插件 |
| 网站与证书 | ✅ | ✅ | ⭕ 反代+证书 | ❌ | ❌ | ⭕ 域名 | ❌ | 官方插件 |
| 数据库管理 | ✅ | ✅ | ✅ | ❌ | ❌ | ⭕ 应用自带 | ❌ | 官方插件 |
| 主机监控 | ✅ | ✅ | ⭕ | ✅ | ⭕ | ❌ | ❌ | 官方插件 |
| 防火墙 | ✅ | ✅ | ❌ | ❌ | ❌ | ⭕ | ⭕ 官方插件 | 官方插件（`firewall` 能力面） |
| 计划任务与备份 | ✅ | ✅ | ✅ | ⭕ | ❌ | ✅ | ⭕ 插件（只调度插件接口） | 官方插件 |
| 应用一键部署 | ✅ | ✅ | ✅ | ✅ | ⭕ 模板 | ✅ | ❌ | 官方插件 |
| 用户与 RBAC | ✅ | ⭕ | ✅ | ✅ | ✅ | ✅ | ✅ 内核 | ✅ 内核 |

> 图例：✅ 内置；⭕ 部分支持或依赖外部组件；❌ 不提供。1Panel/宝塔为宿主直装面板，Coolify/Dokploy 为容器内 PaaS，能力口径不同，仅作功能面对照。

**对照结论**：

1. 竞品的「文件 + 网站 + 监控 + 防火墙」是面板首页级入口，不是可选项；Skiff 缺任何一项，用户都会认为「没做完」。
2. 差异化不在功能数量，而在交付方式：插件是**宿主真进程**（不是容器里塞 sidecar）、权限逐条确认、离线可装、可与 1Panel/宝塔同机共存。
3. 因此官方插件集应覆盖「开箱七件套」，但**按能力就绪度分波次**，不做一次性铺开。

## 3. 官方插件清单

| 优先级 | 插件 id | 名称 | 能力面 | 内核前置 | 竞品对标 | 状态 |
|---|---|---|---|---|---|---|
| P0 | `com.skiff.guard` | 容器守护 | `docker` + 自身 `data/` | **零内核改动**（`containers.list/inspect/restart`） | 部分竞品无 | ✅ 0.1.0 已实现 |
| P0 | `com.skiff.monitor` | 主机监控 | `fs`（`/proc` 只读，`/etc/passwd` 可选） | `path:` 只读授权 + 内核调度器采样（均已就绪） | 1Panel/宝塔 监控与进程管理 | ✅ 0.1.0 已实现（含进程 / 网络只读视图） |
| P0 | `com.skiff.files` | 站点文件 | `fs`（全 6 动词） | `path:` 授权（仅接受 `/www/sites/<域名>/index`）+ 256KiB 分页 | 1Panel/宝塔 文件 | ✅ 0.1.3 已实现（站点级入口） |
| P0 | `com.skiff.cron` | 计划任务 | `identity` + `interop` | interop 契约（内核转发 + 四道闸），**不需要**宿主命令执行 | 1Panel/宝塔 计划任务 | ✅ 0.1.0 已实现（只调度插件接口） |
| P0 | `com.skiff.testkit` | 插件架构测试台 | `fs`（`/proc` 只读）+ `identity` + `interop` + `scheduler`（`read` + `manage`） | `scheduler:read`/`scheduler:manage` 探针（任务列表、缓冲读写）+ 默认停用的执行期 BusyGuard 探针任务 | 无 | ✅ 0.1.0 已实现（随包携带，不自动安装） |
| P1 | `com.skiff.ftp` | FTP 管理 | **新增内核 `ftp` 能力面** + `interop` | 宿主 pure-ftpd（`apt` + `systemd`；仅 Native 形态）；可选登录防护依赖防火墙插件 | 1Panel/宝塔 FTP | ✅ 0.1.0 已实现并上机验收通过（FTP-01/05 残留，见 TEST_LEDGER）；fail2ban 已收口到防火墙插件 |
| P1 | `com.skiff.apps` | 应用模板一键部署 | `docker` | compose.up/down + 模板随包离线 | 1Panel/宝塔 应用商店 | 未开始（卡 compose） |
| P1 | `com.skiff.firewall` | 防火墙 | **新增内核 `firewall` 能力面** + `interop` | 宿主 nftables/iptables + fail2ban（`apt` + `systemd`；仅 Native 形态） | 1Panel/宝塔 防火墙 | ✅ 0.1.0 已实现（未经真机验收） |
| P1 | `com.skiff.backup` | 备份与恢复 | `docker` + `fs` | exec、fs 授权、流式导出 | 1Panel/宝塔 备份 | 未开始 |
| P1 | `com.skiff.db` | 数据库管理 | `docker` | exec + fs 导入导出 | 1Panel/宝塔 数据库 | 未开始 |
| P2 | `com.skiff.logs` | 日志查看与清理 | `fs` | 已具备（`read/list` + `remove`） | 1Panel/宝塔 日志 | 可做，未排期 |
| P2 | `com.skiff.notify` | 告警通知 | `net` | 出网收口 + 出网授权 | 1Panel 告警 | 未开始（卡 net） |
| P2 | `com.skiff.sso` | 身份同步/SSO | `identity` | identity 能力扩展 | YunoHost/Portainer | 未开始 |

> 原 `com.skiff.nginx` 插件已**内核化**：网站（站点列表 / 运行环境 / 设置）由内核直接提供，
> nginx + PHP-FPM 容器的安装、vhost 生成、启停与日志都在内核侧完成，不再作为官方插件发布。

**明确不做 / 暂缓**：

- **不做宿主 root 命令执行能力**（不给插件 shell 面）：计划任务改为「调度**插件接口**」（`interop.call` + 四道闸），要跑命令的插件自己声明 `provides`；给插件 root 等于绕开整套权限模型。真要执行容器内命令仍走 §6 的容器 exec 白名单。
- **不在本期做在线商店**：渠道未决策（§1.4），官方包先随发布物离线分发。
- **不把终端/日志流做成插件**：内核已有容器 exec 终端与日志 SSE，流式场景按 §4.2 不进插件。
