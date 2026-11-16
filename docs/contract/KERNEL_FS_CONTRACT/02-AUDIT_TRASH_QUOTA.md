# 审计、回收站、写前快照与配额（§4–7）

> 上级索引：[KERNEL_FS_CONTRACT.md](../KERNEL_FS_CONTRACT.md) ｜ 拆自 `docs/contract/KERNEL_FS_CONTRACT.md` 原 §4–7（2026-11-16 整理，正文未改）。

---

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
