# 6 个官方插件的实现与前置（§4）

> 上级索引：[PLUGIN_CATALOG.md](../PLUGIN_CATALOG.md) ｜ 拆自 `docs/plugin/PLUGIN_CATALOG.md` 原 §4（2026-11-16 整理，正文未改）。

---

## 4. 当前 6 个：实现与前置

### 4.1 实现顺序（按落地难度，已全部落地）

**容器守护（零内核改动，最小）→ 主机监控（只读 fs，小）→ 文件管理（全 fs 动词 + 宿主写，最重）→ 计划任务（只调度插件接口，最小但不独立）→ FTP 管理（新增 `ftp` 能力面，见 §4.8；Nginx 站点已内核化）→ 防火墙（新增 `firewall` 能力面，见 §4.9）**。

`guard` 排在第一位是因为它**不需要任何内核改动**：只用既有 `containers.list/inspect/restart` 与自身 `data/`，可以先把「manifest → 打包 → 装包 → RPC → UI」整条链路验穿，再往上叠能力面。

`cron` 排最后不是为了难度，而是它**依赖别的插件先存在**才有被调度对象；它的内核前置是 `interop`（`[interop]` 声明 + `interop:call` + `capability/interop/call`），属内核统一契约，不在插件里各写一份。

> 下面每节只写「实现口径 + 与规划的差异」；manifest 全文与 UI 说明见 [`plugins/README.md`](../../archive/ui/README.md)。

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

- **功能面**：纯管理 UI——列表/详情/历史、新建、改表达式、启停、立即执行、删除，全部翻译成内核 `scheduler` 能力面（`task.*`）调用。任务定义、下次触发与运行历史都在内核（[KERNEL_SCHEDULER_CONTRACT.md](../../contract/KERNEL_SCHEDULER_CONTRACT.md)），插件不落任务文件，重启不丢任务。
- **不执行宿主 shell**：这是与 1Panel/宝塔「计划任务」最大的口径差异。插件来源的任务由内核调度器调用**目标插件接口**，能力仍由目标插件的 manifest 权限与授权对象约束；因此内核不必开放 root 命令执行面（见 §3「明确不做」）。
- **幂等与归属**：任务 id 由内核按 `plugin:<插件 id>:<key>` 生成，按 `(source, key)` upsert——`source` 取内核注入的 caller（插件伪造无效）。本插件只能改/删 `plugin:com.skiff.cron:` 前缀内的任务；内置任务（`builtin:`）只读展示、只可启停。
- **目标选择器**：读 `capability/interop/catalog`，只列「当前真的调得通」的 `插件:方法`（四道闸全过才出现），避免用户选了再吃 403。
- **表达式单一来源**：内核装包校验、内核 `tick` 调度、插件新建任务时的表达式校验与预设预览全部走 `crates/cron`（`skiff-cron`），三处同一份解析器，不各写一份。
- **风险**：任务数量 × 表达式频率决定调用量；「每分钟最多调用几次」这类数字**待实测**，不写资源承诺。禁用/删除任务即时生效，但已开始的调用不能中断（15s 超时兜底）。

### 4.7 菜单与槽位归属（P2 已迁移）

6 个官方插件与 testkit（共 7 个包）均为 `api = "1.3"` + `[[menu]]` + `view` + 随包 schema（`ui/schema/<view>.json`）；默认由壳页原生渲染，旧 `ui/<view>.html` 只在显式 `?legacy=1` 时使用。完整契约见 `docs/contract/PLUGIN_UI_CONTRACT.md`，菜单部分见 `docs/contract/MENU_CONTRACT.md`。

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
- **契约与验收**：主体/授权/审计口径见 [KERNEL_FTP_CONTRACT.md](../../contract/KERNEL_FTP_CONTRACT.md)；回归项见 [TEST_MATRIX.md](../../test/TEST_MATRIX.md) `FTP-01`–`FTP-09`。

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
- **契约与验收**：动词、授权、审计与安全边界见 [KERNEL_FIREWALL_CONTRACT.md](../../contract/KERNEL_FIREWALL_CONTRACT.md)；回归项见 [TEST_MATRIX.md](../../test/TEST_MATRIX.md) `FW-01` 起。**本轮代码未经编译验证，未上机验收。**
