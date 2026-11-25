# Q 自建镜像：本机托管 → 测试机安装对接（MIRROR-xx）

> 上级索引：[TEST_MATRIX.md](../TEST_MATRIX.md) ｜ 新建于 2026-11-25。
> 覆盖「开发机当静态服务端（`skiff-server serve`）、测试机当客户端」的**安装 + 目录对接**全链路。
> 这里只写当前实现应通过的验收项；未闭环问题归 [TEST_LEDGER.md](../TEST_LEDGER.md)，坐标（地址、前缀、口令文件）只在
> [TEST_ENV/01-ACCESS.md](../TEST_ENV/01-ACCESS.md)，服务端契约只在 `docs/plugin/SKIFF_SERVER/07-SERVE.md` §16。

## 0. 为什么单列一册

- **执行位置跨两台机**：发布与托管在开发机（Windows），消费在测试机（Debian）。DEPLOY 册只管「有包怎么装」，
  STORE 册只管「镜像符合 §16 契约」；「装完的面板确实把默认源指到了这台镜像、并且真的读到了它」过去没有归属。
- **默认源就是本机**：一期定案的内网源就是 `http://10.66.52.7:8080`（`07-SERVE.md` §15.5），本册验的就是这条默认值。
- **重跑触发**：改 `tools/skiff-server/src/{serve,build,sync,runtimes,check}.rs`、改
  `crates/app/src/{catalog,runtime_index}.rs`、改 `deploy/config.toml.example` 的 `[catalog]` 段、
  改渠道目录 `deploy/skiff-server/catalog/**`、安装包换版本——任一命中即整册重跑。

## 1. 约定

- `$M`：镜像基址 `http://10.66.52.7:8080`（开发机，serve 明文）；`$B`、`$P`、`$J` 同
  [00-USAGE.md](00-USAGE.md) §0。
- 发布侧变量：`SKIFF_SIGNING_KEY`（32 字节种子 hex）、`SKIFF_SERVER_PUBLIC_KEY`（对应公钥 hex）。
  测试密钥与发布密钥**不是同一把**：测试期用临时密钥对，公钥写进测试机 `[catalog].public_key`；
  仓库不保存任何密钥（`06-RELEASE_MIRROR.md` §14.1）。
- 干净重装一律「先备份、再 `uninstall.sh --purge`、再装」。`--purge` 会要求交互输入 `YES`，
  走 ssh 非交互执行时必须 `printf 'YES\n' | sh uninstall.sh --purge`，否则 `read` 卡住整条会话（2026-11-25 实测踩到）。
- 面板登录不再是表单 POST：`POST /{p}/login` 返回 405，登录走 JSON
  `GET /ui/v1/auth/login`（取 `csrf_token`）→ `POST /ui/v1/auth/login`（`{username,password,csrf}`）。

## 2. 用例

| 编号 | 用例 | 档位 | 步骤 | 预期 | 判定 | 落点 |
| --- | --- | --- | --- | --- | --- | --- |
| MIRROR-01 | 发布侧产物签名并自检 | `[本机]` | 设 `SKIFF_SIGNING_KEY`/`SKIFF_SERVER_PUBLIC_KEY` → `skiff-server publish --catalog deploy/skiff-server/catalog --schema deploy/skiff-server/schema --out <dist>` → `check --dist <dist> --require-signature --public-key <hex> --builtin-index crates/app/assets/runtime-index` | publish 退出 0 且打印公钥；`manifest.key_id = sha256(公钥)`、`seq` 与 `catalog/<ch>/channel.json` 一致；check 报「校验通过」且「内置副本一致」（有内置副本的类型才计份数）；产物含三类 `runtime-index/<kind>.json` | ☑通过（2026-11-25） | `tools/skiff-server/src/{sign,build,check}.rs` |
| MIRROR-02 | `serve` 托管产物符合静态服务契约 | `[本机]` | `serve --dist <dist> --listen 0.0.0.0:8080 --http --access-log <log>`；逐路径 `curl -o /dev/null -w '%{http_code}'`：`channels/<ch>/manifest.json`、`…manifest.json.sig`、`…index.json`、`channels/<ch>/runtime-index/{php,node,rust}.json`、短别名 `<ch>/runtime-index/<kind>.json`、`/api/**` | 前三件与三类清单 **200**（含短别名）；`/api/**` 404；带 `If-None-Match: <ETag>` 重放 **304**（无 body）；启动打印「`--http` 仅测试用」警告 | ☑通过（2026-11-25） | `tools/skiff-server/src/serve.rs`、`tests/serve.rs`、`07-SERVE.md` §16.3 |
| MIRROR-03 | 跨机可达 | `[上机]` | 测试机 `curl -m 6 -o /dev/null -w '%{http_code} %{size_download} %{time_total}' $M/channels/<ch>/manifest.json`，再取一份 `runtime-index/php.json` | 均 200，字节数与 `--dist` 下同名文件一致；内网直连耗时应 < 0.3 s；失败先查防火墙/网段，不要先改客户端 | ☑通过（2026-11-25） | `docs/test/TEST_ENV/01-ACCESS.md` §2.2 |
| MIRROR-04 | 测试机干净重装 | `[上机]` 可丢弃环境 | 备份 `config.toml` 与 `/www/skiff` → `printf 'YES\n' \| sh uninstall.sh --purge` → 移走 `/www/{sites,php,runtime}` → `sha256sum -c <pkg>.tar.gz.sha256` → `sh install.sh --listen 0.0.0.0:18080 --tls off --no-start` → 追加 `allow_insecure_http` 与 `[catalog]` → `systemctl restart skiff` | 卸载后 `/etc/skiff`、`/www/skiff`、`/usr/local/bin/skiff*`、unit 全部不存在；安装退出 0、`skiff.service` enabled+active、`/healthz` 200；日志打印新入口前缀与初始口令路径；`skiff doctor` 全绿；`/www/skiff/official/` **7 个**官方包 + `public-key.txt` 在位（`com.skiff.files` 已于 2026-11-24 删除，旧残留包要手工清掉，否则与包内清单不符） | ☑通过（2026-11-25） | `deploy/install.sh`、`deploy/uninstall.sh`、`crates/core/src/bootstrap.rs` |
| MIRROR-05 | 目录对接：商店页读到本镜像 | `[上机]` | 配置 `[catalog] enabled=true`、`mirrors=["http://10.66.52.7:8080"]`、`public_key=<发布公钥>` → 重启 → 登录 → `GET /ui/v1/store?with_schema=true` | `message` 为「已从 http://10.66.52.7:8080 更新到 seq N」；`key_configured=true`、`allow_unsigned=false`、`channel` 与配置一致；`sections` 含插件 / 应用模板 / 运行环境三类且条目数等于 `index.json` 对应计数；镜像侧访问日志出现同一时刻的 `manifest.json` / `.sig` / `index.json` 三条 GET | ☑通过（2026-11-25） | `crates/app/src/catalog/**`、`crates/web/src/handlers/ui/store.rs` |
| MIRROR-06 | 运行环境清单：三类都来自镜像 | `[上机]` | `GET /ui/v1/runtimes/versions?kind=php\|node\|rust&refresh=true`；再看 `/ui/v1/runtimes?limit=1&with_schema=true` 的 `version_*` 下拉 | 三类都返回 `source="registry"`、`failed=false`、`generated_at` 等于渠道 `channel.json` 的时间、`versions` 每条都在目录大版本内；每个大版本下**恰好 3 条**（该分支只发过 1 条的除外，如 PHP `8.6`）；三段版本（Node `24.22.3`）必须出现在所属大版本 `24` 下；`eol` 只列**日期已过期**的版本（未来日期不得标「已 EOL」，2026-11-25 修）；弹窗提示行给出固定路径 `$M/channels/<ch>/runtime-index/{php,node,rust}.json`，下拉项数与该类型 `versions` 条数一致 | ☑通过（2026-11-25） | `crates/app/src/runtime_index.rs`、`crates/web/src/handlers/ui/runtimes.rs`、`docs/contract/SITE_RUNTIME_CONTRACT/02-CREATE_PAGE.md` §4.1 |
| MIRROR-07 | 条件请求与断源降级 | `[上机]` | ① 连续两次 `store?refresh=1`；② 停掉 `serve` 后再 `refresh=1`，并 `runtimes/versions?kind=php&refresh=true`；③ 恢复 `serve` 再 `refresh=1` | ① 第二次为「目录未更新（304），展示 … 的本地缓存」，条目与 `seq` 不变，`fetched_at` 会更新为本次校验时间（**不是**冻结时间）；② 为「镜像源不可达，正在使用本地缓存」且括注含镜像地址与失败原因（`Connection refused (os error 111)`），条目仍可见，`fetched_at` 保持上次成功值；版本清单此时 `failed=true`、`error` 带 URL 与原因，但 `versions` 仍是缓存条目；③ 恢复后回到 fresh/304，不清空已展示的条目 | ☑通过（2026-11-25） | `crates/app/src/catalog/fetch.rs`、`crates/web/src/handlers/ui/store.rs` |
| MIRROR-08 | 篡改与伪造被拒 | `[上机]` | 复制 dist 到临时目录，改 `index.json` 一个字节（保持长度）后起第二个 `serve`，把 `mirrors` 指过去 → 刷新；再改回并重启 | 客户端拒绝该渠道并给出摘要/验签失败原因，**不**把新内容并入商店；改回后恢复 fresh。换一把无关公钥签的渠道同样被拒（`key_id` 不一致） | ☑通过（2026-11-25） | `crates/app/src/catalog/verify.rs`、`tools/skiff-server/src/sign.rs` |
| MIRROR-09 | 版本清单刷新命令（发布侧） | `[本机]` | `skiff-server runtimes --channel stable --proxy <url> [--per-branch 3] [--prune] [--dry-run]` | 有全量条目时摘要复用、不联网；`--dry-run` 不联网不落盘；`--per-branch` 只接受 1..=4；某类全部解析失败即整体退出、旧条目与 `default` 不动 | ☑通过（2026-11-25） | `tools/skiff-server/src/runtimes.rs`、`tests/runtimes.rs` |

## 2.1 本轮实测记录（2026-11-26，包 `a915b6e` / `c038e313…`；04~09 本轮重跑，01~03 沿用同一份 dist——产物字节未变，仅 `test` 侧重新取过）

| 用例 | 实测输出 |
| --- | --- |
| MIRROR-01 | `publish` 出 18 个文件、`seq=1`、`key_id=707cede37fad5c348298ac466c726e248dc7e416dd697e20fbf75de04c441ac2`；`check --require-signature --public-key … --builtin-index crates/app/assets/runtime-index` 报「校验通过 + 内置副本一致：1 份」 |
| MIRROR-02 | `manifest.json` 200/343 B、`.sig` 200/244 B、`index.json` 200/8,303 B、`runtime-index/{php,node,rust}.json` 200/1,030·777·214 B，短别名 `stable/runtime-index/php.json` 200/1,030 B，`/api/x` 404；`If-None-Match: sha256:275892e7…` 重放 304、0 字节 |
| MIRROR-03 | 测试机取 `manifest.json` 200/343 B/**0.066 s**、`runtime-index/php.json` 200/1,030 B/0.066 s，与 `--dist` 下同名文件逐字节一致 |
| MIRROR-04 | 干净首装当日包（2026-11-26 `92949ea` / 12,538,527 B / `6c246450…`）：还原后无残留、安装 exit 0、`skiff.service` active、`/healthz` 200、`skiff doctor` 六项 ok、`official/` 7 个包 + `public-key.txt`、前缀 `5e3yz2yr`；同轮另跑三次**就地升级**（`070ff1d`→`7810675`→`a915b6e`）都 `config.toml` 逐字节相同、前缀不变、插件与容器保留 |
| MIRROR-05 | 首次刷新「已从 http://10.66.52.7:8080 更新到 seq 1」，`key_configured=true`、`allow_unsigned=false`、总条目 26（插件 1 / 应用模板 1 / 运行环境 23 / 升级 1）；再刷为「目录未更新（304），展示 … 的本地缓存」；镜像访问日志同一时刻出现 `manifest.json` / `.sig` / `index.json` 三条 GET |
| MIRROR-06 | `versions?kind=php\|node\|rust&refresh=true` 均为 `source=registry`、`failed=false`、`generated_at=2026-11-25T10:07:00Z`（渠道时间）；PHP 13 条（`8.6.0`；`8.5.13/12/11`；`8.4.28/27/26`；`8.3.37/36/35`；`8.2.36/35/34`）、Node 9 条（`26.13.1/13.0/12.1`、`24.22.3/2/1`、`22.23.5/4/3`）、Rust 1 条（`1.100.0`）；面板弹窗三个下拉共 **23** 项、`已 EOL` **0** 处、`8.6.0（最新）`，提示行给出 `http://10.66.52.7:8080/channels/stable/runtime-index/{php,node,rust}.json` |
| MIRROR-07 | 连续两次刷新都是「304 本地缓存」（`fetched_at` 随校验前移）；停 `serve` 后 store 报「镜像源不可达… Connection refused (os error 111)」且 `fetched_at` 冻在上次成功值，`versions` 报 `failed=true` 但仍返回缓存条目；恢复后回到 304 |
| MIRROR-08 | 篡改 `index.json`（`8.5.13`→`8.5.19`，长度不变，摘要变 `9c65de52…`）：`can_install=false`、「校验失败：索引摘要与 manifest 不符：声明 2921ccf4…，实得 9c65de52…；已拒绝该清单并保留上次成功快照」；换另一把密钥签的产物（清单 `key_id=7c30dc6c…`）：「验签失败: 清单 key_id 与固定公钥不匹配（清单 7c30dc6c…，公钥 707cede3…）」。两次都未被采用；临时 8081/8082 服务与临时 dist 已清理，客户端 `config.toml` 按备份恢复（与升级前逐字节相同） |
| MIRROR-09 | `runtimes --channel stable`（不带 `--proxy`）**4.3 s** 完成，PHP/Node/Rust 均「新增 0 / 更新 0 / 未变 13·9·1」，内置副本「无变化」；`git status` 干净（幂等） |

两条口径修正（实测才发现，已写进上表）：

1. `304` 与「断源」对 `fetched_at` 的语义不同：304 是**校验成功**（时间戳前移、条目复用），断源是**失败**（时间戳冻结）。
2. 面板的版本下拉取的是**本地缓存**（`cached()` 不触网），升级包后旧的 `eol` 标记会留在缓存里，要点一次「刷新版本清单」才按新逻辑重算——这不是缺陷，但排障时先刷新再下结论。

## 3. 与其他册的边界

- 安装包的**结构**（`checksums.txt`、包内文件齐全）归 DEPLOY-01；本册只验「用这个包装完能起来并连上镜像」。
- `serve` 的 TLS / `ca_bundle` / 自签 CA 分支归 STORE-01、STORE-03；本册走一期默认的内网明文 HTTP，不上 TLS。
- 面板侧「新建运行环境时选版本」的交互归 SITE-23 与 `SITE_RUNTIME_CONTRACT/02-CREATE_PAGE.md`；本册只验版本清单的
  来源与条数，不重复按钮级断言。
