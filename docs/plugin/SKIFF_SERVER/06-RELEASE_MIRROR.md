# 签名发布、镜像同步与托管来源（§14–15）

> 上级索引：[SKIFF_SERVER.md](../SKIFF_SERVER.md) ｜ 拆自 `docs/plugin/SKIFF_SERVER.md` 原 §14–15（2026-11-16 整理，正文未改）。

---

## 13. P3 落地状态（内核升级只读段）

P3 只做「提示 + 下载 + 摘要校验」，**不执行升级**：不停服、不替换包、不跑数据迁移。
升级执行仍走 [DEPLOY_TEST.md](../../test/DEPLOY_TEST.md) 的手工流程；本阶段的产出只是把可信的升级包
安全地落到本地暂存，供人工复核后再按部署文档操作。

### 13.1 升级提示

- 数据来源不新增传输：复用 §11 的 `CatalogService` 已验证快照（同一渠道、同一镜像、
  同一缓存与 `seq` 防回滚），只读快照里的 `upgrades` 条目。
- 当前版本取 `skiff_app::commands::upgrade::KERNEL_VERSION`，与 `AppState.version` 同源
  （workspace 统一版本）。候选条目按以下条件过滤后取最高版本：
  - `arch == 宿主机`（`x86_64` / `aarch64`，不沿用 `amd64`/`arm64` 口径）；
  - `channel == 索引渠道`（stable 清单不会出现 night 条目）；
  - `version > 当前版本`。
- 渠道清单没有 `upgrades`、没有匹配条目、目录被拒或离线无缓存时，页面安静显示
  「无可用升级」，不报错、不弹失败提示；商店页的「升级通道」只读列表保持原样。
- 页面展示当前版本、架构、渠道、可用版本与 `min_from`（最低来源版本），并明确标注
  「只下载、不执行升级」。`min_from > 当前版本` 不隐藏条目，但页面仍以最低来源版本提示。

### 13.2 手动下载与校验

- 入口：旧表单 `GET /{prefix}/store/upgrades/{version}/download` 继续保留（无 JS 兜底）；
  budding 商店页走 `POST /ui/v1/store/upgrades/{version}/download`（仅 `Admin`，写路径先过
  `x-csrf-token`）。两条入口共用 `store::apply_upgrade_download` 的目录启用/公钥判定、
  暂存、sha256 校验与 `upgrade.download` 审计，不另起一套业务。
- 版本与摘要只取自已验证索引：条目必须同时具备 `pkg_sha256` / `pkg_url` / `size`，且
  `pkg_url == blobs/sha256/<sha>`；缺任一或 URL 与摘要不一致按 `digest` 拒绝。
- 落盘 `<data_dir>/upgrade/incoming/skiff-<version>-<arch>.tar.gz`：先写 `.part`，
  字节数与 sha256 都通过才 rename；任何失败删除 `.part` 与目标文件，不留半包。
- 目标文件已存在时复核大小与 sha256：一致直接复用（`reused=true`，不再发 blob 请求）；
  不一致视为暂存被截断/替换，丢弃后重下。
- 升级包不单独签名，签名锚点是渠道 `manifest.json`（§9 决策 4）；因此未配置固定公钥时
  直接拒绝下载（`no_key`），不提供「先下后验」。
- 拒绝降级与同版本重下：目标版本不高于当前版本即 `downgrade`。
- 暂存 TTL：`<data_dir>/upgrade/incoming` 与插件 `.incoming` 同口径保留 24h，进程启动与
  每次下载前各扫一次；只删 `*.tar.gz` 与 `*.part`，刚落的包不会被误删，每个清掉的条目
  留一条带路径与年龄的日志。

### 13.3 拒绝语义与审计

审计动作 `upgrade.download`，与商店安装同口径 `reason=<code>`：

| 原因码 | 触发 |
|---|---|
| `permission` | 非 Admin 触发下载（deny） |
| `no_key` | 未配置固定公钥（deny） |
| `disabled` | 商店目录未启用（路由 404，命令层兜底） |
| `not_found` | 已验证目录没有该版本/架构条目（含渠道未发布升级数据） |
| `downgrade` | 目标版本不高于当前版本 |
| `download` / `digest` / `io` | 网络或 HTTP 失败、大小/摘要不符、写暂存失败 |

成功落 `ok`，detail 含
`reason=staged channel=... source=... sha256=... size=... path=... reused=...`。
失败回商店页提示，页面文案带 `[<reason>]` 前缀，便于与审计对照。

### 13.4 代码落点

- `crates/app/src/catalog/mod.rs`：`UpgradePackage`、`latest_upgrade` / `upgrade_package`
  （只从 `Fresh` 或可用缓存快照取条目）、`download_upgrade`（与插件包共用 `download_entry`）。
- `crates/app/src/commands/upgrade.rs`：`KERNEL_VERSION`、`host_arch`、拒绝分类、
  暂存路径、已有文件复核、失败清理与 `INCOMING_TTL` 过期清扫。
- `crates/core/src/bootstrap.rs`：启动时清理 `<data_dir>/upgrade/incoming`。
- `crates/web/src/handlers/store.rs`：商店页「内核升级」面板、下载处理器与
  `upgrade.download` 审计。
- `crates/web/templates/store.html`：升级提示与下载入口；无数据时只渲染空态文案。

### 13.5 已覆盖测试

- `crates/app/src/catalog/tests.rs`：无升级数据返回 `None`（安静空态）、按渠道/架构/当前版本
  选最高条目、当前版本不旧于候选时返回 `None`。
- `crates/app/src/commands/upgrade.rs`：缺公钥不触网、降级/同版本拒绝、成功暂存并校验、
  二次下载复用不再请求 blob、远端摘要不符不留半包、本地暂存被篡改后丢弃重下、24h TTL
  只清过期暂存且不误删新包。
- `crates/web/src/handlers/store.rs`：升级面板空态与有可用版本时的下载入口渲染。
- `crates/web/tests/main_flow.rs`：viewer 403 并落 `upgrade.download` deny 审计、缺公钥 400
  并落 `no_key` 审计、无升级数据回商店页并落 `not_found` 审计且无暂存文件。

### 13.6 未完成

升级执行段（停服、替换包、数据迁移、回滚与健康检查）、已暂存包在页面上的可见状态与人工
清理入口、暂存配额（TTL 已按 24h 落地）、升级包与内核二进制的一致性校验、内置公钥
（发布侧签名已接，见 §14）、发布后的线上目录运维。

## 14. 签名发布与镜像同步（已实现）

服务端产物这条链现在是一条可重复执行的命令路径：`publish`（生成 → 签名 → 自检 → 落盘）与
`sync`（主站 → 镜像）。上传动作本身仍取决于托管方案（§15 未决），因此刻意不固化到命令里。

### 14.1 签名密钥托管

- 私钥形态是 32 字节 Ed25519 种子（64 位十六进制）。**只从环境变量 `SKIFF_SIGNING_KEY` 读取**，
  CI 里映射为 secret；命令没有 `--sign-key` 一类参数，私钥不会出现在进程表、shell 历史或仓库中。
- 仓库不保存任何私钥或密钥派生材料；`tests/` 里的固定种子（如 `11`/`22`/`44` 重复）只用于验证
  签名接口的确定性，其公钥与生产信任根无关，也不参与任何发布。
- 缺 `SKIFF_SIGNING_KEY`、值为空白、非十六进制、不是 32 字节，`publish`/`gen` 都立即失败，
  错误里指明环境变量名与格式要求；**不会退化成一份未签名但看起来正常的清单**。
- 固定公钥来自 `SKIFF_SERVER_PUBLIC_KEY` 或 `--public-key`。发布时若由种子派生的公钥与它不一致
  直接失败——这种产物客户端一定拒绝，提前失败比上线后才发现更省事。
- 轮换：先把新公钥随客户端发布（内置常量或部署配置），再切换 CI secret。客户端只接受
  `key_id == sha256(固定公钥)` 的清单，因此切换必须与客户端发版同步；私钥泄漏按 §5.3 走
  `revoked` 摘要黑名单并递增 `seq`。

### 14.2 `publish`：生成 → 签名 → 自检 → 落盘

1. `build` 生成索引与 manifest，签名覆盖 `SKIFF-SERVER-MANIFEST-v1\0` + manifest 原始字节。
2. **落盘前**在内存做严格校验：schema、`key_id`、真实验签、index 摘要/大小/`seq`、引用 blob 的
   摘要与大小、禁止孤儿 blob。
3. 提供 `--prev` 时比较同渠道 `seq`，必须严格递增；不提供视为首次发布。
4. 全部通过才写输出目录（写前清空）。

任一步失败都非零退出且不改动输出目录：不会出现「目录已生成、只是命令报错」的半成品。
`gen` 保留给本地演练；真签名同样要求环境变量，`--unsigned` 必须显式声明并在终端告警。

### 14.3 `sync`：主站 → 镜像

主站可以是 `https://<base>`，也可以是本地产物目录（离线搬运/演练）；单对象上限 256MiB。
同步固定六步，任何失败的错误都带 `步骤 N/6` 前缀：

| 步骤 | 内容 | 失败语义 |
|---|---|---|
| 1/6 | 拉取主站 `channels/<ch>/{manifest.json,manifest.json.sig,index.json}` | 明确是哪一个文件拉取失败 |
| 2/6 | 固定公钥验签，index 摘要/大小/`seq`/`channel` 与 manifest 对齐 | 验签失败不写任何镜像文件 |
| 3/6 | 读镜像现有 manifest：`seq` 回退拒绝；同 `seq` 不同字节拒绝；同 `seq` 同字节幂等跳过 | 报出主站与镜像的 `seq` |
| 4/6 | 索引引用 blob 去重后逐个比对：已存在且大小+sha256 匹配即复用；缺失才下载，校验后 `.part` + rename | 报 `blob <sha> 大小不符/摘要不符`，清单不更新 |
| 5/6 | 写 `schema/*` → `index.json` → `manifest.json` → `manifest.json.sig` | blob 先于清单，避免清单指向缺失 blob |
| 6/6 | 同步后自检：只看本渠道 + `schema/` + `blobs/`，重新验签、摘要、引用完整性，并断言 `seq` 不低于同步前 | 自检失败即整体失败 |

- **只增不删**：内容寻址的旧 blob 保留，旧清单回放仍可能需要；镜像里的旧 blob 不按孤儿拒绝。
  清理由运维按保留期执行（建议至少覆盖一个发布周期）。
- **半更新窗口**：`manifest.json` 与 `manifest.json.sig` 是两个文件，静态托管下极短时间内可能读到
  新清单配旧签名。客户端验签失败会保留旧快照并重试，不构成信任风险；若要彻底消除，可在镜像前面
  做原子目录切换（§15 的对象存储/CDN 场景天然规避）。

### 14.4 运维步骤（可重复执行）

```sh
# 1) 准备上一版线上产物快照（用于 seq 比较与增量上传），例如上一轮的 dist 目录
# 2) 生成 + 签名 + 自检；密钥只来自 CI secret / 发布机环境
SKIFF_SIGNING_KEY=<64 hex> SKIFF_SERVER_PUBLIC_KEY=<64 hex> \
  cargo run -p skiff-server -- publish --out target/skiff-server/dist --prev <上一版目录>

# 3) 上传（托管方案未定，见 §15；顺序不能变）
#    blobs/ → schema/ → channels/<ch>/index.json → manifest.json → manifest.json.sig

# 4) 至少一个镜像执行同步并自检
SKIFF_SERVER_PUBLIC_KEY=<64 hex> \
  cargo run -p skiff-server -- sync --from https://<主站基地址> --to <镜像目录> --channel stable

# 5) 抽验：对线上目录再跑一次严格校验
cargo run -p skiff-server -- check --dist <线上目录> --prev <上一版目录> \
  --require-signature --public-key <64 hex>

# 6) 自建分发：起静态服务（默认 https）；访问日志默认关闭，需要取证时显式开
skiff-server serve --dist <线上目录> --listen 0.0.0.0:8443 \
  --tls-cert server.pem --tls-key server.key \
  --access-log /var/log/skiff-catalog-access.log

# 7) 304 服务端取证：先取 ETag，再带 If-None-Match 重放，然后读日志那一行
etag=$(curl -sI https://<IP>:8443/channels/stable/manifest.json | awk -F': ' 'tolower($1)=="etag"{print $2}' | tr -d '\r')
curl -sk -o /dev/null -w '%{http_code}\n' -H "If-None-Match: $etag" \
  https://<IP>:8443/channels/stable/manifest.json
grep 'status=304' /var/log/skiff-catalog-access.log | tail -1
```

- 同一渠道的发布与上传必须串行，避免 `seq` 竞争；`stable` 与 `night` 各自独立。
- 线上目录不能被回退到旧 `seq`：客户端会拒绝；撤回用递增 `seq` + `yanked`/`revoked`（§5.3）。

### 14.5 代码落点与已覆盖测试

| 路径 | 内容 |
|---|---|
| `tools/skiff-server/src/sign.rs` | `Signer`、环境变量读取、`key_id`、detached 验签 |
| `tools/skiff-server/src/release.rs` | `publish`：内存严格自检后落盘 |
| `tools/skiff-server/src/sync.rs` | `Source`（HTTPS/本地目录）、六步同步、原子写、同步后自检 |
| `tools/skiff-server/src/check.rs` | 固定公钥验签、孤儿 blob 开关、`seq` 单调校验 |
| `tools/skiff-server/src/main.rs` | `gen` / `publish` / `check` / `sync` / `serve` 子命令与帮助 |
| `tools/skiff-server/src/serve.rs` | `serve`：只读静态服务（§16 契约：路径白名单、条件请求、单区间 Range、https 默认）；可选访问日志默认关闭 |

- `tests/p0.rs`：缺密钥/空白/非法种子报错；公钥不一致与未签名占位拒绝发布；`seq` 未递增拒绝且
  不写目录；发布产物过严格验签、篡改清单后验签失败。
- `tests/sync.rs`：首次同步补全 blob 并自检；二次同步全复用且清单不变；新增版本只传新 blob 且旧
  blob 保留；`seq` 回退拒绝；同 `seq` 不同内容拒绝；错公钥在下载 blob 前失败；篡改 blob 报
  `步骤 4/6` 且不写清单。
- `tests/serve.rs`：条件请求 304、单区间 206/416、`404`/`405`、`HEAD` 无 body、路径穿越与非法 blob 名、
  `--http` 仅测试语义、TLS 自签 CA 建连（见 §16.6）。
