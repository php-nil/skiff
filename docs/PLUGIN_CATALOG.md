# 官方插件集规划：竞品对照、首批选型与目录落地

> 状态：**分析建议稿（未拍板）** ｜ 日期：2026-11-01
> 关联：[§1.4 分层边界](sections/01-requirements.md)、[§3.3 原子能力](sections/03-architecture.md)、[§4 扩展体系](sections/04-plugin-package.md)、[§9 里程碑](sections/09-milestones.md)
> 数字纪律：竞品能力只做定性对照，未核实的具体数量不写；本仓库现状结论均附代码位置，便于复核。

## 0. 一页结论

| 维度 | 结论 |
|---|---|
| 选型依据 | 竞品把面板做成「服务器操作系统」，Skiff 只做三核心，**官方插件集就是产品完整度的上限** |
| 首批 3 个 | **主机监控**、**Nginx 网站管理**、**文件管理器**（按落地难度排序，不是价值排序） |
| 目录 | 顶层 `plugins/`，作为**第二个 cargo workspace**，与 `crates/` 平级 |
| 最大前置 | 能力面缺口：`fs` 无宿主授权目录、`docker` 缺 exec/compose、`net` 一律 501 |
| 开箱即用 | 内置官方公钥 + 随发布包携带官方 `.wbpkg` + 面板一键安装（**权限确认不跳过**） |

**主结论**：竞品用户预期的是文件、网站、监控、防火墙、计划任务/备份、应用部署、数据库七个入口，而 Skiff 把这些全部放在插件层——所以「内核不内置」不能变成「用户装完只有容器页」。首批 3 个必须与内核能力补齐**捆绑交付**，否则插件只是空壳；目录上按独立发布物组织，不混进内核 workspace。

## 1. 能力现状核对（选型的硬约束）

| 能力 | 代码现状 | 位置 | 对官方插件集的含义 |
|---|---|---|---|
| `docker` | 白名单 26 个动词；Broker 实际只执行 `containers.list/inspect/logs`、`images.list`、`networks.list`、`volumes.list`、`info`、`version`、`containers.start/stop/restart/kill`；exec、create、remove、pull、compose 一律返回 Unsupported | `crates/docker/src/client.rs` 的 `Docker::execute` | Nginx 管理、一键部署、数据库管理、备份导出全部卡在 exec/compose |
| `fs` | 只允许插件**自身 `data/`** 下的相对路径 read/write/list；没有宿主目录授权；TOCTOU 的 `open_dir_nofollow` 已实现但未接入判定链 | `crates/core/src/kernel.rs` 的 `resolve_in_data` / `fs_read` / `fs_write` / `fs_list` | 文件管理器、主机监控、日志类插件全部缺前置 |
| `net` | 一律 501 + 审计 | `crates/core/src/kernel.rs` 的 `net_unimplemented` | 防火墙、ACME 证书不可做 |
| `identity` | 仅 `capability/identity/self` | `crates/core/src/kernel.rs` 的 `identity_self` | 用户同步/SSO 暂缓 |
| 插件 UI | 沙箱 iframe（无 same-origin）+ 父页面 postMessage 桥 + CSP `connect-src 'none'`；单帧上限 1MiB | `crates/web/src/handlers/plugins.rs` 的 `plugin_ui` / `rpc`、`crates/web/static/skiff.js` 的 `attachPluginBridges` | 插件 UI 不能直连内核，所有数据走插件 RPC；**大文件上传下载会被帧上限卡死**，文件管理器需要新增传输契约 |
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
| 防火墙 | ✅ | ✅ | ❌ | ❌ | ❌ | ⭕ | ❌ | 官方插件（net 就绪后） |
| 计划任务与备份 | ✅ | ✅ | ✅ | ⭕ | ❌ | ✅ | ❌ | 官方插件 |
| 应用一键部署 | ✅ | ✅ | ✅ | ✅ | ⭕ 模板 | ✅ | ❌ | 官方插件 |
| 用户与 RBAC | ✅ | ⭕ | ✅ | ✅ | ✅ | ✅ | ✅ 内核 | ✅ 内核 |

> 图例：✅ 内置；⭕ 部分支持或依赖外部组件；❌ 不提供。1Panel/宝塔为宿主直装面板，Coolify/Dokploy 为容器内 PaaS，能力口径不同，仅作功能面对照。

**对照结论**：

1. 竞品的「文件 + 网站 + 监控 + 防火墙」是面板首页级入口，不是可选项；Skiff 缺任何一项，用户都会认为「没做完」。
2. 差异化不在功能数量，而在交付方式：插件是**宿主真进程**（不是容器里塞 sidecar）、权限逐条确认、离线可装、可与 1Panel/宝塔同机共存。
3. 因此官方插件集应覆盖「开箱七件套」，但**按能力就绪度分波次**，不做一次性铺开。

## 3. 官方插件清单

| 优先级 | 插件 id | 名称 | 能力面 | 内核前置 | 竞品对标 | 备注 |
|---|---|---|---|---|---|---|
| P0 | `com.skiff.monitor` | 主机监控 | `fs`（只读） | fs 宿主只读授权 + cron 采样 | 1Panel/宝塔 监控 | 首页刚需，只读、无写风险 |
| P0 | `com.skiff.nginx` | 网站管理 | `docker` | `containers.exec` 命令模板 + 对象 grant | 1Panel/宝塔 网站 | §4.4 已承诺的官方示范插件 |
| P0 | `com.skiff.files` | 文件管理器 | `fs` | fs 宿主目录授权 + 大文件传输契约 | 1Panel/宝塔 文件 | 最高频入口，风险面最大 |
| P1 | `com.skiff.apps` | 应用模板一键部署 | `docker` | compose.up/down + 模板随包离线 | 1Panel/宝塔 应用商店 | 不做在线商店 |
| P1 | `com.skiff.firewall` | 防火墙 | `net` | net 能力 + §10.1 #17 出网收口通过 | 1Panel/宝塔 防火墙 | net 未就绪前不排期 |
| P1 | `com.skiff.backup` | 备份与恢复 | `docker` + `fs` | exec、fs 授权、流式导出 | 1Panel/宝塔 备份 | 大文件同样受传输契约约束 |
| P1 | `com.skiff.db` | 数据库管理 | `docker` | exec + fs 导入导出 | 1Panel/宝塔 数据库 | 先做 MySQL 或 Redis 单点 |
| P1 | `com.skiff.cron` | 计划任务 | 待定 | 宿主命令执行能力（当前不存在） | 1Panel/宝塔 计划任务 | 宿主 root 执行风险高，先只做容器内命令 |
| P2 | `com.skiff.logs` | 日志查看与清理 | `fs` | fs 授权（只读 + 截断） | 1Panel/宝塔 日志 | 依赖 fs 授权落地 |
| P2 | `com.skiff.guard` | 容器守护/自愈 | `docker` | **零内核改动**（读 + restart） | 部分竞品无 | 可作第一批的链路验证插件 |
| P2 | `com.skiff.notify` | 告警通知 | `net` | 出网收口 + 出网授权 | 1Panel 告警 | 需要 net 能力 |
| P2 | `com.skiff.sso` | 身份同步/SSO | `identity` | identity 能力扩展 | YunoHost/Portainer | 非面板刚需 |

**明确不做 / 暂缓**：

- **不做宿主 root 命令执行插件**（计划任务类）：当前没有对应能力面，给插件 root 等于绕开整套权限模型；先只支持容器内命令。
- **不在本期做在线商店**：渠道未决策（§1.4），官方包先随发布物离线分发。
- **不把终端/日志流做成插件**：内核已有容器 exec 终端与日志 SSE，流式场景按 §4.2 不进插件。

## 4. 首批 3 个：选型与前置

### 4.1 为什么首批不是「现在就能跑」的三个

零内核改动就能跑的是容器读操作类（容器守护、资产盘点），但它们解决不了「装完没有文件管理」的落差，只能用来验证打包与 UI 链路。官方首批应按**产品完整度**选，并接受它们带着内核前置一起做。

**实现顺序按难度排**：主机监控（只读 fs，最小）→ Nginx（exec 白名单，中等）→ 文件管理器（fs 授权 + 大文件传输 + TOCTOU，最重）。

### 4.2 主机监控 `com.skiff.monitor`

```toml
id = "com.skiff.monitor"
name = "主机监控"
version = "0.1.0"
key_id = "<官方公钥指纹>"
api = "1.0"
run = "on-demand"
requires = ["fs"]
cron = ["* * * * *"]
network_egress = []

[[permissions]]
capability = "fs"
verbs = ["host.read"]

[[ui]]
label = "监控"
path = "index.html"

[config_schema.retention_points]
type = "number"
label = "保留采样点"
default = "720"
```

- **功能面**：采样 CPU、内存、负载、磁盘、网络，写入插件自身 `data/` 环形缓冲；UI 经 RPC 取最近 N 点绘制曲线。
- **内核前置**：`fs` 宿主只读授权（manifest 声明路径 + 安装时确认 + Broker 逐次校验）；只读 `/proc/stat`、`/proc/meminfo`、`/proc/loadavg`、`/proc/diskstats`、`/proc/net/dev` 等。
- **设计取舍**：用 `cron` 每分钟 `tick` 采样、跑完回收，避免 `resident` 常驻吃资源预算；停机错过的 tick 不补跑，曲线允许断点。
- **风险**：采集间隔与保留点数直接影响进程开销，**数字待实测**，不写资源承诺。

### 4.3 Nginx 网站管理 `com.skiff.nginx`

```toml
id = "com.skiff.nginx"
name = "Nginx 管理"
version = "0.1.0"
key_id = "<官方公钥指纹>"
api = "1.0"
run = "on-demand"
requires = ["docker"]
network_egress = []

[[permissions]]
capability = "docker"
verbs = ["containers.list", "containers.inspect", "containers.logs", "containers.exec"]

[[ui]]
label = "网站"
path = "index.html"
```

- **功能面**：站点列表、配置读写、`nginx -t` + reload、访问/错误日志、证书文件挂载。
- **内核前置**：`docker.call` 支持 `containers.exec`，且必须是**命令模板白名单**（固定子命令 + 参数校验与转义），不是透传任意命令；带输出上限、超时、并发与审计。动词枚举里已有 `containers.exec`，但 `Docker::execute` 未实现。
- **对象授权**：目标容器须由 admin 显式授予 `container:<id>` grant，插件不能凭标签自行认领（§4.4）。
- **风险**：exec 白名单设计失败 = 把容器 root 交给插件。模板、参数校验、输出上限三件事必须一起做。

### 4.4 文件管理器 `com.skiff.files`

```toml
id = "com.skiff.files"
name = "文件管理"
version = "0.1.0"
key_id = "<官方公钥指纹>"
api = "1.0"
run = "on-demand"
requires = ["fs"]
network_egress = []

[[permissions]]
capability = "fs"
verbs = ["host.read", "host.write", "host.list"]

[[ui]]
label = "文件"
path = "index.html"
```

- **功能面**：授权根内的浏览、编辑、上传、下载、权限位查看；不做跨根操作。
- **内核前置**：
  1. **宿主路径授权**：新增 `path:` 对象 grant（目录粒度），安装时由 admin 选择/确认；Broker 逐次校验，`..`、符号链接、未授权根一律 403 + 审计。当前 `object_grants` 只接受 `container:` 与 `project:`，manifest 也没有声明授权目录的字段，两处都要补。
  2. **大文件传输契约**：插件 UI 走 postMessage RPC 且单帧 1MiB、`connect-src 'none'`，无法承载文件上传下载。需要内核提供受限流式传输（经父页面桥、绑定插件 grant 与路径），或由内核直接向面板 UI 提供 fs 流式接口（插件只发控制指令）。
- **风险**：TOCTOU 判定链未接入（§10.1 #4），必须在文件管理器上线前补齐；写宿主路径的误伤面大，建议授权根默认只读，写权限单独确认。

## 5. 目录落地：放在哪里

### 5.1 方案对比

| 方案 | 优点 | 缺点 | 结论 |
|---|---|---|---|
| 顶层 `plugins/`（独立 workspace） | 插件是独立发布物；与内核边界清晰；CI 可单独跑；第三方可整目录对照 | 需要第二 workspace、独立 CI job 与独立 lock/target | **推荐** |
| `crates/plugins/*`（根 workspace 成员） | 复用现有三关 | 破坏「`crates/` = 内核 crate」的边界；插件测试/构建被内核发布节奏绑定 | 不推荐 |
| 独立仓库 `skiff-plugins` | 发版与 CI 最干净，社区贡献隔离 | SDK 未发布到 crates.io，跨仓库同步 SDK 变更成本高 | 中期 |
| `deploy/plugins/` | 无额外结构 | `deploy/` 只放部署物料，不含源码 | 不可 |

### 5.2 推荐结构

```
plugins/
├── Cargo.toml              # 第二 workspace：members + workspace.dependencies（skiff-sdk 走 path）
├── README.md               # 官方插件集约定：id、版本、签名、构建、多架构、发布
├── monitor/                # com.skiff.monitor
│   ├── Cargo.toml          # name = skiff-plugin-monitor，[[bin]] name = "plugin"
│   ├── manifest.toml       # key_id = 官方公钥指纹（公开信息，可入库）
│   ├── src/                # main.rs + rpc.rs + 采集实现
│   ├── ui/                 # index.html + app.js（只经 postMessage）
│   └── migrations/
├── nginx/                  # com.skiff.nginx
└── files/                  # com.skiff.files
```

约定：

- crate 名 `skiff-plugin-<x>`；包内可执行名固定 `plugin`（`skiff-plugin pack` 按 `bin/{arch}/plugin` 收）。
- id 用 `com.skiff.<x>`；版本独立 semver；`api = "1.0"`，内核并行兼容 N-1 主版本。
- 插件 UI 只经 `window.__SKIFF__` 桥取数，不携带面板凭据、不发同源请求。
- **私钥不入库**：manifest 的 `key_id` 是公钥指纹，可公开；私钥只在 CI secret 或离线发布机。

### 5.3 构建、签名与发布

- 根 `Cargo.toml` 增加 `exclude = ["plugins"]`，显式声明第二 workspace，避免被根 workspace 误纳入。
- `plugins/Cargo.toml` 作为 workspace 根，`workspace.dependencies` 统一 `skiff-sdk = { path = "../crates/sdk" }`，三个插件共享 lock 与 target。
- CI 新增两个 job：`plugins-check`（Linux：`fmt --check`、`clippy -D warnings`、`test`，只跑插件 workspace）；`plugins-release`（musl amd64/arm64 双架构构建 → `skiff-plugin pack` → `verify`）。
- 开发密钥与发布密钥分离：开发机用 `skiff-plugin new` 生成的临时密钥时，manifest 的 `key_id` 与官方指纹不一致，`pack` 会拒。建议给 `skiff-plugin pack` 增加 `--manifest <dev-manifest>` 开关（SDK 小改），避免开发包与发布包 manifest 分叉。
- 多架构是发布者责任：缺哪个架构，该架构上就拒装（§4.4）。

### 5.4 开箱即用的三条交付线

1. **内置官方公钥**：命中内置指纹时显示「官方」徽章并免去手动导入（权限确认仍逐条）。当前代码缺口：信任库只有 TOFU，没有内置官方公钥。
2. **随包携带**：`install.sh` 把官方 `.wbpkg` 放到 `/var/lib/skiff/official/`（root 只读），**不自动安装**。
3. **面板「官方插件」列表 + 一键安装**：从本地官方源读取，离线可用，安装流程仍走权限确认对话框。

> 只把 `.wbpkg` 放 GitHub Release、让用户手动上传，不算「开箱即用」。

## 6. 建议实施顺序

| 阶段 | 内容 | 验收 |
|---|---|---|
| 0 决策与目录 | 本文件评审 + 建 `plugins/` 第二 workspace | 目录、构建与 CI 门禁就位 |
| 1 能力补齐 | fs 宿主授权（`path:` grant、manifest 字段、Broker 校验、TOCTOU 接入）；docker exec 命令模板；内置官方公钥 | `cargo test -p skiff-policy` 越权用例零 Docker/SQLite 通过 |
| 2 首批插件 | monitor → nginx → files | Linux 真机：安装、权限确认、启用、UI、RPC、越权、卸载 |
| 3 分发 | 官方源目录 + 面板列表 + 随包携带 | 断网一键安装且权限确认不被跳过 |
| 4 第二批 | 防火墙（net #17 通过后）、计划任务/备份、应用模板、数据库 | 待定 |

> 能力补齐与插件开发并行时，先冻结 manifest 字段与协议契约再写插件，否则三份插件要返工。

## 7. 风险与未决

| 风险 | 说明 | 处理 |
|---|---|---|
| exec 白名单形同虚设 | 透传任意命令等于把容器 root 交给插件 | 命令模板 + 参数校验 + 输出/超时/并发上限 + 审计 |
| fs 授权越权 | 路径穿越、符号链接、TOCTOU | 授权根目录粒度；接入 `openat` + `O_NOFOLLOW`；越权 403 + 审计 |
| 大文件传输 | 插件 UI 单帧 1MiB 且 `connect-src 'none'` | 新增受限流式传输契约，或分片 RPC（需评估开销） |
| 资源预算 | 每个插件是独立进程 | 采集频率与保留点数设下限；数字待实测，不写承诺 |
| 信任与分发 | 无内置官方公钥、渠道未决策 | 先做内置公钥 + 随包本地源，不动商店 |
| 插件 UI 静态资源 | opaque origin 下相对 `fetch` 会失败（§12） | UI 只经 postMessage，数据文件随包自包含 |

未决：商店/分发渠道（§1.4）；计划任务是否开放宿主命令执行能力；监控是否改 `resident`；文件管理器默认写权限还是默认只读。
