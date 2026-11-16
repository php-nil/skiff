# exec：宿主机命令执行能力面（粗授权 + 全审计 + 运行记录）

> 状态：**内核底层已落地（2026-11-16）**；确认页风险标注、升级强制复核、试点三项未落地（§6）
> 一句话：命令执行照 `fs` 那套做——内核只提供一条执行接口，授权粒度是「哪个二进制」（`bin:` 对象授权）；**文件读写进审计、任务执行进运行记录**；执行通道 `skiff_host` 是唯一实现，插件与内核自身的动作共用，区别只在要不要授权。**没有命令模板 DSL。**
> 相关：[DESIGN.md](../DESIGN.md) §11 决策 #10 / #13 ｜ [KERNEL_EXEC_CONTRACT.md](../contract/KERNEL_EXEC_CONTRACT.md) ｜ [KERNEL_FIREWALL_CONTRACT.md](../contract/KERNEL_FIREWALL_CONTRACT.md) / [KERNEL_FTP_CONTRACT.md](../contract/KERNEL_FTP_CONTRACT.md)（legacy，冻结） ｜ [plugins/README.md](../../plugins/README.md)

---

## 一、要解决的问题与取舍

### 1.1 问题

插件进程全程降权（决策 #7，不动摇），任何宿主特权动作必须经内核能力面。但能力面词汇表是封闭集合（`skiff-wire` 里逐条写死的 `CAP_*` 路由），插件需要一个表外特权动词时只能给内核加能力面。这不是偶发，已两次结构性失守：`ftp`（内核侧 2265 行）与 `firewall`（内核侧 1815 行）都是「插件只做组合、不新增内核接口」原则的破例，每加一个执行型域都要发内核版。

目标只有一条：**新的宿主特权功能 = 只发插件包，不动内核**。

### 1.2 为什么不做命令模板（方案 A 否决留档）

曾设计过「manifest 里声明 `[[host_action]]` 命令模板 + 内核按模板渲染 argv」的方案（槽类型系统、逐条确认页、firewall/ftp 全量迁移）。否决理由：

| 理由 | 说明 |
| --- | --- |
| 表面积为目标的三倍 | 模板结构 + 槽类型系统 + 渲染器 + 校验器 + 逐条确认页 + 4000 行迁移，而项目定位是轻量面板 |
| 「零内核改动」打折 | 二进制白名单与槽类型表仍是内核侧封闭集合：加 `nginx`、加一个新的值类型照样发内核版 |
| 迁移是纯成本 | firewall/ftp 已有契约与测试、改动频率低；把 4000 行搬进插件包提升不了任何用户可见能力 |
| 类型系统防错了对象 | 丰富槽类型防的是「命令跑坏」（可靠性），不是「提权」（安全）。安全不变量只有几条且与领域无关，见 §3 |

代价是运行期权限面变粗（授权 `apt-get` 就等于授权 root）。这个代价是明示的，不是漏洞：真实边界是**「谁写的包 + 装之前看了确认页 + 每次执行留审计与运行记录」**，与 `fs` 授权一个 `path:` 根后插件能在该根内任意写是同一级别的信任。

### 1.3 为什么不是特权伴随进程

签名 root 组件跑在内核外，签名与审计成本等于内核代码，信任链爆炸——否决，不设过渡态。

---

## 二、模型：与 fs 同构，但记录分流

`fs` 今天就是「粗粒度授权 + 全量审计」，`exec` 不做第二套授权模型：

| `fs` | `exec` |
| --- | --- |
| 权限串 `fs:write` | 权限串 `exec:run`（只有一个动词） |
| 对象授权 `path:/www/sites/foo` | 对象授权 `bin:/usr/bin/fail2ban-client` |
| `fsgrant::resolve` 判授权根包含 | `execgrant::resolve` 判二进制身份（§3） |
| 不预声明「要写哪个文件」 | 不预声明「要跑哪条命令」，插件自己决定 |

**分流点只有一处，但决定了整条链路**：

| | 文件读写 | 命令执行 |
| --- | --- | --- |
| 耗时 | 毫秒级，请求内完成 | 由命令决定（`fail2ban-client status` 几十毫秒，`apt-get install` 几分钟） |
| 记录 | **审计日志**（`audit`，保留 90 天） | **运行记录**（`jobs` + `job_logs`，保留 7 天 / 200 条） |
| 过程 | 无过程可言，记「谁动了哪个路径」 | 有过程：逐行输出、退出码、可取消 |
| 失败 | 当场返回错误码 | 作业结算为 `failed`，过程日志保留 |

命令如果也按请求-响应等结果，「慢」就会变成「超时失败」，插件只能盲目重试；而一份只记了第一行和最后一行结果的审计，复查时什么也说明不了。

### 谁授权、谁执行、谁记录

一条命令涉及四件互相独立的事。混在一起，每个调用点就会各自实现一遍执行逻辑：

| 层 | 负责什么 | 唯一实现在哪 |
| --- | --- | --- |
| 协议 | 插件怎么提请求（提交 / 查询 / 取消） | `crates/core/src/kernel/exec.rs` |
| 授权 | 这条命令**能不能**跑（三道闸，§3） | `skiff_plugin::execgrant` |
| 通道 | **怎么**起进程、读流、超时、取消、清理 | **`crates/host`（`skiff_host`）** |
| 记录 | 谁跑过什么、跑到哪、输出了什么 | `audit`（文件读写）/ `jobs` + `job_logs`（任务执行） |

**授权与通道正交**，这是本设计的骨架：

- 插件调 `exec` 面 → 先授权（§3），通过后进通道 → 作业记录；
- 内核自己发起的动作（安装流程里的宿主命令、只读探测）→ **没有授权问题**（它本来就是内核），直接进通道 → 作业记录。

两条路只在**入口**上不同。`skiff_host` 是唯一实现：`core` 的 firewall/ftp、`app` 的安装与探测、插件经 `exec` 提交的命令，跑的是同一份「起进程 / 逐行读 / 250ms 取消检查 / 超时后清理」。所以「环境安装要不要经过 exec」是个错误问法——它**不经过 `exec` 协议**（不需要 `bin:` 授权），但**必须经过同一条通道**，否则 app 层每处宿主命令都要再写一遍超时与收尾，那才是真正的重复实现。

通道有两种执行模式，共用同一套读流/超时/取消/清理：

| 模式 | 起法 | 用于 |
| --- | --- | --- |
| 特权（`HostSpec::privileged`） | `systemd-run` 瞬态单元，root，不继承主服务沙箱 | `apt`/`systemctl`/`pure-pw`/`nft` |
| 普通（`HostSpec::direct`） | 直接起子进程 | `openssl` 生成证书、`ip` 只读探测 |

不用 `systemd-run` 跑一次 `openssl x509`：单元创建本身比命令还慢，且这类命令不需要 root。区别只在「怎么起进程」，不在「怎么等结果」。

### 2.1 提交执行（`exec/run`）

```
POST /api/v1/capability/exec/run          # wire::routes::CAP_EXEC_RUN
{ "binary": "/usr/bin/apt-get",
  "argv": ["install", "-y", "fail2ban"],
  "stdin": "…可选，敏感内容走这里（不落库）…",
  "timeout_sec": 600,                     # 命令自身的运行上限
  "wait_sec": 0,                          # 本次请求愿意等多久，0 = 纯异步
  "title": "安装 fail2ban" }              # 可选，任务中心列表展示
```

响应：

```json
{ "job_id": "job-1763...", "status": "queued", "active": true,
  "phase": "queued", "phase_label": "排队", "progress": 0,
  "cancellable": true, "cancel_requested": false,
  "logs": { "entries": [], "min_seq": null, "max_seq": 0 } }
```

`wait_sec > 0` 时内核在前台最多等这么久：等到了终态就直接带结果返回，没等到就带着 `running` 返回 `job_id`——**两种情况的响应形状一致**，插件按 `active` 判断要不要继续轮询。

终态快照额外带 `exit_code` / `error_class` / `error_detail` / `result`，并把 `result.success`、`result.code` 提到顶层（`success` / `code`），让「只关心成没成」的编排不必钻进 `result`。

**非零退出不是错误**：作业结算为 `failed` + `error_class=exit`，但 `result` 里保留 `stdout`/`stderr`（各截断 64KiB）——装完配、配完重启的编排要靠它判断下一步。只有被拒（4xx）、同插件已有命令在跑（409）与通道失败才在提交阶段返回错误。

### 2.2 查询与取消

```
POST /api/v1/capability/exec/status       # CAP_EXEC_STATUS
{ "job_id": "job-…", "after": 0, "limit": 200 }   # after = 已读到的最大 seq；limit=0 不要日志

POST /api/v1/capability/exec/cancel       # CAP_EXEC_CANCEL
{ "job_id": "job-…" }
```

`status` 返回作业状态 + 结果 + 增量日志（`seq` 单调、轮转后不重编号，与任务中心 SSE 同一份数据）。**只允许查询/取消本插件提交的作业**：归属写在提交时的 `params.plugin`，由内核写入、不接受调用方传参——否则一个插件能读到别人命令的输出（里面可能有密码）。

### 2.3 manifest

```toml
requires = ["exec", "identity", "fs"]

[[permissions]]
capability = "exec"
verbs = ["run"]

resource_hints = [
  "bin:/usr/bin/apt-get",
  "bin:/usr/bin/fail2ban-client",
  "bin:/usr/bin/systemctl",
  "path:/etc/fail2ban/jail.d",
]
```

`bin:` 与 `path:` 走同一套安装期确认（逐条勾选、落库为 `object` grant）；`exec` 能力在受限形态（1Panel Docker 包）不可授予，与 `firewall` / `ftp` 一致。

### 2.4 与环境安装是同一套作业

运行环境安装（`runtime.install`）、Web 服务安装（`web.install`）、PHP 扩展安装（`runtime.ext.*`）、镜像拉取、站点创建，本来就在 `jobs` / `job_logs` 上：同一张表、同一套状态机、同一个 SSE 详情页、同一套取消。

命令执行接进来之后，任务中心里只有两种形态的差别，没有第二套机制：

| | 安装类作业 | 命令执行作业 |
| --- | --- | --- |
| `kind` | `runtime.install` / `web.install` / … | `host.exec` |
| 进度 | 多步骤快照（`steps_json`），按真实阶段推进 | 无天然进度，`progress = -1`（不确定态），过程看日志 |
| 日志 | 内核按阶段写 note + 命令输出尾巴 | stdout/stderr 逐行写入（`out` / `err`） |
| 取消 | 阶段边界检查 `cancel_requested` | 250ms tick 检查，命中即 `systemctl stop` 单元 |
| 重试 | 有执行器，可重试 | **不提供**：重放一条特权命令必须重新走插件授权（插件可能已卸载、`bin:` 可能已收回） |

安装类作业的实现手段是 Docker API 与文件操作，不需要 `bin:` 授权——但**凡是落到宿主进程的动作（装包、重启服务、生成证书、探测地址）都走 `skiff_host` 同一条通道**，日志写进同一个作业。所以「同一个逻辑」有两层：记录与生命周期统一，执行通道也统一；不统一的是授权——插件要授权，内核自己不要。

`app` 侧的唯一入口是 `AppState::run_host_in_job`（作业内：过程行写 `job_logs`、取消接 `cancel_requested`、错误翻中文）与 `host::run_host_msg`（作业外的只读探测）。**没有例外**：`sites.rs` 的证书解析与配对校验也已改走通道（`direct` 模式、10s 上限），为此 `site_tls_info` 变成 async。`crates/app` / `crates/core` 的非测试代码里 `Command::new` 为 0，由 `xtask check-channel` 守着。

---

## 三、判定：三道闸，缺一不可

判定唯一实现在 `skiff_plugin::execgrant`；纯词汇（前缀、风险档、包装器名单）在 `skiff_model::exec`，打包校验与运行期共用一份，不会出现「打包放行、运行拒绝」。三道闸全部在**提交那一刻**跑完，失败直接 4xx，不会留下一个「建好了但跑不动」的作业。

| 闸 | 内容 | 拒绝理由 |
| --- | --- | --- |
| ① 授权 | `exec:run` 权限串（精确命中，不留 `exec:*` 通配）+ canonical 路径**精确等于**某条 `bin:` 授权 | `NotGranted` |
| ② 本体 | 普通文件、有执行位、**组/其他人不可写**、属主为 root、不在内核数据根内 | `NotTrusted` / `NotExecutable` / `Protected` |
| ③ 语义 | 不是包装器/解释器（shell、`python`、`find`、`awk`/`sed`、编辑器与分页器、`systemd-run`、`crontab`、`su`/`sudo`…） | `Wrapper` |

- **②是 `bin:` 模型独有的交叉提权面**：插件先用自己的 `path:` 授权在可写目录放一个假 `systemctl`，再请求以 root 执行它。`fs` 单独用没有这条路径，所以文件本体必须查。
- **③必须是黑名单而不是白名单**：白名单是封闭集合，加一个二进制就要发内核版——那正是要消灭的成本。黑名单只需要覆盖「白名单永远补不完的包装器类」。
- **目录级授权不支持**：`bin:/usr/bin` 等于授权整台机器，`canonical_bins` 只收 `is_file()` 的授权值。
- **argv 不参与判定**：授权一个二进制就是授权它作为 root 能做的一切。这是本模型的固有粒度（§1.2），内核不假装能校验参数形状。

### 3.1 防护栏（不是授权，是防炸）

| 项 | 上限 | 越界行为 |
| --- | --- | --- |
| argv 条数 / 单条 / 总长 | 64 / 4 KiB / 16 KiB | 拒绝（截断后执行一个和请求不一样的命令更糟） |
| `stdin` | 64 KiB | 拒绝 |
| `timeout_sec` | 默认 600，上限 3600 | 夹到上限（写 99999 秒说明想一直等，那正是要拦的意图，但不值得 400 重试） |
| `wait_sec` | 上限 120 | 夹到上限 |
| 状态响应里的单路输出 | 64 KiB | 按字符边界截断 + `truncated` 标记 |
| 单作业过程日志行数 | 2000 行 | 停止写入并留一行说明（作业表另有 4000 行 / 2 MiB 轮转上限） |
| 同一插件的并发命令 | 1 条 | 409，消息带在跑的那条 `job_id` 与阶段 |

**同一插件同时只跑一条**：用插件 id 作作业目标，与「同目标唯一活跃作业」的部分索引一致。插件需要并发时自己排队——它比内核更清楚哪两条命令能并行。

### 3.2 风险标注

`bin_risk()` 三档：`wrapper`（拒绝）/ `root-equiv`（授出即等价 root：`apt-get`、`systemctl`、`cp`、`chmod`、`tee`…）/ `scoped`（`fail2ban-client`、`pure-pw`、`nft`）。**只用于标注，不用于拦截**——`root-equiv` 名单注定不完整（`cp` + `chmod` 就能种一个 setuid root 二进制），它服务的是审计与确认页，不是判定。

---

## 四、执行期语义

- **通道无例外**：宿主机进程只能经 `skiff_host`——`crates/app` / `crates/core` 的非测试代码出现 `Command::new` 会让 `cargo run -p xtask -- check-channel` 失败（fork 点可能逃逸：漏了超时、漏了单元清理、漏了审计）。插件侧的 `exec` 面与内核自身的作业执行体走的是同一份实现，差别只在入口要不要授权。
- **执行通道**：`skiff_host::run_stream`（`crates/host`）→ `systemd-run --wait --pipe --collect` 瞬态单元，不经 shell，不继承主服务沙箱。超时与取消都走 `systemctl stop` 单元（只 kill 本地客户端不够，真实副作用还在单元里跑）。`direct` 模式直接 kill 子进程；它只用于不需要 root 的短命令，所以不存在「杀不干净」的后果。
- **日志**：stdout → `out`、stderr → `err`，逐行追加。日志是**过程**的唯一事实源；`result` 只放头部累积与退出码。
- **stdin 不落库**：提交时放进进程内 map，执行体启动时取走即删。作业参数里只有 `binary` / `argv` / `timeout_sec` / `risk`——密码、私钥不该因为「有作业记录」就长期留在磁盘上。代价是进程重启时在途作业拿不到 stdin，但重启本来就会把在途作业结算为「中断」。
- **失败分类**映射到任务中心的 `error_class`：`exit`（跑了但非零退出）、`timeout`（到点已停止单元）、`channel`（起不来/管道失败）、`cancelled`（独立状态，不算失败）。四类对用户要采取的动作完全不同。

---

## 五、边界

- **只适用 systemd 直装形态**：能力面在受限形态不可授予；特权模式依赖 `systemd-run` 瞬态单元（`skiff_host` 的前提）。
- **不做**：shell 解释、常驻进程、交互式命令、PTY。编排（顺序、条件、重试）由插件用多次提交组合，`exec:run` 一次只跑一条。
- **firewall/ftp 冻结**：`CAP_FIREWALL_CALL` / `CAP_FTP_CALL` 保留不动、不再扩展，标注 legacy。哪个域下次真要改，再把内核侧的 argv 编排搬进插件包（纯插件侧变更，届时按需付账）。
- **不做旧版本兼容**：当前为测试开发阶段（AGENTS.md 4.2），`exec` 是新能力面，无迁移包袱。

---

## 六、实施状态

### 6.1 已落地（2026-11-16）

| 位置 | 内容 |
| --- | --- |
| `crates/wire` | `CAP_EXEC_RUN` / `CAP_EXEC_STATUS` / `CAP_EXEC_CANCEL` + 稳定性测试 |
| `crates/model/src/exec.rs` | `bin:` 前缀、包装器/root-equiv 名单、`bin_risk`（6 个单测） |
| `crates/plugin/src/execgrant.rs` | 三道闸判定 + `ExecError`（8 个单测） |
| `crates/core/src/kernel/exec.rs` | 提交/查询/取消三个 handler + 审计（7 个单测）；执行体不在这里 |
| `crates/host`（`skiff-host`） | **执行通道唯一实现**：`run` / `run_stream`，特权（`systemd-run`）与普通（直起子进程）两种模式，共用读流 / 250ms 取消检查 / 超时后清理（6 个单测） |
| `crates/app/src/host.rs` | 内核侧唯一执行体 `drive_exec_job`（过程行进 `job_logs`、取消接作业、错误翻中文，2000 行上限）——**插件 `exec` 路由与内核计划任务共用**；`KernelExec::run` / `AppState::exec_kernel`（提交+审计+执行+读回终态）、`AppState::run_host_in_job`、`run_host` / `run_host_msg`（作业外探测） |
| 内核计划任务 | `site-backup` 改为提交 `host.exec` 作业跑 `tar`（原为进程内 `tar` crate）：任务运行记录与作业成对出现，任务 `timeout_secs` 即通道超时 |
| 已收口的调用点 | `docker_daemon`（`systemctl restart docker`，特权）、`sites/network`（`ip` 只读探测，8s 上限）、`sites/crud`（`openssl` 自签生成，30s 上限）、`kernel/{ftp,firewall,exec}` |
| `crates/app/src/jobs.rs` | `KIND_HOST_EXEC`；`finish_err_result`（失败仍保留 stdout/stderr） |
| `crates/pkgfmt/src/manifest.rs` | `Capability::Exec`、`bin:` 资源校验（拒绝包装器/目录）、hints→能力映射 |
| `crates/sdk/src/lib.rs` | `host_exec`（提交+等到结束）/ `host_exec_submit` / `host_exec_status` / `host_exec_cancel` / `bin_grants` |
| `crates/web/src/handlers/jobs.rs` | 任务中心识别 `host.exec`：类型/目标文案（插件提交与计划任务分开显示），隐藏重试 |
| `xtask check` | 通道闸：`app`/`core` 非测试代码出现 `Command::new` 即红；内核文件写计数只减不增（基线 140 处 / 9 个文件） |

### 6.2 未落地（按优先级）

1. **升级强制复核**（安全硬前提）：`install.rs` 现行规则是「相对旧 manifest 新增的对象授权自动补授 + 写审计」，`web/handlers/plugins.rs` 升级不弹确认页。这意味着插件被信任一次之后，作者发新版本即可静默扩大 `bin:`（同理 `path:`）授权面。**这是既有缺陷，不是 exec 引入的**；修一次 `path:` 与 `bin:` 同受益。口径建议：新增对象授权一律重新确认。
2. **确认页风险标注**：`bin:` 行按 `bin_risk` 标 `root-equiv` 并默认不勾；现有确认页把 `resource_hints` 原样列出即可用，但没有风险提示。
3. **试点**：`net.apply` 至今是 501 空壳，用它跑通「manifest → 确认页 → exec → 运行记录 → 审计」全链路。**不要选「改 SSH 端口」**（重启失败即锁死机器），选幂等可恢复的动作（时区、swap、装并 enable 一个无害服务）。
4. **上机验收**：`systemd-run` 那条真实执行路径只在 Linux 跑得到，Windows 开发机上只验证了类型、判定与作业生命周期。

### 6.3 待决策

- `bin:` 是否需要「二进制 + 子命令前缀」的细粒度（如 `bin:/usr/bin/systemctl restart`）。M1 不做：那是一层新的表达式，等真出现误授权再补，语法上仍是单个字符串。
- 同一瞬间多个插件跑 apt 会撞 dpkg lock：现在只保证「同一插件不同时跑两条」，跨插件仍需全局串行锁。当前不做，交给插件重试与报错。

---

## 七、词汇表终态

| 留内核（判定型） | 理由 |
| --- | --- |
| `fs`（路径代数）、`docker`、`identity`、`net` 出网收口 | 判定本身就是安全边界 |
| `exec`（`bin:` 判定 + 包装器黑名单）、`skiff_host`（执行通道）、作业记录、TOFU、审计 | 机制层，一份实现 |

| 下放插件（执行型） | 形态 |
| --- | --- |
| firewall / ftp | 冻结为 legacy，不再扩展 |
| 未来的 backup、SSL、监控采集、net | `exec` + `bin:` 授权，**零内核改动** |
