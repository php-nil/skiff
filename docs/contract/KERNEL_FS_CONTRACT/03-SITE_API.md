# 与站点的关系、页面展示、API 形态与兼容（§8–11）

> 上级索引：[KERNEL_FS_CONTRACT.md](../KERNEL_FS_CONTRACT.md) ｜ 拆自 `docs/contract/KERNEL_FS_CONTRACT.md` 原 §8–11（2026-11-16 整理，正文未改）。

---

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

- 深链：`/{p}/files`、`/{p}/files/trash`、`/{p}/files/quotas`（`core/files` 及其 L3）；站点详情页的「文件」入口可深链到按 `site=<domain>` 过滤的审计流。配额管理仍是「工具」下的独立二级入口。
- 目录归属按 [`docs/contract/MENU_CONTRACT.md`](../MENU_CONTRACT.md) 的三级模型：文件管理是内核固定二级（稳定键 `core/files`），回收站与配额是其 L3（`core/files/trash`、`core/files/quotas`）；三个页面都由 budding 壳页渲染同一套标签栏标签栏，不引入第二套菜单机制。
- 部署形态：`SKIFF_FORM=docker`（1Panel 受限形态，能力面只有 docker/identity，见 §5.4）不提供内核 fs——「工具 → 文件管理 / 回收站 / 配额管理」入口不渲染，`/{p}/files*` 直达一律 403「当前部署形态不提供文件管理能力」（判定单一来源：`crates/web/src/middleware.rs::check_fs_capability`）。
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
  统一规则见 [`docs/contract/URL_CONTRACT.md`](../URL_CONTRACT.md) §4、§7。
- 管理面写操作同样走内核操作面（不是另一套 `std::fs` 实现），保证锁、原子性、审计一致。

## 11. 迁移与兼容

- 测试期直接新建表，不做旧数据搬迁：`trash_entries` 与 `fs_quotas` 随 schema v14（`CURRENT_SCHEMA_VERSION` 从 13 单调递增到 14）一次性创建；旧库启动时自动升到 v14，`store` 单测覆盖 v13→v14。
- 现有 `audit` 表沿用：回收站/配额操作以 `fs.recycle.*` / `fs.quota.*` 动作写入同一张表，不新建独立审计表，不记录文件内容或敏感路径片段。
- 现有 `fsops` 的 256 KiB、授权根解析、禁止跨 scope 改名、禁止删根全部保留，作为新契约的实现基座。
- `remove` 默认行为从「直接删」改为「进回收站」是**行为变更**：测试期允许直接切换；插件无需改代码，但需在文档/UI 说明删除可恢复。
- `copy`/`stat` 为新增 verb，旧插件不声明即不可用，无兼容问题。
