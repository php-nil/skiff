# O 商店目录 / P 发布面 API / 附 待核对项

> 上级索引：[TEST_MATRIX.md](../TEST_MATRIX.md) ｜ 拆自 `docs/test/TEST_MATRIX.md` 原 O–P + 附（2026-11-16 整理，正文未改）。

---

## O. 商店目录（Skiff-server）

> 重跑触发：改了 `crates/app/src/catalog/fetch.rs` 的传输层/TLS 配置、`crates/core/src/config.rs`
> 的 `[catalog]` 校验，或 `docs/plugin/SKIFF_SERVER.md` §4.5 的内网口径。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| STORE-01 | 自签 CA 内网镜像可接入且仍强制 https | `[本机]` crate 单测；`[上机]` 内网静态镜像 + 该 CA 签发的服务器证书 | 不配 `ca_bundle` 访问自签镜像；再把 CA 导出 PEM 配到 `[catalog].ca_bundle` 后重启刷新 | 未配置时连接被拒（证书不受信）；配置后刷新成功、`catalog.refresh` 落库；镜像地址仍只接受 `https://`，无 insecure 开关 | □通过 / □失败 | `crates/app/src/catalog/fetch.rs::{UreqTransport::load_ca_bundle,root_store}`、`catalog::fetch::tls_tests`、`crates/core/src/config.rs::build_catalog_settings` |
| STORE-02 | `ca_bundle` 空值/非法值的启动语义 | `[本机]` 单测 | 不配置、配置不存在路径、配置非 PEM 内容三种启动 | 空值行为与历史一致（`extra_roots` 为空）；缺失/非法明确报「`catalog.ca_bundle` 非法」并拒绝启动，不静默回退内置根 | □通过 / □失败 | `crates/core/src/config.rs::catalog_ca_bundle_is_optional_and_strict`、`skiff_app::panel_settings::CatalogFile::ca_bundle` |
| STORE-03 | `skiff-server serve` 符合静态服务契约 | `[本机]` 集成测试；`[上机]` 产物目录 + 自签证书 + Release 资产 `skiff-server-<target>`（测试机无 Rust 工具链） | 从 Release 取 `skiff-server-<target>` 并 `sha256sum -c checksums.txt` 校验，再启动 `serve --dist`（默认 https），分别验证条件请求、Range、HEAD、404/405、路径穿越与 `/api/`；再用 `--http` 试一次；最后加 `--access-log <path>` 重启，重放一次 `If-None-Match` 后读该文件 | 资产校验通过且可直接执行；304/206/416 语义、`HEAD` 无 body、非法 blob 名与穿越 404、`/api/**` 404、普通方法 405；默认无证书拒绝启动，`--http` 打印「仅测试用」；客户端以 `ca_bundle` 导入 CA 后能刷新；访问日志出现 `status=304 … cache=inm`，未开启时不产生文件或输出 | □通过 / □失败 | `tools/skiff-server/src/serve.rs`、`tools/skiff-server/tests/serve.rs`、`.github/workflows/release.yml`（`skiff-server-<target>`）、`docs/plugin/SKIFF_SERVER.md` §16 |

最小回归命令：

```bash
cargo test -p skiff-app catalog::fetch::tls_tests
cargo test -p skiff catalog
cargo test -p skiff-server
```

## P. 发布面 API（Skiff-server R0/R1/R2a–R2e，2026-11-12 更新）

> 重跑触发：改了 `tools/skiff-server/src/api/**`、`src/events.rs`、`src/notify.rs`、`src/http.rs`、`src/build.rs`、`src/main.rs` 的
> `api`/`notify`/`release` 子命令，或 `docs/plugin/SKIFF_SERVER.md` §17 的签名/状态机/事件/产物口径。
> R0 默认关闭、默认只绑回环；`serve` 与 `api`/`notify` 是不同进程，`serve` 对 `/api/**` 仍一律 404。
> R1 只填充 v1 索引既有字段（`yanked` / `revoked`），不升 schema；R2b 的 `kind` 对插件记录不落盘。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| API-01 | 请求签名与重放防护 | `[本机]` 集成测试；作者/审核两把 Ed25519 钥 | 正例提交；再把 body 改一个字节、改用清单签名域、把时间戳拨到 −301s、复用同一 nonce 重放 | 正例 201；其余全部 401，且不产生提交记录 | □通过 / □失败 | `tools/skiff-server/src/api/auth.rs`、`tests/api.rs` |
| API-02 | 权限分离 | 作者的 `id_prefixes` 限定 `com.example` | 作者钥调 approve；同一把钥同时登记为审核钥后自我审核；提交前缀外 id | 均 403 且响应体统一 `forbidden`；审计不记签名与包内容 | □通过 / □失败 | `api/auth.rs::Authenticated`、`api/store.rs::{approve,reject}` |
| API-03 | 幂等与状态机 | 同一份签名 `.wbpkg` | 同摘要重提；approved 后作者 withdraw；已发布版本再提交 | 重提 200 且 `sid` 相同；非法迁移 412；已发布同版本 409 | □通过 / □失败 | `api/store.rs::{sid_of,approve,withdraw}` |
| API-04 | 传输严格性 | `[本机]` 集成测试 | POST 不带 `Content-Length`；带 `Transfer-Encoding: chunked`；声明 999999999 字节 | 分别 411 / 400 / 413；413 不读取 body | □通过 / □失败 | `tools/skiff-server/src/api/mod.rs::read_body`、`api/multipart.rs` |
| API-05 | 产物生成与失败隔离 | `[本机]` 集成测试；`SKIFF_SIGNING_KEY` + `SKIFF_SERVER_PUBLIC_KEY` | 提交→approve→`release --staging-out`；再构造 schema 缺失让 `release --dist --swap` 失败 | staging 的 `manifest` seq=1 且验签通过、`blobs/sha256/<hex>` 存在；失败时 `--dist` 的 marker 原样保留、提交仍为 `approved` | □通过 / □失败 | `api/job.rs`、`tests/api.rs::failed_release_does_not_touch_dist` |
| API-06 | `serve` 零改动 | `[本机]` 集成测试 | 抽取 `http.rs` 后重跑既有 serve 契约用例（304 / 206 / 405 / 404、HEAD、穿越、访问日志） | 行为与抽取前一致，`/api/**` 仍 404 | □通过 / □失败 | `tools/skiff-server/src/http.rs`、`tests/serve.rs` |
| API-07 | yank 撤包 API 与发布 | 已发布 `com.example.demo 1.0.0` | 作者调 yank；审核钥撤未知版本；审核钥撤真实版本并重复一次；`release` 到 seq+1 | 作者 403、未知版本 404、首次 202、重复 200 且同一记录；索引该条 `yanked=true`、`seq` +1、提交记录出现 `takedown` | □通过 / □失败 | `api/takedown.rs::yank`、`api/job.rs`、`tests/api.rs::yank_*` |
| API-08 | 密钥吊销与历史处置 | 已发布版本 + 待审提交 | 审核钥吊销作者钥并重复；作者再提交；审核该 key 的历史/待审提交；`release` | 首次 202、重复 200、未登记 key 404；该 key 提交/审核均 403；历史版本 `yanked=true` 且进入 `revoked[]`；待审提交 approve 409 | □通过 / □失败 | `api/takedown.rs::revoke_key`、`api/auth.rs::authenticate`、`api/store.rs::approve` |
| API-09 | 客户端离线读取撤包信息 | 已生成的旧索引（seq=2，`yanked=false`、`revoked=[]`）与新索引 | 用当前客户端解析旧索引进安装；再用填充 `yanked`/`revoked` 的新索引安装 | 旧索引解析成功、`yanked=false`、`revoked` 为空，安装链路与 R1 前一致；新索引不报解析失败，安装返回 `revoked` 原因；已安装卡片提示来自本地缓存，不触网 | □通过 / □失败 | `crates/app/src/catalog/{model,mod}.rs`、`commands/store.rs::check_compatibility`、`crates/web/src/handlers/plugins.rs` |
| API-10 | 审计查询 | 已有 submit/approve/yank/revoke 记录 | 按 `sid`、`key_id`、`action`、`result`、`since/until`、`limit/offset` 查询；作者查他人 `key_id` | 审核者可见全部；作者默认只看自己、显式查他人 403；分页返回 `total`/`next_offset`；字段只有时间/主体/动作/对象/结果，不含签名与包内容 | □通过 / □失败 | `api/audit.rs`、`tests/api.rs::audit_query_is_scoped_and_paginated_without_sensitive_fields` |
| API-11 | `revoked.json` 纳入签名覆盖 | 临时源数据（一条插件） | 缺 `revoked.json` 构建一次；写入 `[]` 再构建；写入非法 JSON；写入一条吊销记录 | 缺文件与空数组产物逐字节一致；非法 JSON 构建失败；有效记录进入 `index.revoked` 且 `manifest.index.sha256` 覆盖索引字节 | □通过 / □失败 | `src/build.rs`、`tests/p0.rs::revoked_json_*` |
| API-12 | 撤包原因端到端可见 | 源条目 `yanked=true` + `yank_reason` | 构建索引；客户端解析并取 `plugin_package`/`cached_takedown`；再尝试安装该版本 | 索引含 `yank_reason`；插件页提示带原因；安装被拒文案带原因；无原因时仍是通用文案；旧索引无该字段时解析与安装不变 | □通过 / □失败 | `src/model.rs`、`src/api/job.rs`、`crates/app/tests/takedown_reason.rs`、`commands/store.rs::check_compatibility` |
| API-13 | 审计按 sid 串起发布 | 提交→审核→release，并执行 yank | `GET /api/v1/audit?sid=<sid>` | 同一次查询返回 submit / approve / release 三类动作；release 行含 `submission_ids[]` 且命中该 sid；仍不含签名与包内容 | □通过 / □失败 | `src/api/audit.rs`、`tests/api.rs::audit_sid_chains_submit_approve_and_release` |
| API-14 | webhook 事件生成与去重 | 已配置订阅；执行 submit/approve/release/yank | 观察 `state/outbox/pending/` | 生成 created/approved/published/yanked 事件；同一业务动作重复 emit 只有一条；载荷无包体/签名 | □通过 / □失败 | `src/events.rs`、`tests/api.rs::api_actions_emit_outbox_events` |
| API-15 | webhook 投递与签名 | 本地假接收端 | 500 后 200；400；超时；校验签名 | 5xx/超时重试，4xx 直接 dead；成功移入 delivered；Ed25519 签名可用独立域验证 | □通过 / □失败 | `src/notify.rs` 单测 |
| API-16 | 通知失败隔离 | `state/outbox` 被替换为普通文件 | 正常提交 | 提交仍 201，业务状态不受影响 | □通过 / □失败 | `tests/api.rs::submit_succeeds_when_outbox_cannot_be_written` |
| API-17 | `release.failed` 事件 | 已 approve 提交 + 不存在的 schema 目录 | 执行 release | 发布失败并留下 `release.failed` 事件；不影响线上目录 | □通过 / □失败 | `src/api/job.rs`、`tests/api.rs::release_failure_emits_event` |
| API-18 | 提交记录 `kind` 与旧记录兼容 | 旧格式 `state/submissions/*.json`（无 `kind`） | 提交插件并检查落盘/响应；再 approve→release 检查源条目与索引 | 插件记录不出现 `kind`；无 `kind` 的旧记录解析为 `plugin`；源条目与 `index.json` 字节不变 | □通过 / □失败 | `src/api/store.rs`（`EntryKind`）、`tests/api.rs::plugin_records_omit_kind_and_legacy_records_still_load` |
| API-19 | 应用模板提交→发布全链路 | 作者钥已登记 `com.example` 前缀 | `POST /api/v1/apps`（`entry`+`compose`）→ approve → release | 201/202；`index.apps[]` 带 `compose_sha256`/`compose_url`，`blobs/sha256/<sha>` 与提交原文逐字节一致，源条目 `asset` 指向 `assets/<sha>.yaml` | □通过 / □失败 | `src/api/store.rs::create_app`、`src/api/job.rs`、`tests/api.rs::app_submit_approve_release_writes_compose_blob_and_index` |
| API-20 | compose 严格子集校验 | 含 `build:` 或非 UTF-8 的 compose | 提交应用模板 | 422，不落任何提交状态；不另写第二套解析器（复用 `skiff-compose`） | □通过 / □失败 | `src/api/store.rs::create_app`、`tests/api.rs::app_compose_outside_supported_subset_is_rejected` |
| API-21 | 应用版本不可变与幂等 | 同 id/version 的 compose A、B | 先提 A，再提 A，再提 B | 201；重复 A → 200 同一 `sid`；B → 409 | □通过 / □失败 | `src/api/store.rs::create_app`、`tests/api.rs::same_app_version_with_different_compose_conflicts` |
| API-22 | 插件与应用共享单次 `seq` | 同一渠道各一份 approved 提交 | 一次 release | `seq = 1`、`released.len() == 2`，索引 `plugins[]`/`apps[]` 各 1 条 | □通过 / □失败 | `src/api/job.rs`、`tests/api.rs::plugin_and_app_release_share_one_seq` |
| API-23 | 运行环境提交→发布全链路 | 作者/审核者钥已登记 | `POST /api/v1/runtimes` → approve → release | 201/202；`index.runtimes[]` 的 `kind`/`version`/`image`/`image_digest`/`api_min`/`default` 正确；源条目落 `catalog/<ch>/runtimes/<kind>-<version>.json` | □通过 / □失败 | `src/api/store.rs::create_runtime`、`src/api/job.rs`、`tests/api.rs::runtime_submit_approve_release_writes_index_entry` |
| API-24 | 运行环境 `default` 提交约束 | `entry.default = true` | 提交运行环境 | 422；`default` 只能在发布后走元数据修订 | □通过 / □失败 | `src/api/store.rs::create_runtime`、`tests/api.rs::runtime_default_must_be_false_at_submission` |
| API-25 | 运行环境元数据修订 | 已发布运行环境 | `POST /api/v1/submissions/{sid}/revise`（`eol`/`default`）→ release | 202；下一次 release 应用到源数据并递增 `seq`；`audit.jsonl` 有 `action=revise`；带 `image`/`image_digest` 的修订 422；未发布 412、作者 403 | □通过 / □失败 | `src/api/store.rs::revise_runtime`、`src/api/job.rs::apply_runtime_revisions`、`tests/api.rs::runtime_revision_changes_eol_and_default_with_audit`、`runtime_revision_requires_reviewer_and_published_state` |
| API-26 | 同 kind 多个 `default` 被 release 拒绝 | 同 kind 两个已发布运行环境均修订为 `default=true` | 再次 release | 发布失败并留 `release.failed`；不生成矛盾索引 | □通过 / □失败 | `src/build.rs`（default 唯一性）、`tests/api.rs::release_rejects_multiple_defaults_per_kind` |
| API-27 | 应用撤包进独立命名空间 | 已发布应用 | `POST /api/v1/apps/{id}/versions/{v}/yank` → release | 源条目 `yanked=true`；索引 `apps[].yanked`/`yank_reason` 正确；`revoked_apps[]` 含 compose 摘要；`revoked[]` 为空；提交记录带站内提示 | □通过 / □失败 | `src/api/takedown.rs`、`src/api/job.rs`、`tests/api.rs::yank_app_marks_source_and_revoked_apps_only` |
| API-28 | 运行环境撤包进独立命名空间 | 已发布运行环境 | `POST /api/v1/runtimes/{kind}/versions/{v}/yank` → release | 索引 `runtimes[].yanked` 正确；`revoked_runtimes[]` 含 digest hex；`revoked_apps[]` 不出现 | □通过 / □失败 | `src/api/takedown.rs`、`tests/api.rs::yank_runtime_marks_revoked_runtimes_only` |
| API-29 | 分 kind 撤包权限与幂等 | 作者钥、未知目标 | 作者撤包 / 未知版本 / 重复撤包 | 403 / 404 / 200 且同一记录 id | □通过 / □失败 | `src/api/mod.rs`、`src/api/takedown.rs`、`tests/api.rs::yank_kind_endpoints_are_reviewer_only_and_idempotent` |
| API-30 | 插件撤包字节与命名空间不变 | 已发布插件 | 撤包 → release | 记录不含 `entry_kind`；索引 `plugins[].yanked` 正确；不出现 `revoked_apps`/`revoked_runtimes` | □通过 / □失败 | `src/api/takedown.rs`、`tests/api.rs::plugin_yank_keeps_revoked_namespaces_untouched` |
| API-31 | 客户端分 kind 提示 | 索引含 `yanked` 与两个吊销数组 | 解析并生成商店条目 | 吊销原因优先于 `yank_reason`，其次 EOL；`revoked[]` 仍只表示插件 | □通过 / □失败 | `crates/app/src/catalog/{model,mod}.rs`、`crates/app/src/catalog/tests.rs::app_and_runtime_takedown_use_their_own_namespaces` |

最小回归命令：

```bash
cargo test -p skiff-server --test api
cargo test -p skiff-server --test serve
cargo test -p skiff-server
```

上机（R0 全链路，仅测试姿态）：先起 `skiff-server api --state <0700> --authors <0600> --reviewers <0600>`，
用作者钥提交并让审核钥 approve，再在可持有 `SKIFF_SIGNING_KEY` 的环境执行
`skiff-server release --state <dir> --catalog <dir> --staging-out <dir>`；把 staging 交给分发主机后，
现有 Skiff 刷新目录应看到新版本。**生产不得把发布私钥放到分发主机上同机 `--swap`。**

## 附：待核对项结论（2026-11-07 已全部定性）

| 编号 | 结论 | 依据 |
| --- | --- | --- |
| FS-04 | 后端+管理员 UI 已接（第二步） | 回收站接 SQLite `trash_entries`，`/{p}/files/trash` 提供恢复/彻底删除/清空；能力面仍无 `fs.trash` 动词（插件不直接操作回收站） |
| FS-05 | 后端+管理员 UI 已接（第二步） | 目录树配额接 `fs_quotas`，`/{p}/files/quotas` 提供设置/修改/取消/重扫；能力面无 quota 动词 |
| TASK-06 | ☑通过（上机，2026-11-09） | `buffer.push`/`buffer.stat` 在能力面实现并有限速单测；testkit `buffer_probe`/`buffer_stat`/`buffer_push` 上机实测覆盖容量/丢弃/高水位/限速/权限，数字见 G 章「调度器补充实测」 |
| TASK-07 | 有入口 | cron 新建任务落库为 `plugin:com.skiff.cron:<key>`、`source=plugin:<target>`，2026-11-06 实测创建/删除均 200 |
| DOCKER-10 | 运行时入口已接（单文件） | `/$P/docker/compose` 走 `skiff-compose` 解析 + `Docker::execute` 的 `compose.up/down`；不支持 `-f` 多文件叠加，源文件不落盘；`xtask check` 字段子集校验保留 |
| DEPLOY-09 | 部分通过：qemu-user 冒烟，未实机装机 | `file`/`ldd` 显示 ARM aarch64 静态链接，`version`/`--help`/`doctor` 通过（2026-11-09 `2f63aeb`）；复现条件=arm64 Linux + systemd，装包后验启动/建站 |
| DEPLOY-10 | 部分通过：等价镜像实测，未走真实 1Panel 商店流程 | `SKIFF_FORM=docker` 下 healthz 200、登录 303、fs/net/ftp 插件 400 拒装、容器 stop/start 303；文件管理入口已在 `607d09b` 按形态隐藏、页面/深链/API 直达 403 且文案可读 |
| DEPLOY-11 | 因无可复现超时/断网条件阻塞 | 正常下载+摘要校验通过（c39fe7d2…）；代码核对 `tools/iter.sh::cmd_fetch` 单次 `curl --max-time 600`，无重试/结构化错误输出；复现条件=对下载域名注入超时或丢包 |
