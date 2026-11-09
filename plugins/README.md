# Skiff 官方插件集

这里是官方插件的**第二 workspace**（与根 `crates/` 分开构建），主程序发布包会随包携带
本目录产出的 `.wbpkg`。插件原则上只做「能力面的组合」，不新增内核接口；唯一例外是
`com.skiff.ftp`——它需要宿主软件包与 systemd，新增了内核 `ftp` 能力面，
契约见 [`docs/KERNEL_FTP_CONTRACT.md`](../docs/KERNEL_FTP_CONTRACT.md)。

| 插件 | id | 能力 | 用途 |
| --- | --- | --- | --- |
| `guard/` | `com.skiff.guard` | docker + fs + identity | 容器守护：每分钟巡检，`dead`/`restarting`/`unhealthy`（可选 `exited`）按冷却与小时预算重启 |
| `monitor/` | `com.skiff.monitor` | fs（`path:/proc`） | 主机监控：CPU/内存/负载/网卡/运行时长，5 分钟采样进环形缓冲 |
| `files/` | `com.skiff.files` | fs + identity | 文件管理：授权根内浏览/新建/删除/改名/编辑（256KiB 分页） |
| `cron/` | `com.skiff.cron` | identity + interop + scheduler | 计划任务管理 UI：新建/编辑统一在弹窗内完成（基本信息 / 调度 / 高级分组），按调度预设（每 N 分钟/小时、每天/每周/每月/工作日）选择并预览，落库仍是标准 5 段表达式；亦支持自定义表达式。调度与执行全在内核，插件只经 `scheduler` 能力面增删改查（不执行宿主 shell）；内置任务只可启停，越界任务在插件 RPC 层硬拦截 |
| `ftp/` | `com.skiff.ftp` | **ftp（新增能力面）** | FTP 管理：站点账号增删改与启停、服务安装/配置/回收/重启；服务配置面板可改监听端口、被动端口范围、连接上限、TLS 模式与证书/私钥路径、fail2ban 开关（保存后立即重载或安装后沿用）；服务配置以内核 `/etc/skiff/ftp-service.json` 为唯一真相，插件不再写 `service-config.json`；宿主 pure-ftpd，账号 uid/gid 与站点 PHP 容器对齐；仅 Native 形态 |
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
| monitor | 工具 → 主机监控 | 总览 / 历史采样 | `overview.health` |
| files | 工具 → 文件管理 | 无（单视图 `main`） | — |
| cron | 工具 → 计划任务 | 任务列表 / 新建任务 | — |
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

### FTP 与 fail2ban 生命周期（`com.skiff.ftp`）

- **fail2ban 不是内核内置**，是宿主系统包（Debian/Ubuntu 的 `fail2ban`）。它由
  `com.skiff.ftp` **首次安装**且 `login_guard = true`（内核配置默认值）时，与
  `pure-ftpd` 一起经 `apt-get install` 装上；机器上原本已有 fail2ban 则只复用，不再重复安装。
- **专用 jail**：安装时写入 `/etc/fail2ban/jail.d/skiff-pureftpd.conf`，内容为
  `[pure-ftpd] enabled = true / backend = systemd / maxretry = 3 / findtime = 10m / bantime = 10m`。
  仅当 `fail2ban` 是本次新装（安装前不存在）时，同一文件追加 `[sshd] enabled = false`，
  避免新装 fail2ban 把宿主既有 SSH 登录封掉；机器原本已有 fail2ban 时**不改**其既有 sshd jail。
- **开关口径**：`login_guard=false` 只移除本插件的 pure-ftpd jail；若同文件里还留着安装时写的
  `[sshd] enabled=false`，则重写为仅含该段，避免顺手把宿主 sshd 防护打开。随后
  `systemctl restart fail2ban`；**不卸载 fail2ban 软件包**，也不改动系统级
  `/etc/fail2ban/jail.conf` 或其它 jail。
- **卸载口径**：`uninstall` 走同一套 jail 清理逻辑；即使勾选 `remove_package`，也只
  `apt-get remove pure-ftpd` 并清理 pure-ftpd 的 passwd/pdb/conf，不执行 `apt purge`。
  `/etc/skiff/ftp-service.json` 保留，重装/升级后沿用。
- **状态可观测性**：内核 `ftp.status` 回传 `fail2ban.installed / running / jail_configured /
  jail_enabled / currently_banned / total_banned / recent_ban`；取不到时为 `null`，
  UI 显示「未知（读取失败）」，不伪造 0。`recent_ban` 口径为
  `fail2ban-client get pure-ftpd banip --with-time`。
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

- `monitor`：必须勾 `path:/proc`（否则采样 403）。
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
