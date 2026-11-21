# 一页结论、背景、授权模型与操作面（§0–3）

> 上级索引：[KERNEL_FS_CONTRACT.md](../KERNEL_FS_CONTRACT.md) ｜ 拆自 `docs/contract/KERNEL_FS_CONTRACT.md` 原 §0–3（2026-11-16 整理，正文未改）。

---

> 状态：已落地（插件侧 `fs` 能力面 + 管理员「文件管理」第一步，2026-11-07；其余管理员操作面按 §3 逐步接入）
> 关联：`docs/contract/KERNEL_SCHEDULER_CONTRACT.md`（任务落盘走本契约）、`docs/contract/SITE_RUNTIME_CONTRACT.md`（站点目录结构）
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

### 2.1 三类主体

| 主体 | 路径范围 | 审计 | 备注 |
| --- | --- | --- | --- |
| 管理员（Admin 会话） | 宿主任意路径；**内核数据根（`skiff.db`、`data/plugins`、`data/official`）一律硬拒** | 全量、逐条 | 无 `path:` 授权根限制，但数据根不可经文件页读写；**不可关闭审计**；危险操作二次确认 |
| 插件（UDS 调用） | 仅安装时勾选的 `path:` 授权根 + 自身 `data/` | 全量、逐条/聚合 | 越界直接拒绝；不能申请运行时扩权 |
| 内核自身（`system`） | 宿主任意路径（含数据根与 `/etc` 下的系统配置） | 全量、逐条（`actor=system`） | 它是授出方，不需要授权根；但**没有「悄悄写」的权限**——同样经 `Fs` 实例、同一张审计表（2026-11-16） |

- `path:` 授权根在安装确认页由管理员显式勾选，写入 grants；插件无法在运行时新增。
- 插件自身 `data/` 是隐式授权根（仅限自己的数据目录），不与宿主路径混用；改名禁止跨 scope（沿用现有约束）。
- 管理员页面是内核客户端，不经过插件；它同样走 §3 操作面与 §4 审计，确保「无限制 ≠ 无记录」。**口径澄清（2026-11-07）**：内核持有的是 fs 能力面的管理员客户端，不是把宿主文件业务塞回内核业务层——页面路由与模板归 Web，授权/原子写/锁/审计归 `skiff-fs`。
- **数据根硬拒**是管理面在调用原语前的额外代数：即使管理员主体不受 `path:` 根限制，`skiff.db`、`data/plugins`、`data/official` 也一律拒绝读写与列目录；判定复用 `skiff_plugin::fsgrant::is_protected`，并对软链绕回做 canonical 复核。
- **`fs` 能力面不支持通配授权**：`fs:*` **不匹配任何方法**，插件必须逐条持有 `fs:read` / `fs:write` / `fs:list` / `fs:mkdir` / `fs:remove` / `fs:rename` / `fs:copy` / `fs:stat` / `fs:chmod` 等具体授权。理由：
  1. `fs` 的每个方法都对应宿主文件的一次真实副作用，`fs:*` 等价于「授权根内任意写入」，与 §2.2 的最小权限、审计归属、配额三条同时冲突；
  2. 安装确认页按方法逐条展示，管理员才能看清「这个插件能删/能改权限」；一条通配会让确认页失去判别力；
  3. 现行实现中 `plugin::capability::has_perm` 就是精确匹配——本条规定是把既有行为写成契约，不改变任何运行时判定。
  （对照：`ftp` / `firewall` 允许 `<cap>:*`，因为它们的动作集合固定且都不触碰任意路径。判定入口统一为 `skiff_model::Grant::permits` / `permits_wildcard`。）

**内核自身也是主体，不是例外。** 站点 vhost、证书、`daemon.json` 这些写入由内核发起，没有「谁是授权方」的问题，但同样必须留痕——否则「谁在什么时候改了系统配置」就答不上来。落点是 `AdminFs::write_kernel` / `remove_kernel`（`crates/app/src/admin_fs.rs`）：

- 走**同一个** `Fs` 实例：同一套临时文件 + rename 原子替换、同一份路径代数、同一张 `audit` 表；
- 主体固定 `Principal::admin("system").with_protected_write(true)`：内核要写自己的 `data/`，因此带受保护写开关——**这是 `system` 独有的**，管理员与插件都拿不到；
- `remove_kernel` 一律 `permanent=true`：系统配置删掉就删掉，进回收站只会让「删了但还在」误导排障；
- 业务模块**不得**直接 `std::fs::write` / `remove_file` / `rename`；`cargo run -p xtask -- check-channel` 把这条做成 CI 闸门（2026-11-21 基线 96 处 / 13 个文件，只减不增）。

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
