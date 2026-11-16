# 发布面 R0–R2e 落地说明（§17.9–17.15）

> 上级索引：[SKIFF_SERVER.md](../SKIFF_SERVER.md) ｜ 拆自 `docs/plugin/SKIFF_SERVER.md` 原 §17.9–17.15（2026-11-16 整理，正文未改）。

---

### 17.9 R0 落地说明（`tools/skiff-server`）

**进程与传输**

- 两个子命令：`skiff-server api`（六个端点）与 `skiff-server release`（发布 job，不在六端点内）。
- 不引 web 框架与数据库：沿用 `serve` 的 std 监听 + 每连接一线程；JSON 用 `serde_json`；状态是 `state/` 下的文件。
- HTTP 传输件（读请求头、写响应、TLS）抽到 `tools/skiff-server/src/http.rs`，`serve` 与 `api` 共用；`serve` 的路径白名单、条件请求、Range、访问日志**未改动**，`tests/serve.rs` 全绿。
- `api` 默认 `127.0.0.1:9443`，明文；TLS 由前置反代终止。非回环监听必须显式 `--allow-remote`，否则拒启。

**启动强校验（任一不满足即退出码 1）**

- `--state` 目录必须存在且 0700（Unix；Windows 由 ACL 兜底）。
- `--authors` / `--reviewers` 必须是普通文件且 0600，`schema` 固定，`key_id` 必须等于 `sha256(公钥)`。
- 运行期白名单每次请求重读：改文件即生效、无需重启；读取或解析失败按 503 失败关闭，绝不放行。

**请求与错误码**

- 签名域、四头、±300s 时间窗与 §17.2 一致；nonce 落 `state/nonces/<YYYYMMDDHH>`，保留 2 小时，重复即 401。
- 必须有 `Content-Length`（POST 缺失 → 411）；任何 `Transfer-Encoding`（含 chunked）→ 400；声明长度超 64 MiB → 413，**不读 body**。
- multipart 严格模式：boundary 1..=70 且字符集受限；每部分头部 ≤16 行、单行 ≤1024 B、总头部 ≤4096 B；只接受 `entry` 与 `package` 两个命名部分；**文件名只出现在头部，不参与任何路径拼接**。
- 401 签名缺失/无效/过期/重放；403 密钥未登记、命名空间不匹配、非审核钥调 approve/reject、自我审核、动别人的提交（统一 `forbidden` 体）；409 版本或摘要冲突；412 非法状态迁移与并发 approve 抢锁失败；422 包/元数据校验失败。
- 日志与审计（`state/audit.jsonl`）只记 `key_id`、动作、`sid`、结果与时间，**不记签名、不记包内容**。

**存储与状态机**

- `state/submissions/<sid>.json` + `<sid>/package.wbpkg`；`sid = sub-<sha256(id|version|sha256)[..24]>`，同一三元组天然幂等。
- `pending → approved | rejected | withdrawn`，`approved → published`；其余迁移 412；approve 用 `<sid>.lock` 独占，并发后到者 412。
- 被驳回/撤回后，同一摘要重提会把记录重新置为 `pending`（不新建）；已 `published` 的同版本一律 409。

**产物生成与私钥姿态**

```text
skiff-server api      → 只写 state/（提交、审核、审计），不读签名私钥
skiff-server release  → 写源数据 → 调既有 publish → 产出签名产物
```

- `release --staging-out <dir>`：只产出 staging，**不碰线上目录**，交发布机/CI 搬运（推荐）。
- `release --dist <dir> --swap`：显式换入线上目录，先备份旧目录、换入失败自动回滚。
- `seq` 基准：`--seq <n>` 由调用方给定，或从 `--prev`/`--dist` 的 manifest **只读**读取后 +1；缺省从 1 开始。
- 生产环境 `SKIFF_SIGNING_KEY` 只在离线发布机或 CI，分发主机只接收产物；**同机 `api` + `release --dist --swap` 仅测试姿态**，不得用于生产。
- `publish` 在内存完成全部校验后才写盘，失败时线上目录不变；`release` 失败也不回写 `published`。

**R0 测试**

- `tests/api.rs`：签名正例（提交→审核→发布出可拉取的 `manifest/index/blobs`）、篡改 body、错误域前缀、过期时间戳、重放 nonce、非审核钥 approve、自我审核、同摘要幂等、非法状态迁移、缺 `Content-Length`/`Transfer-Encoding`、超长 `Content-Length`、失败不污染 `--dist`、`serve` 对 `/api/**` 仍 404。
- 复用 `skiff-pkgfmt` 校验（与 `xtask verify-pkg` 同源），不另写验签。

### 17.10 R1 落地说明（`tools/skiff-server`）

- **端点与存储**：`yank` / `keys/{key_id}/revoke` / `audit` 三个新端点与 R0 六端点共用同一签名域与错误体。撤包记录落 `state/takedowns/<tak-…>.json`，密钥吊销落 `state/revoked_keys.json`；`api` 仍不读签名私钥、不写 `--dist`。
- **鉴权**：`authenticate` 在公钥一致性检查后、验签前查 `revoked_keys.json`，命中即 403；读取失败 503 失败关闭。密钥吊销即时生效，不等待发布。
- **发布 job**：`release` 同时处理 `approved` 提交与 `pending` 撤包记录。一次运行只递增一个 `seq`：先写源数据（新条目 / `yanked=true` + `yank_reason` / `revoked.json`），再 `publish`，最后回写 `published`、`released` 与提交记录的 `takedown`。没有待发布项时直接失败，不空转递增 `seq`。
- **并发**：yank 记录按 `(kind, id, version)` 稳定命名并用文件锁串行；重复 yank 返回 200 同一记录。发布 job 是 `pending → released` 的唯一写入者；并发到达的新 yank 留给下一次 `release`。
- **`revoked.json` 与签名覆盖**：`build` 读取 `<channel>/revoked.json` 填入 `index.revoked`；缺文件等价于空数组，产物逐字节一致；文件存在但 JSON 非法时构建直接失败。manifest 的 `index.sha256` 继续覆盖整份索引字节，因此吊销列表也被签名间接覆盖。
- **撤包原因**：`build` 把源条目的可选 `yank_reason` 原样带进索引；该字段不出现时输出与 R1 前逐字节一致。旧客户端忽略未知字段，因此不需要 schema 版本迁移。
- **客户端**：`skiff-app` 的索引模型对 `yank_reason` 用 `serde(default)`；`CatalogService::cached_takedown` 只读内存/本地缓存，不触网，有原因时返回原因，无原因时保持通用文案。安装侧仍在 `commands::store::prepare` 拒绝 `revoked` 与 `yanked` 并带出原因。
- **审计串联**：`release` 行携带 `submissions[]`；`GET /api/v1/audit` 解析为 `submission_ids[]` 并让 `?sid=` 命中任一元素，因此一个 sid 能同时追到 submit / approve / release。
- **R1 测试**：`tests/api.rs` 覆盖 yank 幂等/404/403、吊销后提交与审核 403、历史版本进入 `revoked[]`、审计分页/越权/sid 串联；`tests/p0.rs` 覆盖 `revoked.json` 缺省/非法/签名字节与 `yank_reason` 进入索引；`crates/app` 覆盖旧索引解析不变、新索引拒绝原因，以及「源数据 → 构建产物 → 客户端视图」的端到端用例。

### 17.11 R2a 落地说明：webhook 事件通知（`tools/skiff-server`）

- **形态**：业务侧只写 `state/outbox/pending/<event_id>.json`，`skiff-server notify` 独立进程负责投递；API 与 `release` 都不发起网络请求。`notify` 默认常驻轮询，`--once` 适合 systemd timer/CI。
- **事件集**：`submission.created`（`reopened` 带标记）、`submission.approved`、`submission.rejected`、`submission.withdrawn`、`submission.published`（带 `seq` 与 `submission_ids`）、`takedown.yanked`、`key.revoked`、`release.failed`。`duplicate` 不发；事件载荷不含包体、签名、请求体。
- **事件 id**：`evt-` + `sha256(type\0业务键)[..24]`。同一业务动作重复 emit 命中同一文件名，`create_new` 去重；`submission.published` 的业务键是 `channel:seq:release`。
- **订阅**：`state/webhooks.json` 手工维护，字段 `id/url/events[]/channels[]/enabled`；`events`/`channels` 为空表示全部。只允许 `https://`，默认拒绝回环/私有地址，内部目标需 `--allow-private`；本地联调才用 `--allow-http`。
- **投递**：at-least-once。2xx 成功；网络错误/超时/408/429/5xx 重试，其余 4xx 进 `dead/`；退避默认 5s/30s/2m/10m/1h/6h，最多 8 次，尊重 `Retry-After`。接收方按 `X-Skiff-Event-Id` 去重；不承诺事件顺序。
- **签名**：独立域 `SKIFF-SERVER-WEBHOOK-v1`，输入为 `域\0event_id\0timestamp\0sha256(body)`；私钥只在 `notify` 的 `SKIFF_WEBHOOK_SIGNING_KEY`，不复用 API 签名域。请求头：`X-Skiff-Event-Id`、`X-Skiff-Timestamp`、`X-Skiff-Signature: v1=<hex>`。
- **失败隔离**：outbox 写失败只记日志/返回 `Err` 给调用方忽略，业务响应不变；`notify` 与 API 是两个进程，投递失败不影响提交、审核、发布与撤包。
- **保留期**：pending 默认 30 天（超期转 dead）、dead 默认 90 天（超期删除），`NotifyOptions` 可配置；`notify.lock` 防止多实例并发投递，崩溃遗留锁超过 10 分钟视为过期。
- **R2a 测试**：`events` 覆盖确定性 id、重复 emit 去重、载荷不含敏感字段；`notify` 覆盖签名可验、5xx 重试后成功、4xx 不重试、超时重试并最终 dead、事件过滤；`tests/api.rs` 覆盖 outbox 不可写仍 201、`release.failed` 事件、submit/approve/release/yank 事件集合。

### 17.12 R2b 落地说明：提交记录 `kind` 抽象（`tools/skiff-server`）

- **模型**：`Submission` 新增 `kind: EntryKind`（`plugin` 缺省、`app`、`runtime`）。`#[serde(default)]` + `skip_serializing_if = "EntryKind::is_plugin"`：解析缺字段的旧记录得到 `plugin`，插件记录写盘与响应里都不出现 `kind`。
- **范围**：R2b 只做模型与兼容性，不新增接口。提交、审核、发布、撤包、事件、`sid` 派生、源条目与签名产物仍全按插件路径执行，字节不变。
- **后续用途**：R2c/R2d 按 `kind` 分别校验模板与运行环境，R2e 按 `kind` 撤包并写入 `revoked_apps[]` / `revoked_runtimes[]`；`revoked[]` 继续只表示插件。
- **R2b 测试**：`tests/api.rs::plugin_records_omit_kind_and_legacy_records_still_load` 断言插件提交响应与落盘记录都没有 `kind`、无 `kind` 的旧记录解析为 `plugin`、approve→release 后源条目与 `index.json` 都不出现 `kind`；其余 22 项 API 测试（签名、幂等、状态机、撤包、吊销、审计、事件）全绿，证明插件全链路行为未变。

### 17.13 R2c 落地说明：应用模板提交与发布（`tools/skiff-server`）

- **提交端点**：`POST /api/v1/apps`（`entry` + `compose`，见 §17.3）。作者身份仍由请求签名确立，`id` 必须落在 `authors.json` 的前缀命名空间内。
- **compose 校验单一实现**：直接依赖 `skiff-compose`（与 `skiff-docker`、`xtask` 同源），严格子集外的字段（如 `build:`）一律 422；服务端不另写解析器，也不靠测试向量对齐两份实现。
- **存储**：compose 原文 `state/submissions/<sid>/compose.yaml`；记录 `kind = "app"`、`app` 字段保存 `min_kernel`/`images`/`env`/`ports`/`volumes`；`api` 留空表示不适用。
- **发布**：`release` 把 compose 复制成 `catalog/assets/<sha>.yaml`，写源条目 `catalog/<channel>/apps/<id>-<version>.json`（`asset` 指向该文件），`build` 已有的 apps 分支据此生成 `index.apps[]` 与 `compose_sha256`/`compose_url`；`seq` 每次 `release` 只 +1，插件与应用同批发布共享同一个 `seq`。
- **客户端边界**：客户端继续只消费签名索引，`CatalogEntries.apps` 已展示「应用模板」分区；**不提供安装入口**，安装来源仍是内置目录，`serve` 与 §16 契约零变化。
- **失败隔离**：`submission.created/approved/published` 等事件仍走 best-effort outbox，写 outbox 失败不影响提交/审核/发布结果。
- **R2c 测试**（`tests/api.rs`）：
  - `app_submit_approve_release_writes_compose_blob_and_index`：201 → 202 → release 后 `index.apps[0]` 的 `compose_sha256`/`compose_url` 正确、blob 字节与提交原文一致、源条目 `asset` 指向 `assets/<sha>.yaml`；
  - `app_compose_outside_supported_subset_is_rejected`：`build:` 与非 UTF-8 均 422 且不落状态；
  - `same_app_version_with_different_compose_conflicts`：同摘要 200 同一 `sid`，同版本不同摘要 409；
  - `plugin_and_app_release_share_one_seq`：一次 `release` 同时发布插件与应用，`seq = 1`、`released.len() == 2`，索引两数组各 1 条。
  - 合计 `cargo test -p skiff-server`：34 单测 + 27 API + 4 CLI + 15 P0 + 7 serve + 6 sync = 93 项全绿。

### 17.14 R2d 落地说明：运行环境提交、审核与元数据修订（`tools/skiff-server`）

- **提交**：`POST /api/v1/runtimes`（JSON `entry`，见 §17.3）。作者 `require_author`、审核者 `require_reviewer`；提交时 `default` 必须为 `false`，镜像与 digest 落记录后不可改。
- **身份与版本**：`sid = sha256("runtime\0kind\0version\0image_digest")`；同 `(kind, version)` 不同 digest 或已发布 → 409；同一 digest 换了 `image` 引用 → 409。
- **发布**：`release` 写 `catalog/<channel>/runtimes/<kind>-<version>.json`（`RuntimeEntry`），由既有 `build` 生成 `index.runtimes[]`；`build` 同时兜底校验「同 kind 至多一个 `default`」，违反则发布失败并留 `release.failed` 事件。
- **元数据修订**：`POST /api/v1/submissions/{sid}/revise`（审核者、仅 `published`）。只接受 `eol`/`default`；写 `state/runtime_revisions/<id>.json`（`seq=null` 表示未应用）并审计 `revise`；下一次 `release` 校验源条目 digest 与修订一致后应用、递增 `seq` 并标记 `seq`。镜像与 digest 没有任何写入路径，因此不可改。
- **范围**：只做提交/审核/展示；客户端继续从签名索引 `runtimes[]` 展示，安装仍走内置目录，`serve` 与 §16 契约零变化。
- **R2d 测试**（`tests/api.rs`）：
  - `runtime_submit_approve_release_writes_index_entry`：201→202→release 后 `index.runtimes[0]` 的 `kind`/`version`/`image`/`image_digest`/`api_min`/`default` 正确，源条目落在 `catalog/stable/runtimes/php-8.5.json`；
  - `runtime_default_must_be_false_at_submission`：提交带 `default=true` → 422；
  - `runtime_revision_changes_eol_and_default_with_audit`：带 `image_digest` 的修订 422；`eol`+`default` 修订 202，下一次 release 生效，`audit.jsonl` 有 `action=revise`；
  - `runtime_revision_requires_reviewer_and_published_state`：未发布 412、作者 403；
  - `release_rejects_multiple_defaults_per_kind`：两个同 kind 条目都改 `default=true` 后 release 失败。
  - 合计 `cargo test -p skiff-server`：34 单测 + 32 API + 4 CLI + 15 P0 + 7 serve + 6 sync = 98 项全绿。

### 17.15 R2e 落地说明：分 kind 撤包与独立吊销命名空间（`tools/skiff-server`）

- **端点**：`POST /api/v1/plugins/{id}/versions/{v}/yank`、`POST /api/v1/apps/{id}/versions/{v}/yank`、`POST /api/v1/runtimes/{kind}/versions/{v}/yank`，共用 `takedown::yank` 单实现；审核者调用，幂等 200，目标不存在 404，已撤回/已在吊销列表 409。
- **记录**：`Takedown` 增加 `entry_kind`（缺省 `plugin` 且不落盘，旧 `state/takedowns/*.json` 无需迁移）；记录 id 对插件保持 R1 派生式不变，其它 kind 加类型域前缀，避免同 id/version 撞记录。
- **源数据**：`release` 按 kind 定位 `plugins/` / `apps/` / `runtimes/` 目录（运行环境的 id 字段是 `kind`），只改 `yanked` / `yank_reason`；非插件撤包同时追加 `revoked_apps.json` / `revoked_runtimes.json`（应用取 compose 摘要，运行环境取镜像 digest 的 hex）。
- **索引**：`IndexFile` 新增 `revoked_apps` / `revoked_runtimes`（空时省略），`apps[]` / `runtimes[]` 条目在撤包后带 `yanked` / `yank_reason`（未撤包不落字段）；`revoked[]` 语义与字节不变。
- **客户端提示**：`crates/app` 的索引模型解析新数组与 `yanked` 字段，商店「应用模板」「运行环境」分区的 `warn` 按「吊销命中 → 已撤回：原因」「yanked → 已下架：原因」「EOL」优先级展示；不新增安装入口，`serve` 与 §16 契约零变化。
- **R2e 测试**：
  - `yank_app_marks_source_and_revoked_apps_only`：应用撤包后索引 `apps[0].yanked`、`revoked_apps[0]`（含 compose 摘要）正确，`revoked[]` 为空，提交记录带站内提示；
  - `yank_runtime_marks_revoked_runtimes_only`：运行环境撤包写入 `revoked_runtimes[]`（digest hex），`revoked_apps[]` 不出现；
  - `yank_kind_endpoints_are_reviewer_only_and_idempotent`：作者 403、未知目标 404、重复撤包 200 同一 id；
  - `plugin_yank_keeps_revoked_namespaces_untouched`：插件撤包记录不含 `entry_kind`，索引不出现两个新数组；
  - `crates/app/src/catalog/tests.rs::app_and_runtime_takedown_use_their_own_namespaces`：客户端解析新数组并按命名空间给出提示。
  - 合计：`cargo test -p skiff-server` 102 项、`cargo test -p skiff-app catalog` 57 项全绿；`clippy -D warnings` 通过。
