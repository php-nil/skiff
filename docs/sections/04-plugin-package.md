# 4. 扩展体系（一）包格式 · 生命周期 · 信任模型

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§4.1–4.3 ｜ 上一章：[§3 总体架构](03-architecture.md) ｜ 下一章：[§4 扩展体系（二）](04-plugin-publish.md)

## 4.1 包格式与 manifest

`.wbpkg` = tar.zst：

```
manifest.toml          # 包内原始字节参与签名（不做规范化重排）
bin/{arch}/plugin      # amd64 / arm64 可执行文件
ui/                    # 静态资源（由内核托管，CSP 收紧）
migrations/            # 声明式迁移输入，由新版插件迁移 RPC 使用（§4.5）
files.sha256           # 载荷哈希清单；明确排除自身与 sig
sig                    # ed25519 签名（manifest.toml 与 files.sha256 的包内原始字节）
```

**包编码契约**：`files.sha256` 覆盖 `manifest.toml`、`bin/`、`ui/`、`migrations/` 等全部载荷普通文件（目录不单独列哈希），但不得包含 `files.sha256` 与 `sig`。每行格式为 `<64 位小写 sha256 hex><两个空格><UTF-8 相对路径>\n`，按路径 UTF-8 字节升序排列；路径统一使用 `/`，禁止绝对路径、`..`、空路径、重复路径、符号链接、硬链接与特殊文件。签名输入为 `b"SKIFF-WBPKG-v1\0" || u64be(len(manifest.toml)) || manifest.toml || u64be(len(files.sha256)) || files.sha256`，避免拼接歧义；`sig` 只存 64 字节原始 ed25519 签名，验签前不得规范化 TOML 或重排清单。

manifest 字段分两组：**包标识与兼容性**、**能力、权限与运行配置**。

**表 1｜包标识与兼容性**

| 字段 | 说明 |
|---|---|
| `id` / `name` / `version` | 反向域名风格唯一 id、展示名、插件版本 |
| `key_id` | 签名公钥指纹（SHA-256(raw public key) 的小写 hex）；仅用于选择信任库中的公钥，最终信任仍以验签与用户确认的指纹为准 |
| `api` | 插件 API 版本（semver，取值形如 `"1.0"`）。内核声明支持区间，不匹配**拒绝加载** |
| `run` | 取值 `on-demand`（默认）或 `resident`；`resident` 须写 `resident_reason` 且计入资源预算 |
| `requires_full_sandbox` | 默认 `true`；当前 landlock/cgroups 未实现，`true` 的插件默认拒装，admin 开启 `allow_reduced_sandbox` 后可降级安装并收到警告；声明 `false` 的插件直接按 reduced sandbox 运行 |

**表 2｜能力、权限与运行配置**

| 字段 | 说明 |
|---|---|
| `requires` | 必需内核能力数组（内核提供 `docker`、`identity`、`fs`、`net`、`interop`）。**安装时**校验是否提供，未提供直接拒装（而不是装上再坏）。受限形态（DockerRestricted）只提供 `docker` / `identity` / `interop` |
| `permissions` | 调用范围：`docker` / `identity` / `fs` / `net` / `interop` 动词白名单（Broker 逐次校验）。安装时逐条向用户确认；Docker 动词还须叠加 §6 对象 grant。**`fs` 是两把钥匙**：权限串（`read/write/list/mkdir/remove/rename`）+ 安装时确认的 `path:<绝对路径>` 对象授权根；绝对路径（宿主文件）缺授权根一律 403。单次读写上限 256KiB（留在 1MiB 单帧内），大文件由调用方按 `offset` 分页。`interop` 目前只有一个动词 `call`（跨插件接口调用，见下方 `interop` 字段） |
| `resource_hints` | 安装确认页的**建议**授权对象（`path:/var/www`、`container:demo`、`plugin:com.skiff.cron` 等），只用于预填勾选项，首次安装不是自动授权；重装/升级时对**新 manifest 相对已装 manifest 新增**的 hint 自动补授（只增不减，回收只走显式卸载，补授写 `plugin.grant_upgrade` 审计）。形式须与 `requires` 对得上（声明 `path:` 建议却不需要 `fs`、声明 `plugin:` 建议却不需要 `interop` 视为 manifest 非法）。空数组表示不预设（如 `files`、`guard` 让管理员自己选）。`plugin:` 支持 `plugin:*`（全量）但不允许 `plugin:com.*` 这类部分通配 |
| `network_egress` | 可选出网目标范围（MVP 仅 IP/CIDR + 端口，不做动态域名放行；`unrestricted` 表示接受完全开放）；默认空集即拒绝出网，安装时单独确认。reduced sandbox 无法保证执行时，见 §5.4 |
| `ui` | 菜单注册项：`label` 即插件界面名，挂到顶栏「插件」一级菜单的二级项（与内核页面「已安装」并列；不再单占一级入口）；`path` 是 `ui/` 下的相对入口。导航口径见 §3.4。内核托管 `/plugins/{id}/ui/*` 后以**沙箱 iframe**（opaque origin）加载，通信只走父页面 postMessage 桥——能力调用 `skiff.rpc`、插件间跳转 `skiff.open`（§4.2「UI 托管」） |
| `idle_timeout_sec` / `max_restarts` | 空闲回收秒数与重启次数上限，默认 300 / 3 |
| `config_schema` | 键值表（类型 + `secret` 标记），UI 自动渲染配置表单；`secret` 项不落日志与审计（落盘口径见 §5.2） |
| `cron` | 周期任务：5 段 cron 表达式数组；到点拉起插件调 `tick`，跑完即回收（§4.2）。表达式由 `skiff-cron` 一处解析/校验（装包与运行时同口径） |
| `interop` | 插件间接口：`provides`（本插件对外开放的方法，逐条 `method` / 可选 `label`/`description`）+ `consumes`（允许调用的目标，写作 `<插件 id>:<方法>` 或 `*`）。声明 `consumes` 必须同时有 `interop:call` 权限；安装确认页会按 `consumes` 自动预填 `plugin:<id>` / `plugin:*` 对象授权。**四道闸**：调用方 `interop:call` 权限、调用方 `plugin:<目标>` 对象授权、调用方 `consumes` 白名单、目标 `provides` 声明；禁止自调，内核用 busy 集合挡重入 |
| `memory_limit_mb` | 内存上限（MB）。**声明已入 manifest、当前未生效**——cgroups v2 未实现，只有 `setrlimit` 兜底（§12） |
| `pids_limit` | 进程/线程数上限，映射到 `RLIMIT_NPROC`（默认 128）；cgroups `pids.max` 未实现 |

`requires` 与 `permissions` 的区别：前者是“这个插件离了某能力就废”（装前拦），后者是“这个插件被允许碰哪些东西”（每次调用拦）。

## 4.2 生命周期与资源兜底

- **启动**：首次请求拉起，经继承 FD 一次性交付插件身份 token（绑定插件 id + 权限集，**与进程同生命周期**，退出即吊销）+ socket 路径；5s 内 `GET /healthz` 不通过即判失败。插件进程提前退出时立即收口，不再空等满 5s；外部 kill 后下一次 ensure 会先 `try_wait` 摘掉已退出的旧条目再重新拉起，旧 stderr 尾巴随新一次拉起写进 journal。
- **输出采集**：拉起时插件 stdout/stderr 以管道接入，由内核逐行转成结构化日志（stdout→info、stderr→warn），因此启动横幅、panic 与初始化失败原因都能在 `journalctl -u skiff` 看到，不与内核 stdout 原始字节互相穿插。单行超 1KB 截断、按每路 20 行/秒（突发 128 行）限速，超限只丢弃并汇总告警，不阻塞插件、不刷爆 journald。stderr 另留最近 2KB 环形缓冲：拉起失败时整段尾巴写进错误、审计与插件状态详情；每次新拉起会先把上一次尾巴冻结（并写一条 `warn`），新运行的输出不会覆盖上一次崩溃原因。
- **回收**：空闲 300s SIGTERM（`resident` 除外）；崩溃退避 0.5s/2s/8s，连续 3 次熔断 + UI 告警。
- **周期任务**：manifest 声明 cron 表达式，内核到点拉起插件调 `tick` RPC，跑完即回收——不让定时器把插件钉成常驻；**停机错过的 tick 不补跑**（要对账的插件在 tick 内自行幂等处理）。实现口径：内核每 30s 巡检一次，按 `{插件 id}|{表达式}` 记住「已触发的分钟序号」去重，所以 30s 的扫描节奏不会让 `* * * * *` 变成每分钟两次；单次 `tick` 调用有超时上限，超时按失败计并写审计。
- **进程降权**：每个插件使用按插件 id 稳定分配的独立 UID/GID（不进 docker 组），拉起后依次重置 supplementary groups（仅保留 `skiff-plugins`）、`setgid`/`setuid`、清空 capabilities 并设 `PR_SET_NO_NEW_PRIVS`；独立 UID 同时防止同 UID 插件互发信号、ptrace 或读取 `/proc/<pid>/environ`。`skiff.db`/`/etc/skiff/config.toml`/`initial-admin.txt` 保持 0600 root，插件仅自身目录（`/www/skiff/plugins/{id}/`）可写，目录内部分层 `versions/`（代码，只读）/ `data/` / `cache/` / `run/`（语义见 §5.2），SDK 注入 `SKIFF_PLUGIN_DATA_DIR` / `SKIFF_PLUGIN_CACHE_DIR` 约定路径、插件不硬编码——直连 `docker.sock`（root:docker 0660）与读内核数据两条旁路封死，越权只剩「经 Broker」一条路。
- **内核崩溃兜底（cgroup 部分未实现）**：拉起时 `prctl(PR_SET_PDEATHSIG)` + 插件端约定 UDS 断连即退出。设计目标是每个插件及其子孙放在独立 cgroup，正常回收或内核重启发现遗留实例时用 cgroup 清理整棵进程树；当前只杀直接子进程，进程树清理仍是 §12.1 缺口。
- **沙箱（设计目标；landlock / cgroups v2 / project quota / nftables 均未实现）**：设计口径为 landlock fs 白名单（仅自身目录 + manifest 声明且用户授权的目录）+ cgroups v2 `memory.max`（默认 256MB，manifest 可配）、`pids.max`（默认 128）、`cpu.max` + `setrlimit`（AS/NOFILE/NPROC/FSIZE）兜底；`data/` 与 `cache/` 在文件系统支持时启用 project quota；出网按插件 UID 用 nftables owner-match 收口，**默认拒绝**，仅开放 manifest 声明且用户授权的目标范围。**当前实际只有独立 UID + 文件权限 + `setrlimit`**，`memory_limit_mb` / `pids_limit` 的 cgroup 部分不生效；完整沙箱落地前不得宣称资源隔离。`requires_full_sandbox=true` 的插件默认拒装；admin 显式开启 `allow_reduced_sandbox` 后可降级安装，并在安装结果中返回降级警告。landlock 需内核 ≥5.13，RLIMIT_AS 计虚拟地址空间，Go/JVM/.NET 等托管运行时预留大地址空间易误伤——**支持矩阵以 M4 实测为准，未验证前只承诺 C/Rust 插件**。不做自定义 seccomp（manifest 预留开关）。**诚实声明**：沙箱 = 降权 + landlock + cgroups（后两者未实现）；插件自身 exec/文件/网络不经内核，manifest 权限只约束**经内核的调用**——装插件 = 信任作者进程，沙箱降低事故与越权面，但不承诺抵抗内核漏洞或沙箱逃逸（§10 风险 9）。
- **IPC**：Supervisor 预建每插件运行目录 `/run/skiff/plugins/{id}/`（插件 UID 0700），插件服务 socket 为其中的 `plugin.sock`（0600），避免共享可写目录内互相删除 socket；插件仅暴露 `GET /healthz` 与 `POST /rpc`（JSON-RPC 2.0）；插件回调内核走 `/run/skiff/kernel.sock` 上**同一套 `/api/v1`**，不开放 TCP 端口。`kernel.sock` 为 root:skiff-plugins 0660，父目录 0711（可穿越不可列），插件靠附加组 `skiff-plugins` 连接；内核同时校验 `SO_PEERCRED` 的 UID ↔ 插件 id 映射和进程级 Bearer token，不能只凭 socket 可达或 token 单一因素授权。身份 token 经继承 FD 一次性交付，不写环境变量；Broker 每次调用查库校验权限（不做长缓存），未授权 403 + 审计。默认单消息帧 ≤1MiB、RPC 超时 30s、单插件并发 8、等待队列 32，超限快速失败；持续超时进入熔断。插件 API 是**请求-响应控制面**；流式场景（终端/日志流）不经过插件——UI 直连内核 API，插件只发控制指令，属 API 契约的一部分。
- **UI 托管**：内核托管 `/plugins/{id}/ui/*`，详情页以**沙箱 iframe**（`sandbox="allow-scripts allow-modals allow-forms"`，不给 `allow-same-origin` / `allow-top-navigation`）嵌入——iframe 是 opaque origin，插件脚本拿不到面板会话与 DOM（`allow-modals` 仅为了让插件能用自己的 `window.confirm` 做删除二次确认；`allow-forms` 只为让插件 UI 的原生 `<form>` 提交事件能触发，CSP `form-action 'none'` 阻断任何真实提交/导航）。桥接层是父页面里的 `crates/web/static/skiff.js::attachPluginBridges`，插件侧直接 `parent.postMessage({type:"skiff.rpc", nonce, id, method, params})`——没有 `window.__SKIFF__` 这类父页面注入对象。每次加载由父页面生成随机 nonce，桥接层校验 `event.source === iframe.contentWindow`、nonce、消息 schema 与 URL 里的入口前缀，再代发到 `/{prefix}/plugins/{id}/rpc`（带 CSRF 头）并逐次审计 `plugin.rpc`。父向子回包虽需 `targetOrigin="*"`，仍绑定同一 source+nonce 且不携带面板凭据。插件 CSP 收紧 `connect-src 'none'`、`form-action 'none'`、`base-uri 'none'`、`object-src 'none'`，`X-Frame-Options` 改 `SAMEORIGIN` 以便被详情页嵌入。
- **插件间跳转（`skiff.open`）**：插件可发 `{type:"skiff.open", nonce, id, plugin, path}` 请面板切到**另一个插件**的界面（例：NGINX 站点页 → 「在文件管理里打开该目录」）。这条消息**不转发给任何插件进程**，只由父页面校验目标 id（正则 + 详情页下发的 `data-plugins` 白名单，白名单 = 已启用且有 UI 入口的插件）与 `path`（长度 ≤512、禁控制字符，服务端 `?open=` 二次清洗）后改浏览器地址。`path` 只是**定位建议**：能否读仍由目标插件自己的 `path:` 授权根判定，跳转本身不授予任何能力。
- **插件间接口调用（interop）**：需要「真的执行」对方能力时（例：NGINX 站点页点「新增备份任务」→ 写入计划任务），插件调内核 `POST /api/v1/capability/interop/call`（SDK `Plugin::interop_call(target, method, params)`），内核判定四道闸后把请求转发到目标插件的 `POST /rpc`，并把目标 RPC 的 `result` 原样回给调用方；`POST /api/v1/capability/interop/catalog` 返回「调用方**真正调得通**的接口目录」，UI 的目标选择器只列这个目录，避免用户选到点了必 403 的项。转发时内核在 `params._interop.caller` 注入调用方 id（覆盖调用方自带值），目标插件据此记录来源、做幂等 upsert。目标插件仍按自己的授权读写，interop 不放宽任何一方；调用方与目标都不能在同一调用链里重复进入（busy 集合），单次转发上限 15s，超时/失败逐次审计 `interop.call`。
- **热插拔的定义**：安装 / 卸载 / 升级不需要重启内核（不是运行中代码热替换）。

## 4.3 信任模型（第三方插件的前提）

第三方插件 = 任意原生可执行代码，属于高信任工作负载。威胁模型只承诺：隔离普通崩溃、封住已列出的权限旁路、在沙箱机制正常且无内核/运行时漏洞时限制越权；不承诺抵抗恶意 admin、宿主内核 0day 或未知沙箱逃逸。签名证明来源，不证明代码安全；没有信任链就谈不上“第三方发布”：

1. **完整性与安全解包**：上传先流式写入 root 0700 临时目录并限制压缩包大小；第一遍只解析、不释放条目，校验 §4.1 路径规则、条目类型、哈希与签名，默认上限为压缩包 100MiB / 解压总量 512MiB / 4096 条目 / manifest 1MiB。全部通过后第二遍解到 root 所有的 staging 目录，只允许目录和普通文件、拒绝链接与特殊文件、文件一律排他新建、禁止覆盖已有条目并检查剩余磁盘，最后原子切换版本目录。任何一步失败都清理临时目录，不得留下“部分安装成功”。
2. **信任源**：内置官方公钥；第三方公钥由用户通过带外渠道取得并在安装确认对话框中导入（TOFU——首次信任），UI 展示的指纹须与 manifest `key_id` 和实际公钥一致。**不自建中心 PKI**：极简定位，信任决策留给用户。
3. **未签名包默认拒装**；`allow_unsigned = true` 仅为开发配置项，UI 上标红提示。
4. **升级同源**：新包签名公钥须与已装版本一致；更换密钥视为新信任，需用户重新确认指纹。密钥撤销本期用本地信任库手动撤销，不承诺在线 CRL/OCSP。
5. 权限确认与签名确认是**两道独立闸**：签名回答“这个包是不是宣称的作者发布的”，权限回答“这个包被允许做什么”；二者都不能把恶意原生代码变成可信代码。
