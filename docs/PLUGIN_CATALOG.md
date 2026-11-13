# 官方插件集规划：竞品对照、首批选型与目录落地

> 状态：**首批已落地（2026-11-01）** ｜ 日期：2026-11-01
> 关联：[§1.4 分层边界](sections/01-requirements.md)、[§3.3 原子能力](sections/03-architecture.md)、[§4 扩展体系](sections/04-plugin-package.md)、[§9 里程碑](sections/09-milestones.md)
> 落地细节（目录约定、构建自检、装机授权方法）见 [`plugins/README.md`](../plugins/README.md)，此处不重复。
> 数字纪律：竞品能力只做定性对照，未核实的具体数量不写；本仓库现状结论均附代码位置，便于复核。

## 0. 一页结论

| 维度 | 结论 |
|---|---|
| 选型依据 | 竞品把面板做成「服务器操作系统」，Skiff 只做三核心，**官方插件集就是产品完整度的上限** |
| 当前 6 个 | **容器守护**（零内核改动，链路验证）→ **主机监控**（含进程/网络只读视图）→ **站点文件**（站点级浏览/编辑）→ **计划任务** → **FTP 管理** → **防火墙**（Nginx 站点已内核化，按落地难度排序） |
| 目录 | 顶层 `plugins/`，作为**第二个 cargo workspace**，与 `crates/` 平级（已完成） |
| 能力前置 | `fs` 宿主授权已补齐（`path:` 对象 grant + 分块读写 + 六动词）；`docker` 已支持 `containers.exec`，仍缺 create/remove/pull/compose；`net` 一律 501 |
| 开箱即用 | 官方 `.wbpkg` 随发布包携带到 `/www/skiff/official/` + 公钥文件（**不自动安装、权限确认不跳过**）；内置官方公钥仍未做 |
| 能力面扩张 | 新增两个内核原语：`ftp`（FTP 管理）与 `firewall`（防火墙 / fail2ban），均涉及 `wire`/`pkgfmt`/SDK/插件 manifest 同步；契约见 [KERNEL_FTP_CONTRACT.md](KERNEL_FTP_CONTRACT.md) 与 [KERNEL_FIREWALL_CONTRACT.md](KERNEL_FIREWALL_CONTRACT.md) |

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
| 分发 | 渠道未决策（§1.4），安装全程离线可用 | [§4.4](sections/04-plugin-publish.md) | 开箱即用只能靠「随包携带 + 本地官方源」，不能依赖在线商店 |

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

## 4. 当前 6 个：实现与前置

### 4.1 实现顺序（按落地难度，已全部落地）

**容器守护（零内核改动，最小）→ 主机监控（只读 fs，小）→ 文件管理（全 fs 动词 + 宿主写，最重）→ 计划任务（只调度插件接口，最小但不独立）→ FTP 管理（新增 `ftp` 能力面，见 §4.8；Nginx 站点已内核化）→ 防火墙（新增 `firewall` 能力面，见 §4.9）**。

`guard` 排在第一位是因为它**不需要任何内核改动**：只用既有 `containers.list/inspect/restart` 与自身 `data/`，可以先把「manifest → 打包 → 装包 → RPC → UI」整条链路验穿，再往上叠能力面。

`cron` 排最后不是为了难度，而是它**依赖别的插件先存在**才有被调度对象；它的内核前置是 `interop`（`[interop]` 声明 + `interop:call` + `capability/interop/call`），属内核统一契约，不在插件里各写一份。

> 下面每节只写「实现口径 + 与规划的差异」；manifest 全文与 UI 说明见 [`plugins/README.md`](../plugins/README.md)。

### 4.2 容器守护 `com.skiff.guard`

```toml
run = "on-demand"
requires = ["docker", "identity", "fs"]
cron = ["* * * * *"]                 # 每分钟巡检；内核按分钟去重
resource_hints = []                 # 守护对象由管理员逐条勾选
# docker: containers.list / containers.inspect / containers.restart
# fs: read / write / list（只写自身 data/ 的冷却与计数状态）
```

- **功能面**：每次 `tick` 列出容器 → 只对**已授权对象**（`container:NAME` / `container:ID` / `project:P`）判定是否 `unhealthy` / `exited` → 按 `cooldown_sec`、`max_per_hour` 限流后 `restart`；支持 `dry_run`、`restart_exited`、`watch` 白名单。
- **零内核改动口径**：新增代码只落在 `plugins/guard/`，内核侧没有任何新路由、新动词、新授权类型。
- **取舍**：未授权容器只展示、不重启（「先授权再动手」；跳过确认等于给了个万能重启器）。
- **坑（已修）**：内核 `docker` 回调把结果包成 `{"result": …}`，插件必须脱一层（`common::unwrap_result`），否则永远读不到容器行。

### 4.3 主机监控 `com.skiff.monitor`

```toml
run = "on-demand"
requires = ["fs"]
cron = ["*/5 * * * *"]              # 5 分钟一个采样点
resource_hints = ["path:/proc", "path:/etc/passwd"]  # 后者可选，仅用于 uid → 用户名
# fs: read / list（只读）
```

- **功能面**：读 `/proc/{stat,meminfo,loadavg,uptime,net/dev,cpuinfo}`，算 CPU 使用率（按两次采样的 `total/idle` 差值）、内存、负载、网卡入站/出站速率（`net_rx_rate`/`net_tx_rate`，字节差/秒 = B/s，另回累计 `net_rx`/`net_tx`）、在线时长、CPU 核数。
- **进程视图**（`processes`）：遍历 `/proc/<pid>/{stat,status,cmdline}`，展示 PID、用户、CPU%、内存、启动时间、状态与命令行；排序（cpu/mem/pid/started）、按 PID/用户/命令筛选、分页（1–200/页）都在插件内完成。CPU% 是**距上次刷新**的差值（基线 `prev_proc.json`），首刷为 `null`；PID 扫描上限 512，超出标 `truncated`。**只读**：不提供结束进程等写操作（权限模型未定，列为决策项）。
- **网络视图**（`network`）：解析 `/proc/net/{tcp,tcp6,udp,udp6}`，展示协议、本地/对端地址、TCP 状态、监听标记、套接字属主 uid 与 inode；支持协议/状态筛选、搜索与分页。插件降权运行，读不到其它用户的 `/proc/<pid>/fd`，**无法可靠把套接字映射到 PID**，界面明确以 uid 呈现属主。
- **写盘口径**：内核每 60s 调 `sample`（只更新 `prev.json` 基线），每 5 分钟调 `execute` 把批次**追加**到 `data/samples.jsonl`（一行一点，超 480 行压缩回 240 行）；UI `snapshot` 只算实时值、用独立基线 `prev_ui.json`，不写历史。
- **前置**：`path:/proc` 只读授权（安装时勾选）+ 内核调度器任务 `plugin:com.skiff.monitor:sample`（manifest `[[task]]` 自动种入）。未授权时 RPC 会返回「缺少 path:/proc 授权」而不是空数据。
- **风险**：采集频率与保留点数直接影响进程开销，**数字待实测**，不写资源承诺。

### 4.4 网站（已内核化，不再作为插件）

原 `com.skiff.nginx` 已迁移为内核「网站」能力：运行环境安装（nginx + PHP-FPM 容器）、站点表、
vhost 生成、启停 / 删除 / 日志都由内核实现，菜单归属固定为「网站 → 网站列表 / 运行环境 / 设置」。
插件侧不再维护该 manifest、UI 与备份接口；定时备份仍可复用 `com.skiff.cron` 对内核站点的能力
（待后续提供 interop 入口）。

### 4.5 站点文件 `com.skiff.files`

```toml
run = "on-demand"
requires = ["fs", "identity"]
resource_hints = []                 # 授权哪些目录由管理员在安装确认页填写
# fs: read / write / list / mkdir / remove / rename（全 6 动词）
```

- **功能面**：站点内容目录内浏览、读（256KiB 分页）、写（覆盖 / 追加 / 按 offset 续写）、新建目录、删除、改名；UI 带文本编辑器与手写 UTF-8 ⇄ base64。入口挂在「网站」一级下，不与内核「工具 → 文件管理」抢位置。
- **范围收口**：只接受 `/www/sites/<域名>/index` 形态的授权根，其余根一律拒绝；`resource_hints` 为空，站点域名在打包时未知，必须安装确认页手动添加。回收站、配额、上传/下载、归档、chmod/chown 一律走内核页（插件页给跳转）。
- **限制（未做）**：大文件仍是分页 + 文本编辑，**没有**二进制上传/下载与流式传输契约——那需要新的内核传输面（规划见 §7）。回收站、配额、归档、chmod/chown 也不在插件内，统一走内核「工具 → 文件管理」页。
- **审计**：走能力面时 actor 是 `plugin:com.skiff.files`，`detail` 追加 `grant=<授权根> site=<域名>`；这是「不把 `/www/sites` 全量授权给插件」换来的定责可读性（见 KERNEL_FS_CONTRACT §4.4）。
- **风险**：插件侧 `remove` 是**永久删除**（内核页默认进回收站），UI 文案与审计必须显式区分；越权一律 403 + 审计。

### 4.6 计划任务 `com.skiff.cron`

```toml
id = "com.skiff.cron"
run = "on-demand"
requires = ["identity", "interop", "scheduler"]
resource_hints = []                 # 「能调谁」由管理员在安装页勾 plugin:<目标>
# identity: self
# interop: call
# scheduler: read / manage / run / admin（admin 只授予这个内核指定 UI）
# [interop].provides = [] / consumes = []：调度已收归内核，本插件不再对外提供可调度接口
```

- **功能面**：纯管理 UI——列表/详情/历史、新建、改表达式、启停、立即执行、删除，全部翻译成内核 `scheduler` 能力面（`task.*`）调用。任务定义、下次触发与运行历史都在内核（[KERNEL_SCHEDULER_CONTRACT.md](KERNEL_SCHEDULER_CONTRACT.md)），插件不落任务文件，重启不丢任务。
- **不执行宿主 shell**：这是与 1Panel/宝塔「计划任务」最大的口径差异。插件来源的任务由内核调度器调用**目标插件接口**，能力仍由目标插件的 manifest 权限与授权对象约束；因此内核不必开放 root 命令执行面（见 §3「明确不做」）。
- **幂等与归属**：任务 id 由内核按 `plugin:<插件 id>:<key>` 生成，按 `(source, key)` upsert——`source` 取内核注入的 caller（插件伪造无效）。本插件只能改/删 `plugin:com.skiff.cron:` 前缀内的任务；内置任务（`builtin:`）只读展示、只可启停。
- **目标选择器**：读 `capability/interop/catalog`，只列「当前真的调得通」的 `插件:方法`（四道闸全过才出现），避免用户选了再吃 403。
- **表达式单一来源**：内核装包校验、内核 `tick` 调度、插件新建任务时的表达式校验与预设预览全部走 `crates/cron`（`skiff-cron`），三处同一份解析器，不各写一份。
- **风险**：任务数量 × 表达式频率决定调用量；「每分钟最多调用几次」这类数字**待实测**，不写资源承诺。禁用/删除任务即时生效，但已开始的调用不能中断（15s 超时兜底）。

### 4.7 菜单与槽位归属（P2 已迁移）

6 个官方插件与 testkit（共 7 个包）均为 `api = "1.3"` + `[[menu]]` + `view` + 随包 schema（`ui/schema/<view>.json`）；默认由壳页原生渲染，旧 `ui/<view>.html` 只在显式 `?legacy=1` 时使用。完整契约见 `docs/PLUGIN_UI_CONTRACT.md`，菜单部分见 `docs/MENU_CONTRACT.md`。

| 插件 | 一级 → 二级 | 三级标签（`view`） | 固定页槽位 |
| --- | --- | --- | --- |
| `com.skiff.monitor` | 工具 → 主机监控 | 总览 / 历史采样 / 进程 / 网络（`overview` / `history` / `process` / `network`） | `overview.health`（`sm`，30s 刷新） |
| `com.skiff.files` | 网站 → 站点文件 | 无（单视图 `main`） | — |
| `com.skiff.cron` | 工具 → 计划任务 | 任务列表 / 新建任务（`list` / `new`） | — |
| `com.skiff.guard` | 工具 → 容器守护 | 容器 / 事件（`containers` / `events`） | — |
| `com.skiff.testkit` | 工具 → 架构测试台 | 主视图 / 检查 / 分组（`main` / `check` / `group`）+ 单视图对照 | `overview.actions` |
| `com.skiff.firewall` | 工具 → 防火墙 | 防火墙规则 / fail2ban（`rules` / `fail2ban`） | — |
| `com.skiff.ftp` | 工具 → FTP 管理 | 站点账号 / 服务状态（`accounts` / `runtime`） | — |

- `/{p}/plugins/<id>` 是内核信息与管理页，功能页固定 `/{p}/plugins/<id>/<view>`；老包（无 `[[menu]]`）`main` 回落 `ui/index.html`。
- 不造「磁盘 / 执行记录 / 守护设置」等假标签：插件当前没有对应视图时不声明 L3。
- `monitor` 的 `ui/health.html` 是槽位专用紧凑卡；无采样时显示明确空态，内核负责外框、标题与失败降级。
- `testkit` 只用于验证架构：多视图/单视图、槽位、RPC 成功与失败、授权读与越界拒绝、主题/CSP；不执行宿主写操作。
- `testkit` 带 `scheduler` 读写探针：`scheduler_probe`（`task.list`）、`buffer_probe`/`buffer_stat`（`buffer.stat`，原样回报容量/条数/丢弃/拒绝/高水位）、`buffer_push`（按 `count` 循环调 `buffer.push`，上限 100 对齐每任务 100 点/秒限速），仅作用于本插件来源任务；不新增能力面方法。
- `testkit` 另带默认停用的 `busy-probe` 任务（`scheduler:read`）：任务执行期回调 `scheduler` 应被内核以 403 拒绝（BusyGuard），用于上机复验「插件任务执行 → 再注册任务」的自我繁殖防护。该任务同时声明内存缓冲（`sample_method=buffer_sample`、`capacity_bytes=4096`，默认停用不自动采样），作为 TASK-06 `buffer_probe`/`buffer_push` 的落点。

### 4.8 FTP 管理 `com.skiff.ftp`（能力面扩张）

```toml
run = "on-demand"
requires = ["ftp"]
[[permissions]]
capability = "ftp"
verbs = ["status", "install", "service.configure", "uninstall", "sites.list", "accounts.list",
         "account.create", "account.password", "account.delete", "account.set_enabled",
         "service.restart"]
```

- **为什么必须新增内核原语**：`fs` 只能在授权根内读写，装不了宿主包；`docker` 白名单不开放 `create/exec`，且 FTP 不走容器；`scheduler` 只调度任务，不执行宿主命令。三条现有能力面都不足以完成「apt 安装 pure-ftpd + `pure-pw` 建号 + chown 对齐 + systemd 启停」，因此新增 `ftp` 能力面，插件只做 UI 与编排。
- **安装形态**：宿主 `pure-ftpd` 软件包 + systemd（Debian 默认 standalone），不用容器——避免第三方镜像供应链与 Docker 端口段发布，生命周期与 systemd 直装一致；1Panel 受限形态（`Form::DockerRestricted`）一律 503，`Capability::available_in` 也不把它列入。
- **账号模型**：账号绑定站点，每站可多账号；登录名 `skiff_<slug16>_<fnv4>_<account>`，密码只经 stdin 交给 `pure-pw`，落 `pureftpd.pdb` 哈希；chroot 固定 `<site_root>/index`。
- **权限对齐**：站点统一身份固定 `skiff-web` 201000:201000（不再探测容器内 `www-data`，静态站也不再动态建 `skiff-ftp`）；`MinUID` 只降不升；`Umask 002 002`（conf 文件写两个空白分隔的三位八进制数，wrapper 不接受冒号）；只递归对齐 `index/`（目录 2775、文件 0664/0775），不碰 `log/`、`ssl/`、`conf/` 与其它站点。
- **安全边界**：`ChrootEveryone`、`NoAnonymous`、`PureDB`；连接上限与每 IP 上限可配（默认 30/3）；TLS 可关/显式可选/显式强制，证书与私钥路径可配，非法路径直接报错不静默降级。
- **fail2ban 收口**：FTP 不再安装、配置或卸载 fail2ban，安装时显式 `login_guard=false` 阻止旧内核路径代管；「启用 fail2ban 防护」是可选开关，经 interop 调 `com.skiff.firewall:fail2ban.*`（jail `pure-ftpd`，filter `pure-ftpd`，port 取内核 `listen_port`）。未安装防火墙插件时该选项不可用，不影响 FTP 其它功能。
- **状态与账号**：`ftp.status` 回传监听端口、是否在监听、当前连接数；登录防护状态另由 `protection` 汇总防火墙插件返回（`available=false` + 原因表示未安装/调用失败）；账号可启用/禁用；最后登录 pure-ftpd 用户库不记录，明确标注不可得；配额/速率列为后续项。
- **配置持久化**：`/etc/skiff/ftp-service.json` 是唯一真相来源，插件本地不再保留 `service-config.json`。
- **契约与验收**：主体/授权/审计口径见 [KERNEL_FTP_CONTRACT.md](KERNEL_FTP_CONTRACT.md)；回归项见 [TEST_MATRIX.md](TEST_MATRIX.md) `FTP-01`–`FTP-09`。

### 4.9 防火墙 `com.skiff.firewall`（能力面扩张）

```toml
run = "on-demand"
requires = ["firewall"]
resource_hints = []                 # 能力面动词即权限，不落宿主文件授权
[[permissions]]
capability = "firewall"
verbs = ["status", "rules.list", "rule.add", "rule.remove", "port.allow",
         "fail2ban.status", "fail2ban.install", "fail2ban.start", "fail2ban.stop",
         "fail2ban.uninstall", "fail2ban.jail.ensure",
         "fail2ban.jail.remove", "fail2ban.banned", "fail2ban.ban", "fail2ban.unban"]
```

- **为什么必须新增内核原语**：`net` 仍一律 501；`fs` 只能读写授权根，改不了内核规则表；`docker` 与防火墙无关。宿主防火墙与 fail2ban 都需要 root 特权，因此新增 `firewall` 能力面，插件只做 UI 与编排。
- **规则口径**：只管理自有作用域——nftables `inet skiff` 表的 `input` 链、iptables `SKIFF` 链；默认策略 accept。新规则带 `comment "skiff-<nanos>"` 标识，删除按 nft handle 或 iptables `-S` 序号，不触碰其它表/链。
- **fail2ban 口径**：`fail2ban.install` 安装/复用宿主 fail2ban（新装时写 `skiff-defaults.conf` 关闭 `sshd` jail）；`start` / `stop` / `uninstall` 提供显式生命周期管理（`enable --now` / `disable --now` / `apt-get purge`），`status` 如实返回未安装/已安装未运行/运行中三态与可读原因；`jail.ensure` / `jail.remove` 管理 `/etc/fail2ban/jail.d/skiff-<jail>.conf`，改动后 `systemctl restart fail2ban`；`banned` / `ban` / `unban` 走 `fail2ban-client`。不改系统级 `jail.conf`，不删用户创建的其它 jail 文件。
- **对外接口**：`[interop].provides` 暴露 `fail2ban.*`，FTP 等插件按需调用；调用方需 `interop:call` 与 `plugin:com.skiff.firewall`。四道闸、15s 超时与审计沿用既有 interop 契约。
- **特权通道**：能力面实现只在 `crates/plugin/src/firewall.rs`（判定 + 命令编排），内核 `crates/core/src/kernel/firewall.rs` 只做鉴权、审计与 `systemd-run` 瞬态单元执行；受限形态（非 Native）一律 503。
- **契约与验收**：动词、授权、审计与安全边界见 [KERNEL_FIREWALL_CONTRACT.md](KERNEL_FIREWALL_CONTRACT.md)；回归项见 [TEST_MATRIX.md](TEST_MATRIX.md) `FW-01` 起。**本轮代码未经编译验证，未上机验收。**

## 5. 目录落地：放在哪里

### 5.1 方案对比

| 方案 | 优点 | 缺点 | 结论 |
|---|---|---|---|
| 顶层 `plugins/`（独立 workspace） | 插件是独立发布物；与内核边界清晰；CI 可单独跑；第三方可整目录对照 | 需要第二 workspace、独立 CI job 与独立 lock/target | **已采用（2026-11-01）** |
| `crates/plugins/*`（根 workspace 成员） | 复用现有三关 | 破坏「`crates/` = 内核 crate」的边界；插件测试/构建被内核发布节奏绑定 | 不推荐 |
| 独立仓库 `skiff-plugins` | 发版与 CI 最干净，社区贡献隔离 | SDK 未发布到 crates.io，跨仓库同步 SDK 变更成本高 | 中期 |
| `deploy/plugins/` | 无额外结构 | `deploy/` 只放部署物料，不含源码 | 不可 |

### 5.2 实际结构

```
plugins/
├── Cargo.toml              # 第二 workspace：members = common/guard/monitor/files/cron/firewall/ftp/testkit
├── Cargo.lock              # 必须入库（CI 与 pack.sh 都用 --locked）
├── pack.sh                 # 官方插件打包：build → stamp → pack → verify → official/
├── README.md               # 官方插件集约定（id、构建自检、打包、装机授权、多架构）
├── common/                 # skiff-plugin-common：共享小工具（unwrap_result / path / 状态读写 / base64）
├── guard/                  # com.skiff.guard
│   ├── Cargo.toml          # name = skiff-plugin-guard，[[bin]] name = "plugin"
│   ├── manifest.toml       # key_id = 64 个 0 的占位（打包时回填）
│   ├── src/main.rs
│   └── ui/                 # containers.html / events.html + app.js（只经 postMessage）
├── monitor/                # com.skiff.monitor
├── files/                  # com.skiff.files
├── cron/                   # com.skiff.cron（provides jobs.create/list/remove/run；consumes *）
├── firewall/               # com.skiff.firewall（firewall 能力面 + provides fail2ban.*）
├── ftp/                    # com.skiff.ftp（consumes com.skiff.firewall:fail2ban.*，可选登录防护）
└── testkit/                # com.skiff.testkit（架构验收：多视图/单视图/槽位/RPC/能力面）
```

约定：

- crate 名 `skiff-plugin-<x>`；包内可执行名固定 `plugin`（`skiff-plugin pack` 按 `bin/{arch}/plugin` 收）。
- id 用 `com.skiff.<x>`；版本独立 semver；`api = "1.0"`，内核并行兼容 N-1 主版本。
- 插件 UI 只经父页面 postMessage 桥取数（`skiff.rpc` / `skiff.rpc.result`），不携带面板凭据、不发同源请求；`[[ui]].label` 直接进主界面顶栏菜单，插件间切换用面板级消息 `skiff.open`（只改地址，不授予能力）。
- **第三方前端库必须 vendor 进 `ui/`**：插件 UI 的 CSP 是 `script-src 'self'; connect-src 'none'`，CDN 与内联脚本都加载不了（例：`plugins/files/ui/vendor/` 里的 highlight.js 11.9.0 + LICENSE）。
- **仓库里不含任何密钥的指纹**：manifest `key_id` 是 64 个 0 的占位，打包时才用 `skiff-plugin stamp --key` 在**临时目录**回填；私钥只在 CI secret 或离线发布机。

### 5.3 构建、签名与发布（已实现）

- 根 `Cargo.toml` 增加 `exclude = ["plugins"]`，显式声明第二 workspace（已完成）。
- 每个插件 crate 直接 `skiff-sdk = { path = "../../crates/sdk" }`；不设 `workspace.dependencies`（各有依赖，收敛收益不大）。
- `plugins/pack.sh`：`build`（`CARGO`/`--target` 交叉）→ 复制 `manifest.toml` + `ui/` + `migrations/` + `bin/<arch>/plugin` 到临时目录 → `stamp` 回填指纹 → `pack --arch` → `verify --arch` 自检 → 输出 `com.skiff.<name>-<version>.wbpkg` 与 `public-key.txt`。
- CI 只加**一个** job `plugins`（fmt / clippy `-D warnings` / test，只跑插件 workspace）；真打包冒烟放在 `pkg-chain`（临时密钥打 7 个包并逐个验签）。
- 发布：`deploy/pack-release.sh --plugin-key <私钥>` 调 `pack.sh`，产物塞进发布包 `official/`；不给私钥且未 `--skip-plugins` 时**拒绝出包**（详见 [`docs/CI.md`](CI.md) §4）。
- 交叉打包需要 `skiff-plugin pack/verify --arch`（否则按打包机架构找载荷，arm64 包在 amd64 机上会被误判缺载荷）。
- 多架构是发布者责任：缺哪个架构，该架构上就拒装（§4.4）。

### 5.4 开箱即用的三条交付线（1/3 已落地）

1. **内置官方公钥**：命中内置指纹时显示「官方」徽章并免去手动导入（权限确认仍逐条）。**当前仍是缺口**：信任库只有 TOFU，发布包只带 `official/public-key.txt`，需要管理员手动导入。
2. **随包携带（已落地）**：发布包内有 `official/`（7 个已签名 `.wbpkg` + `public-key.txt`，含 testkit）；`install.sh` 复制到 `/www/skiff/official/`（0755），**不自动安装**。
3. **面板「官方插件」列表 + 一键安装**：从本地官方源读取、离线可用、安装仍走权限确认对话框。**未做**（现在只能 `skiff plugin install` 或面板上传）。

> 只把 `.wbpkg` 放 GitHub Release、让用户手动上传，不算「开箱即用」——第 3 条是下一步。

## 6. 实施顺序（含进度）

| 阶段 | 内容 | 状态 |
|---|---|---|
| 0 决策与目录 | 建 `plugins/` 第二 workspace + 三关门禁 | ✅ 2026-11-01（`exclude`、CI `plugins` job、`plugins/Cargo.lock`） |
| 1 能力补齐 | fs 宿主授权（`path:` grant、`resolve_data`/`resolve` 双分支、manifests 的 `resource_hints`、Broker 复验）；cron 运行时；interop 契约（manifest `[interop]`、`plugin:` 对象授权、`capability/interop/{call,catalog}`、审计） | ✅ fs / cron / interop 就绪；❌ docker exec、内置官方公钥仍未做 |
| 2 首批插件 | guard → monitor → files → cron → ftp → firewall（nginx 已内核化） | ✅ 代码 + UI，已编译验证（2026-11-10）；单测：guard 3 / monitor 13 / files 2 / cron 8 / ftp 3 / firewall 5 / common 7 |
| 3 分发 | 官方源目录 + 随包携带 | ✅ 随包携带（`official/` + `public-key.txt`）；❌ 面板内官方插件列表 |
| 4 第二批 | 备份与恢复、应用模板、数据库 | 未开始 |

> 6 个官方插件与 testkit（共 7 个包）的**真机验收**（安装、权限确认、启用、UI、RPC、越权、插件间调用、卸载）见 [TEST_LEDGER.md](TEST_LEDGER.md) 未闭环项；FTP 主链路已上机通过，卸载回收仍待复验；防火墙与 monitor 进程/网络本轮未上机。

> 能力补齐与插件开发并行时，先冻结 manifest 字段与协议契约再写插件，否则三份插件要返工。

## 7. 风险与未决

| 风险 | 说明 | 处理 |
|---|---|---|
| exec 白名单形同虚设 | 透传任意命令等于把容器 root 交给插件 | 命令模板 + 参数校验 + 输出/超时/并发上限 + 审计 |
| fs 授权越权 | 路径穿越、符号链接、TOCTOU | 已做：授权根目录粒度（`path:` grant）+ 插件侧预检 + 内核侧复验 + 目标存在时按 `canonicalize` 重判 + 越权 403 并审计；未做：全程 `O_NOFOLLOW` 持有句柄（判定与具体 IO 之间仍有窗口） |
| 大文件传输 | 插件 UI 单帧 1MiB 且 `connect-src 'none'` | 现状：256KiB 分页够文本编辑，**没有**二进制上传/下载；需要新增受限流式传输契约（未排期） |
| 资源预算 | 每个插件是独立进程 | 采集频率与保留点数设下限；数字待实测，不写承诺 |
| 信任与分发 | 无内置官方公钥、渠道未决策 | 已做随包携带（`official/` + `public-key.txt`）；内置公钥与面板官方列表未做；不动商店 |
| 插件间调用 | 一个插件拉着另一个插件跑，失败会顺链传播 | 四道闸 + 禁止自调 + busy 集合单层防环 + 15s 超时 + 逐次审计 `interop.call`；目标被停用时 409 |
| 插件 UI 静态资源 | opaque origin 下相对 `fetch` 会失败（§12） | UI 只经 postMessage，数据文件随包自包含 |

未决：商店/分发渠道（§1.4）；监控是否改 `resident`；文件管理器默认写权限还是默认只读；计划任务是否支持「按秒」触发（当前 5 段表达式最小粒度 1 分钟）。
