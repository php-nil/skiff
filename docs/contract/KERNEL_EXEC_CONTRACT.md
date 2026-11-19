# 内核命令执行原语契约（Exec Contract）

> 状态：**内核底层与轻量通道 `run_light` 已落地，上机验收未做（2026-11-19）**。
> 本文件是 `exec` 能力面的契约：授权、协议、判定顺序、审计、运行记录、错误码与边界。设计取舍见
> [`docs/plugin/HOST_EXEC_DESIGN.md`](../plugin/HOST_EXEC_DESIGN.md)。
> 判定实现 `crates/plugin/src/execgrant.rs` + 词汇 `crates/model/src/exec.rs`；
> 内核 handler `crates/core/src/kernel/exec.rs`；**执行通道 `crates/host`（`skiff_host`）与
> 作业执行体 `skiff_app::host::drive_exec_job` 只有一份——插件、内核安装动作、内核计划任务共用**；
> 运行记录 `crates/app/src/jobs.rs`。

## 0. 一页结论

- **两条通道、一个权限串**：作业化 `exec/run`（提交→轮询→终态）+ 轻量 `exec/run_light`
  （一次 HTTP 返回），权限串都是 `exec:run`，不做细分。查询 `exec/status`、取消 `exec/cancel`
  只对作业化通道有效。
- **授权粒度是二进制**：真正的门是安装时逐条确认的 `bin:<绝对路径>` 对象授权；一条 `bin:`
  对应一个可执行文件，**不支持目录与通配**。
- **argv 不参与判定**：插件给什么内核执行什么，不经 shell。**授权 `apt-get` = 授权 root**
  （能以 root 身份写任意文件、起任意服务），这是明示的模型粒度，不是缺陷——真实边界是
  「谁写的包 + 装之前看了确认页 + 每次执行留审计与运行记录」。
- **三道闸是判定性质的分组，不是执行顺序**：① 授权命中（`exec:run` + `bin:`）、
  ② 本体可信（root 属主 / 组/其他人不可写 / 有执行位 / 不在数据根内 / 非包装器）、
  ③ 非包装器（黑名单，见 §3）。实际执行顺序见 §3。
- **作业化通道的语义**：`run` 提交一条 `host.exec` 作业并立即返回 `job_id`；过程输出、
  退出码、取消都在任务中心，与环境安装共用 `jobs` / `job_logs`。同插件同时只跑一条作业化命令。
- **轻量通道的语义**：同授权、并行跑、不进作业中心、单次返回。timeout 默认 10s / 上限 30s；
  每插件信号量限流（建议 8，超出 429）。适用于短命令 / 只读探测（如 `nft --version`、
  `systemctl is-active fail2ban`）。
- **打包期与运行期各拦一层**：pkgfmt 拦「形式+包装器」（装包前就拒），execgrant 拦
  「存在+可信+授权」（运行时才能判）。同一份包装器名单两边共用，不会出现
  「打包放行、运行拒绝」。
- **审计记请求、运行记录记过程**：审计保留 90 天（谁请求对哪个二进制做什么、拒因），
  作业保留 7 天 / 200 条（怎么跑的、输出了什么）。敏感内容走 `stdin`，只记字节数且**不落库**。
- **只在 Native 形态提供**：受限形态（1Panel Docker 包）返回 503，`pkgfmt::available_in` 不列。
- **内核自己也是客户**：计划任务里的命令动作（站点备份打包）与安装动作走**同一个执行体**，
  同样提交 `host.exec` 作业、同样写 `exec.run` 审计；差别只有两处——谁发起（`system` 而非
  `plugin:<id>`）与要不要授权（内核是授出方，不查 `bin:`）。见 §4.1。

## 1. 授权

| 项 | 口径 |
| --- | --- |
| manifest `requires` | 必须含 `exec`（否则 `[[permissions]]` 校验失败） |
| manifest `permissions` | `capability = "exec"`、`verbs = ["run"]`；精确命中，不接受 `exec:*` 通配 |
| manifest `resource_hints` | 每个要跑的可执行文件一条 `bin:<绝对路径>`；安装确认页逐条勾选后落库为 `object` grant |
| 判定顺序 | 先 `exec:run`（缺→403），再 `bin:` 命中（不中→403），两者缺一不可 |

`bin:` 与 `path:` 同一套确认与落库机制；升级时新增对象授权的处理见 §8（**未落地**）。

**授权自动失效**：`execgrant::canonical_bins()` 会自动过滤掉 canonicalize 失败（二进制被卸载/路径变了）或不是文件（目标变成了目录）的授权值。这意味着插件被卸载一个二进制后，下次请求自动被拒，不会因为库里存了旧字符串而继续放行——与 `fsgrant::canonical_roots` 同一取舍。

## 2. 协议

### 2.1 `POST /api/v1/capability/exec/run`（作业化通道）

```json
{
  "binary": "/usr/bin/apt-get",
  "argv": ["install", "-y", "fail2ban"],
  "stdin": "可选，敏感内容投递",
  "timeout_sec": 600,
  "wait_sec": 0,
  "title": "安装 fail2ban"
}
```

| 字段 | 必需 | 约束 |
| --- | --- | --- |
| `binary` | 是 | 绝对路径、无 `..` 段、无 NUL |
| `argv` | 否 | 字符串数组；条数 ≤ 64、单条 ≤ 4KiB、总长 ≤ 16KiB、无 NUL |
| `stdin` | 否 | ≤ 64KiB；只投递不落审计、不进作业参数（内存传递，执行体取走即删） |
| `timeout_sec` | 否 | 命令运行上限，默认 600；夹到 `[1, 3600]`，超出夹取而非报错 |
| `wait_sec` | 否 | 本次请求愿意等多久，默认 0（纯异步）；夹到 `[0, 120]`。**这是 UDS 连接级的等待**，超时返回 running 快照，由调用方后续查 `exec/status` |
| `title` | 否 | 任务中心展示名；去空白、最长 80 字符 |

响应（HTTP 200，作业已受理）：

```json
{ "job_id": "job-1763...", "status": "queued", "active": true,
  "phase": "queued", "phase_label": "排队", "progress": 0,
  "cancellable": true, "cancel_requested": false,
  "created_at": 1763000000, "started_at": null, "finished_at": null,
  "duration_ms": 0, "exit_code": null, "error_class": null, "error_detail": null,
  "logs": { "entries": [], "min_seq": null, "max_seq": 0 } }
```

`wait_sec > 0` 且作业已结束时，响应额外带 `result` 与顶层 `success` / `code`：

```json
{ "job_id": "job-…", "status": "failed", "active": false, "exit_code": 100,
  "success": false, "code": 100,
  "result": { "success": false, "code": 100, "stdout": "…", "stderr": "…",
              "truncated": { "stdout": false, "stderr": false } },
  "logs": { "entries": [ … ], "min_seq": 1, "max_seq": 12 } }
```

**两种响应形状一致**，`active=true` 时 `result` / `success` / `code` 为 null。客户端按 `active`
判断要不要继续轮询。`result.stdout` / `result.stderr` 各截断 64KiB（按字符边界），头部截断标
`truncated`。

### 2.2 `POST /api/v1/capability/exec/run_light`（轻量通道）

```json
{
  "binary": "/usr/sbin/nft",
  "argv": ["--version"],
  "timeout_sec": 10
}
```

| 字段 | 必需 | 约束 |
| --- | --- | --- |
| `binary` | 是 | 与 `exec/run` 同口径 |
| `argv` | 否 | 与 `exec/run` 同口径（条数 ≤ 64、单条 ≤ 4KiB、总长 ≤ 16KiB、无 NUL） |
| `timeout_sec` | 否 | 默认 10s、上限 **30s**；夹取而非报错 |

响应（HTTP 200，成功或失败）：

```json
{ "success": true, "code": 0,
  "stdout": "nft v1.0.9",
  "stderr": "",
  "truncated": { "stdout": false, "stderr": false } }
```

| 情形 | 状态码 | 说明 |
| --- | --- | --- |
| 正常返回（成功或非零退出） | 200 | `result.code` 即退出码 |
| 超时 | 504 | 结果未知，瞬态单元仍在跑（见 §8 待决策） |
| 起进程失败 / 通道错误 | 500 | `channel` 类错误 |
| 并发槽位满 | 429 | 每插件信号量（上限 8） |

**与 `exec/run` 完全同口径的前置判定**：`exec:run` → Native+Linux → `execgrant::resolve`
（三道闸全跑）→ timeout 夹到 `[1, 30]`。**不提交作业**、**不写 jobs**、**不抢活跃槽位**；
只同步写一条精简审计（`exec.run_light`），结果只在 HTTP 响应里。

**适用场景**：短命令（<5s）/ 只读探测（`nft --version`、`dpkg-query`、`systemctl is-active`）。
长任务、有副作用、需要取消/过程日志的命令**必须**走 `exec/run`。

### 2.3 `POST /api/v1/capability/exec/status`（仅作业化通道）

```json
{ "job_id": "job-…", "after": 0, "limit": 200, "wait_ms": 0 }
```

返回与 §2.1 同形的快照 + 增量日志：`after` 是已读到的最大 `seq`，`limit` 上限 500、`0` 表示
只要状态不要日志。`seq` 单调递增、日志轮转后不重编号（与任务中心 SSE 同一份数据源）。

`wait_ms`（可选，默认 0）是**服务端长轮询**：作业未结束时内核最多挂起这么久（`[0, 30000]`，
超出夹取），状态变化或到点后返回，响应形状不变。**它占用服务端信号量槽位**（进程内上限 64），
不是无限挂起——槽位不足时立即返回当前快照。SDK 的 `host_exec_status_wait` / `host_exec_full`
用长轮询替代固定 200ms 短轮询。

> **与 `wait_sec` 的区别**：`wait_sec` 是 `exec/run` 提交时**调用方在 UDS 连接上等的时间**；
> `wait_ms` 是 `exec/status` 查询时**服务端挂起等状态变化的时间**。两者互不相关、资源消耗模型
> 也不同（一个占连接，一个占服务端槽位）。

**归属校验**：`params.plugin` 由内核在提交时写入，不接受调用方传参；只允许查询/取消本插件
提交的作业（否则一个插件能读到别人命令的输出，里面可能有密码）。越权返回 403。

### 2.4 `POST /api/v1/capability/exec/cancel`（仅作业化通道）

```json
{ "job_id": "job-…" }
```

返回 `{ "outcome": "cancelled" | "requested" | "terminal" }`：

- `cancelled`：作业还在排队，直接结算为取消；
- `requested`：已在执行，执行体在下一个 250ms tick 停下并 `systemctl stop` 瞬态单元；
- `terminal`：已是终态，无需处理。

### 2.5 状态码汇总

| 情形 | 状态码 | 通道 |
| --- | --- | --- |
| 请求体非法（缺 `binary`/`job_id`、超限、类型错） | 400 | 两条 |
| 缺 `exec:run` / 未授权 `bin:` / 包装器 / 数据根内 / 非本插件作业 | 403 | 两条 |
| 非 Native 或非 Linux | 503 | 两条 |
| 二进制不存在、非普通文件、本体不可信 | 409 | 两条 |
| 同插件已有作业化命令在跑（唯一活跃作业） | 409，消息带在跑的 `job_id` 与阶段 | 作业化 |
| 作业不存在 | 404 | 作业化 |
| 轻量通道并发槽位满 | 429 | 轻量 |
| 轻量通道超时（结果未知） | 504 | 轻量 |
| 通道失败（起不来 / 管道失败） | 500 | 轻量 |

## 3. 判定（唯一实现 `skiff_plugin::execgrant::resolve`）

**三道闸是按判定性质的分组，不是执行顺序**。实际执行顺序如下——全部在提交那一刻跑完，失败
直接返回 §2.5 的 4xx/409，不会留下「建好了但跑不动」的作业。

| # | 步骤 | 拒绝理由 | HTTP | 审计 verdict | 对应性质分组 |
| --- | --- | --- | --- | --- | --- |
| 1 | 形式：非空、无 NUL、无 `..` 段、绝对路径 | `Illegal` | 400 | deny | 本体可信 |
| 2 | 存在性：`canonicalize` 成功（软链按真实路径说话） | `Missing` | 409 | fail | 本体可信 |
| 3 | 本体类型：`is_file()` | `NotExecutable` | 409 | fail | 本体可信 |
| 4 | 包装器：`skiff_model::exec::is_wrapper_bin` 为假 | `Wrapper` | 403 | deny | 语义（黑名单） |
| 5 | 数据根：不在内核数据根及其 `plugins/`、`official/` 下 | `Protected` | 403 | deny | 本体可信 |
| 6 | 属主可信（unix 上）：root 属主 / `mode & 0o022 == 0` / `mode & 0o111 != 0` | `NotTrusted` | 409 | fail | 本体可信 |
| 7 | 授权：canonical 路径精确等于某条 `bin:` 授权的 canonical 路径 | `NotGranted` | 403 | deny | 授权 |

**verdict 分类**：
- **deny**（安全判定拒绝）：`NotGranted` / `Wrapper` / `Protected` / `Illegal` → 调用方应
  找管理员改授权或确认二进制安全；
- **fail**（环境/形式问题）：`Missing` / `NotExecutable` / `NotTrusted` → 二进制在这台机器上
  不可用，不是权限问题。

**包装器名单**（shell / 解释器 / `find`·`awk`·`sed` / 编辑器与分页器 / `systemd-run`·`crontab`·`su`
等）以 `skiff_model::exec::WRAPPERS` 为唯一来源，`pkgfmt` 打包期用同一份名单拒收——两边共用
一份，避免「打包放行、运行拒绝」。

### 3.1 打包期 vs 运行期校验分工

| 检查项 | 打包期（pkgfmt） | 运行期（execgrant） | 理由 |
| --- | --- | --- | --- |
| 形式（绝对路径、无 `..`、无 `/` 结尾） | ✓ | ✓ | 低成本，提前拦 |
| 包装器黑名单 | ✓ | ✓ | 同一份名单，打包期就拒掉避免无效安装 |
| NUL 字符 | ✗ | ✓ | 打包期 JSON 解析天然不会有，但运行期要再验一次 |
| 存在性（canonicalize 成功） | ✗ | ✓ | 打包时不知道目标机器上有没有这个二进制 |
| 本体类型（is_file） | ✗ | ✓ | 打包时无法判断目标路径是文件还是目录 |
| 属主 root | ✗ | ✓ | 打包时不知道目标机器上的属主 |
| 组/其他人不可写 | ✗ | ✓ | 打包时不知道目标机器上的权限位 |
| 有执行位 | ✗ | ✓ | 打包时不知道目标机器上的权限位 |
| 不在内核数据根内 | ✗ | ✓ | 数据根是部署时确定的，打包期不知道 |
| 授权命中 | ✗ | ✓ | 授权是安装后才落库的，打包期还没有 |

## 4. 运行记录（`host.exec` 作业）

作业由 `skiff_app::jobs::JobService` 承载，与环境安装共用同一张表、同一套状态机、同一个
SSE 详情页与取消语义。

| 作业字段 | 值 |
| --- | --- |
| `kind` | `host.exec`（`skiff_app::jobs::KIND_HOST_EXEC`） |
| `target_type` / `target_id` | `host` / 发起插件 id（同一插件同时只跑一条：唯一活跃作业索引） |
| `title` | 请求的 `title`，缺省为 `执行 <binary> <argv[0]>` |
| `params` | `{ plugin, binary, argv, timeout_sec, risk }`——**不含 stdin** |
| `created_by` / `created_by_role` | `plugin:<id>` / `plugin` |
| `progress` | 执行期为 `-1`（不确定态，界面显示为脉冲进度条），成功收尾置 100 |
| `exit_code` | 命令退出码；被信号杀掉时为 `null` |
| `result` | `{ success, code, stdout, stderr, truncated }`（成功与失败都写） |
| `error_class` | `exit` / `timeout` / `channel` / `interrupted`（重启收尾）；取消走 `cancelled` 状态 |
| 过程日志 | stdout → `out`，stderr → `err`；单作业 ≤ 2000 行，超出停止写入并留一行说明 |

**不提供重试**：`retry_job` 对 `host.exec` 以「无执行器」拒绝，任务中心也不显示重试按钮。
重放一条特权命令必须重新走插件授权（插件可能已卸载、`bin:` 可能已收回），不能由管理员单方重放。

### 4.1 内核自有作业（计划任务 / 安装动作）

同一张 `jobs` 表，发起者换成内核，字段口径：

| 作业字段 | 插件提交 | 内核计划任务 |
| --- | --- | --- |
| `target_type` / `target_id` | `host` / 插件 id（同插件串行） | `host` / 任务 id（同任务串行，不同站点备份互不阻塞） |
| `created_by` / `created_by_role` | `plugin:<id>` / `plugin` | `system` / `system` |
| `params` | `{ plugin, binary, argv, timeout_sec, risk }` | `{ origin: "sched", binary, argv, timeout_sec, risk }` |
| 审计 | `actor=plugin:<id>`、`source=uds` | `actor=system`、`source=sched`、detail 带 `origin=` 与 `target=` |
| 归属查询 | 只能查自己的作业 | 任务中心（管理员），插件侧查不到（无 `params.plugin`） |

**不查 `bin:` 授权**：内核是授权面本身，`system` 主体按 §5 的「内核自身也是主体」处理——
不是例外，是主体不同。**但逐次审计与作业记录不能少**：这是「用记录代替复杂权限配置」的落点，
计划任务也不能因为「是自己的」就绕过通道。

**超时是同一个数**：任务规格里的 `timeout_secs` 直接作为作业与通道的超时，超时即
`systemctl stop` 瞬态单元——比过去「放弃等待、进程继续跑」的语义更强。

## 5. 审计

提交阶段写一条，执行结果由运行记录承担（不重复写两条审计）。

### 5.1 作业化通道（`exec.run`）

| 字段 | 值 |
| --- | --- |
| actor | `plugin:<id>` |
| source | `uds` |
| action | `exec.run` |
| target | 空（定位信息在 detail 里） |
| verdict | `ok`（作业已受理）/ `deny`（授权、判定、Busy 拒绝）/ `fail`（二进制不可用、属主不可信等环境问题） |
| detail | `bin=<canonical> risk=<wrapper / root-equiv / scoped> job=<job_id> argv=<全文，截断 512B，超出加 …(截断) 标记> [stdin=<n>B] timeout=<n>s` |

### 5.2 轻量通道（`exec.run_light`）

| 字段 | 值 |
| --- | --- |
| actor / source | 同作业化 |
| action | `exec.run_light` |
| verdict | 只有 `ok`（写成功即放行）；判定失败返回 4xx，同作业化 |
| detail | `bin=<canonical> risk=<…> argv=<截断>`——**不写 job_id**（没作业）、**不写 exit_code**（属运行日志范畴） |

### 5.3 内核自有作业

同结构，只有主体与来源不同：`actor=system`、`source=sched`，
detail 前缀为 `bin=… risk=… job=… origin=sched target=<任务 id>`。

**不进审计的内容**：stdout / stderr / stdin 内容（stdin 只记字节数）。`risk=root-equiv`
表示该二进制授出即等价 root（`apt-get`、`systemctl`、`cp`、`chmod`…），供事后复查时快速
定性；它只标注，不拦截。

**两条记录的分工**：审计回答「谁在什么时候请求对哪个二进制做什么」（保留 90 天，安全追溯），
运行记录回答「这一次实际怎么跑的、输出了什么、退出码多少」（保留 7 天 / 200 条，过程复查）。
两者的检索入口、保留期与可见范围都不同，不能互相替代。轻量通道的审计与作业化通道
**按 action 区分**（`exec.run_light` vs `exec.run`），90 天追溯里可分别 filter。

## 6. stdin 槽（进程内，不落库）

stdin 用于投递密码 / 私钥 / `.pgpass` 等敏感内容——**进内存、不进审计、不进作业参数**。

| 阶段 | 行为 |
| --- | --- |
| 提交时 | 放入进程内 `HashMap<job_id, stdin>`，**不进 `jobs.params_json`** |
| 执行体启动时 | `take_stdin(job_id)` 取走即删，传给 `HostSpec` |
| 作业提交后、执行体启动前被取消 | stdin 在 HashMap 里等执行体取——如果 tokio::spawn 还没开始，执行体永远不会启动，**stdin 会留在 HashMap 里直到进程重启** |
| 进程重启 | 进程内内存清零，在途作业的 stdin 丢失；但重启本来就会把在途作业结算为 `interrupted` |

**已知边界**：stdin 槽在进程内，无持久化；进程重启后在途作业拿不到 stdin——这是可接受的，
因为重启本来就是极端场景。未来如果需要更严格的 stdin 清理，可考虑给每个 stdin 设 TTL
（作业取消/结算后自动过期），但目前以「take_stdin 即删 + 进程重启自然清」为够用基线。

## 7. 边界

- **不做**：shell 解释、常驻进程、交互式命令、PTY；参数形状校验（那是可靠性，由插件自己负责）。
- **不做目录级授权**：`bin:/usr/bin` 等于授权整台机器。
- **不做 argv 白名单**：那会重新变成「每加一个命令都要内核发版」的封闭集合。
- **不做跨插件并发**：同一插件串行（唯一活跃作业，作业化通道）；跨插件仍可能撞 dpkg lock，
  由插件重试兜底。轻量通道走每插件信号量。
- **legacy 面冻结**：`firewall` / `ftp` 两条专用路由保留但不再扩展，新功能一律走 `exec`。
- **不做旧版本兼容**：当前为测试开发阶段（AGENTS.md 4.2），`exec` 是新能力面，无迁移包袱。
- **只适用 systemd 直装形态**：能力面在受限形态不可授予；特权模式依赖 `systemd-run` 瞬态单元
  （`skiff_host` 的前提）。

## 8. 未落地与待决策

### 8.1 未落地（跟本契约直接相关的硬前提）

1. **升级强制复核**（安全硬前提）：`install.rs` 现行规则是「相对旧 manifest 新增的对象授权
   自动补授 + 写审计」，升级不弹确认页。这意味着插件被信任一次后，作者发新版即可静默扩大
   `bin:`（同理 `path:`）授权面。**这是既有缺陷**，修一次两条授权类型同受益；口径建议：
   新增对象授权一律重新确认。在修复前，本契约的「安装期确认即人类闸门」只对首装成立。
2. **确认页风险标注**：`bin:` 行按 `bin_risk` 标 `root-equiv` 并默认不勾；现有确认页把
   `resource_hints` 原样列出即可用，但没有风险提示。
3. **试点**：选 `net.apply` 或其他幂等可恢复动作跑通全链路（不要选「改 SSH 端口」——
   重启失败即锁死机器）。
4. **上机验收**：`systemd-run` 那条真实执行路径只在 Linux 跑得到，Windows 开发机上只验证了
   类型、判定与作业生命周期；轻量通道的 429/504 行为同样待上机确认。

### 8.2 待决策

| 决策 | 当前口径 | 备选 | 理由 |
| --- | --- | --- | --- |
| 轻量通道超时后的瞬态单元 | 超时即 504（结果未知），**未带 `RuntimeMaxSec`** | 补 `RuntimeMaxSec=timeout` 到 systemd-run | 当前内核侧被 drop 只杀本地客户端，UDS 断开后单元可能继续跑；需上机验证是否补该属性 |
| 轻量通道并发上限 | 每插件 8（已实现，常量 `LIGHT_CONCURRENCY`） | 上机实测后调整 | 跨插件不设全局上限 |
| 轻量通道审计保留期 | 同 `exec.run`（90 天） | 单独保留期或采样 | 探测按 90 天保留堆积，取决于实测调用量 |
| `bin:` 细粒度 | 不做子命令前缀 | 未来可能需要 | M1 不做：那是一层新的表达式，等真出现误授权再补 |
| 跨插件全局串行锁 | 不做，交给插件重试与报错 | 未来可能加 | 同一瞬间多个插件跑 apt 会撞 dpkg lock |

## 9. 测试

| 层面 | 用例位置 |
| --- | --- |
| 词汇（前缀、包装器、风险档） | `crates/model/src/exec.rs`（6 例） |
| 判定（形式/本体/数据根/授权/包装器） | `crates/plugin/src/execgrant.rs`（9 例） |
| handler（解析、上限、时间夹取、审计 detail、stdin 槽） | `crates/core/src/kernel/exec.rs`（7 例） |
| 通道（argv 拼接、`--` 分隔、透传、两种模式默认值一致、超时文案） | `crates/host/src/lib.rs`（6 例） |
| 内核侧执行封装（作业日志、错误翻译、不替调用方结算） | `crates/app/src/host.rs`（3 例） |
| 内核命令作业（提交 / 审计 / 结算 / 归属） | `crates/app/src/host.rs` 的 `kernel_exec_settles_the_job_and_audits` |
| 计划任务的命令动作走 exec（作业成对出现、平台分支断言） | `crates/app/src/tasks.rs` 的 `site_backup_runs_through_an_exec_job` |
| 作业生命周期（提交/阶段/日志/取消/重启收尾） | `crates/app/src/jobs.rs`（7 例） |
| 打包期收口（`bin:` 形式与包装器） | `crates/pkgfmt/src/manifest.rs` 的 `validate_resource_shapes` |
| 路由稳定性 | `crates/wire/src/lib.rs` 的 `route_table_is_stable` |
| 轻量通道（timeout 夹取、审计 detail、每插件信号量） | `crates/core/src/kernel/exec.rs`（3 例：`light_timeout_*` / `light_audit_detail_*` / `light_permits_*`） |
| SDK 轻量调用（路由、请求体、非零退出不是 Err） | `crates/sdk/src/lib.rs` 的 `host_exec_light_posts_to_light_route` |
