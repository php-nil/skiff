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
- `crates/web/src/handlers/ui/store.rs`：升级提示与下载入口（壳页渲染）；无数据时只给空态文案。

### 13.5 已覆盖测试

- `crates/app/src/catalog/tests.rs`：无升级数据返回 `None`（安静空态）、按渠道/架构/当前版本
  选最高条目、当前版本不旧于候选时返回 `None`。
- `crates/app/src/commands/upgrade.rs`：缺公钥不触网、降级/同版本拒绝、成功暂存并校验、
  二次下载复用不再请求 blob、远端摘要不符不留半包、本地暂存被篡改后丢弃重下、24h TTL
  只清过期暂存且不误删新包。
- `crates/web/src/handlers/store.rs`：升级面板空态与有可用版本时的下载入口渲染。
- `crates/web/tests/main_flow/plugins.rs`：viewer 403 并落 `upgrade.download` deny 审计、缺公钥 400
  并落 `no_key` 审计、无升级数据回商店页并落 `not_found` 审计且无暂存文件。

### 13.6 未完成

升级执行段（停服、替换包、数据迁移、回滚与健康检查）、已暂存包在页面上的可见状态与人工
清理入口、暂存配额（TTL 已按 24h 落地）、升级包与内核二进制的一致性校验、内置公钥
（发布侧签名已接，见 §14）、发布后的线上目录运维。

## 14. 签名发布与镜像同步（已实现）

服务端产物这条链现在是一条可重复执行的命令路径：`runtimes`（上游版本 → 源条目 → 清单 → 内置副本）、
`publish`（生成 → 签名 → 自检 → 落盘）与 `sync`（主站 → 镜像）。上传动作本身仍取决于托管方案
（§15 未决），因此刻意不固化到命令里。

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
| 5/6 | 写 `schema/*` → `runtime-index/<kind>.json` → `index.json` → `manifest.json` → `manifest.json.sig` | blob 先于清单，避免清单指向缺失 blob；版本清单 404/410 只删本地残留，不算失败 |
| 6/6 | 同步后自检：只看本渠道 + `schema/` + `blobs/`，重新验签、摘要、引用完整性，并断言 `seq` 不低于同步前；同步进来的版本清单要能过 schema 且 `kind`/`channel` 自洽 | 自检失败即整体失败 |

- **只增不删**：内容寻址的旧 blob 保留，旧清单回放仍可能需要；镜像里的旧 blob 不按孤儿拒绝。
  清理由运维按保留期执行（建议至少覆盖一个发布周期）。唯一例外是**版本清单**：主站已不发布的
  类型会删掉镜像残留——它不进签名链，留着等于对外发一份过期版本表，宁可让面板回退。
- **半更新窗口**：`manifest.json` 与 `manifest.json.sig` 是两个文件，静态托管下极短时间内可能读到
  新清单配旧签名。客户端验签失败会保留旧快照并重试，不构成信任风险；若要彻底消除，可在镜像前面
  做原子目录切换（§15 的对象存储/CDN 场景天然规避）。

### 14.4 `runtimes`：运行环境版本刷新

一条命令把「上游有哪些版本」变成可发布的数据，不用再手工抄版本号与镜像 digest：

```sh
# 先看会改什么（不联网取 digest、不写任何文件）
skiff-server runtimes --dry-run

# 真刷新：写 catalog 条目 → build 出清单 → 回填面板内置副本
skiff-server runtimes --channel stable

# 直连不到 docker.io 的机器：走内网 HTTP 代理（手册里的代理清单见
# docs/test/ITERATION/02-ISSUES_ENV.md，首选 10.66.10.22:1010）
skiff-server runtimes --channel stable --proxy http://10.66.10.22:1010

# 只要每个分支的最新版（清单更短，但老站点升级没有降级余量）
skiff-server runtimes --per-branch 1
```

- **出网与代理**：`--proxy <url>` 显式指定 HTTP 代理，取 digest 与版本表都经它。
  **不读 `HTTPS_PROXY` 环境变量**：本 crate 的 `ureq` 关了 `proxy-from-env`（开了会连本机
  httptest 也走代理，设了代理的环境里测试必红），所以 `HTTPS_PROXY=… skiff-server runtimes`
  不生效，只能写 `--proxy`。报错信息里会带首个失败原因（仓库不可达 / tag 未就绪 / 代理配错）。
- **Docker Hub 的 API 域名**：`docker.io` 是网页域名，`/v2/…` 会 301 到 `www.docker.com`
  （响应没有 `Docker-Content-Digest`），实际打的是 `registry-1.docker.io`；私有仓库按镜像名
  原样透传。

- **数据源**：版本与 EOL 取 `endoflife.date`（PHP 取全部受支持分支；Node.js 只取已进 LTS 的分支；
  Rust 只取当前 stable；EOL 日期已过的不收）。镜像 tag 的 `sha256` 摘要走镜像仓库 `v2` API
  （默认 `docker.io`，需要能连通），取不到就不写——认证走 `WWW-Authenticate` 的 Bearer 挑战。
- **每个分支留几个版本**：默认 `--per-branch 3`，即一个发布分支保留最近三个版本，给老站点留
  降级余量（PHP `8.5` → `8.5.13 / 8.5.12 / 8.5.11`；Node.js `24` → `24.22.3 / 24.22.2 / 24.22.1`）。
  各类型「分支」的口径不同：PHP 的分支是 `major.minor`、Node.js 是 major，都在分支内往前数；
  Rust 的分支是 `1.x`，一次发布通常只有一个版本，所以实际只有 1 条。
  历史版本靠各类型的官方接口确认存在（PHP 逐个补丁号查 `releases/index.php?json&version=`，
  以响应里的 `version` 字段为准——不存在的版本它仍回 200；Node.js 用 `dist/index.json` 全量索引；
  Rust 用 `channel-rust-<ver>.toml` 的 200/404）。查询失败只降级为该分支的 `latest` 并打印原因，
  不会让整类失败。同一分支的补丁共用分支的 EOL。
- **写哪些文件**：`catalog/<channel>/runtimes/<kind>-<version>.json`（每个版本一份）、
  `channels/<channel>/runtime-index/<kind>.json`（由 `build` 从条目产出）、
  `crates/app/assets/runtime-index/php.json`（内置副本，取产物的**同一份字节**，
  所以紧接着跑 `check --builtin-index` 就是绿的）。
- **默认版本**：原 `default` 条目还在目标集合里就保持；否则落到目标集合里版本最高的那个。
  被保留但不再入选的旧条目会把 `default` 摘掉——默认版本只能有一个，否则清单里会出现两个默认项。
- **失败语义**：某一类「有目标版本但一个都拿不到 digest」时整体退出且**不写任何文件**
  （先写一半再失败会留下半新半旧的目录）；个别版本拿不到只跳过该版本并打印原因。
  上游某类没有任何可用版本时不动该类旧条目，`--prune` 也不会动手。
- **旧条目**：默认保留（老站点升级还要选到它们），要清理加 `--prune`；`yanked` 条目一律不动。
- **其它旗标**：`--kinds php,node` 收窄范围、`--per-branch <n>` 改每分支保留数（1–4，默认 3；
  上限与面板的「每大版本最多 4 条」对齐）、`--proxy <url>` 走代理出网、
  `--builtin <dir>` / `--no-builtin` 改内置副本位置、
  `--out <dir>` 顺手把整份产物写到该目录、`--today YYYY-MM-DD` 固定 EOL 判定基准日（复现用）。

### 14.5 运维步骤（可重复执行）

```sh
# 0) 运行环境版本刷新（有新上游版本时）：写源条目 → 清单 → 内置副本
cargo run -p skiff-server -- runtimes --channel stable

# 1) 准备上一版线上产物快照（用于 seq 比较与增量上传），例如上一轮的 dist 目录
# 2) 生成 + 签名 + 自检；密钥只来自 CI secret / 发布机环境
SKIFF_SIGNING_KEY=<64 hex> SKIFF_SERVER_PUBLIC_KEY=<64 hex> \
  cargo run -p skiff-server -- publish --out target/skiff-server/dist --prev <上一版目录>

# 3) 上传（托管方案未定，见 §15；顺序不能变）
#    blobs/ → schema/ → channels/<ch>/index.json → manifest.json → manifest.json.sig

# 4) 至少一个镜像执行同步并自检
SKIFF_SERVER_PUBLIC_KEY=<64 hex> \
  cargo run -p skiff-server -- sync --from https://<主站基地址> --to <镜像目录> --channel stable

# 5) 抽验：对线上目录再跑一次严格校验（带 --builtin-index 时同时核对面板内置副本）
cargo run -p skiff-server -- check --dist <线上目录> --prev <上一版目录> \
  --require-signature --public-key <64 hex> \
  --builtin-index crates/app/assets/runtime-index

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
- `--builtin-index <dir>`：拿产物与面板内置副本（`crates/app/assets/runtime-index/` 里的
  `php`/`node`/`rust` 清单）逐字节比对，不一致即失败。内置副本一旦比产物新，面板会把远端清单
  **永久**判成更旧而拒用；手写这份文件迟早写错，所以回填后必须跑这一步。产物有多个渠道时
  用 `--channel` 指定比哪个。

### 14.6 代码落点与已覆盖测试

| 路径 | 内容 |
|---|---|
| `tools/skiff-server/src/sign.rs` | `Signer`、环境变量读取、`key_id`、detached 验签 |
| `tools/skiff-server/src/release.rs` | `publish`：内存严格自检后落盘 |
| `tools/skiff-server/src/sync.rs` | `Source`（HTTPS/本地目录）、六步同步、原子写、同步后自检 |
| `tools/skiff-server/src/runtimes.rs` | `runtimes`：上游版本规划（每分支多版本、EOL 过滤、默认版本）、catalog 条目写盘、清单与内置副本回填（`--dry-run` / `--prune` / `--per-branch`） |
| `tools/skiff-server/src/upstream.rs` | 上游取数：`endoflife.date` 分支表、各分支历史版本确认（PHP 版本页 / Node.js 全量索引 / Rust channel 清单）、镜像仓库 v2 的 tag 摘要（Bearer 挑战，`docker.io` 映射到 `registry-1.docker.io`）、可选 HTTP 代理 |
| `tools/skiff-server/src/check.rs` | 固定公钥验签、孤儿 blob 开关、`seq` 单调校验、内置副本字节比对（`--builtin-index`） |
| `tools/skiff-server/src/main.rs` | `gen` / `publish` / `check` / `sync` / `serve` 子命令与帮助 |
| `tools/skiff-server/src/serve.rs` | `serve`：只读静态服务（§16 契约：路径白名单、条件请求、单区间 Range、https 默认）；可选访问日志默认关闭 |

- `tests/p0.rs`：缺密钥/空白/非法种子报错；公钥不一致与未签名占位拒绝发布；`seq` 未递增拒绝且
  不写目录；发布产物过严格验签、篡改清单后验签失败。
- `tests/sync.rs`：首次同步补全 blob 并自检；二次同步全复用且清单不变；新增版本只传新 blob 且旧
  blob 保留；`seq` 回退拒绝；同 `seq` 不同内容拒绝；错公钥在下载 blob 前失败；篡改 blob 报
  `步骤 4/6` 且不写清单；版本清单随同步落镜像且与主站字节一致、主站停发后镜像残留被删。
  `src/sync.rs` 单测另覆盖 HTTP 数据源：404 视为未发布（`None`），5xx 仍报错。
- `tests/serve.rs`：条件请求 304、单区间 206/416、`404`/`405`、`HEAD` 无 body、路径穿越与非法 blob 名、
  `--http` 仅测试语义、TLS 自签 CA 建连（见 §16.6）。
- `tests/cli.rs`：`--unsigned` / `--require-signature` / `--builtin-index` 真的改变行为（不只是能被解析）：
  内置副本与产物字节一致时通过、改一个字节即失败，缺 `--dist` 时明确报错。
- `tests/runtimes.rs`：假上游覆盖三类版本选择（PHP 全分支、Node 只 LTS、Rust 只 stable）、EOL 过滤、
  每分支保留三个版本（`--per-branch 1` 只留最新）、历史版本查询失败降级为 `latest`、
  默认版本转移与唯一性、digest 复用与跳过、二次运行幂等、`--prune` 清理、`--dry-run` 不落盘、
  「全部拿不到 digest 时一个字都不写」。`src/upstream.rs` 单测覆盖镜像名拆分、生命周期表解析、
  补丁号递减（到 0 停、单段版本不动）、Node.js 索引按 major 分组（预发布版剔除），
  以及 Registry API 主机映射（`docker.io` → `registry-1.docker.io`，私有仓库原样透传）。
