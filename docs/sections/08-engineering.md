# 8. 工程结构

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§8 ｜ 上一章：[§7 资源预算](07-budget.md) ｜ 下一章：[§9 里程碑](09-milestones.md)

**系统 = 三块核心能力**：**Docker 管理**（产品内置主业）、**插件体系**（宿主开放能力面，内核稳定性的来源）、**Web 管理服务**（远程管理面）。工程结构按三核心 + 支撑层组织，代码位置与产品能力一一对应：

```
skiff/
├── crates/
│   ├── web/        # ★核心① Web 管理服务：axum 路由、四个全局中间件（由内到外：可信代理→会话→安全入口→CatchPanic）
│   │               #   + 非流式路由组的 request_timeout；budding 壳页回落、/ui/v1 JSON、WS/SSE（终端/日志流）、
│   │               #   静态托管、反代 X-Forwarded-* 处理；
│   │               #   CSRF/RBAC/审计在 handler 内，限流只在登录路径；
│   │               #   static/ + templates/ 前端资源（无 node，rust-embed；askama 只剩错误页与插件工作台）
│   ├── docker/     # ★核心② Docker 管理：bollard 封装、动词白名单 + 对象 grant + 参数字段策略、
│   │               #   路径校验(§5.5)、Compose 生命周期、容器 exec 终端(hijack)、日志流
│   ├── plugin/     # ★核心③ 插件体系（内核侧）：manifest / ed25519 签名校验、Supervisor
│   │               #   （拉起/退避/熔断/回收/降权；landlock/cgroups 未实现）、UDS JSON-RPC、权限接 Broker（§4）
│   │               #   + 能力面实现（fsops/capability/interop/fsgrant，能力面的正确归属地）
│   ├── sdk/        # ★核心③（第三方侧）：插件 SDK + skiff-plugin CLI（脚手架/构建/打包/签名/自检）
│   ├── policy/     # 支撑：三层策略判定（动词白名单/对象授权/参数字段白名单）+ 宿主路径校验
│   │               #   零数据库、零 Docker（路径校验触碰文件系统）——`cargo test -p skiff-policy` 即可跑全量判定
│   ├── compose/    # 支撑：Compose MVP 子集解析（纯 serde_yaml，docker 与 xtask 共用）
│   ├── identity/   # 支撑：用户/会话/TOTP/API Token/RBAC（web 与插件调用共用）
│   ├── store/      # 支撑：SQLite 迁移与仓储
│   ├── pkgfmt/     # 支撑：.wbpkg 包格式/manifest/签名编解码（内核与 SDK 共用，禁止各写一份）
│   ├── cron/       # 支撑：5 段 cron 表达式解析/匹配/下次触发（装包校验、内核调度、计划任务插件共用）
│   ├── sched/      # 支撑：调度纯逻辑（cron 匹配、单飞、错过补偿）；不能反向依赖 app
│   ├── fs/         # 支撑：文件操作原语（路径代数 + 临时文件/rename 原子替换 + 回收站），内核与插件共用
│   ├── host/       # 支撑：宿主命令执行通道（systemd-run 瞬态单元 / 普通子进程）——**唯一实现**，
│   │               #   插件能力面与内核动作共用；授权判定不在这里
│   ├── wire/       # 支撑：插件↔内核 UDS 协议帧编解码 + 路由常量 + 协议版本闸（纯函数，无运行时）
│   ├── model/      # 支撑：跨 crate 共享词汇（Role/Grant/Form/Permission/Cidr/Verb/ContainerSpec/Actor）
│   │               #   + 路径代数（within/normalize/..段判定），零 I/O（纯值类型，不碰文件系统）
│   ├── app/        # 支撑：应用上下文（AppState/WebConfig）+ 用例层；按域分 sites / nginx / tasks /
│   │               #   catalog / files-admin / menu / theme，不含路由与模板，core 与 web 共同依赖
│   └── core/       # 支撑：单二进制入口与装配、kernel.sock 能力面服务端（core/src/kernel/）、配置优先级
├── deploy/
│   ├── systemd/skiff.service + install.sh + uninstall.sh   # 主交付：宿主直装
│   ├── Dockerfile                                  # 可选 Docker 形态
│   └── 1panel/apps/skiff/                          # 可选：商店包（受限形态，§5.4）
├── plugins/        # 第二个 workspace（根 Cargo.toml exclude）：common 共享库 + 6 个官方插件 + testkit 测试台
├── docs/           # DESIGN.md（索引+一页结论）+ sections/（§1–§12 正文）+ 七个主题入口
│                   # （contract / impl / test / ops / plugin / sdui / archive，各自 <topic>.md + 同名目录）
│                   # + changes/（按日期变更记录）
├── tools/          # 闸门（cargo.cmd / git.cmd）、gate.ps1、linux-check.sh、iter.sh、perf/、skiff-server/
└── xtask/          # 工程校验：check（1Panel 变量 + Compose 子集 + 宿主通道）、verify-pkg（.wbpkg 结构/摘要/签名）
```

**核心 ↔ 代码对照**：

| 核心 | 职责边界 | 代码在哪 |
|---|---|---|
| **Docker 管理** | 容器/镜像/网络/卷/Compose/日志/终端；Docker 白名单与路径安全 | `crates/docker`（I/O）+ `crates/policy`（判定） |
| **插件体系** | 保证内核稳定 + 开放能力面：打包、签名、生命周期、沙箱、IPC、SDK | `crates/plugin` + `crates/sdk` |
| **Web 管理服务** | 远程 HTTP(S) 管理面：UI、API、实时推送、认证与审计入口 | `crates/web`（含 `static/`、`templates/`）+ `crates/identity` |

**依赖方向只有一条**（下层不得反向依赖上层）：

| crate | 直接依赖 | 明确禁止 |
|---|---|---|
| `model` | serde（仅 derive，零 I/O） | `store`、`docker`、`web` |
| `policy` | `model` | `store`、`docker`、`web` |
| `compose` | `model` | `store`、`docker`、`web` |
| `store` | `model`、rusqlite | `policy`、`docker`、`web` |
| `identity` | `model`、`store` | `docker`、`web` |
| `pkgfmt` | `model`、`cron` | `store`、`docker`、`web` |
| `cron` | chrono | `store`、`docker`、`web` |
| `wire` | serde_json | `store`、`docker`、`web` |
| `docker` | `policy`、`compose`、`model`、bollard | `web`、`core` |
| `plugin` | `pkgfmt`、`wire`、`store`、`model`（不含 HTTP 框架：能力面只回「状态码数字 + 消息」，响应组装在 `core`） | `docker`、`web`、`core`、`app` |
| `sdk` | `pkgfmt`、`wire` | `store`、`docker`、`web` |
| `host` | tokio（+ tracing） | `store`、`docker`、`web`、`app`、`plugin` |
| `fs` | `model`（+ serde / sha2 / 系统调用封装） | `store`、`docker`、`web`、`app` |
| `sign` | ed25519-dalek、sha2、hex（零 I/O） | `store`、`docker`、`web`、`app` |
| `app` | `model`、`store`、`identity`、`docker`、`plugin`、`pkgfmt`、`sched`、`fs`、`host`、`sign`（+ `ureq`/`rustls-pemfile`/`ed25519-dalek`：商店目录客户端，本仓唯一出网组件） | `web`、`core` |
| `web` | `app`、`model`、`store`、`identity`、`docker`、`plugin`、`pkgfmt` | `core` |
| `core`(bin) | 以上全部 | — |

判定类代码（`policy`）与格式/协议类代码（`pkgfmt`/`wire`）都不允许依赖 `store`/`docker`：
它们的安全结论必须能在没有数据库、没有 Docker 的环境里被单独验证。

`model` 只放**没有行为、没有 I/O** 的词汇与纯值类型（`Role`/`Grant`/`Form`/`Permission`/
`Verb`/`ContainerSpec`/`Actor`/路径代数）：判定在 `policy`，领域对象在 `store`。
`compose` 为拿 `ContainerSpec` 而依赖 `policy` 会把 `libc` 拖进一个「纯 serde_yaml 演算」的
解析器，因此这类值类型一律走 `model`。

`app` 是装配层（`AppState`/`WebConfig` + 用例层），不含路由与模板：`web` 与 `core` 都依赖它，
它不依赖 `web`。插件能力面的**权限判定与命令编排**住在 `plugin`，但 `plugin` 不得反向依赖
`app`——宿主上下文经 `plugin::CapabilityHost` 这个 trait 注入（依赖倒置）。`app` 的准入规则：
用例编排、领域服务、读模型可以进；路由、模板、HTTP 形状不进；跨域共享词汇进 `model`。
当前 `app` 内按域划分为 sites / nginx / tasks / catalog / files-admin / menu / theme；`views/` 存与模板无关的读模型（`/ui/v1` 与插件工作台共用），`commands/` 存用例编排（含 `plugin.rs` 插件生命周期）。

**能力面 × 实现位置**（能力面的判定与编排只在 `plugin`；HTTP 形状与宿主特权执行属内核装配）：

| 能力面 | 判定与编排 | HTTP 形状 / 特权执行 |
|---|---|---|
| fs / interop | `crates/plugin`（`fsops` / `fsgrant` / `interop`） | `crates/core/src/kernel/capability.rs`；内核自身写入走 `crates/app/src/admin_fs.rs`（`write_kernel` / `remove_kernel`） |
| exec | `crates/plugin/src/execgrant.rs`（`bin:` 三道闸）+ `crates/core/src/kernel/exec.rs`（提交/状态/取消） | 执行通道 `crates/host`（`skiff_host`），作业执行体 `crates/app/src/host.rs`（`drive_exec_job`）——插件与内核计划任务共用 |
| ftp | `crates/plugin/src/ftp.rs` | `crates/core/src/kernel/ftp.rs`（特权执行经 `skiff_host::run`） |
| firewall | `crates/plugin/src/firewall.rs` | `crates/core/src/kernel/firewall.rs`（同上） |
| scheduler | `crates/app/src/tasks.rs`（`sched` 为纯逻辑，不能反向依赖 `app`） | `crates/core/src/kernel/capability.rs` |
| docker / identity / config | 判定复用 `policy` / `store` | `crates/core/src/kernel/capability.rs` |

与 §3.3 的关系：内核原子能力是 `docker` / `identity` / `fs` / `net` / `exec` / `scheduler` 六项（`interop` 是插件间调用的转发面，`ftp` / `firewall` 保留但冻结）；上面三块是**产品形态面**——Docker 管理 = 内置主业（白名单/路径安全/Compose 全在 `crates/docker`），Web 管理服务 = 管理面（承载 `identity` 与 api/ui），插件体系 = 开放能力面（宿主上的第三方扩展）。`crates/docker` 独立成包正因为它是主业，不是普通适配器。

**最小可用版本 = 三块核心跑通**：能登录、能管容器、能装插件且崩不了内核。支撑层跟着三块成形，不单独立项。

`xtask check` 固化三类校验：**校验可选 Docker 形态 compose 里每个 `${VAR}` 都能在版本级 data.yml 的 formFields 找到同名 envKey**；校验 compose 只包含 §6 的 MVP 字段（未支持字段直接失败）；第三项是**宿主通道闸**：`app`/`core` 非测试代码出现 `Command::new` 即失败，内核文件写只允许 `xtask` 白名单 `FS_WRITE_ALLOWED` 登记的例外（当前只有 `catalog/mod.rs` 的 2 处流式下载），新增文件或超出登记处数即失败。`.wbpkg` 的结构、哈希清单与签名输入编码由独立子命令 `xtask verify-pkg <file.wbpkg>` 校验（与内核安装路径共用同一份 `skiff_pkgfmt`）。这些是上架与安装链路最高频的失败原因，值得用代码堵住。
