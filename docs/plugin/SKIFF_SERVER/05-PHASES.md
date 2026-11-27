# P1–P3 落地状态（§11–13）

> 上级索引：[SKIFF_SERVER.md](../SKIFF_SERVER.md) ｜ 拆自 `docs/plugin/SKIFF_SERVER.md` 原 §11–13（2026-11-16 整理，正文未改）。

---

## 11. P1 落地状态（客户端只读接入）

P1 只到「看得见、校验过、失败可回退」：内核拉取渠道清单并在「插件 → 商店」只读展示
插件 / 应用模板 / 运行环境 / 升级通道四类条目，**不提供安装按钮**，不改动既有插件安装与
TOFU 信任链路。默认关闭。P2 在此基础上补了插件安装入口（见 §12）。

### 11.1 配置

`/etc/skiff/config.toml` 新增 `[catalog]` 段（环境变量 `SKIFF_CATALOG_*` 同义）：

| 键 | 默认 | 说明 |
|---|---|---|
| `enabled` | `false` | 关闭时不拉取、不显示商店入口；页面路由也返回 404 |
| `channel` | `stable` | 只允许 `stable` / `night` |
| `mirrors` | `["http://10.66.52.7:8080"]` | 镜像基地址，**按优先级回落、不合并**；允许 `http://` / `https://`（可带域名/IP/端口/路径前缀），其它协议启动失败。显式 `[]` 表示不联网、只用本地快照 |
| `public_key` | 空 | 固定 Ed25519 公钥（64 位十六进制）；未配置时拒绝一切已签名渠道 |
| `allow_unsigned` | `false` | 仅开发模式（`--dev` / loopback 明文 / `tls_enabled=false`）可开，生产形态启动失败 |
| `timeout_secs` | `10` | 单次 HTTP 请求的总超时（连接 + 读响应），上限 120；**建连超时固定 3 秒且不超过该值** |
| `ca_bundle` | 空 | 额外信任的 CA 根（PEM 文件路径，可含多张证书）；空值 = 只用内置根。文件不存在/格式非法则启动失败 |

- 默认值就是「本机内网镜像」，不做留空引导；端口若实测不同只改这一处默认值。
- 合并顺序：环境变量 > 配置文件 > 默认值。**文件里显式写了 `mirrors` 就原样生效**，默认值只补「没写过这一项」的实例（例如测试机已有的 `https://10.10.15.4:8443` 不会被覆盖）。
- 多源只按顺序回落，不跨镜像合并条目；换镜像列表**不清空本地快照**，新源不可达时继续展示上次成功快照（§4.4）。
- 允许 HTTP 不等于放松信任：无论 `http` / `https`，都必须验签、校验索引摘要、拒绝 `seq` 倒退。
- `ca_bundle` 只对 `https` 镜像有意义（追加信任锚），HTTP 镜像不需要它。

内置公钥常量 `skiff_app::catalog::verify::BUILTIN_PUBLIC_KEY` 目前为空：仓库不放真实私钥，
因此部署侧必须显式配置 `public_key` 才能接受签名渠道。

### 11.2 缓存与状态

缓存根 `<data_dir>/catalog/`：

```text
catalog/
  channels/<channel>/{manifest.json,manifest.json.sig,index.json}
  blobs/sha256/<hex>          # P1 不主动下载，仅本地已有 blob 时复核
  meta.json                   # 已见 seq、ETag/Last-Modified、上次成功时间、来源与写入时的公钥指纹
```

页面状态（`CatalogState`）四档：`已更新`（200 + 验签通过）、`目录未更新`（304 或 TTL 内直接复用
缓存）、`离线`（全部镜像不可达，展示上次成功快照）、`校验失败`（验签/防回滚/摘要/结构
失败，**拒绝新清单并保留旧快照**）。

**缓存 TTL（面板自管，不依赖 HTTP 304）**：

| 内容 | 有效期 | 说明 |
|---|---|---|
| JSON 类（`manifest.json` / `.sig` / `index.json`） | **1 天** | 以 `meta.json` 的 `fetched_at` 判定，进程重启后同样生效；未过期时页面直接复用快照、不联网 |
| `blobs/sha256/*` | **永久** | 内容寻址，摘要即身份；不做时间淘汰 |
| 拉取失败（离线 / 校验失败） | **5 分钟** | 失败不顶满一天，5 分钟后自动刷新可重试 |

- 「刷新目录」按钮**强制刷新**：同时绕过 TTL、失败冷却与镜像指数退避，立刻重试。
- **默认源不可达**：面板照常启动并展示本地快照，提示「镜像源不可达，正在使用本地缓存」；
  失败冷却 5 分钟内页面加载不会重复请求，不产生错误风暴。
- **静默丢包也按失败处理**：单次建连超时 3 秒、单请求总超时默认 10 秒，一次刷新中所有镜像
  共享 10 秒总预算（`FETCH_BUDGET`）；超时与连接被拒走同一条离线路径，页面始终返回 200，
  不会把框架页面预算耗成 504。超时同样进入 5 分钟冷却。
- **不可达且无缓存**：提示「尚未连上任何镜像源，也没有本地缓存」，并引导到
  「设置 → 商店目录」填写镜像地址，不呈现空白页或裸错误串。
- 自动刷新（页面加载）在 TTL 内不发请求；TTL 到期后按 §4.4 的镜像退避选择目标，全部退避时直接展示缓存。
- 缓存写入失败不影响本次展示，但下次仍会重新拉取（不把「写不进去」误当成「已缓存」）。

换 `public_key` 后热缓存立即作废：`meta.json` 记录写入时生效的公钥指纹（`key_id =
sha256(公钥)`），启动或载入缓存时与当前配置不一致即丢弃该缓存。丢弃等价于「没有缓存」——
新清单生效前不展示旧条目、不允许凭旧条目安装，且条件请求头为空（忽略 ETag）强制重拉。

### 11.3 校验口径

- 先 `manifest.json` 原文验签：`key_id == sha256(公钥)`、签名文件 `key_id` 与清单一致、
  签名对象为 `SKIFF-SERVER-MANIFEST-v1\0` + manifest 原始字节。
- 索引必须与 manifest 声明的 `sha256` / `size` 一致，且 `channel` / `seq` 相同。
- `seq` 小于磁盘上已见值时拒绝（防回滚），不覆盖缓存。
- 条目摘要三件套（`*_sha256` / `*_url` / `size`）必须同时存在且 URL 为 `blobs/sha256/<sha>`；
  本地已存在 blob 时复核大小与 sha256。
- 拒绝与离线都落审计：验签 / 防回滚 / 摘要失败记 `catalog.refresh`（`result = fail`，
  `detail` 前缀 `reason=signature|seq|digest`），全部镜像不可用记 `reason=offline`；审计写失败
  只记日志（`audit_or_log` 口径），不改变拒绝语义。

### 11.4 代码落点

| 路径 | 内容 |
|---|---|
| `crates/app/src/catalog/model.rs` | 清单/索引模型与字段校验 |
| `crates/app/src/catalog/fetch.rs` | `Transport` 抽象 + ureq（HTTP/HTTPS）实现 + 测试假传输层 |
| `crates/app/src/catalog/verify.rs` | Ed25519 验签、公钥解析与 `key_id` |
| `crates/app/src/catalog/store.rs` | 缓存原子读写与 `meta.json` |
| `crates/app/src/catalog/mod.rs` | `CatalogService`：按镜像回退、条件请求、状态机 |
| `crates/web/src/handlers/store.rs` + `crates/web/src/handlers/ui/store.rs` | 只读商店数据（`/ui/v1/store`，登录即可访问），页面由 budding 壳渲染 |
| `crates/app/src/menu.rs` | 仅 `enabled` 时追加「插件 → 商店」菜单项 |

### 11.5 已覆盖测试

`crates/app/src/catalog/tests.rs`：验签失败拒绝并保留旧快照、`seq` 回退拒绝、索引摘要不符拒绝、
本地 blob 大小/摘要不符拒绝、条件请求 304 复用缓存、离线回退、未签名默认拒绝且显式允许后放行、
多镜像按序回退、自动刷新跳过退避中的镜像、全部镜像退避时直接吃缓存不发请求、手动刷新绕过
退避并在镜像恢复后清零失败计数、TTL 内不重拉且强制刷新绕过 TTL、失败 5 分钟后才重试、
进程重启后未过一天的磁盘缓存不联网、默认关闭时不触网、离线提示「镜像源不可达，正在使用本地缓存」、
无缓存时给出「设置 → 商店目录」指引、**静默丢包在总预算内失败并进入冷却**、
**多镜像共享一个总预算**、四类拒绝/离线各自断言 `catalog.refresh`
审计落库、换钥后旧缓存
作废并忽略 ETag 强制重拉、换钥且离线时不展示旧条目且 `plugin_package` 返回 `None`。
`crates/web/src/handlers/store.rs` 单测覆盖商店页最少渲染
（状态提示、四列数据、空态、无安装入口）；`crates/web/tests/main_flow/plugins.rs` 覆盖
「镜像静默丢包（accept 后不回包）时商店页仍 200、提示降级、冷却内不再建连」。`crates/app/src/catalog/fetch.rs` 端到端覆盖自定义
CA：自签 CA 签发的服务端证书在未导入 CA 时被拒、导入后建连成功、CA 文件缺失/损坏返回可读错误。
`crates/core/src/config.rs` 单测覆盖默认关闭、默认镜像为内网 HTTP、**显式 `mirrors` 不被默认值覆盖**、
http/https 均放行、
非法协议/非法渠道/非法公钥/零超时被拒、显式空 `mirrors` 被尊重、`allow_unsigned` 仅开发模式放行、
`ca_bundle` 空值不改行为且缺失/损坏时启动失败。

### 11.6 未完成

安装链路（插件/应用模板/运行环境安装与升级）、blob 预下载与本地缓存淘汰、真实签名流水线与
内置公钥、镜像同步脚本、增量索引。

## 12. P2 落地状态（从商店目录安装插件）

P2 只补「插件」这一类：已验证目录条目可在商店页直接进入安装；Docker 应用模板与运行环境
仍只展示，不做安装。信任口径与既有上传安装完全一致——目录清单的固定公钥只证明「条目可信」，
插件包本身仍走 `.wbpkg` 验签与 TOFU/固定公钥（`plugin::trust::resolve`），权限确认页仍是
独立一道闸，最终安装复用 `install_package` 与 `plugin.install` 审计。

### 12.1 安装链路

```text
商店页「安装」→ 权限判定（PluginManage）→ 目录条目兼容性预检
  → 按条目摘要下载 blob（`blobs/sha256/<sha>`）→ 校验大小 + sha256
  → 写 `<data_dir>/plugins/.incoming/<token>.wbpkg`（0600，`.part` 原子 rename）
  → `resolve_trust` 验签 → 权限确认页 → POST /plugins/confirm → install_package
```

- 下载与校验在 `skiff_app::commands::store::prepare`（阻塞线程池），页面与审计在
  `crates/web/src/handlers/store.rs`；确认页直接复用既有模板与 `/plugins/confirm`。
- 任一步失败都删除暂存文件及 `.part`，不存在「已下载未装」或「半装」残留。
- 放弃权限确认页会遗留暂存包：`.incoming` 有 24 小时 TTL，进程启动与每次 `prepare` 前各扫一次，
  删除超期的 `.wbpkg` / `.part`（目录内其它文件不动，删除失败只留痕）；未过期的暂存不受影响。
- 安装入口只在 `[catalog].enabled = true` **且**配置了固定公钥时出现；未配置公钥时
  按钮隐藏，直连路由返回 400。

### 12.2 安装前兼容性校验

| 维度 | 判定 |
|---|---|
| 内核 API | 条目 `api` 主版本必须等于 `KERNEL_API`，次版本不得高于内核（当前 `1.3`） |
| 能力面 | 条目 `requires` 必须全部在 `Capability::available_in(form)` 内 |
| 版本 | 低于已装版本拒绝（同版本重装、较高版本升级放行） |
| 撤回 | `yanked = true` 或索引 `revoked` 命中该摘要即拒绝 |

### 12.3 拒绝语义与审计

统一动作 `store.install`，`detail` 前缀 `reason=<code>`：

| 原因码 | 场景 | 页面表现 |
|---|---|---|
| `permission` | 当前角色无 `PluginManage` | 403 错误页 |
| `no_key` | 未配置固定公钥 | 400 错误页 |
| `not_found` | 已验证目录无该条目（未更新/被拒/版本不存在） | 回商店页提示 |
| `revoked` | 条目被撤回 | 回商店页提示 |
| `compatibility` | 内核 API / 能力面 / 降级不满足 | 回商店页提示 |
| `download` | 网络、HTTP 非 200、传输层错误 | 回商店页提示 |
| `digest` | 条目缺三件套、大小或 sha256 不符 | 回商店页提示 |
| `signature` | 包结构/签名失败，或公钥未导入 | 回商店页提示 |
| `io` / `store` | 写暂存或读库失败 | 回商店页提示 |

成功暂存记 `result = ok`（`reason=prepared`，含版本/渠道/来源），最终安装仍由
`plugin.install` 记录一次。

### 12.4 代码落点

| 路径 | 内容 |
|---|---|
| `crates/app/src/catalog/mod.rs` | `plugin_package`（已验证索引取条目）与 `download_blob`（大小/摘要校验、原子落盘、失败清理） |
| `crates/app/src/catalog/fetch.rs` | `Transport::get_blob`（二进制放宽到 128MiB，目录 JSON 仍 8MiB） |
| `crates/app/src/commands/store.rs` | `prepare`：兼容性预检、下载、验签、暂存；`StoreInstallError` 原因码 |
| `crates/web/src/handlers/store.rs` | 插件分区的安装入口、权限判定与 `store.install` 审计、确认页渲染 |
| `crates/web/src/handlers/store.rs`（`load_view`） | 可安装性投影（仅插件分区；撤回/告警条目不显示安装），壳页按 `installable` 渲染按钮 |

### 12.5 已覆盖测试

- `crates/app/src/commands/store.rs`：整链路（含 `install_package` 收尾）、下载失败 / 摘要不符 /
  验签失败 / 能力面缺失四类拒绝、内核 API 不符、未配置公钥不触网、条目不存在、`api` 兼容矩阵；
  每类失败都断言 `.incoming` 无残留。暂存 TTL：23h 不删 / 25h 只删 `.wbpkg`/`.part`、
  `prepare` 前清掉 25h 遗留且不误删刚下载的包。`crates/core/src/bootstrap.rs`：启动时清理 25h
  遗留、保留未过期暂存。
- `crates/web/src/handlers/store.rs`：有公钥时渲染安装入口、无公钥/告警条目不渲染。
- `crates/web/tests/main_flow/plugins.rs`：viewer 403 且落 `store.install` deny 审计、未配置公钥 400 并落
  `no_key` 审计、目录不可用回商店页并落 `not_found` 审计且无暂存文件。

### 12.6 未完成

应用模板与运行环境安装、blob 本地缓存与淘汰、内置公钥（发布侧签名已接，见 §14）、
增量索引、商店内的升级/回滚操作。
