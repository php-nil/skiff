# 发布面 `/api/**`：角色、提交与审核（§17.1–17.8）

> 上级索引：[SKIFF_SERVER.md](../SKIFF_SERVER.md) ｜ 拆自 `docs/plugin/SKIFF_SERVER.md` 原 §17.1–17.8（2026-11-16 整理，正文未改）。

---

## 17. 发布面 `/api/**` 接口（R0/R1/R2a–R2e 已实施）

> 定位：终态链路「开发者提交 → 管理员审核 → 生成静态产物 → 静态分发」里，**分发面已由 §16 定稿并上机闭环**，
> 本节补**发布面**的接口边界。**R0/R1/R2a/R2b 已在 `tools/skiff-server` 落地**（`api`、`notify` 与 `release` 子命令），
> R2c/d/e 仍是草案。客户端不访问本节任何接口。
> 一句话原则：**客户端只读静态文件，不访问任何 `api`；API 只服务人与发布流水线，且永不持有清单签名私钥。**

### 17.1 与既有口径的关系（含一处措辞冲突）

| 既有条目 | 本节的关系 |
|---|---|
| §16 `serve` 只读静态、`/api/**` 一律 404 | 不变。API 是**独立子命令与独立进程**（`skiff-server api`），独立监听、默认关闭；`serve` 不实现、也不转发任何 API |
| §5.1 源数据 + PR/CI 审核 | 保留为离线/自托管路径。API 只是把「合并 PR」换成一个受控写入入口，落盘仍是同一种源数据格式（`deploy/skiff-server/catalog/`） |
| §14.1/§14.2 签名与 `publish` | 不变。API 不读 `SKIFF_SIGNING_KEY`、不签 manifest、不写 `--dist`；签名与产物生成仍只由 `publish` 在受控环境完成 |
| §0 / §1.2 / §1.4 的分发面表述 | 已对齐：§0「形态」把**现状**（静态分发、`serve` 只读、`/api/**` 一律 404）与**已落地的可选审核 API**（R0，默认关闭）分开写；§1.2/§1.4 明确客户端不依赖 `/api/**` |
| §15 托管未决 | 不受影响：API 与静态托管解耦，换托管不改变本节接口 |

### 17.2 角色、凭据与签名密钥归属

| 主体 | 凭据 | 能做的事 | 私钥位置 |
|---|---|---|---|
| 开发者 | Ed25519 作者密钥对，**与 `.wbpkg` manifest 的签名密钥是同一把** | 提交新版本、查看自己的提交、撤回待审提交 | 开发者本机，永不上传 |
| 审核者（管理员） | Ed25519 审核密钥对，只用于签 API 请求 | 查看待审、通过、驳回 | 审核者本机 |
| 发布流水线 | `SKIFF_SIGNING_KEY`（§14.1） | 生成并签名清单、自检、落盘 | CI secret 或离线发布机，API 进程不可见 |
| Skiff 客户端 | 无 | 只 GET 静态文件（§16.5） | 不适用 |

- **身份即公钥**：主体标识沿用 `key_id = sha256(公钥)` 小写 hex（§3.3 与 [§4.1](../../sections/04-plugin-package.md) 同一口径），服务端只存公钥；没有账号、密码、Cookie 与会话表。
- 注册是**手工配置**而非自助：作者公钥与命名空间前缀写入 `authors.json`，审核公钥写入 `reviewers.json`（均 0600，启动加载）；R0 不提供注册接口。
- **包签名绑定提交者**：上传包内 manifest 的 `key_id` 必须等于请求签名所用的 `key_id`，否则 403。「谁能提交这个包」由既有的包内签名回答，不引入第二套作者身份。
- **三把钥、三种权**：作者钥签包（不进服务端）、审核钥签请求（与包签名无关）、发布钥签清单（只在受控环境）。泄漏影响面与轮换节奏不同，不能合并（§7.2）。
- 吊销与轮换：
  - 作者钥：`authors.json` 标记 `disabled` → 新提交 403；已发布版本按需进 `revoked` 并递增 `seq`（§5.3）。换钥按新作者登记，已发布历史不从索引删除。
  - 审核钥：删白名单条目即可；已执行的审核动作以审计日志为准，不做追溯撤销。
  - 发布钥：按 §14.1 先发内置新公钥的客户端，再切 CI secret。

请求签名（每请求一次，无会话）：

```text
signed = "SKIFF-SERVER-API-v1\0" + method + "\n" + path + "\n" + query + "\n"
       + body_sha256 + "\n" + timestamp + "\n" + nonce
```

- 请求头：`X-Skiff-Key-Id`、`X-Skiff-Timestamp`（Unix 秒）、`X-Skiff-Nonce`（≥128 bit 随机）、`X-Skiff-Signature`（Ed25519 hex）。
- `body_sha256` 为请求体原始字节的 sha256（空体用空串摘要常量）；`query` 按原样参与签名，不重排。
- 时钟窗 ±300 秒（设计目标，待实测）；`nonce` 去重表按小时滚动落 `state/nonces/`，保留 ≥ 10 分钟，重启后仍能拦截窗口内重放。
- 签名域前缀与 §14.2 的 `SKIFF-SERVER-MANIFEST-v1\0` 分开，避免跨协议重放。

### 17.3 提交侧

提交内容（`multipart/form-data`，两个部分）：

**明确选择：只收签名 `.wbpkg` + 条目元数据**，不接受「清单 + 散装 bundle」。包格式、验签与安装链路已由 [§4.1](../../sections/04-plugin-package.md) 定稿，另开一种分发物会让客户端出现两条安装路径。

| 部分 | 内容 | 说明 |
|---|---|---|
| `entry` | 条目元数据 JSON：`summary`、`requires`、`changelog_url`、`license`、`source_url`、目标 `channel` | 与源数据条目同形；不填派生字段 |
| `package` | 签名 `.wbpkg` 原始字节 | 单包上限 64 MiB（设计目标，待实测） |

- **不采信客户端给的派生字段**：`pkg_sha256`、`pkg_url`、`size` 由服务端按字节派生（与 §10.1 `asset` 口径一致）；带上了必须与派生值一致，否则 422。
- 服务端校验顺序（任一失败即 422 或 409，且不落任何状态）：
  1. 包大小与 `sha256`；
  2. `.wbpkg` 结构与验签（复用 `skiff-plugin verify` 语义），`key_id` 必须等于请求者；
  3. manifest schema、`api`、反向域名 `id`、`requires` 合法、`(id, version)` 语义（[§4.1](../../sections/04-plugin-package.md)）；
  4. `id` 是否落在该 `key_id` 的命名空间前缀内（`authors.json`），否则 403；
  5. 版本不可变：同 `(id, version)` 已 `published`、或已在 `pending`/`approved` 且摘要不同 → 409。
- 落状态：`pending`（待审）。作者可在 `pending` 撤回为 `withdrawn`；进入 `approved` 之后不可撤回。
- 记录类型 `kind`：`plugin`（缺省）/`app`/`runtime`。插件记录不落该字段，旧 `state/submissions/*.json` 与旧响应字节不变（§17.12）。
- 幂等与重传：

| 情形 | 语义 |
|---|---|
| 同 `(id, version, sha256)` 重复提交 | 200 + 同一 `submission_id`，状态不变，不重复通知 |
| 同 `(id, version)` 不同 `sha256`，尚未发布 | 409，作者需撤回后重新提交 |
| 已 `published` 的 `(id, version)` | 409，版本不可变（§2.4） |
| 上传中断 | 不做分块续传，整体重传；服务端按摘要判定是否已有相同 blob |

**应用模板提交（R2c，`POST /api/v1/apps`）**：载荷同样是两个部分，但第二段是 compose 原文。

| 部分 | 内容 | 说明 |
|---|---|---|
| `entry` | `id`、`version`、`summary`、`min_kernel`、`images[{ref,digest}]`、`env[]`、`ports[]`、`volumes[]` | 派生字段（`asset`/`compose_sha256`/`compose_url`/`size`）一律由服务端生成，带上即 422（`deny_unknown_fields`） |
| `compose` | 严格子集内的 compose YAML 原文（UTF-8，≤ 1 MiB） | 校验复用 `skiff-compose`，与运行时/xtask 同一实现 |

- 信任模型按裁决 1：**API 请求签名 + 服务端摘要 + 审核**，不要求作者额外签名信封；服务端是提交时刻的信任边界。
- compose 原文落在 `state/submissions/<sid>/compose.yaml`；发布时按 `sha256` 复制成 `catalog/assets/<sha>.yaml` 并作为内容寻址 blob 进入产物（`compose_url = blobs/sha256/<sha>`）。
- `sid` 派生对应用加 `app` 域前缀（`app\0id\0version\0sha256`），与插件同 id 同版本不撞 id。
- 版本规则与插件一致：同 `(id, version)` 不同摘要、或已发布 → 409；同摘要重复提交 → 200 同一 `sid`。
- 审核/发布复用 `submissions/{sid}/approve|reject|withdraw` 与 `release`，事件 `subject.kind = "app"`；一次 `release` 只 +1 `seq`，插件与应用同批进入同一索引。
- **R2c 不做安装**：客户端只从签名索引展示 `apps[]`（商店「应用模板」分区），安装仍走内置目录，索引不驱动安装。

**运行环境提交（R2d，`POST /api/v1/runtimes`）**：JSON 载荷，无二进制资产，只做提交/审核/展示。

| 字段 | 说明 |
|---|---|
| `kind` | `php` / `node` / `rust` |
| `version` | 明文版本号（如 `8.5`） |
| `image` / `image_digest` | 镜像引用与 `sha256:` 摘要；**提交后不可改** |
| `eol` | 可选，`YYYY-MM-DD` |
| `api_min` | 兼容的内核 API 版本 |
| `default` | 提交时必须为 `false`；发布后只能通过元数据修订设置 |

- 身份是 `(kind, version, image_digest)`；同 `(kind, version)` 已有别的 digest、或已发布 → 409。
- **元数据修订**（独立动作）：`POST /api/v1/submissions/{sid}/revise`，body 只接受 `eol` / `default`（`deny_unknown_fields`，带 `image`/`image_digest` 一律 422），仅审核者可调用，只有 `published` 记录可修订。修订先落 `state/runtime_revisions/<id>.json` 并写审计 `action=revise`，由下一次 `release` 应用到源数据并递增 `seq`。
- **`default` 唯一性在 release 时校验**：同 kind 出现多个 `default=true` 时发布失败（`release.failed`），不会生成相互矛盾的索引。
- 运行环境只展示，不驱动安装：客户端从索引 `runtimes[]` 渲染版本/EOL/默认标记，安装仍走内置目录。

### 17.4 审核侧

```text
pending --approve--> approved --release ok--> published
   |                    |
   |reject              |release fail
   v                    v
rejected            approved（保留 last_error，可重试，不发布）
pending --withdraw（作者）--> withdrawn
published --yank/revoke（R1 走 API）--> 条目保留 + 递增 seq
```

- 审核动作只改状态并写入源数据，**不直接改 `--dist`**；产物仍由 `publish` 生成。
- **不得自我审核**：请求者 `key_id` 与提交者相同 → 403（R0 规则，避免单人闭环）。
- 驳回必须带 `reason_code`（`invalid_signature`、`schema`、`id_ownership`、`policy`、`other`）与可选说明；作者按新摘要重提同一 `(id, version)` 允许（尚未发布，不违反版本不可变）。
- `seq` 单调：**API 不预分配 `seq`**。发布 job 在受控环境读线上当前值后 +1（§14.2 的 `--prev` 语义不变），同渠道发布串行；并发审批后到者自然拿到后一个 `seq`。
- 只增不删：索引条目只追加；下架用 `yanked`（条目保留、拒绝新装），安全处置用 `revoked`（摘要黑名单），两者都递增 `seq`（§5.3）。R1 起 `yank` 与密钥吊销走 API：API 只写 `state/takedowns/`，由 `release` 把 `yanked`/`revoked[]` 写进源数据并递增 `seq`。
- **R2e 分 kind 撤包**：`POST /api/v1/plugins|apps|runtimes/{id}/versions/{version}/yank`，审核者调用，body 可带 `reason`；重复撤包 200 幂等，目标不存在 404。
  - 插件保持 R1 语义：源条目 `yanked=true` + `yank_reason`，索引 `plugins[]` 带标记，`revoked[]` 继续只表示插件密钥吊销。
  - 应用与运行环境没有作者密钥：撤包在打 `yanked` 标记的同时，把该版本登记进独立命名空间 `catalog/<ch>/revoked_apps.json` / `revoked_runtimes.json`，索引输出 `revoked_apps[]` / `revoked_runtimes[]`（应用记 compose 摘要、运行环境记镜像 digest 的 hex），`revoked[]` 不参与。
  - 撤包记录 `state/takedowns/*.json` 新增可选 `entry_kind`（缺省 `plugin`，旧记录无需迁移）；`release` 按 kind 写回对应源目录并回写提交的站内提示。
  - 兼容：`revoked_apps[]` / `revoked_runtimes[]` 为空时不落盘（`skip_serializing_if`），插件索引与旧客户端看到的字节不变；客户端忽略不认识的数组。
- 审计：提交、审核、发布结果追加到 `state/audit.jsonl`（JSON Lines：`key_id`、动作、`submission_id`、结果、时间）；这是发布面审计，与内核 `audit` 表无关。

### 17.5 产物生成

审核通过后的唯一写路径（与 PR 路径完全同构）：

```text
approved 提交 → 写入源数据（等价于合并 PR）→ 触发发布 job
             → publish：生成 index + manifest → 受控环境签名 → 内存严格自检
             → 落盘 staging → 按 §14.4 顺序上线（blobs → schema → index → manifest → sig）
             → published（回写提交状态与 seq）
```

- **签名位置：受控环境，不在 API 进程**。API 只产出待签名的源数据与候选索引；私钥仍只在 CI secret 或离线发布机（§14.1）。不新增「在线签名服务」，也不再造一把 API 专用清单密钥。
- **布局与 §16 完全一致**：产物由 `publish` 生成，必然是 `channels/<ch>/{manifest.json,manifest.json.sig,index.json}` + `blobs/sha256/<hex64>` + `schema/`（§16.2），不存在第二套布局。
- **失败不污染**：`publish` 在内存完成全部校验后才写输出目录（§14.2），失败时线上目录不变；上线顺序保证客户端读到的清单永远指向已存在的 blob（§14.4 第 5 步）。`manifest.json` 与 `.sig` 的极小不一致窗口按 §14.3 处理（客户端验签失败保留旧快照）。
- **发布幂等**：发布 job 以 `(channel, submission_id, sha256)` 为幂等键；重试前先检查源数据/线上索引是否已含该摘要，已含则只回写状态，不重复递增 `seq`。
- **没有内容回滚**：`seq` 单调意味着线上内容只能前进；发现坏版本只能 `yanked`/`revoked` 并发布更高 `seq`（§14.4 末条）。「换回旧字节」属于托管层操作，客户端会拒绝低 `seq`。

### 17.6 与 Skiff 客户端的接口边界

- **客户端不依赖 `/api/**`**，一个字节都不需要；依赖点仍是 §16.5 的三跳：`manifest.json` → `index.json` → `blobs/sha256/<hex>`。
- 吊销、yank、新版本提示全部在签名索引里（§2.3 条目字段与 `revoked`），客户端不查询在线服务即可完整工作；离线、内网、纯镜像部署与今天行为一致。
- `serve` 的 `/api/**` 保持 404（§16.2），避免分发节点意外暴露写入面；API 默认关闭、单独监听、单独部署。
- 未来若出现「客户端在线巡检」类需求，必须新开评审，不得把 API 塞进客户端现有拉取路径，也不得让客户端为拉目录而持有任何发布侧凭据。

### 17.7 阶段划分与最小可用集

本节阶段与 §8 的 P0–P4 是**两条独立轴**：§8 是分发与客户端，本节是发布面。为避免与既有编号冲突，本节用 R0/R1/R2。

**R0（最小可用，已实施）**：一名开发者能提交、一名管理员能审核、产物自动生成并被现有 Skiff 拉到。

| 方法 | 路径 | 主体 | 幂等 | 说明 |
|---|---|---|---|---|
| POST | `/api/v1/submissions` | 开发者 | 按 `(id, version, sha256)` | multipart：`entry` + `package`；返回 `submission_id` 与 `state=pending` |
| GET | `/api/v1/submissions?state=pending` | 审核者 | — | 待审列表；开发者调用只返回自己的 |
| GET | `/api/v1/submissions/{sid}` | 提交者或审核者 | — | 状态、派生字段、驳回原因、发布 `seq` |
| POST | `/api/v1/submissions/{sid}/approve` | 审核者 | 按 `sid` | 202；转 `approved` 并触发发布 job |
| POST | `/api/v1/submissions/{sid}/reject` | 审核者 | 按 `sid` | body 带 `reason_code` 与可选说明 |
| POST | `/api/v1/submissions/{sid}/withdraw` | 提交者 | 按 `sid` | 仅 `pending` 可撤回 |

R0 的运维侧（不新增 API）：`authors.json` 与 `reviewers.json` 手工维护；发布 job 用「扫描源数据变更」或「显式触发」二选一（实现时定，语义等价）；状态与审计落 `state/` 文件，不引入数据库（`tools/skiff-server` 刻意保持零原生 C 依赖，§10.1）。

**R1（已实施）**：撤包与密钥吊销 API 化、站内状态通知、审计查询。模板与运行环境仍留 R2。

| 方法 | 路径 | 主体 | 幂等 | 说明 |
|---|---|---|---|---|
| POST | `/api/v1/plugins/{id}/versions/{version}/yank` | 审核者 | 按 `(id, version)` | 202；body 可带 `reason`（1..=200 字符）；重复 200；未知版本/已撤回 404/409 |
| POST | `/api/v1/keys/{key_id}/revoke` | 审核者 | 按 `key_id` | 202；重复 200；未登记 key 404；吊销立即生效 |
| GET | `/api/v1/audit` | 作者或审核者 | — | 查询参数 `sid`/`key_id`/`action`/`result`/`since`/`until`/`limit`/`offset`；作者默认只看自己，显式查他人 403 |

**索引契约（不升 schema）**

- `plugins[].yanked` 与 `revoked[]` 是 `skiff.server.index/v1` 的既有字段；R1 只开始填充，schema 版本不变。
- R1 后续增量新增**可选**字段 `plugins[].yank_reason`（string，1..=200）：旧客户端忽略未知字段，旧索引不出现该字段，因此**仍不升 schema 版本**。有原因时客户端展示原因，无原因时保持通用文案。
- 旧版客户端读到未知字段一律忽略；读到 `yanked=true` 或 `revoked` 命中时，按已有拒绝语义给原因（不是解析失败）。R1 前的旧索引（`yanked=false`、`revoked=[]`、无 `yank_reason`）解析与安装行为完全不变。
- `seq` 单调与「已发布只增不删」继续成立：撤包是给既有条目打 `yanked` 标记并追加 `revoked[]`，**不物理删除**；每次生效都递增 `seq`，客户端防回滚不受影响。

**状态机与边界**

- `published → yanked` 由**审核钥**触发，不可逆；恢复只能发更高版本，不提供 `unyank`。
- 密钥吊销（`revoked`）按 `key_id` 一次性覆盖该 key 的历史版本：源条目 `yanked=true` 且追加 `revoked[]`；`revoked` 不可逆，也不能再审——该 key 之后既不能提交也不能审核，换钥按新作者重新登记。
- 已有安装者不自动卸载；`GET /api/v1/submissions/{sid}` 与内核插件页从索引的 `yank_reason` / `revoked[].reason` 读取撤包原因并提示（无原因时用通用文案）。
- 撤销/吊销只写 `state/` 记录，产物与签名仍由 `release` 在受控环境生成。

**通知的最小形态**

不引入 SMTP、webhook 或第三方依赖：撤包/吊销立即落 `state/audit.jsonl`，同时把状态回写到提交记录 `takedown` 并在插件页展示。通知失败只影响可见性，不影响撤包生效；同一 `(id, version)` 重复请求幂等，不重复产生记录。确需外推时再新增可重试的 webhook，R1 不做。

**审计查询**

- 数据源是 `state/audit.jsonl`，倒序返回；`limit` 默认 50、上限 200，`offset` 分页，返回 `total` 与 `next_offset`。
- 过滤：`sid`（匹配 `submission_id` 或 release 行的 `submission_ids[]` 任一元素，可串起 submit / approve / release）、`key_id`、`action`、`result`、`since`/`until`（UTC RFC3339）。
- 鉴权：审核者看全部；作者默认只看自己，显式查他人 `key_id` 一律 403。
- 返回字段只有 `at`/`key_id`/`action`/`submission_id`/`submission_ids`/`result`，**不含签名、包内容或请求体**。

**R2（分步实施）**：R2a webhook 事件通知、R2b 提交记录 `kind` 抽象、R2c 应用模板提交/审核/发布、R2d 运行环境提交与元数据修订、R2e 分 kind 撤包与独立吊销命名空间已实施（§17.11–§17.15）。裁决：apps 接受「API 请求签名 + 服务端摘要 + 审核」，不要求额外签名信封；compose 校验保持单一实现；runtimes 只做提交/审核/展示，不驱动安装；`eol`/`default` 允许独立元数据修订（image/digest 不变）；吊销用独立 `revoked_apps[]`/`revoked_runtimes[]`。组织级命名空间与发布 job 队列化仍不做。

统一错误体：`{"code": "...", "message": "...", "detail": {...}}`；`401` 签名缺失/无效/过期，`403` 密钥未登记、命名空间不匹配或自我审核，`409` 版本或摘要冲突，`413` 超限，`422` 校验失败，`500` 内部错误（不回显路径与堆栈）。

### 17.8 明确不做

- 账户体系（注册、登录、找回、邮箱验证）、多租户、组织与团队、细粒度权限；主体只有「已登记公钥」。
- 计费、付费内容、订阅、授权码。
- Web 控制台与前端页面；R0 只提供 HTTP 接口，管理界面后置。
- 客户端侧在线 API、遥测上报、在线 CRL/OCSP（吊销仍走签名索引，§7.3）。
- API 进程内签名、API 专用清单私钥、把 `SKIFF_SIGNING_KEY` 放进 API 环境。
- 包体 CDN/对象存储管理、评分评论举报等 UGC。
