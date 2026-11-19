# 插件 ↔ 内核 IPC 优化方案（UDS + HTTP/1.1 子集）

> 状态：**二次审查并执行（2026-11-19）**。P0 四项与 P1-1（插件→内核方向）已落地；
> P1-2 / P1-3 / P2 明确暂缓，理由见 §四。
> 前提不变：插件 ↔ 内核继续使用 Unix socket 上的 HTTP/1.1 极子集 + JSON，不换协议。
> 相关：[HOST_EXEC_DESIGN.md](HOST_EXEC_DESIGN.md) ｜ [KERNEL_EXEC_CONTRACT.md](../contract/KERNEL_EXEC_CONTRACT.md#22-post-apiv1capabilityexecstatus) ｜ `crates/wire` / `crates/plugin` / `crates/sdk` / `crates/core/src/kernel`

---

## 一、审查结论：原方案哪些成立、哪些要改

| 原判断 | 审查结果 |
| --- | --- |
| HTTP 解析不是主要成本 | **成立**。`skiff-wire` 只有几十行编解码，两端都不依赖 hyper/axum 做插件侧。 |
| `uid → 插件 id` 每请求 `list_plugins()` | **成立，且比原文更重**：`uid_to_plugin` 已投递 `spawn_blocking`（不堵 worker），但 `list_plugins()` 是 **N+1 查询**（每个插件再查一次 grants），每次能力调用都全表扫。 |
| `plugin_grants()` 同步 SQLite 堵 tokio worker | **成立**。`AppState` 的 `CapabilityHost` 实现直接同步查库；`fs/read` 一次请求查 **3 次** grants（`has_perm`、`grants_of`、审计 `audit_scope`）。 |
| SDK `read_to_end` 无读超时 | **成立**。内核卡死会让插件 RPC 处理线程永久挂起，且插件无法区分「没执行」与「执行了但没回」。 |
| 长 RPC 会被空闲回收杀掉 | **成立**。`last_used` 只在 RPC **开始前**刷新，`reap_idle` 只看它；超过 `idle_timeout_sec` 的作业会在插件轮询途中被 SIGTERM。 |
| `host_exec_full` 固定 200ms 轮询 | **成立**。几分钟的作业要发几百次请求。 |
| 连接复用要在两侧都做 | **修正**：内核侧本来就是 hyper，原生支持 keep-alive；真正缺的是 SDK 客户端按 `Content-Length` 分帧并复用连接。内核→插件方向仍是「一请求一连接」，原因见 §四。 |
| 失效点要逐个命令调用 `refresh_auth` | **修正为更稳的做法**：缓存键直接用 `Db::plugin_rev()`。任何改插件记录/授权的写入都在仓储层推进版本号，**不依赖调用方记得清缓存**，从根上消除「漏一处就长期放行旧授权」。 |

---

## 二、现状成本剖面（实施前 → 实施后）

| 动作 | 位置 | 实施前 | 实施后 |
| --- | --- | --- | --- |
| UDS 连接 | 插件 SDK → 内核 | 每 RPC 一次 connect/close | 每线程复用一个连接（`keep-alive`） |
| `uid → 插件 id` | `core/src/kernel/auth.rs` | 每请求 N+1 查询 | 进程内快照（按 rev 失效） |
| grants 查询 | `plugin/src/capability.rs` | 每请求 2–3 次同步查库 | 快照读锁 + `Arc` 克隆 |
| 插件侧超时 | `sdk/src/lib.rs` | 无 | 读 40s（`exec/run` 135s）/ 写 10s |
| `exec/status` | `core/src/kernel/exec.rs` | 200ms 固定短轮询 | 最长 30s 长轮询 |
| 审计落库 | 各 handler | 同步写库（有意为之） | 不变 |

> token 比对仍走 `Supervisor::token_of`（内存锁），**不并入快照**：令牌的生命周期绑定
> 「进程这一次运行」，与库内记录不是同一份事实；混在一起会让重启后的旧令牌继续可用。

---

## 三、已实施

### P0-1 鉴权与授权快照

- `Db` 增加进程内 `plugin_rev`：`upsert_plugin` / `delete_plugin` / `set_plugin_enabled_state` /
  `add_grant` / `remove_grant` 全部自增；纯读不推进。
- `AppState` 持有只读快照：`rev` + `id → Arc<[Grant]>` + `uid → id`。命中时一次读锁；
  版本变化时重建一次（`list_plugins`）。构建失败清空快照（`rev = None`），下个请求重试，
  期间按「无授权」处理（fail closed）。
- `CapabilityHost::plugin_grants` 返回 `Arc<[Grant]>`，能力面热路径不再查库；`plugin_auth`
  中间件用快照完成 `uid → id`；`docker_call` 也改用快照。
- **失效覆盖**：安装、升级、卸载、启停、授权增删；官方同步发生在启动早期，天然在快照首次构建之前。
  另加 **5s 兜底 TTL**：`skiff plugin install` 这类 CLI 命令是另一个进程直接写库，本进程的
  `plugin_rev` 看不到变化，靠 TTL 最迟 5s 重建（本进程内的授权变更仍按 rev 立即失效）。

### P0-2 在途 RPC 不得被空闲回收

- `Running` 增加 `inflight: Arc<AtomicUsize>`；`Supervisor::call` 用 RAII 守卫在进入 RPC 前 +1、
  返回/出错/panic/取消时 -1。
- `reap_idle` 只在 `inflight == 0 && 空闲超时` 时停止进程；`last_used` 语义不变。
- 补一道**外部忙碌判定**：内核 RPC 上限是 30s，`host_exec_full` 这类长作业会在内核放弃等待后
  继续在插件里轮询，此时 `inflight` 已归零。回收 tick 改调 `reap_idle_with`，传入
  「该插件是否还有活跃 `host.exec` 作业」，两种忙碌都挡住回收。
- 用例：`reap_idle_skips_inflight_rpc` 断言在途期间不回收、外部忙碌时不回收、都结束后照常回收。

### P0-3 插件 → 内核超时

- SDK 的 `UnixStream` 设置读 40s（长轮询上限 30s + 余量）与写 10s；`exec/run` 单列 135s——
  内核 `exec/run` 的前台等待上限是 120s，客户端超时必须比它长，否则插件显式 `wait_sec = 120`
  的提交会被自己掐断。超时按路由选择，连接复用时每次都重设。
- 超时映射为 `SdkError::Timeout`，文案明确「结果未知」——调用方不能当成「没执行」。
- 内核 → 插件已有 30s 上限（`ipc::RPC_TIMEOUT`），两方向口径一致。
- 用例：假内核只 accept 不响应，断言读超时返回 `Timeout`。

### P0-4 `exec/status` 长轮询

- `exec/status` 新增可选 `wait_ms`（`[0, 30000]`，超出夹取）：作业未结束时最多挂起这么久，
  进入终态或到点后返回，响应形状与日志游标语义不变。
- 并发挂起上限 64（进程内信号量）；槽位不足直接返回当前快照——长轮询只是省请求，不是正确性依赖。
- SDK 新增 `host_exec_status_wait`，`host_exec_full` 改为**纯异步提交（`wait_sec = 0`）+ 5s 长轮询**：
  长作业不再让单条 `exec/run` 请求挂满 120s，也就不会贴着内核 RPC 上限跑。
- 契约同步：[KERNEL_EXEC_CONTRACT.md §2.2](../contract/KERNEL_EXEC_CONTRACT.md#22-post-apiv1capabilityexecstatus)。

### P1-1 插件 → 内核连接复用

- `skiff-wire` 新增 `build_request_keep_alive` 与 `parse_response_at`（返回本帧消费字节数）；
  默认 `build_request` 仍是 `Connection: close`，其余调用方零改动。
- SDK 每线程对 `kernel.sock` 保留一条连接；按 `Content-Length` 精确分帧，缓冲区保留未消费前缀。
- 连接级失败即丢弃；**只有幂等只读路由**（config / identity / fs read / fs list / exec status /
  interop catalog）重连重试一次。`exec/run` 这类写操作不自动重发，避免同一命令提交两次。
- 用例：同一线程两次调用只触发服务端一次 `accept`；`wire` 用例覆盖 keep-alive 头与消费长度。

---

## 四、暂缓（本次不做，附理由）

| 项 | 理由 |
| --- | --- |
| **内核 → 插件连接复用**（插件侧 `handle_conn` 循环 + Supervisor 连接池） | 插件侧现在是「每连接一线程」，复用要同时改插件侧读循环并给 Supervisor 加按插件连接池、失效与并发语义。P1-1 已拿掉插件→内核方向的主要开销；这一项应先用指标确认线程数是瓶颈。 |
| **P1-2 只读批量端点** | 批量端点是新的协议面（白名单、部分失败、审计顺序都要定契约）。keep-alive 已把 N 次往返压到一条连接，收益明显下降。 |
| **P1-3 SDK 串行队列 helper** | 属于插件侧便利封装，不影响内核成本；等出现第二个需要排队的官方插件再收口，避免过早抽象。 |
| **interop 目录/转发的 `list_plugins`** | `interop_catalog` 仍每请求同步 `list_plugins()` + `plugin()`（需要 `enabled`、`api`、完整 manifest，当前快照只存 id/uid/grants）。要么把快照扩成 `Arc<[PluginRecord]>`，要么单独给互调做一份目录快照——涉及 manifest 内存占用，单独评估。 |
| **P2-1 插件侧有界并发** | 需要与重连风暴的实际观测数据一起定上限；当前线程数受 `RLIMIT_NPROC` 约束。 |
| **P2-2 指标与 trace id** | 需要先确定 `/metrics` 暴露面与脱敏口径；属于独立需求。 |
| **P2-3 大文件直传** | `fs/read` 已支持分页与可选 base64，当前不在主要矛盾上。 |

---

## 五、明确不做

| 不做 | 理由 |
| --- | --- |
| gRPC / protobuf | 当前几乎全是短请求；代价是插件侧引入 tokio 与代码生成。 |
| 自定义二进制 / 长度前缀协议 | 省下的头部解析远小于查库与轮询成本，排障工具还会退化。 |
| 本地 UDS 压缩 | 本地 IPC，CPU 换带宽不划算。 |
| 插件缓存能力面结果 | 授权变更后的失效复杂度与正确性风险大于收益；快照只缓存「授权」，不缓存调用结果。 |
| TCP 回环替代 UDS | 丢掉 `SO_PEERCRED` 与文件权限这一层身份语义。 |

---

## 六、验收与待实测

已落地的行为用例（交 CI 的 `test` job）：

- 撤销/变更授权后快照按 rev 重建（`plugin_rev_advances_on_auth_inputs`、能力面既有 403/放行用例）；
- 在途 RPC 不被回收（`reap_idle_skips_inflight_rpc`）；
- SDK 读超时返回 `Timeout`；连接复用只 accept 一次；
- `wait_ms` 缺省为 0、超上限夹到 30s。

待实测（本文数字均为目标，不是已验证事实）：

1. 单次能力调用 P50/P95（快照缓存前后）。
2. `exec/status` 请求数（长轮询前后）。
3. keep-alive 生效后的插件线程数与常驻内存。
4. 超过 `idle_timeout_sec` 的长作业下插件存活情况（P0-2 前后）。
5. 插件侧线程数是否值得继续做「内核 → 插件」复用（决定 §四第一项是否启动）。

**口径**：本机只做 `fmt` + `clippy` 编译级校验；用例与性能数字以 CI 与测试机为准。
