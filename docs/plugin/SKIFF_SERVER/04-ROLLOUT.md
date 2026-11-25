# 阶段划分与 P0 落地状态（§9–10）

> 上级索引：[SKIFF_SERVER.md](../SKIFF_SERVER.md) ｜ 拆自 `docs/plugin/SKIFF_SERVER.md` 原 §9–10（2026-11-16 整理，正文未改）。

---

## 9. 待定与建议

| # | 决策点 | 选项 | 推荐 | 理由 |
|---|---|---|---|---|
| 1 | 清单源数据放在哪里 | 同仓库子树 / 独立 `skiff-index` 仓库 | 先同仓库单目录，规模上来再拆 | 起步成本最低；与内核发版解耦可等审核人数变多再做 |
| 2 | 索引签名密钥是否复用插件发布密钥 | 复用 / 分离 | 分离 | 影响面与轮换节奏不同，见 §7.2 |
| 3 | 运行环境清单是否替换内核硬编码目录 | 仅展示 / 服务端驱动 | P1 只展示；P4 起服务端驱动但保留本地兜底 | 执行期判定要保持单一来源，服务端清单只是数据源 |
| 4 | 内核升级包是否单独签名 | 只用索引摘要 / 每包 detached 签名 | 只用索引摘要 | 与发布物去签名的既有决策一致，签名锚上移到清单 |
| 5 | 是否引入增量索引 | 只全量 / 引入 delta | P0–P3 只全量，P4 视规模再评估 | 全量索引足够小之前，delta 只增加一致性风险 |
| 6 | 客户端是否默认自动拉取 | 默认每日一次 / 默认关闭 | 默认关闭，以手动刷新为主 | 出网最小化；内网与离线环境不应有隐式外联 |
| 7 | 第三方镜像是否登记 | 官方登记列表 / 不登记 | 不登记，仅文档给示例地址 | 登记近似背书，与「镜像不可信」原则冲突 |
| 8 | 内网镜像的传输协议 | 只允许 https / **允许 http + https** | **已定案：允许 `http://` 与 `https://`** | 内容由签名 + 摘要 + 单调 `seq` 保护，篡改与回滚都能被检出，HTTP 只损失传输保密性。一期默认源就是内网 HTTP `serve`，域名与证书后置；HTTPS + `ca_bundle` 仍是可选的更强保密形态（§15.5） |

> 以上 8 项均不阻塞 P0 开工；其中 1、2、6 建议在 P0 实现前拍板，其余可在对应阶段前定。第 8 项已于 2026-11-12 定案。

## 10. P0 落地状态

P0 只落**服务端产物这一侧**：源数据 → 校验 → 静态产物树。客户端拉取/验签/缓存/回退已在 P1
接入（§11），插件从目录安装已在 P2 接入（§12）；签名流水线已接（§14），镜像同步命令已做（§14）。
待定项 1 按推荐实现：源数据先放本仓库单目录。

### 10.1 实际目录

| 路径 | 内容 |
|---|---|
| `deploy/skiff-server/catalog/` | 源数据：`<channel>/channel.json` + `plugins/` `apps/` `runtimes/` `upgrades/` 四类条目 JSON，`assets/` 下有对应原始资产 |
| `deploy/skiff-server/schema/` | 9 份 JSON Schema：`channel` / `plugin` / `app` / `runtime` / `runtime-index` / `upgrade` / `index` / `manifest` / `signature` |
| `tools/skiff-server/` | 纯 Rust 实现（零原生 C 依赖，故不放进依赖 `zstd`/`rusqlite` 的 `xtask`）；根 workspace 成员 |
| `target/skiff-server/dist/` | `gen` 的输出目录（不入库）：`channels/<name>/{manifest.json,manifest.json.sig,index.json,runtime-index/<kind>.json}` + `blobs/sha256/<hex>` + `schema/` |

源数据形态与产物形态不同：源条目用 `asset` 指向 `catalog/assets/...`，生成器读文件后派生 `*_sha256` / `*_url` / `size` 并移除 `asset`；产物侧若仍带 `asset` 视为非法。

### 10.2 命令

```sh
# 本地演练：生成未签名占位产物（不得发布）
cargo run -p skiff-server -- gen --unsigned

# 发布：生成 → 签名 → 严格自检 → 落盘；密钥只从环境变量读
SKIFF_SIGNING_KEY=<64 hex> SKIFF_SERVER_PUBLIC_KEY=<64 hex> \
  cargo run -p skiff-server -- publish --out target/skiff-server/dist --prev <上一版产物目录>

# 发布前校验：schema 合规 + 摘要一致 + 索引引用完整 + seq 不回退
cargo run -p skiff-server -- check --dist target/skiff-server/dist --prev <上一版产物目录>

# 发布门禁：占位/空签名直接失败；带公钥时做真实验签
cargo run -p skiff-server -- check --dist target/skiff-server/dist --require-signature \
  --public-key <64 hex>
```

`check` 不带 `--dist` 时先在内存生成再校验，适合本地快速回归；CI 用默认值即可，无需先跑 `gen`。
`publish` 不产出未签名产物：缺 `SKIFF_SIGNING_KEY`、种子非法、签名公钥与固定公钥不一致、验签/
摘要/`seq` 自检不通过时一律非零退出，且自检在落盘前完成，失败不改动输出目录。

### 10.3 校验实现路径

自研最小 JSON Schema 子集校验器（`schema.rs`：`type` / `required` / `properties` / `additionalProperties` / `items` / `enum` / `const` / `minLength` / `maxLength` / `minimum` / `maximum` / `allOf` / 相对文件 `$ref`），加 `model.rs` 的 Rust 语义校验（反向域名 id、semver、`sha256:` 摘要、时间戳格式、URL 形态）。不引入 JSON Schema 引擎，也不把 schema 逻辑塞进任何内核 crate。

摘要与引用校验在 `check.rs`：每渠道 manifest/sig/index 过 schema 与语义 → `key_id` 一致 → index 的 `sha256`/`size`/`seq`/`channel` 与 manifest 对齐 → 每个引用 blob 存在且摘要与大小匹配 → 禁止孤儿 blob。

### 10.4 已覆盖测试

`tools/skiff-server/tests/p0.rs`：样例过 schema 与语义、非法条目被拒（未知字段 / 坏枚举 / 缺必填 / 坏类型）、生成索引含样例、篡改或缺失 blob 被拒、确定性生成（同输入两次字节一致，含固定测试种子签名）、`seq` 回退被拒、未签名产物在严格模式失败、签名接口产出可验签且篡改后验签失败。

### 10.5 未完成

| 项 | 状态 | 说明 |
|---|---|---|
| 签名流水线 | 已接（§14） | `publish` 从环境变量取种子、发布前用固定公钥自检；上传步骤待托管方案（§15）定后落地 |
| 镜像同步 | 已做（§14） | `sync` 按内容寻址只补缺失 blob、`seq` 单调、同步后自检并带步骤号报错；版本清单随同步落镜像（主站停发的类型删本地残留） |
| 运行环境版本刷新 | 已做（§14.4） | `runtimes` 从上游取版本与 EOL、每个分支默认保留最近三个版本、按镜像摘要写 catalog 条目，并回填内置副本；镜像仓库不可达时整体不写 |
| 客户端接入 | P1 只读 + P2 插件安装 + P3 升级只读已完成 | 拉取/验签/缓存/回退/只读商店页见 §11；插件从目录安装见 §12；内核升级提示与下载见 §13；应用模板/运行环境安装、blob 本地缓存与内置公钥仍未接 |
| 增量索引 | 未做 | 按 §2.4 属 P4 |
