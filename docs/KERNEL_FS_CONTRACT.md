# 内核文件系统原语契约（FS Contract）

> 状态：已落地（插件侧 `fs` 能力面 + 管理员「文件管理」第一步，2026-11-07；其余管理员操作面按 §3 逐步接入）
> 关联：`docs/KERNEL_SCHEDULER_CONTRACT.md`（任务落盘走本契约）、`docs/SITE_RUNTIME_CONTRACT.md`（站点目录结构）
> 原则：**写入必须经内核**；插件 UI 可以留在外面，但所有宿主文件操作都受授权、审计、回收站与配额约束。

## 0. 一页结论

1. 两套主体：**管理员界面**（无路径限制，全部审计）与**插件**（仅 `path:` 授权根，越界拒绝）。不存在「插件可任意读写宿主」的形态。
2. 内核能力面统一为 `list / read / write / remove / rename / mkdir / copy / stat`，写操作默认**临时文件 + rename 原子替换**，同路径串行、跨路径并行。
3. **删除默认进回收站**：保留原路径元数据，支持恢复与彻底删除；`remove(permanent=true)` 仅管理员可用并单独审计。
4. 审计记录元数据（主体、操作、源/目标、字节数、结果、哈希、request id），**不记录文件内容**；破坏性操作逐条记录，高频小写入按分钟聚合。
5. 容量三层：单次请求（默认 256 KiB，P1 沿用现有上限）、单文件（默认 4 GiB）、每个授权根配额（默认 10 GiB / 可配）。超限返回结构化错误码。
6. 符号链接**永不跟随**：按 lstat 处理；目标规范化后仍须落在授权根内，否则拒绝；硬链接按链接本身移动并记录 `nlink`。
7. 站点目录 `/www/sites/<domain>/{index,log,ssl,conf}` 中，`ssl/`、`conf/`、`log/` 为受保护路径：插件即使被授权也默认只读，写入需额外开关并单独审计。
8. P1 落审核计、回收站、原子写、路径锁、配额与站点归属；站点整体删除进回收站、写前快照为 P2。

## 1. 背景与现状

### 1.1 现状事实

| 事实 | 位置 |
| --- | --- |
| 插件 fs 能力面已有 `fs_list / fs_write / fs_mkdir / fs_remove / fs_rename`，每次操作写审计 | `crates/plugin/src/capability.rs:200-339` |
| 单次读写上限 256 KiB（`MAX_IO`） | `crates/plugin/src/fsops.rs:25-27` |
| 路径解析拒绝相对路径、`..`、NUL；读操作 canonicalize 后检查授权根 | `crates/plugin/src/fsops.rs:82-151` |
| 授权根必须是绝对路径、无 `..`；创建看父目录、读取看目标 | `crates/plugin/src/fsgrant.rs:75-124` |
| 删除禁止删授权根本身；改名禁止覆盖目标、禁止在 `data/` 与宿主路径间搬运 | `crates/plugin/src/capability.rs:263-339` |
| `files` 插件依赖这些 verbs，按 256 KiB 分页读写 | `plugins/files/manifest.toml:1-21`、`plugins/files/src/main.rs:131-187` |
| 内核站点页只有**只读**目录浏览（`?tab=dir`），无写入接口 | `crates/app/src/sites.rs:331-349,770-812` |
| 站点删除是整目录 `remove_dir_all`，没有回收站 | `crates/app/src/sites.rs:2523-2558` |
| 审计表只有 `ts/actor/ip/action/target/result/detail` | `crates/store/src/model.rs:28-38`、`crates/store/src/db/audit.rs:7-24` |
| 无 `copy` / `stat` / 分页流式（仅 offset/max_bytes）/ 路径锁 / 配额 / 回收站 | 全仓搜索无对应实现 |

### 1.2 目标

- 内核作为**唯一宿主写入通道**：管理员页面、插件、内置任务都走同一操作面与审计。
- 删除可恢复、容量可预测、越权可阻断、事故可追溯。
- 插件不再需要（也不允许）直接拿宿主路径句柄写入。

### 1.3 非目标

- 不做通用对象存储、版本控制系统、跨机同步。
- P1 不做站点整体删除回收站与写前快照（P2）。
- 不提供「插件无限制读写」的授权模式；管理员无限制仅限人类管理员会话。

## 2. 授权模型

### 2.1 两类主体

| 主体 | 路径范围 | 审计 | 备注 |
| --- | --- | --- | --- |
| 管理员（Admin 会话） | 宿主任意路径；**内核数据根（`skiff.db`、`data/plugins`、`data/official`）一律硬拒** | 全量、逐条 | 无 `path:` 授权根限制，但数据根不可经文件页读写；**不可关闭审计**；危险操作二次确认 |
| 插件（UDS 调用） | 仅安装时勾选的 `path:` 授权根 + 自身 `data/` | 全量、逐条/聚合 | 越界直接拒绝；不能申请运行时扩权 |

- `path:` 授权根在安装确认页由管理员显式勾选，写入 grants；插件无法在运行时新增。
- 插件自身 `data/` 是隐式授权根（仅限自己的数据目录），不与宿主路径混用；改名禁止跨 scope（沿用现有约束）。
- 管理员页面是内核客户端，不经过插件；它同样走 §3 操作面与 §4 审计，确保「无限制 ≠ 无记录」。**口径澄清（2026-11-07）**：内核持有的是 fs 能力面的管理员客户端，不是把宿主文件业务塞回内核业务层——页面路由与模板归 Web，授权/原子写/锁/审计归 `skiff-fs`。
- **数据根硬拒**是管理面在调用原语前的额外代数：即使管理员主体不受 `path:` 根限制，`skiff.db`、`data/plugins`、`data/official` 也一律拒绝读写与列目录；判定复用 `skiff_plugin::fsgrant::is_protected`，并对软链绕回做 canonical 复核。

### 2.2 为什么「插件无限制读写」不成立

1. **最小权限**：插件是第三方包，签名只证明来源，不证明可信；一旦拥有全盘写，任何漏洞都是宿主级事故。
2. **审计归属**：无限制写入无法回答「谁在什么时候改了哪个站点文件」；授权根 + 主体 id 才能定责。
3. **回收站与配额**：按授权根统计容量、按主体隔离回收站，才能做恢复与限额；全盘写会让两者失效。
4. **站点边界**：`ssl/`、`conf/` 由内核生成，插件误写会直接打断 HTTPS 或 vhost；必须默认只读。
5. **破坏半径**：`remove`/`rename` 在全盘范围等于任意删除；授权根把破坏半径限制在管理员明确勾选的目录。

### 2.3 受保护路径

| 路径 | 默认插件权限 | 说明 |
| --- | --- | --- |
| `/www/sites/<domain>/index/` | 读 + 写（需授权该站点根） | 站点内容，最常见的插件目标 |
| `/www/sites/<domain>/log/` | 只读 | 内核轮转管理；插件写会破坏轮转假设 |
| `/www/sites/<domain>/ssl/` | 只读 | 证书/私钥，误写直接中断站点 |
| `/www/sites/<domain>/conf/` | 只读 | vhost 由内核生成；改写会绕过站点模型 |
| 插件 `data/` | 读写 | 隐式根，仅本插件 |

- 受保护路径写入需 manifest 额外声明 `fs:write-protected`，并在安装确认页单独勾选；每次写入审计 `protected=true`。
- 管理员不受此限，但仍逐条审计并标记 `protected=true`。

## 3. 操作面

### 3.1 操作清单

| 操作 | 参数 | 语义 | 备注 |
| --- | --- | --- | --- |
| `list` | `path, page, page_size` | 目录项（名称/类型/大小/时间/权限/链接目标） | 不递归；分页默认 100，上限 500 |
| `read` | `path, offset, max_bytes` | 字节区间 | P1 单次 ≤ 256 KiB；越界返回实际长度 |
| `write` | `path, offset/append, data/b64` | 原子写或追加 | 默认原子；`append=true` 显式非原子 |
| `remove` | `path, permanent?` | 进回收站或彻底删除 | `permanent` 仅管理员；默认回收站 |
| `rename` | `path, to, overwrite?` | 改名/移动 | 默认不覆盖；`overwrite` 仅管理员 |
| `mkdir` | `path` | 建单层目录 | 沿用现有单层语义 |
| `copy` | `path, to, overwrite?` | 服务端复制（文件/目录） | 新增；目录复制异步任务化（>1000 项） |
| `stat` | `path` | 元数据（大小/类型/模式/属主/链接/哈希可选） | 新增；哈希按需计算，默认不算 |
| `chmod` | `path, mode` | 修改权限位（`0000-7777`） | 新增；**禁止 setuid（4000）**，允许 setgid（2000）；受保护路径拒绝 |
| `chown` | `path, uid, gid` | 修改属主/属组 | 新增；任意 `uid:gid` 允许；uid 0 在审计 `detail` 单独标 `root_owner=true` |
| `archive` | `sources[], dest, format, overwrite?` | 打包（`zip` / `tar.gz`） | 新增；限条目数、源总字节、递归深度；符号链接/特殊文件跳过并回报 |
| `extract` | `archive, dest_dir, overwrite?` | 解压（按扩展名识别 `zip` / `tar.gz`） | 新增；边解边校验绝对路径/`..`/条目数/单条体积/累计字节，超限中止并清理半成品；链接不解出 |

- 所有操作返回结构化结果：`{ok, code?, message?, path?, bytes?, hash?, meta?}`；错误码见 §7.3。
- 所有路径参数必须是**绝对路径**（插件）或管理员选择的路径；相对路径、`..`、NUL、空串一律拒绝。
- 符号链接：`list/stat` 返回 `link_target` 但不跟随；`read/write/copy/remove/rename` 对链接本身操作，不解析目标；若目标在授权根外，拒绝并返回 `fs.escape`。

### 3.2 大文件与流式

- P1 沿用 **256 KiB 分页**（`MAX_IO`），不引入新帧大小；客户端按 `offset` 循环，服务端返回 `size` 与 `eof`。
- `read` 超过 256 KiB 时按 `max_bytes` 截断并返回 `truncated=true`，不静默放大。
- P2 增加 `fs.stream.open/read/write/close`，帧上限 1 MiB，支持断点续传；需要协议版本升级，P1 不做。
- 目录列表分页：`page/page_size`，总数超过 500 时返回 `has_more`；不一次拉全量。

### 3.3 原子写

- 默认流程：同目录创建 `.<name>.tmp-<random>` → 写入 → `fsync(file)` → `rename(tmp, target)` → `fsync(dir)`。
- 读方要么看到旧内容，要么看到完整新内容；不会看到半截文件。
- `append=true` 走 `O_APPEND`，不保证原子替换；审计标记 `atomic=false`，仅管理员与显式授权插件可用。
- 失败时清理临时文件；进程崩溃遗留的 `.tmp-*` 由内核启动清理（>24h 且无活动句柄）。
- 覆盖写前若目标存在且受保护路径，按 §6 可选快照。

### 3.4 并发锁

- 内核维护路径锁表：key 为规范化绝对路径。
- 同路径：读-读并行；写-写、写-读串行（读写锁）。
- 跨路径：并行；`rename/copy` 同时锁源与目标，按规范化路径字典序加锁，避免死锁。
- 锁等待上限 5s，超时返回 `fs.busy`；不无限阻塞请求。
- 目录级操作（删除/复制目录）锁目录本身，不锁全部子项（子项由操作内部顺序处理）。

## 4. 审计

### 4.1 字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | INTEGER | 自增 |
| `ts` | INTEGER | 秒级时间戳 |
| `actor_type` | TEXT | `admin` / `plugin` / `system` |
| `actor_id` | TEXT | 用户名或插件 id |
| `action` | TEXT | `fs.read/write/list/mkdir/remove/rename/copy/stat/chmod/chown/archive/extract/recycle.list/recycle.restore/recycle.purge/recycle.clear/quota.scan/quota.set/quota.cancel` |
| `src_path` | TEXT | 源路径（规范化） |
| `dst_path` | TEXT | 目标路径（改名/复制/恢复） |
| `bytes` | INTEGER | 读/写/复制字节数；目录操作为 0 |
| `result` | TEXT | `ok / deny / fail` |
| `code` | TEXT | 错误码（失败时） |
| `sha256` | TEXT | 写入内容/复制结果的哈希（可选，P1 默认开启写入） |
| `site` | TEXT | 推导出的站点域名（路径命中 `/www/sites/<domain>` 时） |
| `protected` | INTEGER | 是否受保护路径 |
| `request_id` | TEXT | 内核生成的请求 id，贯穿一次插件调用 |
| `detail` | TEXT | 补充说明（不含文件内容） |

- **绝不记录文件内容**，也不记录私钥/证书正文；`sha256` 只用于完整性核对。
- 管理员操作与插件操作同表，靠 `actor_type/actor_id` 区分；插件调用链上的 `request_id` 与审计一致。

### 4.2 聚合与采样

- **逐条记录**：`remove / rename / copy / fs.recycle.restore / fs.recycle.purge / fs.recycle.clear / fs.quota.set / fs.quota.cancel / protected=true` 的写入、以及所有 `deny/fail`。
- **按分钟聚合**：同一 `(actor_type, actor_id, action, src_path)` 的 `read/list/stat` 与普通 `write`，每分钟一行，字段为 `count/sum(bytes)/first_ts/last_ts`，避免日志刷屏。
- 聚合行 `action` 后缀 `#agg`，详情页可展开原聚合窗口；破坏性操作永不聚合。
- 采样上限：单主体每分钟最多 1000 个聚合桶；超出丢弃并记 `audit.sampled_out`。

### 4.3 保留与查询

- 保留 90 天（固定，不采样，保留完整追溯）；由内置任务 `audit-prune` 每日按时间窗删除（`ts < now-90d`），删除条数写任务运行详情。
- 查询接口：管理员页面按时间/主体/动作/站点/结果过滤，分页 50；插件只能查自己 `actor_id` 的记录（P2 可选）。
- 导出：CSV（P2），P1 只提供页面与分页 API。
- 审计写入失败不阻断文件操作（best-effort），但记内核 `tracing::error`；安全事件（deny）写入失败时仍继续拒绝请求。

### 4.4 站点文件插件的审计取舍（2026-11-12）

- 站点级插件（`com.skiff.files`）只申请 `path:/www/sites/<域名>/index`，不申请 `/www/sites` 或 `/`；走能力面时主体固定是 `plugin:<id>`。这是**有意取舍**：用「主体不是管理员」换取「不跨站点授权」，避免一个插件拿到全部站点内容。
- 能力面对 `fs.*` 的审计在 `detail` 追加 `grant=<命中的授权根> site=<域名>`（能从路径推导时）；不新增 DB 列、不改审计表结构。非站点路径不写 `site`。
- 插件侧 `remove` 是**永久删除**，内核页 `remove` 默认进回收站：两者 action 相同，因此插件 UI 文案与审计 `detail` 必须写明「永久删除」，避免被误读为可撤回。

## 5. 回收站

### 5.1 模型

```jsonc
{
  "id": "trash-<ulid>",
  "actor_type": "admin|plugin",
  "actor_id": "admin 或插件 id",
  "original_path": "/www/sites/example.com/index/a.txt",
  "trash_path": "data/trash/<yyyy>/<mm>/<ulid>",
  "kind": "file|dir|symlink",
  "size": 1234,
  "files": 1,
  "mode": "0644",
  "owner": "uid:gid",
  "sha256": "…",
  "site": "example.com",
  "protected": false,
  "quota_root": "/www/sites/example.com",
  "deleted_at": 1760000000,
  "expires_at": 1762592000,
  "nlink": 1,
  "restored_at": null,
  "purged_at": null
}
```

- `size` 是条目总字节数（目录为子树内普通文件大小之和），`files` 是普通文件数；恢复时按这两个值回补配额计数。
- `quota_root` 记录删除时命中的配额根（无规则时为空串），便于审计与迁移核对。
- 落盘位置：SQLite `trash_entries`（schema v14）。它与 `skiff-fs` 的 `TrashStore` 通过 `skiff-app` 的 `DbTrashStore` 适配，`skiff-fs` 本身不依赖 `skiff-store`。

- `remove` 默认把对象**移动到** `trash_path`（同文件系统 rename；跨盘时复制到 `trash_path` 同父临时物 → 原子 rename → 删源，失败清理临时物），并写 `trash_entries`。
- 原路径元数据（父目录、文件名、模式、属主）完整保留，用于恢复。
- 同名冲突：目标已存在时拒绝恢复，返回 `fs.conflict`，不覆盖。

### 5.2 保留、容量与清理

| 项 | 默认 | 说明 |
| --- | --- | --- |
| 保留期 | 30 天 | 到期由内置任务 `builtin:trash-prune`（每日 04:30）清理；配置键为顶层 `trash_retention_days`，环境变量 `SKIFF_TRASH_RETENTION_DAYS`，取值夹在 1–365 |
| 单授权根配额 | 1 GiB | 超过后新删除仍进回收站但按最旧淘汰（先清最旧，再放入） |
| 全局配额 | 10 GiB | 超过后拒绝新删除并返回 `fs.trash_full`（提示彻底删除） |
| 单条目上限 | 2 GiB | 超过走「异步回收站」任务，进度可查；P1 直接拒绝并提示 `permanent` |
| 保留条目数 | 10,000 | 超过按最旧淘汰 |

- 清理与占用统计按 `actor_id` 与 `site` 维度展示；插件卸载（删除数据）时其回收站条目一并清理。
- 回收站目录 `data/trash/` 权限 0711，不通过 Web 暴露；不参与站点备份。
- 清空回收站（`fs.recycle.clear`）仅管理员可用，逐条彻底删除；失败条目保留并在响应里回报原因。

### 5.3 恢复与彻底删除

- `fs.recycle.restore(id)`：检查原路径父目录存在且可写、目标不存在；父目录缺失返回 `fs.not_found` 并带「原路径的父目录已不存在」的可读说明，同名冲突返回 `fs.conflict` 且不覆盖（显式 `overwrite` 才替换）；写审计 `fs.recycle.restore`。
- `fs.recycle.purge(id)`：彻底删除，写审计 `fs.recycle.purge`；仅 Admin 或条目所属主体。
- **受保护路径双路径硬拒**：删除（`remove`）、别名（`rename`）、覆盖写（`write`/`copy`）与恢复（`restore`）都在 `skiff-app` 的 `AdminFs::guard` 与 `skiff-fs` 授权层各拦一次；命中数据根、`plugins/`、`official/`、`skiff.db`，以及内核自身运行配置（`/etc/skiff/config.toml`、`/etc/skiff/**` 的 TLS 材料、自定义 `config_path` 与 `tls_cert/tls_key`）一律返回 403（`fs.denied` / `fs.protected`），旧回收站条目也不能绕过。
- **读取与列目录同样 403（有意设计）**：`fs.list` / `fs.read` 命中同一 `protected_hit` 时也返回 403，避免管理员经 Web 文件管理预览或下载内核运行配置（含监听/证书/密钥路径与令牌）；受保护区不是「只读可见」，需要排障时走宿主 shell 或 `skiff doctor`，不开放网页读。
- **只保护内核运行路径**：保护区按路径组件比较，`/etc/skiff` 不会误伤 `/etc/skiff-other`；`/etc/nginx/nginx.conf`、`/etc/hosts` 等宿主其它配置仍可正常管理。`config_path` 的父目录仅在其专用目录名为 `skiff` 时整体保护，避免把 `/etc` 整体锁死。
- 判定代数单一来源：`skiff-fs` 的 [`protected_hit`]，Web/应用层只注入前缀列表，不另写 `starts_with` 判断。
- 站点删除：见 §9.3（P2 整站进回收站）。
- 与备份的关系：回收站是**短期误删保护**，不替代站点备份；备份任务不读取回收站，清理也不触发备份。

### 5.4 链接、目录与大文件

| 对象 | 策略 |
| --- | --- |
| 符号链接 | 只移动链接本身，不跟随；恢复时重建链接；目标失效不报错 |
| 硬链接 | 按链接移动（`rename` 不复制内容），记录 `nlink>1`；彻底删除只减引用计数 |
| 目录 | 整体 rename 进回收站；恢复整体 rename 回原路径（父目录缺失则拒绝） |
| 大文件 | 单条 ≤ 2 GiB 直接移动；超过拒绝并提示永久删除或走备份 |
| 权限不足 | 拒绝并审计 `deny`，不做「尽力而为」的部分移动 |

## 6. 可选写前快照（P2）

- 目标：受保护路径（`ssl/conf/log`）与站点 `index/` 的覆盖写前，保留上一版。
- 实现建议：写前把目标复制到 `data/snapshots/<site>/<path-hash>/<ts>`，保留 3 版 / 24h；默认关闭，按站点或按路径开启。
- 成本：磁盘约等于写入量的 3 倍；大文件写会显著放大 IO，只对文本类配置默认建议开启。
- 与回收站区别：快照针对**覆盖写**，回收站针对**删除**；两者不合并。
- P1 不实现，但审计中保留 `prev_sha256` 字段占位，便于后续对齐。

## 7. 容量与配额

### 7.1 三层限制

| 层级 | 默认 | 上限 | 错误码 |
| --- | --- | --- | --- |
| 单次请求 | 256 KiB | 1 MiB（P2 流式） | `fs.too_large` |
| 单文件 | 4 GiB | 16 GiB | `fs.file_too_large` |
| 单授权根 | 10 GiB | 可配到磁盘剩余 80% | `fs.quota_exceeded` |
| 回收站（全局） | 10 GiB | 可配 | `fs.trash_full` |
| 目录条目 | 1,000,000 | — | `fs.too_many_entries` |

### 7.1.1 管理员目录树配额（第二步）

- **规则存储**：SQLite `fs_quotas`（schema v16），字段 `root_path / max_bytes / max_files / used_bytes / used_files / truncated / updated_at / scanned_at`。
- **命中规则**：对整个路径匹配已设置的根，**最长祖先根优先**；同一棵树被多条规则覆盖时只按最近的一条判定。两个上限可单独或同时设置，至少一项；全部留空即视为未设置。
- **统计口径**：`used_bytes` = 子树内普通文件大小之和；`used_files` = 普通文件个数；符号链接不跟随，目录本身不计入。
- **刷新口径**：默认是**缓存增量计数**——`skiff-fs` 在每次成功的写/删/改名/复制/恢复后按操作的 `delta_bytes/delta_files` 修正缓存；设置或修改配额、以及管理员点「重新统计」时做**全量重扫**（`scanned_at` 记录时间），成本 O(条目数)，目录越大越慢，页面会二次确认并提示。外部改动（SSH/脚本）不会自动更新计数，需重扫校准。
- **执行点（唯一接线点）**：`skiff-fs` 的 `write`（含 append/定位写）、`copy`、`rename`（跨根时按目标根预检）、`restore`（回补回收站条目用量）在落盘前调用 `enforce_quota`；Web 层不重复判定。超限返回 `fs.quota_exceeded`，消息含配额根、写入后用量与上限。
- **统计截断**：全量重扫的条目数达到 `max_dir_entries` 上限时停止计数并置 `truncated=true`；此时 `used_*` 只是**下界**，API 与页面必须标注「不完整（仅下界）」，写前预检按「未知」放行（宁可放过，不因统计不全误拒写入）。置标记后需重新统计或降低目录规模才会清掉。
- **回收站与配额的关系**：对象移入回收站后**从目录树用量中扣除**（回收站自身由 §5.2 的全局配额单独约束），避免同一份数据被两套上限重复计算；恢复时再回补。
- **取消配额**：清空 `max_bytes/max_files`，保留用量计数；页面不再展示该条目的上限（列表只显示已设置上限的规则）。
- **受保护路径**：数据根、`plugins/`、`official/`、`skiff.db`、内核运行配置与 TLS 材料不允许设置为配额根；`quota_set` / `quota_scan` / `quota_cancel` / `quota_list` 都用同一份前缀列表拒绝/过滤。最长祖先根命中时若命中受保护根，按拒绝处理，不会把配置目录纳入统计。

### 7.2 现有 256 KiB 读限制的调整

- P1 保持 256 KiB/次，不改协议帧大小；读取大文件由客户端循环 offset。
- P2 引入流式后，单帧 1 MiB，单次逻辑读取可到 64 MiB；仍受单文件与配额限制。
- `read` 返回 `size`、`eof`、`truncated`，客户端不需要猜文件大小。

### 7.3 错误码

| code | HTTP/状态 | 含义 |
| --- | --- | --- |
| `fs.denied` | 403 | 无权限/未授权根 |
| `fs.escape` | 403 | 路径越界（含符号链接逃逸） |
| `fs.protected` | 403 | 受保护路径未开写权限 |
| `fs.not_found` | 404 | 路径不存在 |
| `fs.conflict` | 409 | 目标已存在/恢复冲突 |
| `fs.busy` | 423 | 路径锁超时 |
| `fs.too_large` | 413 | 单次请求超限 |
| `fs.file_too_large` | 413 | 单文件超限 |
| `fs.quota_exceeded` | 507 | 授权根配额不足 |
| `fs.trash_full` | 507 | 回收站满 |
| `fs.read_only` | 403 | 目标文件系统只读（宿主 `ProtectSystem`/只读挂载等策略限制） |
| `fs.no_space` | 507 | 宿主磁盘空间不足（`ENOSPC`）或文件系统配额用尽（`EDQUOT`） |
| `fs.unsupported` | 400 | 操作不支持（如跨 scope 改名） |

- 插件侧错误经 UDS 返回结构化 JSON；HTTP 管理面映射到上表状态码，页面内联展示。
- **宿主拒绝≠内部错误**：`EROFS` 映射 `fs.read_only`（403，提示「该路径所在文件系统为只读，无法写入（宿主策略限制）」）；`EACCES`/`EPERM` 归 `fs.denied`（403）；`ENOSPC`/`EDQUOT` 归 `fs.no_space`（507）。只有真正未归类的 I/O 故障才落 `fs.io`（500），避免把环境限制伪装成面板故障，也避免把内部错误降级成 4xx。
- **插件能力面同一口径**：`crates/plugin/src/fsops.rs` 维护独立的 `FsError`（不并入 `skiff-fs`），但状态码与文案语义以本表为准：`EROFS`/`EACCES`/`EPERM`→403，`ENOSPC`/`EDQUOT`→507，未归类→500。两处改动须同步对照，避免同一失败在管理员面与插件面表现不一致。

## 8. 与站点的关系

### 8.1 目录归属

| 路径 | 归属 | 谁能写 |
| --- | --- | --- |
| `/www/sites/<domain>/index/` | 站点内容 | 管理员；被授权该根的插件（默认） |
| `/www/sites/<domain>/log/` | 内核轮转 | 管理员；插件只读 |
| `/www/sites/<domain>/ssl/` | 内核证书 | 管理员；插件只读 |
| `/www/sites/<domain>/conf/` | 内核 vhost | 管理员；插件只读 |
| `data/backups/sites/<domain>/` | 内核备份任务 | 仅内核任务；插件不可写 |
| `data/trash/` | 内核回收站 | 仅内核 |

- 站点域名从路径推导（`/www/sites/<domain>/...`），写入审计自动填 `site` 字段；即使插件被授权的是父目录 `/www/sites`，也按实际路径归属到具体站点。
- 站点文件被插件修改时，审计主体是 `plugin:<id>`，`target` 保留完整路径，详情页可按站点过滤。
- **属主例外**：插件经 `fs` 写宿主路径后，内核默认保留原有属主；唯一例外是路径命中
  `/www/sites/<domain>/index/` 子树（`skiff_model::is_site_index_path`），写完用 `lchown`
  归还统一身份 `skiff-web` 201000:201000（不跟随符号链接）。这样站点内容无论由建站、
  FTP、面板文件管理还是插件写入，`ls -ln` 都呈现同一身份。`log/`、`ssl/`、`conf/`
  不在例外范围，仍保持原属主。

### 8.2 站点页与内核写接口

- P1 站点页文件浏览保持**只读**（现有 `?tab=dir`）；管理员要写，走内核「文件管理」页（`/{p}/files`，第一步已上线）或受控的站点文件编辑接口。
- 新增管理员写接口时必须走本契约操作面，不直接 `std::fs::write`；否则审计与原子性会分叉。

### 8.3 站点删除与回收站

- 现状：删除站点 = 删 vhost + `remove_dir_all` 整站目录（`crates/app/src/sites.rs:2523-2558`），不可恢复。
- P1：保持现有语义，但删除前写一条 `site.delete` 审计（含域名、目录、大小估算）；不做回收站，避免大目录移动拖垮删除流程。
- P2：整站目录移动进回收站（单条目 `kind=dir`），保留 7 天；恢复只恢复目录，**不自动重建站点记录**，管理员需重新建站（域名可能已被占用）；恢复前校验域名空闲。
- 站点删除与备份：删除站点时其 `site-backup` 任务级联删除（调度契约 §3.2），备份产物按保留期保留 7 天后清理，不随站点立即删除。

## 9. 页面与展示

| 页面 | 路由 | 权限 | 内容 |
| --- | --- | --- | --- |
| 文件管理（第一步） | `/{p}/files` | Admin | 目录浏览、文本打开/编辑/保存、新建目录/文件、重命名、删除（移入回收站）；错误按稳定 `code` 内联提示；数据根硬拒 |
| 文件审计流 | `/{p}/fs/audit` | Admin（或 `audit.read`） | 时间/主体/动作/路径/站点/结果/字节/哈希；分页与过滤 |
| 回收站（第二步） | `/{p}/files/trash` | Admin | 「文件管理」的 L3 视图（标签栏「浏览 / 回收站」），不再是独立二级菜单；条目、原路径、大小/文件数、操作者、删除/到期时间；恢复（冲突二次确认覆盖）、彻底删除、清空（二次确认并写清后果） |
| 配额管理（第二步） | `/{p}/files/quotas` | Admin | 已设置配额的目录树、已用/上限、最近统计时间；设置/修改/取消、重新统计（成本提示） |
| 授权根容量 | `/{p}/fs/grants` | Admin（规划中） | 每个 `path:` 根占用/配额/回收站占用 |

- 深链：`/{p}/m/core/files`、`/{p}/m/core/files/trash`（`core/files` 的 L3，旧独立二级深链原样保留）、`/{p}/m/core/fs/audit`、`/{p}/m/core/files/quotas`；站点详情页的「文件」入口可深链到按 `site=<domain>` 过滤的审计流。配额管理仍是「工具」下的独立二级入口。
- 目录归属按 [`docs/MENU_CONTRACT.md`](MENU_CONTRACT.md) 的三级模型：文件管理是内核固定二级（稳定键 `core/files`），回收站是其 L3（`core/files/trash`）；两个页面都由内核渲染同款 `.workbench-tabs` 标签栏，不引入第二套菜单机制。
- 部署形态：`SKIFF_FORM=docker`（1Panel 受限形态，能力面只有 docker/identity，见 §5.4）不提供内核 fs——「工具 → 文件管理 / 回收站 / 配额管理」入口不渲染，`/{p}/files*` 与 `/{p}/m/core/files*` 直达一律 403「当前部署形态不提供文件管理能力」（判定单一来源：`crates/web/src/middleware.rs::check_fs_capability`）。
- 恢复/彻底删除/清空：POST + CSRF + 二次确认；结果内联提示（成功/冲突/父目录缺失/失败条目）。
- 站点文件插件（`com.skiff.files`）保留在「网站」菜单，只服务 `path:/www/sites/<域名>/index`：默认入口为壳页原生 schema（浏览 + 文本编辑/保存），旧完整工作台仅接受显式 `?legacy=1`；全盘管理、上传、回收站与配额仍只在 `core/files`，不提供第二套语义。插件未安装/未启用或当前形态没有 fs 能力时，`/{p}/plugins/com.skiff.files/main` 301 到 `/{p}/files`。

## 10. API 形态

### 10.1 插件侧（能力面 `fs`）

| verb | 说明 | P1 |
| --- | --- | --- |
| `fs:read` | 分页读 | ✅（现有） |
| `fs:write` | 原子写/追加 | ✅（现有，补原子与配额） |
| `fs:list` | 目录分页 | ✅（现有，补分页） |
| `fs:mkdir` | 建单层目录 | ✅（现有） |
| `fs:remove` | 进回收站/彻底删除 | ✅（补 trash） |
| `fs:rename` | 改名/移动 | ✅（现有，补锁与审计字段） |
| `fs:copy` | 服务端复制 | 🆕 P1 |
| `fs:stat` | 元数据 | 🆕 P1 |
| `fs:write-protected` | 受保护路径写 | 🆕 P1（需额外勾选） |
| `fs:stream.*` | 流式读写 | P2 |

- manifest 声明 verbs；未声明即 403 + 审计 `fs.deny`。
- 每次调用带 `request_id`（内核在 UDS 入口生成），审计与错误返回都带同一 id。

### 10.2 管理面

| 路由 | 方法 | 说明 |
| --- | --- | --- |
| `/{p}/files` | GET | 文件管理页（Admin） |
| `/{p}/files/list` | GET | 列目录（`path`） |
| `/{p}/files/read` | GET | 读文本（`path`，上限 256 KiB） |
| `/{p}/files/write` | POST | 原子替换写（CSRF + Admin） |
| `/{p}/files/mkdir` | POST | 新建目录（CSRF + Admin） |
| `/{p}/files/rename` | POST | 重命名/移动（CSRF + Admin） |
| `/{p}/files/remove` | POST | 删除（移入回收站，返回 `trash_id`；CSRF + Admin） |
| `/{p}/files/trash` | GET | 回收站页面 |
| `/{p}/files/trash/list` | GET | 回收站列表 |
| `/{p}/files/trash/restore` | POST | 恢复（`id`、`overwrite`；CSRF/Admin） |
| `/{p}/files/trash/purge` | POST | 彻底删除（`id`；CSRF/Admin） |
| `/{p}/files/trash/clear` | POST | 清空回收站（CSRF/Admin；返回删除数/字节/失败条目） |
| `/{p}/files/quotas` | GET | 配额管理页面 |
| `/{p}/files/quotas/list` | GET | 已设置上限的配额列表 |
| `/{p}/files/quotas/set` | POST | 设置/修改（`root/max_bytes/max_files`；CSRF/Admin） |
| `/{p}/files/quotas/cancel` | POST | 取消上限（`root`；CSRF/Admin） |
| `/{p}/files/quotas/scan` | POST | 全量重扫用量（`root`；CSRF/Admin，O(条目数)） |
| `/{p}/fs/audit` | GET | 审计流 |
| `/{p}/fs/grants` | GET | 授权根容量（规划中） |

- 页面内 JSON 与页面路径同构，不插 `api` 中缀；旧 `/{p}/files/api/*` 以 308 兼容到新路径，
  统一规则见 [`docs/URL_CONTRACT.md`](URL_CONTRACT.md) §4、§7。
- 管理面写操作同样走内核操作面（不是另一套 `std::fs` 实现），保证锁、原子性、审计一致。

## 11. 迁移与兼容

- 测试期直接新建表，不做旧数据搬迁：`trash_entries` 与 `fs_quotas` 随 schema v14（`CURRENT_SCHEMA_VERSION` 从 13 单调递增到 14）一次性创建；旧库启动时自动升到 v14，`store` 单测覆盖 v13→v14。
- 现有 `audit` 表沿用：回收站/配额操作以 `fs.recycle.*` / `fs.quota.*` 动作写入同一张表，不新建独立审计表，不记录文件内容或敏感路径片段。
- 现有 `fsops` 的 256 KiB、授权根解析、禁止跨 scope 改名、禁止删根全部保留，作为新契约的实现基座。
- `remove` 默认行为从「直接删」改为「进回收站」是**行为变更**：测试期允许直接切换；插件无需改代码，但需在文档/UI 说明删除可恢复。
- `copy`/`stat` 为新增 verb，旧插件不声明即不可用，无兼容问题。

## 12. 测试矩阵

| 类别 | 用例 | 期望 |
| --- | --- | --- |
| 越界 | `..`、相对路径、NUL、空串；`/etc` 未授权；符号链接指向授权根外 | 全部 `fs.denied/fs.escape`，审计 deny |
| 符号链接 | 删除链接本身；恢复链接；目标失效 | 只动链接，不跟随；恢复后链接重建 |
| 并发 | 同路径两写；跨路径并行；rename 双向交叉 | 写串行、无半截；无死锁；`fs.busy` 兜底 |
| 原子性 | 覆盖写中断（注入失败）；append；临时文件清理 | 读方只见旧/新；无 `.tmp` 残留 |
| 审计 | 读/写/删/改名/复制/恢复/彻底删除；聚合窗口；request id | 字段完整；破坏性逐条；内容不入库 |
| 回收站 | 保留期清理；配额淘汰；恢复冲突；父目录缺失；硬链接/目录/大文件；清空 | 到期清理；`fs.conflict` 需显式覆盖；父目录缺失可读提示；nlink 记录；>2 GiB 拒绝；清空回报失败条目 |
| 配额 | 单次 >256 KiB；单文件 >4 GiB；授权根超限；目录树字节/文件数超限；最长根优先；取消后不再拦截 | 对应错误码；预检在落盘前生效；消息含配额根与用量/上限；统计与重扫一致 |
| 受保护路径 | 插件写 `ssl/conf/log` 未开开关；管理员写 | 插件 `fs.protected`；管理员成功且 `protected=true` |
| 权限 | viewer 访问审计/回收站；插件跨根；未声明 verb | 403 + 审计 |
| 站点 | 路径推导 site；站点删除审计；P2 整站回收站 | site 字段正确；删除可追溯 |

## 13. 决策点（推荐）

| # | 问题 | 推荐答案 | 理由 |
| --- | --- | --- | --- |
| F1 | 插件是否可申请无限制读写 | **不；仅 `path:` 授权根 + data/** | 最小权限、可审计、可配额；全盘写等于宿主级后门 |
| F2 | 删除是否默认进回收站 | **默认进；`permanent` 仅管理员** | 误删恢复是运维刚需；插件无永久删除权 |
| F3 | 审计粒度 | **破坏性逐条，读写按分钟聚合** | 安全事件不能采样；高频小写入不能刷爆表 |
| F4 | 写入原子性 | **临时文件 + rename 默认；append 显式非原子** | 避免半截配置/脚本；append 语义需显式声明 |
| F5 | 单次读写上限 | **P1 沿用 256 KiB，P2 流式 1 MiB 帧** | 与现有 UDS 帧/客户端兼容；流式需协议升级 |
| F6 | 站点删除是否进回收站 | **P2 整站进，P1 保持永久删 + 审计** | 大目录移动影响删除时延与磁盘；先保 P1 稳定 |
| F7 | 受保护路径插件写入 | **默认只读，需 `fs:write-protected` 单独勾选** | 防止插件误写证书/vhost 直接打断站点 |
| F8 | 回收站配额 | **每授权根 1 GiB，全局 10 GiB，30 天** | 可控磁盘成本；超限淘汰最旧而不是拒绝所有删除 |

## 14. P1 / P2 范围

### P1（必须真跑通）

- 审核计表 `fs_audit` + 逐条/聚合规则 + 查询页；request id、bytes、sha256、site、protected。
- 回收站：`remove` 默认进回收站、恢复、彻底删除、30 天清理、每根 1 GiB/全局 10 GiB 配额。
- 操作面补 `copy`、`stat`、分页 `list`、`read` 的 `size/eof/truncated`。
- 原子写（tmp+rename+fsync）、同路径串行锁、跨路径并行、rename 双锁。
- 配额三层（单次/单文件/授权根）与结构化错误码。
- 受保护路径默认只读 + `fs:write-protected` 开关（管理员勾选）。
- 管理页：审计流、回收站、授权根容量；Admin + CSRF；viewer 403。
- 测试矩阵中除「站点整站回收站」外的全部用例。

### P2

- 站点整体删除进回收站（7 天），恢复只恢复目录、不重建站点记录。
- 流式 1 MiB 帧、断点续传、单次逻辑读取 64 MiB。
- 写前快照（受保护路径/按站点，3 版 24h）。
- 插件侧审计查询、CSV 导出、容量趋势图。
- 目录复制异步任务化与进度（>1000 项）。

## 15. 与现有实现的差距

| 差距 | 现状 | 需要补什么 |
| --- | --- | --- |
| 无回收站 | `remove` 直接删（`crates/plugin/src/capability.rs:263-293`） | `trash_entries` 表、移动/恢复/清理、配额 |
| 无配额 | 仅单次 256 KiB（`crates/plugin/src/fsops.rs:25-27`） | 授权根/文件/回收站配额与预检 |
| 审计字段不足 | `audit` 仅 7 列（`crates/store/src/model.rs:28-38`） | `fs_audit` 新表：bytes/hash/site/protected/request_id；聚合与保留 |
| 无原子写 | `fsops::write_chunk` 直接写 | tmp+rename+fsync；append 显式标记 |
| 无路径锁 | 无 | 同路径读写锁、rename 双锁、`fs.busy` |
| 无 `copy/stat` | 仅 list/read/write/remove/rename/mkdir | 新增 verbs 与实现 |
| 无分页 list | 一次返回目录项（`crates/plugin/src/fsops.rs:265-327`） | page/page_size/has_more |
| 受保护路径未建模 | 授权根内一律可写 | `ssl/conf/log` 默认只读 + 额外开关 |
| 站点页只读 | `?tab=dir` 只读（`crates/app/src/sites.rs:770-812`） | P1 保持；写操作统一走新 fs 管理面 |
| 站点删除不可恢复 | `remove_dir_all`（`crates/app/src/sites.rs:2523-2558`） | P1 加审计；P2 整站回收站 |
| 无 request id | 现有 fs 审计用路径作 target | UDS 入口生成 request id 并贯穿审计/错误 |
| 无写前快照 | 无 | P2 `prev_sha256` + 快照目录 |

## 16. 变更记录

| 日期 | 变更 |
| --- | --- |
| 2026-11-05 | 首版评审稿：授权模型、操作面、审计、回收站、配额、站点归属、测试矩阵 |
| 2026-11-07 | 第一步接线：内核「工具 → 文件管理」页（`/{p}/files`）上线，IO 走 `skiff-fs` 管理员客户端；撤销 `files` 插件菜单入口、旧深链 301；管理员数据根硬拒；回收站/配额 UI 留第二步 |
| 2026-11-10 | §8.1 增加插件写宿主 `sites/<domain>/index/` 后归还统一身份 `skiff-web` 201000:201000 的属主例外；面板文件管理写入同样归还 |
