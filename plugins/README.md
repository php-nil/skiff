# Skiff 官方插件集

这里是官方插件的**第二 workspace**（与根 `crates/` 分开构建），主程序发布包会随包携带
本目录产出的 `.wbpkg`。插件原则上只做「能力面的组合」，不新增内核接口；两个例外是
`com.skiff.ftp` 与 `com.skiff.firewall`——它们需要宿主软件包与特权通道，分别新增了内核
`ftp` 与 `firewall` 能力面，契约见 [`docs/KERNEL_FTP_CONTRACT.md`](../docs/KERNEL_FTP_CONTRACT.md)
与 [`docs/KERNEL_FIREWALL_CONTRACT.md`](../docs/KERNEL_FIREWALL_CONTRACT.md)。

| 插件 | id | 能力 | 用途 |
| --- | --- | --- | --- |
| `guard/` | `com.skiff.guard` | docker + fs + identity | 容器守护：每分钟巡检，`dead`/`restarting`/`unhealthy`（可选 `exited`）按冷却与小时预算重启 |
| `monitor/` | `com.skiff.monitor` | fs（`path:/proc`、可选 `path:/etc/passwd`） | 主机监控：CPU/内存/负载/网卡/运行时长，5 分钟采样进环形缓冲；进程视图（PID/用户/CPU/内存/启动时间/命令行，排序/筛选/分页）与网络视图（监听端口、连接状态、对端地址、套接字属主 uid），均为只读 `/proc` |
| `files/` | `com.skiff.files` | fs + identity | 文件管理：授权根内浏览/新建/删除/改名/编辑（256KiB 分页） |
| `cron/` | `com.skiff.cron` | identity + interop + scheduler | 计划任务管理 UI：新建/编辑统一在弹窗内完成（基本信息 / 调度 / 高级分组），按调度预设（每 N 分钟/小时、每天/每周/每月/工作日）选择并预览，落库仍是标准 5 段表达式；亦支持自定义表达式。调度与执行全在内核，插件只经 `scheduler` 能力面增删改查（不执行宿主 shell）；内置任务只可启停，越界任务在插件 RPC 层硬拦截 |
| `firewall/` | `com.skiff.firewall` | **firewall（新增能力面）** + interop | 防火墙与 fail2ban：nftables（`inet skiff`）/iptables（`SKIFF` 链）规则查看、增删与端口放行；fail2ban 安装、jail 启停、封禁列表与解封。对外经 interop 提供 `com.skiff.firewall:fail2ban.*`，FTP 等插件按需调用；仅 Native 形态 |
| `ftp/` | `com.skiff.ftp` | **ftp（新增能力面）** + interop | FTP 管理：站点账号增删改与启停、服务安装/配置/回收/重启；服务配置面板可改监听端口、被动端口范围、连接上限、TLS 模式与证书/私钥路径（保存后立即重载或安装后沿用）；服务配置以内核 `/etc/skiff/ftp-service.json` 为唯一真相；宿主 pure-ftpd，账号 uid/gid 与站点 PHP 容器对齐；**不再安装或维护 fail2ban**，「启用 fail2ban 防护」是可选开关，经 interop 调 `com.skiff.firewall`；仅 Native 形态 |
| `testkit/` | `com.skiff.testkit` | fs + identity + interop | 插件架构测试台：多视图/单视图、概览槽位、RPC 成功与失败、能力面授权与越界、主题与 CSP，不执行任何宿主写操作 |

## 约定

- crate 名 `skiff-plugin-<x>`；包内可执行名固定 `plugin`（`skiff-plugin pack` 按 `bin/<arch>/plugin` 收）。
- id 用 `com.skiff.<x>`；声明菜单/槽位用 `api = "1.2"`，未迁移的老包保持 `"1.0"` / `"1.1"` 并走 `[[ui]]` 降级。版本独立 semver。
- 功能页文件按视图命名：`view = "main"` → `ui/main.html`，其余 `view = "<x>"` → `ui/<x>.html`；同一插件内视图名唯一。路由分工见 `docs/PLUGIN_UI_CONTRACT.md`。
- `manifest.toml` 的 `key_id` 在源码里是 **64 个 0 的占位**：发布私钥只在打包机/CI secret，
  打包时 `skiff-plugin stamp` 在临时目录回填指纹（仓库里不含任何密钥指纹）。
- 权限遵循最小化：guard 只要 `containers.list/inspect/restart`，
  monitor/files 只读或按需读写，具体见各自 manifest。
- 插件 UI 只经父页面 postMessage 桥取数（`skiff.rpc`），不持有面板凭据、不发同源请求。
  菜单由 manifest 的 `[[menu]]` 声明、内核渲染；`/{p}/plugins/{id}` 是内核信息与管理页，
  插件只负责 `/{p}/plugins/{id}/<view>` 加载的 `ui/<view>.html`。
- **UI 资源全部同源随包分发**：插件 UI 的 CSP 是 `script-src 'self'; connect-src 'none'`，
  CDN 与内联脚本都加载不了。第三方前端库（如 `files/ui/vendor/` 里的 highlight.js）
  必须把文件 vendor 进 `ui/` 并在页面里用相对路径引用，同时保留其 LICENSE。
- **插件间跳转**：发 `parent.postMessage({type:"skiff.open", nonce, plugin:"com.skiff.<x>", path})`
  请面板切到另一插件界面（`path` 为定位建议，越权仍由目标插件 `path:` 授权根判定）。
  目标须是「已启用且有 UI 入口」的插件，否则父页面回 `skiff.open.result {ok:false}`。
- **跳转 ≠ 调用**：跳转只改浏览器地址、不授予能力；要**真的执行**对方能力（目标插件须在
  manifest 声明 `[interop].provides`），用 `manifest.toml` 的 `[interop]` 声明 + SDK
  `Plugin::interop_call(target, method, params)`，由内核四道闸判定后转发到目标插件 RPC。
  - `[interop].provides` = 本插件对外开放的方法（`method` + 可选 `label`/`description`）；
    `consumes` = 允许调用的目标，写作 `com.skiff.<x>:<method>` 或 `*`。
  - 声明 `consumes` 必须同时有 `interop:call` 权限；装包时内核按 `consumes` 预填
    `plugin:<id>` 对象授权（四道闸缺一即 403）。`resource_hints` 里也可写 `plugin:com.skiff.cron`。
  - 内核在转发时注入 `params._interop.caller`（覆盖调用方自带值），目标插件据此记录来源；
    同一任务用 `(source, key)` 幂等 upsert，重复点击不会堆出多条。
  - 调用方与目标都不能在同一调用链里重复进入（内核 busy 集合），单次转发上限 15s。
  - 目标选择器不要写死插件列表：`interop_catalog` 只返回**真正调得通**的接口，UI 按它渲染。

## 插件 UI 交互与主题

- **主题协议**：插件页缺省深色（`html[data-theme="dark"]`）；父面板切换主题时向 iframe 发
  `{type:"skiff.theme", theme:"light"|"dark"}`，插件写 `html[data-theme]` 由 CSS token 覆盖即可。
  iframe 先于父脚本就绪时主动发 `skiff.theme.request` 索取一次；不监听消息的老插件保持深色缺省。
- **files 文件管理**：左目录树 + 快捷入口、可点面包屑、名称/大小/修改时间列表；支持
  Ctrl/Cmd 点击多选、Shift 连选、方向键/Home/End/Enter/F2/Delete/Backspace/空格、右键菜单、
  行内重命名、新建目录、编辑弹窗（行号 + 等宽 + 未保存提示）。只用 `list/read/write/remove/rename/mkdir`；
  单次读写 256 KiB，超限只读第一页；保存是全量覆盖；不提供上传/下载。

## 菜单与槽位声明（P2）

一级固定六项，插件只能挂到固定一级下；二级/三级都由内核渲染，插件只负责 `ui/` 内容。
完整规则见 `docs/MENU_CONTRACT.md`。

| 插件 | 二级归属 | 三级标签 | 固定页槽位 |
| --- | --- | --- | --- |
| guard | 工具 → 容器守护 | 容器 / 事件 | — |
| monitor | 工具 → 主机监控 | 总览 / 历史采样 / 进程 / 网络 | `overview.health` |
| files | 工具 → 文件管理 | 无（单视图 `main`） | — |
| cron | 工具 → 计划任务 | 任务列表 / 新建任务 | — |
| firewall | 工具 → 防火墙 | 防火墙规则 / fail2ban | — |
| ftp | 工具 → FTP 管理 | 账号 / 运行状态 | — |
| testkit | 工具 → 架构测试台 | 主视图 / 检查 / 分组 + 单视图对照 | `overview.actions` |

cron（完整 L2 + L3；`view` 决定 `ui/<view>.html`）：

```toml
api = "1.2"

[[ui]]
label = "计划任务"
path = "list.html"

[[menu]]
id = "jobs"
parent = "tools"          # 只能是固定一级 id
label = "计划任务"
order = 20
icon = "clock"
view = "list"             # 单视图 L2：ui/list.html，不渲染 L3 标签栏
```

> cron 只有一个功能页 `ui/list.html`：右上角「新建任务」是按钮，点击在页内弹窗完成；
> 编辑已有任务复用同一弹窗，不跳页、不与抽屉混用。内置任务不渲染删除按钮，只提供启停。

> 原 `com.skiff.nginx` 已内核化，网站一级与「网站列表 / 运行环境 / 设置」由内核提供，
> 不再随官方插件发布。

### 防火墙与 fail2ban 生命周期（`com.skiff.firewall`）

- **fail2ban 不是内核内置**，也不是 FTP 插件的附属物，统一由 `com.skiff.firewall` 管理。
  防火墙插件首次 `fail2ban.install` 时经 `apt-get install fail2ban` 安装；机器上已有则只复用。
- **防火墙规则只动自己的表/链**：nftables 用 `inet skiff` 表的 `input` 链，iptables 用
  `SKIFF` 链；按 `comment "skiff-<nanos>"`（nft handle）或 `-S` 序号删除，不触碰其它表/链，
  默认策略保持 accept。`port.allow` 是「放行某端口/协议」的便捷入口。
- **jail 文件**：写入 `/etc/fail2ban/jail.d/skiff-<jail>.conf`，由 `fail2ban.jail.ensure`
  生成、`fail2ban.jail.remove` 删除；`fail2ban.install` 新装时另写
  `/etc/fail2ban/jail.d/skiff-defaults.conf` 关闭 `sshd` jail，避免新装 fail2ban 封掉宿主 SSH；
  已有 fail2ban 的机器不改其既有 sshd jail。改动后 `systemctl restart fail2ban`。
- **封禁管理**：`fail2ban.banned` 读 `fail2ban-client status <jail>` 的封禁列表，
  `fail2ban.unban` 调 `fail2ban-client set <jail> unbanip`；**不卸载 fail2ban 软件包**，
  也不改系统级 `/etc/fail2ban/jail.conf` 或其它插件写的 jail。
- **对外接口**：`[interop].provides` 暴露 `fail2ban.status` / `fail2ban.install` /
  `fail2ban.jail.ensure` / `fail2ban.jail.remove` / `fail2ban.banned` / `fail2ban.ban` /
  `fail2ban.unban`，
  调用方写 `com.skiff.firewall:<method>` 并声明 `interop:call` 与 `plugin:com.skiff.firewall`。
- **权限与审计**：能力面动词即 `firewall:<method>`，内核按 manifest 授权逐次校验，
  成功/失败都记审计，`detail` 只含 `jail/port/protocol/source/action/ip/id`，不含命令行。

### FTP 的可选登录防护（`com.skiff.ftp`）

- FTP 插件**不再安装、配置或卸载 fail2ban**，也不再向内核传 `login_guard=true`；
  安装时显式 `login_guard=false`，避免旧内核路径继续代管 fail2ban。
- 「启用 fail2ban 防护」是**可选项**：运行状态页的防护卡片经 interop 调
  `com.skiff.firewall:fail2ban.install` 与 `fail2ban.jail.ensure`（jail `pure-ftpd`，
  filter `pure-ftpd`、port 取内核 `ftp.status.config.listen_port`、backend `systemd`），
  关闭时调 `fail2ban.jail.remove`。封禁列表与解封走 `fail2ban.banned` / `fail2ban.unban`。
- **未安装防火墙插件时功能不可用，但不影响 FTP 本身**：`protection` 返回
  `available=false` 与原因，UI 只展示提示；FTP 的安装、账号、配置与启停不依赖它。
- **卸载口径**：`uninstall` 只回收 pure-ftpd 自身（勾选 `remove_package` 时
  `apt-get remove pure-ftpd`，不做 `apt purge`）；fail2ban 与 jail 由防火墙插件管理，
  FTP 卸载不触碰。`/etc/skiff/ftp-service.json` 保留，重装/升级后沿用。
- **账号口径**：`account.set_enabled` 通过给 passwd 行加/去 `#` 前缀并 `mkdb` 实现；禁用后登录失败，
  账号与目录保留。最后登录不由 pure-ftpd 用户库记录，`accounts.list` 明确返回
  `last_login=null`、`last_login_source=unavailable`；配额与速率是后续项。

monitor（固定页槽位；内核负责外框/标题/失败降级）：

```toml
[[mount]]
slot = "overview.health"
ui = "health.html"       # 必须存在于包内 ui/
height = "sm"            # sm / md / lg
refresh = 30             # 0 / 30 / 60 / 300 秒
roles = ["admin", "operator", "viewer"]
```

插件 UI 收到 `?tab=<view>` 后可切换同一文件内的局部视图；不同 `view` 由内核更换 iframe
文件（`ui/<view>.html`）。同一文件内切换标签时父页面只 `postMessage({type:"skiff.menu",
nonce, menu, tab})`，不重载页面。无 L3 的 L2 不渲染标签栏。

工作台外框只保留页面标题区 + 顶部 L3 通栏（吸顶、与内容区同宽），iframe 是纯内容区。
插件应主动上报内容高度（`{type:"skiff.height", nonce, height}`），父页面只对工作台
iframe 生效，避免「iframe 内部滚动 + 页面滚动」双滚动；旧包不发消息时维持默认高度并
由外层滚动。主题仍用 `skiff.theme` / `skiff.theme.request`，加载阶段的 iframe 底色由
宿主 `--plugin-frame` 提供。

## 构建与自检

```sh
# 插件 workspace 的常规门禁（命令与 CI 同源，勿在此另立口径）
cargo fmt    --manifest-path plugins/Cargo.toml --all --check
cargo clippy --manifest-path plugins/Cargo.toml --all-targets --locked -- -D warnings
cargo test   --manifest-path plugins/Cargo.toml --locked
```

> `plugins/` 是独立 workspace（根 `Cargo.toml` 已 `exclude`），根三关不会编译这里；
> 闸门入口与串行约束见 [AGENTS.md](../AGENTS.md) §3.1，CI job 与命令清单见 [CI.md](../docs/CI.md) §2、§6。

## 打包（发布物携带）

```sh
# 发布私钥：离线保存，0600；CI 用 secrets.PLUGIN_SIGNING_KEY 落到临时文件
sh plugins/pack.sh --key /path/to/skiff-signing.key \
    [--target x86_64-unknown-linux-musl|aarch64-unknown-linux-musl] \
    [--out plugins/official]
```

产物在 `plugins/official/com.skiff.<x>-<ver>.wbpkg`，每个包都做过一次 `verify` 自检。
`deploy/pack-release.sh --plugin-key <文件>` 会自动调用本脚本，并把 `.wbpkg` 放进发布包的
`official/` 目录；`install.sh` 再把它们复制到 `/www/skiff/official/`（**不自动安装**，
权限确认不能跳过）。

## 装机后安装

**第一步先导入官方公钥**：`official/` 里的 `.wbpkg` 只是分发物，`install.sh` 只负责复制、
不自动安装，也不会自动进信任库；未导入时安装会被 TOFU 直接拒掉（`key_id` 不在信任列表）。

- UI：`/{入口前缀}/settings` → 「信任公钥」→ 粘贴 `/www/skiff/official/public-key.txt` 全文 → 保存；
- 安装：插件页上传 `.wbpkg` → 确认页逐条勾权限与对象授权 → 「确认并安装」；
- 非交互（测试机实际用法）：

```sh
ls /www/skiff/official
skiff plugin install /www/skiff/official/com.skiff.files-0.1.0.wbpkg \
    --resource path:/var/www          # 交互逐条确认；非交互用 --yes --grant ... --resource ...
```

各插件的授权要点：

- `monitor`：必须勾 `path:/proc`（否则采样与进程/网络视图 403）；可选勾 `path:/etc/passwd`
  把 uid 映射成用户名，不勾则显示数字 uid。
- `firewall`：只需能力面授权 `firewall:*`（安装确认页按 manifest 动词逐条列出）；它不直接读写
  文件，规则与 jail 由内核能力面经特权通道执行。要供其它插件调用时，对方需 `interop:call`
  与 `plugin:com.skiff.firewall` 对象授权。
- `ftp`：可勾 `path:` 站点根用于账号目录校验；要启用可选登录防护，FTP 侧需 `interop:call`
  与 `plugin:com.skiff.firewall`，防火墙插件未安装时该选项显示不可用，不影响 FTP 其它功能。
- `cron`：每装一个「被调度」的插件，勾一次对应的 `plugin:<插件 id>`（或 `plugin:*`）；
  只勾了 `plugin:` 却不给目标插件 `interop:call` 权限的一侧，内核按四道闸拒绝，不影响其他调用。
  `cron` 本身不需要 `fs`/`docker`：它只经内核 `scheduler` 能力面管理任务，**不执行宿主 shell**。
- `guard`：逐个勾要守护的 `container:<name|id>` 或 `project:<p>`；未授权容器只展示不重启。
- `files`：勾要管理的 `path:` 根（如 `/var/www`、`/etc`）；授权根之外的路径一律 403。
- `testkit`：勾 `path:/proc`（验证授权读与越界拒绝）；要验证 `interop:call` 时再勾
  `plugin:com.skiff.cron`。注意 cron 当前 `[interop].provides` 为空（调度已收归内核），
  `com.skiff.cron:jobs.list` 会被四道闸拒绝，只能验拒绝路径；成功路径需另装声明 provides 的插件。
  它只用只读能力，不执行任何写操作，适合在任何测试环境安装。

## 多架构

多架构是发布者责任：每个架构的发布包只带该架构的 `bin/<arch>/plugin`。交叉构建时
`pack.sh --target <triple>` 会用 `CARGO=cross`（CI 里传入），打包工具本身仍跑在打包机。
