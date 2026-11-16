# 内核命令执行原语契约（Exec Contract）

> 状态：**内核底层已落地，未经上机验收（2026-11-16）**。
> 本文件是 `exec` 能力面的契约：授权、协议、判定顺序、审计、运行记录、错误码与边界。设计取舍见
> [`docs/plugin/HOST_EXEC_DESIGN.md`](../plugin/HOST_EXEC_DESIGN.md)。
> 判定实现 `crates/plugin/src/execgrant.rs` + 词汇 `crates/model/src/exec.rs`；
> 内核 handler `crates/core/src/kernel/exec.rs`；**执行通道 `crates/host`（`skiff_host`）与
> 作业执行体 `skiff_app::host::drive_exec_job` 只有一份——插件、内核安装动作、内核计划任务共用**；
> 运行记录 `crates/app/src/jobs.rs`。

## 0. 一页结论

- **一个动词、三条路由**：`exec/run`（提交）、`exec/status`（状态+日志）、`exec/cancel`（取消），
  权限串固定 `exec:run`。
- **授权粒度是二进制**：真正的门是安装时逐条确认的 `bin:<绝对路径>` 对象授权；一条 `bin:`
  对应一个可执行文件，**不支持目录与通配**。
- **argv 不参与判定**：插件给什么内核执行什么，不经 shell。授权 `apt-get` 即等价授权 root，
  这是明示的模型粒度，不是缺陷。
- **执行是作业**：`run` 提交一条 `host.exec` 作业并立即返回 `job_id`；过程输出、退出码、取消
  都在任务中心，与环境安装共用 `jobs` / `job_logs`。
- **三道闸**：授权命中 → 文件本体可信（root 属主 / 不可被非 root 写 / 有执行位 / 不在数据根内）
  → 非包装器。任一不过即拒、记 `deny` 审计，**不建作业**。
- **审计记请求、运行记录记过程**：审计保留 90 天（谁请求对哪个二进制做什么），作业保留
  7 天 / 200 条（怎么跑的、输出了什么）。敏感内容走 `stdin`，只记字节数且**不落库**。
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

`bin:` 与 `path:` 同一套确认与落库机制；升级时新增对象授权的处理见 §7（**未落地**）。

## 2. 协议

### 2.1 `POST /api/v1/capability/exec/run`

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
| `wait_sec` | 否 | 本次请求愿意等多久，默认 0（纯异步）；夹到 `[0, 120]` |
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

**两种响应形状一致**，客户端按 `active` 判断要不要继续轮询。`result.stdout` / `result.stderr`
各截断 64KiB（按字符边界），头部截断标 `truncated`。

### 2.2 `POST /api/v1/capability/exec/status`

```json
{ "job_id": "job-…", "after": 0, "limit": 200 }
```

返回与 §2.1 同形的快照 + 增量日志：`after` 是已读到的最大 `seq`，`limit` 上限 500、`0` 表示
只要状态不要日志。`seq` 单调递增、日志轮转后不重编号（与任务中心 SSE 同一份数据源）。

**归属校验**：`params.plugin` 由内核在提交时写入，不接受调用方传参；只允许查询/取消本插件
提交的作业（否则一个插件能读到别人命令的输出，里面可能有密码）。越权返回 403。

### 2.3 `POST /api/v1/capability/exec/cancel`

```json
{ "job_id": "job-…" }
```

返回 `{ "outcome": "cancelled" | "requested" | "terminal" }`：

- `cancelled`：作业还在排队，直接结算为取消；
- `requested`：已在执行，执行体在下一个 250ms tick 停下并 `systemctl stop` 瞬态单元；
- `terminal`：已是终态，无需处理。

### 2.4 状态码

| 情形 | 状态码 |
| --- | --- |
| 请求体非法（缺 `binary`/`job_id`、超限、类型错） | 400 |
| 缺 `exec:run` / 未授权 `bin:` / 包装器 / 数据根内 / 非本插件作业 | 403 |
| 非 Native 或非 Linux | 503 |
| 二进制不存在、非普通文件、本体不可信 | 409 |
| 同插件已有命令在跑（唯一活跃作业） | 409，消息带在跑的 `job_id` 与阶段 |
| 作业不存在 | 404 |

## 3. 判定（唯一实现 `skiff_plugin::execgrant::resolve`）

三道闸在**提交那一刻**全部跑完：失败直接返回上面的 4xx，不会留下「建好了但跑不动」的作业。

1. **形式**：非空、无 NUL、无 `..` 段、绝对路径 → 否则 `Illegal`（400）
2. **存在性**：`canonicalize` 成功（软链按真实路径说话：`/bin/systemctl` 与 `/usr/bin/systemctl`
   在 usr-merge 发行版上等价）→ 否则 `Missing`（409）
3. **本体**：`is_file()`；unix 上另有属主 root、`mode & 0o022 == 0`、`mode & 0o111 != 0`
   → 否则 `NotExecutable` / `NotTrusted`（409）
4. **数据根**：不在内核数据根及其 `plugins/`、`official/` 下（`fsgrant::is_protected` 同口径）
   → 否则 `Protected`（403）
5. **包装器**：`skiff_model::exec::is_wrapper_bin` 为假 → 否则 `Wrapper`（403）
6. **授权**：canonical 路径精确等于某条 `bin:` 授权的 canonical 路径 → 否则 `NotGranted`（403）

包装器名单（shell / 解释器 / `find`·`awk`·`sed` / 编辑器与分页器 / `systemd-run`·`crontab`·`su` 等）
以 `skiff_model::exec::WRAPPERS` 为唯一来源，`pkgfmt` 打包期用同一份名单拒收，避免
「打包放行、运行拒绝」。

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

| 字段 | 值 |
| --- | --- |
| actor | `plugin:<id>` |
| source | `uds` |
| action | `exec.run` |
| target | 空（定位信息在 detail 里） |
| verdict | `ok`（作业已受理）/ `deny`（授权、判定、Busy 拒绝） |
| detail | `bin=<canonical> risk=<wrapper / root-equiv / scoped> job=<job_id> argv=<全文，512B 上限> [stdin=<n>B] timeout=<n>s` |

内核自有作业（§4.1）同结构，只有主体与来源不同：`actor=system`、`source=sched`，
detail 前缀为 `bin=… risk=… job=… origin=sched target=<任务 id>`。

stdout/stderr 不进审计。`risk=root-equiv` 表示该二进制授出即等价 root（`apt-get`、`systemctl`、
`cp`、`chmod`…），供事后复查时快速定性；它只标注，不拦截。

**两条记录的分工**：审计回答「谁在什么时候请求对哪个二进制做什么」（保留 90 天，安全追溯），
运行记录回答「这一次实际怎么跑的、输出了什么、退出码多少」（保留 7 天 / 200 条，过程复查）。
两者的检索入口、保留期与可见范围都不同，不能互相替代。

## 6. 边界

- **不做**：shell 解释、常驻进程、交互式命令、PTY；参数形状校验（那是可靠性，由插件自己负责）。
- **不做目录级授权**：`bin:/usr/bin` 等于授权整台机器。
- **不做 argv 白名单**：那会重新变成「每加一个命令都要内核发版」的封闭集合。
- **不做跨插件并发**：同一插件串行（唯一活跃作业），跨插件仍可能撞 dpkg lock，由插件重试兜底。
- **legacy 面冻结**：`firewall` / `ftp` 两条专用路由保留但不再扩展，新功能一律走 `exec`。

## 7. 未落地（跟本契约直接相关的硬前提）

**升级时的对象授权复核**：现行规则是「相对旧 manifest 新增的对象授权自动补授 + 写审计」，
升级不弹确认页（`crates/plugin/src/install.rs`、`crates/web/src/handlers/plugins.rs`）。这意味着
插件被信任一次后，作者发新版即可静默扩大 `bin:`（同理 `path:`）授权面。**这是既有缺陷**，
修一次两条授权类型同受益；口径建议：新增对象授权一律重新确认。在修复前，本契约的
「安装期确认即人类闸门」只对首装成立。

## 8. 测试

| 层面 | 用例位置 |
| --- | --- |
| 词汇（前缀、包装器、风险档） | `crates/model/src/exec.rs`（6 例） |
| 判定（形式/本体/数据根/授权/包装器） | `crates/plugin/src/execgrant.rs`（8 例） |
| handler（解析、上限、时间夹取、审计 detail、stdin 槽） | `crates/core/src/kernel/exec.rs`（7 例） |
| 通道（argv 拼接、`--` 分隔、透传、两种模式默认值一致、超时文案） | `crates/host/src/lib.rs`（6 例） |
| 内核侧执行封装（作业日志、错误翻译、不替调用方结算） | `crates/app/src/host.rs`（3 例） |
| 内核命令作业（提交 / 审计 / 结算 / 归属） | `crates/app/src/host.rs` 的 `kernel_exec_settles_the_job_and_audits` |
| 计划任务的命令动作走 exec（作业成对出现、平台分支断言） | `crates/app/src/tasks.rs` 的 `site_backup_runs_through_an_exec_job` |
| 作业生命周期（提交/阶段/日志/取消/重启收尾） | `crates/app/src/jobs.rs`（7 例） |
| 打包期收口（`bin:` 形式与包装器） | `crates/pkgfmt/src/manifest.rs` 的 `validate_resource_shapes` |
| 路由稳定性 | `crates/wire/src/lib.rs` 的 `route_table_is_stable` |
