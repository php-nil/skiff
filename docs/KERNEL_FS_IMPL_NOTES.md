# 内核文件系统原语：分析与实现说明

> 对应契约：[`KERNEL_FS_CONTRACT.md`](KERNEL_FS_CONTRACT.md)。
> 原始范围：底层能力库 `crates/fs`（包名 `skiff-fs`），不接插件能力面、不改插件 / Web 路由 / SQLite 表。
> 2026-11-07 追加：第一步管理面接线完成——`skiff-app::admin_fs::AdminFs` 把 `skiff-fs` 接成管理员侧文件服务，Web 提供 `/{p}/files` 页面与 JSON API；回收站/配额仍为内存实现，UI 留第二步。
> 2026-11-08 追加：第二步落地——回收站接 SQLite `trash_entries`，配额接 `fs_quotas`（schema v14），新增 `/{p}/files/trash` 与 `/{p}/files/quotas` 页面/API；删除默认进回收站，内置任务 `builtin:trash-prune` 每日清理过期条目。详见 §9。
> 状态：独立库 + 单元/集成测试已落地；页面/API 已接线，`skiff-fs`/`skiff-store`/`skiff-app`/`skiff-web`/`skiff` 门禁测试与 clippy 均已通过，尚未部署上机。

## 0. 一句话结论

契约的授权模型、操作面、审计分级、回收站与三层配额都可以按现状落地，但有三处必须降级实现或在接线时补强：

1. **「永不跟随符号链接」在纯 `std::fs` API 下无法做到零窗口**：Unix 用 `O_NOFOLLOW` 收口最后一段，父目录链只能「先 `symlink_metadata` 逐段校验、再操作」，存在 TOCTOU 残留窗口；真正零窗口需要 `openat` 系列（`openat2`/`O_PATH`）或 fd 相对操作，属于接线阶段的增强项。
2. **跨文件系统 rename 不是原子操作**：`remove` 进回收站、`rename`、`copy` 跨盘时只能退化为「复制 + 删除」，中途失败可能留下源与目标各一份（不会产生半文件，但会短暂双份）。
3. **回收站每根 1 GiB 是软配额**：超限时按最旧淘汰而非拒绝；全局 10 GiB 才是硬拒绝。契约文字同时写了「超过后新删除仍进回收站」和「超限淘汰最旧」，实现以「淘汰」为准。

## 1. 实现补充结论

> 授权模型、操作面、审计字段、回收站、配额与 API 的规范以 [KERNEL_FS_CONTRACT.md](KERNEL_FS_CONTRACT.md) §2–§10 为准；本节只记实现取舍与必须补强的点。

### 1.1 落地有坑、需要接线阶段补强的点

- **TOCTOU（符号链接）**：`resolve` 是「先检查、后操作」。Unix 最后一段用 `O_NOFOLLOW` 关闭了主要缺口，但父目录链的检查与实际 `open` 之间仍可被替换；Windows 无 `O_NOFOLLOW`，只能二次 `symlink_metadata`。
  - 插件是主要威胁面；管理员路径没有授权边界，风险等级较低。
  - 根治方案：Unix 改为 `openat(O_PATH|O_NOFOLLOW)` 逐段下钻，后续操作全部用 fd 相对 API；Windows 用句柄 + `GetFinalPathNameByHandle` 复核。
- **跨文件系统**：`EXDEV` 时 `move_object` 退化为复制 + 删除。契约允许该降级，但无法保证「要么全有要么全无」；需要在审计 `detail` 标记 `crossed_fs=true`（已做）。
- **回收站与配额**：契约要求「包含回收站占用」，但回收站与授权根通常不在同一棵树下。实现把配额键记为 `quota_root`（授权根字符串），删除时从该根扣减、恢复时加回；真实 SQLite 实现需保证同一口径，否则统计会漂移。
- **目录删除/复制的锁粒度**：契约只锁目录本身，不锁子项。并发写子项期间删除目录会出现「写成功但随即被删」或删除失败；这是契约明确接受的语义，需在页面上说明。
- **`remove` 语义变更**：默认进回收站是行为变更；旧调用方若依赖「删完即释放空间」，需要显式传 `permanent`（仅管理员）。
- **站点整站删除**：契约 P1 明确保持 `remove_dir_all` + 审计，不进回收站；本轮未实现站点感知，属于 P2。
- **写前快照**：契约 §6 明确 P2，本轮未实现。

## 2. crate 结构与公开 API

```
crates/fs/
  Cargo.toml            # skiff-fs：serde + sha2；cfg(unix) 额外 libc
  src/
    lib.rs              # 模块声明与公开 re-export
    error.rs            # FsError + code()/http_status()/audit_result()
    subject.rs          # Subject / Principal / Scope / 授权根 canonical 化
    path.rs             # normalize_absolute / resolve / site_of / protected_subdir
    lock.rs             # PathLocks：键控读写锁 + 超时
    audit.rs            # AuditEvent / AuditSink / AggregatingSink / MemoryAuditSink
    trash.rs            # TrashEntry / TrashStore / MemoryTrashStore / 淘汰选择
    config.rs           # FsConfig / TrashConfig / QuotaTracker / MemoryQuotaTracker
    hash.rs             # sha256_hex
    ops/
      mod.rs            # Fs 门面 + 请求/结果类型 + list/stat/read/mkdir
      io.rs             # 不跟随链接的打开、原子写、删除、大小/元数据
      copy.rs           # copy_object / move_object（含跨盘回退）
      mutate.rs         # write/remove/rename/copy/restore/purge/prune
  tests/
    support/mod.rs      # 临时目录夹具 + 内存审计/回收站/配额
    authorization.rs    # 越界、`..`、符号链接、受保护路径、非 UTF-8
    ops.rs              # 分页、哈希、原子写并发、rename 死锁、权限位
    trash.rs            # 回收站往返、配额淘汰、过期清理、跨主体
    audit.rs            # 字段、无内容、聚合
```

### 2.1 主体与授权

```rust
pub enum Subject { Admin { user: String }, Plugin { id: String } }

pub struct Principal { /* subject, grants, data_root, allow_protected_write */ }

impl Principal {
    pub fn admin(user: &str) -> Self;
    pub fn plugin(id: &str, grants: Vec<PathBuf>, data_root: Option<PathBuf>) -> Self;
    pub fn with_protected_write(self, allow: bool) -> Self;
    pub fn root_of(&self, path: &Path) -> Option<&PathBuf>; // 最长匹配根
}
```

### 2.2 路径解析

```rust
pub enum PathMode { Existing, ExistingLink, Create, CreateLink }
pub enum PathKind { File, Dir, Symlink, Other, Missing }

pub struct ResolvedPath {
    pub path: PathBuf,           // lexical 归一后的绝对路径
    pub kind: PathKind,
    pub grant_root: Option<PathBuf>,
    pub site: Option<String>,
    pub protected: bool,
}

pub fn normalize_absolute(raw: &str) -> Result<PathBuf, FsError>;
pub fn resolve(p: &Principal, raw: &str, mode: PathMode, site_root: &str)
    -> Result<ResolvedPath, FsError>;
```

### 2.3 服务门面

```rust
pub struct Fs { /* cfg, locks, audit, trash, quota */ }

impl Fs {
    pub fn new(cfg: FsConfig, audit: Arc<dyn AuditSink>,
               trash: Arc<dyn TrashStore>, quota: Arc<dyn QuotaTracker>) -> Self;

    pub fn list(&self, p: &Principal, ctx: &OpContext, req: &ListRequest) -> Result<ListPage, FsError>;
    pub fn stat(&self, p: &Principal, ctx: &OpContext, req: &StatRequest) -> Result<StatInfo, FsError>;
    pub fn read(&self, p: &Principal, ctx: &OpContext, req: &ReadRequest) -> Result<ReadChunk, FsError>;
    pub fn mkdir(&self, p: &Principal, ctx: &OpContext, raw: &str) -> Result<MkdirOutcome, FsError>;

    pub fn write(&self, p: &Principal, ctx: &OpContext, req: &WriteRequest) -> Result<WriteOutcome, FsError>;
    pub fn remove(&self, p: &Principal, ctx: &OpContext, raw: &str, permanent: bool) -> Result<RemoveOutcome, FsError>;
    pub fn rename(&self, p: &Principal, ctx: &OpContext, from: &str, to: &str, overwrite: bool) -> Result<RenameOutcome, FsError>;
    pub fn copy(&self, p: &Principal, ctx: &OpContext, from: &str, to: &str, overwrite: bool) -> Result<CopyOutcome, FsError>;

    pub fn restore(&self, p: &Principal, ctx: &OpContext, trash_id: &str, overwrite: bool) -> Result<RestoreOutcome, FsError>;
    pub fn purge(&self, p: &Principal, ctx: &OpContext, trash_id: &str) -> Result<u64, FsError>;
    pub fn prune_trash(&self, ctx: &OpContext) -> Result<u64, FsError>; // 系统主体
}
```

`WriteMode::{Replace, Append, At(offset)}`：`Replace` 原子；`Append` 显式非原子；`At` 契约未单列，按定位写实现并在审计 `detail` 标注。

### 2.4 审计与回收站抽象

```rust
pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);
    fn flush(&self) {}
}

pub trait TrashStore: Send + Sync {
    fn insert(&self, entry: TrashEntry);
    fn get(&self, id: &str) -> Option<TrashEntry>;
    fn remove(&self, id: &str) -> Option<TrashEntry>;
    fn mark_restored(&self, id: &str, ts: i64) -> Option<TrashEntry>;
    fn mark_purged(&self, id: &str, ts: i64) -> Option<TrashEntry>;
    fn list(&self) -> Vec<TrashEntry>;
    // 默认实现：total_usage / usage_for_root / expired
}

pub trait QuotaTracker: Send + Sync {
    fn usage(&self, root: &Path) -> u64;
    fn adjust(&self, root: &Path, delta: i64);
}
```

## 3. 测试结果

- `cargo test -p skiff-fs --locked`：**57 passed, 0 failed**
  - 单元 19（审计聚合 4、配额 1、哈希 1、锁 3、路径 3、主体 3、回收站 4）
  - 集成 38（audit 3、authorization 11、ops 14、trash 10）
- `cargo clippy -p skiff-fs --all-targets --locked -- -D warnings`：**0 warning / 0 error**
- `cargo fmt -p skiff-fs -- --check`：**通过**
- 说明：`cargo fmt --all` 当前被并行会话的 `crates/sched/src/scheduler/tests.rs`（文件尚不存在）阻断，与 `crates/fs` 无关；因此本轮以 `cargo fmt -p skiff-fs` 验证。

覆盖的契约测试矩阵项：越界（含 `..`、相对路径、NUL）、父目录符号链接逃逸、末段符号链接不跟随、硬链接只删链接、并发写无半文件、rename 双向换名不死锁、复制条目/深度上限、回收站恢复/过期/配额淘汰/跨主体拒绝、审计聚合与不记内容、非 UTF-8 列表跳过、Unix 权限位保留。

Windows 环境限制：本机沙箱无法对符号链接执行 `rename`，因此「删除链接只进回收站」的断言仅在 Unix 执行；语义本身由 `symlink_metadata` + `remove_file` 保证。

## 4. 与契约的差异

| # | 契约 | 实现 | 原因 |
| --- | --- | --- | --- |
| 1 | 「符号链接永不跟随，无 TOCTOU」 | Unix 末段 `O_NOFOLLOW`；父链为预检 + 二次校验 | 纯 `std::fs` 无法零窗口；根治需 fd 相对 API |
| 2 | `append` 仅管理员与显式授权插件 | 未单独鉴权，任何有写权限的主体可用 | 与 `write` 共用授权链路；如需收紧应在接线层加 verb 判定 |
| 3 | `fs:copy` 目录 >1000 项异步任务 | P1 直接 `fs.too_large` | 契约 §14 把异步化列为 P2 |
| 4 | 回收站每根 1 GiB「超过后仍进回收站」 | 超限先淘汰最旧再放入；全局超限才拒绝 | 契约 §5.2 同时要求淘汰，取可执行语义 |
| 5 | `remove` 目录整体 rename | 同盘 rename；跨盘复制 + 删除 | 契约 §5.1 已允许跨盘降级 |
| 6 | 审计 `id` 自增、保留 90 天 | 本轮无落库；内存 sink 无 id | 明确不接 SQLite；字段已预留 |
| 7 | 错误码表未列 `fs.io` | 新增 `fs.io`（500） | 承载真实 I/O 错误，避免误映射为 `fs.not_found` |
| 8 | §7.1 有 `fs.too_many_entries`，§7.3 表未列 | 已实现该码（413） | 以 §7.1 为准，建议回填 §7.3 |
| 9 | 管理员「危险操作二次确认」 | 未实现（UI 层职责） | 本 crate 无路由/页面 |
| 10 | 站点删除 P1 审计 `site.delete` | 未实现 | 站点模型不在本 crate，属 app 接线 |
| 11 | 配额「异步刷新 ≤60s」 | 同步内存计数 | 真实实现由 SQLite/后台刷新替换 |
| 12 | `prev_sha256` 快照占位 | 未加字段 | P2；避免无消费者的占位字段 |

## 5. 接入现有 capability 与 SQLite 的 trait 边界

### 5.1 能力面（`crates/plugin`）

- 现有 `fsops` 继续负责 UDS 编解码与 `path:` 授权解析；新代码只替换**执行层**：
  - 入口：把 `path:` grants 与插件 `data/` 转成 `Principal::plugin(id, grants, Some(data_root))`。
  - 每个 verb 映射到一个 `Fs` 方法；`request_id` 从 UDS 入口生成，放进 `OpContext`。
  - 错误：`FsError -> {code, http_status}` 直接复用，UDS JSON 字段与契约 §10.1 对齐。
  - `fs:write-protected` 勾选状态映射到 `allow_protected_write`。
- 不要在新 crate 里再实现一遍授权：`crates/model` 的 `path` 代数仍是单一来源；`Principal` 的 canonical 化只做形式统一。

### 5.2 SQLite（`crates/store`）

需要三个实现（均在接线会话中新增，不在本轮）：

1. `SqliteAuditSink: AuditSink`：`record` 写 `fs_audit`；`AggregatingSink` 包在外层，`flush` 由定时器每分钟调用。
2. `SqliteTrashStore: TrashStore`：`insert/get/remove/mark_* / list` 映射 `trash_entries`；`expired` 可用 `expires_at <= now` 索引查询。
3. `SqliteQuotaTracker: QuotaTracker`：`usage` 读缓存表；`adjust` 增减；写入后复核由调用方触发。

建议表结构（仅边界，不在本轮建表）：

```
fs_audit(id INTEGER PK, ts, actor_type, actor_id, action, src_path, dst_path,
         bytes, result, code, sha256, site, protected, request_id, detail, count)
trash_entries(id TEXT PK, actor_type, actor_id, original_path, trash_path, kind,
              size, mode, owner, sha256, site, protected, deleted_at, expires_at,
              nlink, restored_at, purged_at, quota_root)
fs_quotas(quota_root TEXT PK, used_bytes INTEGER, updated_at)
```

### 5.3 接线顺序（建议）

1. **只读接入**：`list/stat/read` 走 `Fs`，先不带回收站/配额；用现有插件跑回归，确认路径与错误码兼容。
2. **审计落地**：`SqliteAuditSink` + `AggregatingSink`，后台每分钟 `flush`；先把 `fs_audit` 跑通再加页面。
3. **写路径切换**：`write/mkdir/rename/copy` 切到 `Fs`；`append` 保留语义标记。
4. **回收站**：`SqliteTrashStore` + `remove` 默认进站；加 `trash-prune` 删除任务；最后接管理页。
5. **配额**：`SqliteQuotaTracker` 先做「启动扫描 + 写后增量」，再做写前预检；观察误差后再决定是否 60s 刷新。
6. **管理面**：`/files`（第一步已接：浏览/编辑/新建/改名/删除）、`/fs/audit`、`/fs/trash`、`/fs/grants` 路由复用同一 `Fs`，禁止另写 `std::fs`。
7. **强化**：Unix 改 fd 相对操作；Windows 句柄复核；最后再评估 P2 流式与快照。

## 6. 风险清单

| 风险 | 等级 | 说明 / 缓解 |
| --- | --- | --- |
| 符号链接 TOCTOU 残留 | 高（插件主体） | 预检 + `O_NOFOLLOW`；接线阶段改 fd 相对 API 才能根治 |
| 跨盘 move 非原子 | 中 | 复制后校验 `size/sha256` 再删；审计标记 `crossed_fs` |
| 回收站配额口径漂移 | 中 | `quota_root` 必须与 `SkiffStore` 的 grant 字符串完全一致；建议单测固定 |
| 目录锁粒度粗 | 中 | 契约接受；删除大目录时可能有并发写失败，需要页面提示 |
| `remove` 默认语义变更 | 中 | 旧调用方需显式 `permanent`；发布说明必须写清 |
| Windows 符号链接能力 | 低（部署目标是 Linux） | 测试跳过/限制；生产以 Linux 为准 |
| 管理员全盘写 | 高但可控 | 数据根（`skiff.db`、`plugins`、`official`）已在 `AdminFs::guard` 硬拒；其余路径仍全盘可写，靠逐条审计 + `request_id` 追溯；UI 危险操作二次确认已接删除 |
| 内存审计/回收站实现非持久 | 低（历史） | 审计接 `DbAuditSink` 落 `audit` 表；回收站/配额已由第二步替换为 SQLite（`trash_entries` / `fs_quotas`），内存实现仅保留给 `skiff-fs` 单测 |
| 配置默认值未实测 | 低 | 256 KiB / 4 GiB / 10 GiB / 1 GiB / 30 天均沿用契约设计目标，不宣称已实测 |

## 7. 原始轮次不做（历史边界）

- 不修改 `crates/plugin`（含 `fsops.rs`）、`crates/store`、Web 路由与模板。
- 不建 `fs_audit` 表（`trash_entries` / `fs_quotas` 已由第二步在 schema v14 建立；不新建独立 fs 审计表）。
- 不实现站点整站回收站、写前快照、流式读写、目录复制异步任务。
- 不部署、不提交、不推送。

## 8. 第一步管理面接线（2026-11-07）

### 8.1 接线位置

| 层 | 文件 | 职责 |
| --- | --- | --- |
| 应用装配 | `crates/app/src/lib.rs` | `AppState` 持有 `AdminFs`，提供 `admin_fs()` 访问器 |
| 管理客户端 | `crates/app/src/admin_fs.rs` | 管理员主体调用原语；数据根硬拒；审计落 `audit` 表；256 KiB 编辑上限 |
| Web 页面/API | `crates/web/src/handlers/files.rs` | `/{p}/files` 与 `/files/api/{list,read,write,mkdir,rename,remove}`；Admin + CSRF |
| 模板/静态 | `templates/files.html`、`static/files.css`、`static/files.js` | 独立样式全部挂 `.files-page`，不改 `skiff.css` |
| 菜单/深链 | `menu.rs`、`handlers/plugins.rs` | 新增内置 `core/files`；过滤 `com.skiff.files` 插件菜单；旧深链 301 |

### 8.2 语义与边界

- **数据根硬拒**：`AdminFs::guard` 先做 lexical 校验，再对已存在目标 / 待创建目标父目录 canonical 化，最后复用 `skiff_plugin::fsgrant::is_protected`；软链绕回同样拒绝。`list` 时用 `hides_listed_entry` 过滤受保护子项。
- **编辑上限**：读、写都限制 256 KiB；超限文件只读并禁用保存，避免「读到一半再保存」截断原文件。二进制（非 UTF-8）返回 `fs.unsupported`，页面提示并保留原编辑内容。
- **写语义**：`write` 走 `WriteMode::Replace` 原子替换；`rename` 默认不覆盖；`remove` 第一步 `permanent = true`（永久删除），第二步改 `false` 接 SQLite 回收站，**路由与页面不需要改**。
- **审计**：`DbAuditSink` 把 `AuditEvent` 压成 `detail` JSON 写现有 `audit` 表（`actor=用户名`、`ip=web`、`action=fs.*`），本轮不建 `fs_audit` 表；回收站/配额的持久化仍在第二步。
- **菜单退役**：`menu::build` 与 `nav_plugins` 同时过滤 `com.skiff.files`；旧深链 `/{p}/plugins/com.skiff.files/main` 返回 **301** 到 `/{p}/files`（注意 axum `Redirect::permanent` 是 308，已手写 301）。
- **错误提示**：API 统一 `{ok:false,error:{code,message}}`，HTTP 状态取 `FsError::http_status()`；前端按 `code` 显示且保存失败不清空 textarea。

### 8.3 第二步落地结果

- 回收站：`AdminFs::remove` 改为 `permanent=false`，接 `DbTrashStore`（SQLite `trash_entries`）；新增 `/{p}/files/trash` 页面与 `files/api/trash[/restore|/purge|/clear]` API。
- 配额：`DbQuotaTracker` 替换 `MemoryQuotaTracker`，`/{p}/files/quotas` 展示已设置上限的目录树与用量；写前预检与写后增量均由 `skiff-fs` 完成。
- 审计仍写现有 `audit` 表，动作统一 `fs.recycle.*` / `fs.quota.*`；未建 `fs_audit` 表。

## 9. 第二步实现（2026-11-08）

### 9.1 适配层放在 `skiff-app`

`skiff-fs` 只依赖 trait，`skiff-store` 不反向依赖 `skiff-fs`；适配实现在 `crates/app/src/admin_fs.rs`：

| trait | 适配器 | 落点 |
| --- | --- | --- |
| `TrashStore` | `DbTrashStore` | `trash_entries`（`trash_insert/get/live/remove/mark_restored/mark_purged`） |
| `QuotaTracker` | `DbQuotaTracker` | `fs_quotas`（`fs_quota_list/get/set/set_usage/adjust/set_limits/clear_limits`） |

- `DbTrashStore::list` 按 `deleted_at ASC` 返回存活条目；`trash_list` 再按主体过滤，管理员可见全部。
- `DbQuotaTracker::limit_for` 每次读库，按「最长祖先根」匹配；`adjust_stats` 使用 `MAX(0, used + delta)` 防止计数下溢。

### 9.2 回收站链路

1. `remove` → `skiff-fs` 计算 `object_stats`（不跟随符号链接）→ 按 `quota_target` 从目录树扣减 → `remove_to_trash`（跨盘退化为复制+删除）→ `DbTrashStore::insert`。
2. `restore` → 读条目 → 父目录存在性检查（缺失返回 `fs.not_found` 可读消息）→ 目标冲突检查（不覆盖，除非 `overwrite`）→ 受保护路径再判一次 → 落回原位置 → 回补配额 → `mark_restored`。
3. `purge` → 删物理对象 → `mark_purged`；`clear` 逐条处理并收集失败原因，返回 `ClearTrashOutcome{removed, bytes, failed}`。
4. 自动清理：内置任务 `builtin:trash-prune` 每日 04:30 调 `AdminFs::trash_prune`，按 `expires_at` 删除过期条目。

### 9.3 配额执行点

- 预检在 `skiff-fs::ops` 内完成，Web/应用层不重复判定：
  - `write`：按目标根计算 `delta_bytes/delta_files`（覆盖写先减旧文件）；
  - `copy`：只按目标根预检，覆盖文件/链接时减旧目标；
  - `rename`：跨配额根时按目标根预检；同根移动不额外拦截；
  - `restore`：按恢复条目回补，覆盖恢复时减旧目标。
- 统计缓存：成功写后 `adjust_stats` 增量修正；`quota_set` 与 `quota_scan` 做全量重扫并写 `scanned_at`，成本 O(条目数)。
- 回收站对象不计入目录树用量（移入时扣减、恢复时回补），避免与回收站全局配额重复计算。
- 保护：`AdminFs::guard` 在 `quota_set/cancel/scan/list` 前执行，数据根 / `plugins/` / `official/` / `skiff.db` 不可设为配额根；列表侧过滤旧数据。

### 9.4 内核运行配置保护（加固）

- `FsConfig.protected_paths` 为唯一前缀列表，由 `AdminFs` 在构造时注入；判定函数是 `skiff-fs::path::protected_hit`（按路径组件比较，含前缀本身）。
- `AdminFs::kernel_protected_paths(data_dir, config_path, tls_cert, tls_key)` 组装：
  - 数据根、`data/plugins`、`data/official`；
  - `config_path` 文件本身；
  - `config_path` 父目录仅在目录名为 `skiff` 时整体保护（默认 `/etc/skiff`）；
  - 默认 `/etc/skiff` 目录（即使 config 被自定义，FTP/服务配置仍在此）；
  - `tls_cert` / `tls_key` 文件（自定义 TLS 路径也不会漏）。
- 每个前缀同时保留 canonical 与 lexical 两种形态：`AdminFs::guard` 用 canonical 比对已存在目标，`skiff-fs` 操作用 lexical 路径做纵深防御。
- 执行点：`write`/`copy` 的目标、`remove` 的目标、`rename` 的源与目标、`restore` 的原路径；命中返回 `fs.protected`（HTTP 403）。`AdminFs` 早拦返回 `fs.denied`（同为 403），消息含「内核运行配置或数据目录受保护」与命中根。
- 不锁 `/etc`：只加 `/etc/skiff` 及显式文件；`/etc/nginx/nginx.conf` 等仍可删除/改名/覆盖。
