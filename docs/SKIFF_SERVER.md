# Skiff-server：中心化静态数据分发方案

> 状态：**方案稿（未实施）** ｜ 日期：2026-11-09
> 定位：官方维护的**中心化数据源**，最终打包成一份**静态资产目录**，供任何 Skiff 实例拉取或自行镜像——插件商店清单、Docker 应用模板、运行环境版本清单、内核升级元数据。
> 关联：[§1.4 分层边界](sections/01-requirements.md)、[§4.1 包格式与信任](sections/04-plugin-package.md)、[§4.4–4.5 发布与部署](sections/04-plugin-publish.md)、[PLUGIN_CATALOG.md](PLUGIN_CATALOG.md)、[PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md)、[CI.md](CI.md)、[DEPLOY_TEST.md](DEPLOY_TEST.md)、[SITE_RUNTIME_CONTRACT.md](SITE_RUNTIME_CONTRACT.md)。
> 本文只定协议、流程与阶段，不含实现代码；具体规模、频率与超时数字未实测，均标注为「设计目标 / 待实测」。

## 0. 一页结论

| 维度 | 决策 |
|---|---|
| 形态 | **现状（已实施）**：静态目录（`manifest.json` + `index.json` + 内容寻址的 `blobs/`）；分发面无账号、无写入、无常驻业务服务，`serve` 只读且 `/api/**` 一律 404。**发布面（R0/R1/R2a–R2e 已实施，默认关闭）**：可选的审核 API，独立子命令 `skiff-server api`、独立进程、默认只绑回环、客户端不依赖，详见 §17 |
| 托管（已定案） | 一期用内网静态服务（`skiff-server serve`）直出产物，默认镜像 `http://10.66.52.7:8080`，**只走 HTTP**；安全性由签名 + `seq` 保证（内容不可伪造、不可回滚），域名与证书后置。对象存储/CDN 按改判条件再评估（§15.5） |
| 信任锚 | 每渠道一份 Ed25519 签名的清单，客户端内置官方公钥集；**镜像不被信任，只被校验** |
| 内容寻址 | 包、模板、升级包按 `sha256` 存入不可变 `blobs/`；索引只引用摘要 |
| 回滚防护 | 签名清单内单调 `seq`；客户端持久化最大值，拒绝更旧的清单 |
| 渠道 | `stable` / `night` 两份独立清单，各自递增，互不覆盖 |
| 客户端出网 | 仅访问配置的镜像地址（允许 `http://` / `https://`），由**内核**拉取；插件 `net` 仍为 501，不参与 |
| 阶段 | P0 服务端产物 → P1 客户端只读接入 → P2 商店安装 → P3 内核升级 → P4 镜像生态与模板；发布面另见 §17 的 R0/R1/R2（**R0/R1/R2a–R2e 已实施**） |

**主结论**：官方只承担「编辑、审核、签名、发布」，镜像方只承担「搬运与托管」，客户端只信签名不信源。因此商店与升级通道不需要常驻服务、账号体系与在线状态，断网、内网、自建镜像都消费同一份产物；插件安装仍沿用现有 `.wbpkg` 验签与权限确认链路，商店只补「发现与下载」。发布面若要自动化审核，另走 §17 的**可选**独立服务，不改变本节对分发面的结论，也不被客户端访问。

## 1. 目标与非目标

### 1.1 目标

| # | 目标 | 说明 |
|---|---|---|
| G1 | 单一官方数据源 | 插件、Docker 应用、运行环境、内核升级四类清单同仓管理、同一套校验与签名流程 |
| G2 | 静态可镜像 | 产物是纯文件；静态服务器、CDN、对象存储均可托管，第三方可整目录同步 |
| G3 | 客户端可验证 | 离线可验签、验摘要、检测回滚；镜像被篡改必须被发现并拒绝 |
| G4 | 与既有交付物兼容 | 插件仍是签名 `.wbpkg`（[§4.1](sections/04-plugin-package.md)），安装校验链路不变 |
| G5 | 增量演进 | P0 只读展示即可上线，后续阶段不推翻格式 |

### 1.2 为什么分发面是静态产物，而非常驻 API 服务

| 维度 | 常驻 API（registry 服务） | 静态资产（本方案） |
|---|---|---|
| 数据写入 | 需要在线写入接口、鉴权、并发控制 | 写入 = 仓库 PR + CI，天然串行、可审计（自动化入口是可选的 §17 审核服务，产物仍由 `publish` 生成） |
| 可用性 | 服务挂了商店与升级全断 | 任意静态托管；多镜像冗余 |
| 攻击面 | 登录、会话、注入、限流、DDoS 全要防 | 只读文件；无凭据可窃 |
| 成本 | 常驻进程、数据库、监控、值班 | 存储 + 带宽 |
| 内网/离线 | 需代理或私有部署 | 整目录拷贝即可 |
| 一致性 | 需要事务与版本管理 | `seq` + 不可变 blob，顺序即语义 |

数据是低频变更的「目录 + 不可变包」，读多写少，官方写入量以「天」为单位；需要在线写入接口的场景（账号、计费、实时数据）已列入非目标。常驻服务只在出现这些需求时才值得引入。

`skiff-server serve`（§16）只是把**已生成的静态产物**原样用 HTTP 提供出去（测试与过渡自托管），没有写入、鉴权或业务逻辑，不属于本节所说的常驻 API 服务，也不改变托管方案（§15）。

§17 的发布面审核 API 是**另一个面**：它只服务开发者与审核者，不参与分发，也不被客户端访问；形态是独立子命令 `skiff-server api`（独立进程、默认关闭、默认只绑回环、可以完全不部署），R0/R1 已实施。它不构成「为分发引入常驻 API 服务」，本节结论保持不变：分发面只要静态文件，`serve` 的 `/api/**` 一律 404（§16.2），`serve` 侧不存在任何写入接口。

### 1.3 职责划分

| 角色 | 负责 | 不负责 |
|---|---|---|
| 官方 | 录入与审核、兼容性判定、私钥签名、发布、撤回、公告与变更日志 | 托管可用性、带宽、地域加速（交给镜像方） |
| 镜像方 | 原样同步字节、正确 HTTP 缓存语义、可用性与带宽；可自行决定同步频率 | 修改内容、重新签发、宣称比签名清单更新、代替客户端做信任决策 |
| 客户端 | 按渠道拉取、验签与摘要、缓存与回退、镜像健康判断、安装与权限确认 | 信任未验签数据、绕过权限确认、自动卸载已安装项 |

### 1.4 明确不做

- 账号、登录、订阅、计费、授权码与付费内容（发布面同样不做账号体系：身份即已登记公钥，§17）。
- 在线状态上报、安装量统计、遥测；如需统计须另立明确同意的方案。
- 远程控制、下发命令、在线改配置（客户端只读消费）。
- 客户端侧在线 API（吊销、版本提示等一律走签名清单）；`/api/**` 只属于 §17 的发布面（R0/R1 已实施、默认关闭），客户端不访问。
- 评分、评论、举报等 UGC 与推荐算法。
- 服务端搜索、个性化排序、实时数据。
- 在线 CRL/OCSP：吊销走签名清单内的撤销列表。
- 代替客户端做信任决策：索引不能自动让客户端信任第三方作者。

> [§1.4](sections/01-requirements.md) 中「商店与分发渠道未决策」即由本方案回应：**自建中心化数据源，但不做常驻服务**（发布面的可选审核服务见 §17，R0 未实施）。现有交付物（systemd 安装包、签名 `.wbpkg`）不变。

## 2. 数据模型与目录结构

### 2.1 对象

| 对象 | 内容 | 变更频率 | 签名来源 | 客户端用途 |
|---|---|---|---|---|
| 渠道清单 `manifest` | `seq`、生成时间、索引摘要、撤销列表摘要 | 每次发布 | 官方索引密钥 | 信任锚 + 回滚防护 |
| 渠道索引 `index` | 四类条目的元数据 | 每次发布 | 由 manifest 摘要间接覆盖 | 展示与选择 |
| 插件条目 | id、版本、`api`、`.wbpkg` 摘要与地址、作者 `key_id`、是否撤回 | 插件发版 | 包内作者签名 + 索引签名 | 商店列表与安装 |
| Docker 应用模板 | id、版本、compose 摘要、镜像与 digest、env/端口/卷声明 | 模板更新 | 索引签名 | 应用一键部署（P4） |
| 运行环境版本 | `kind`、版本、镜像与 digest、EOL、兼容 `api` | 上游发版 | 索引签名 | 版本选择（P1 只读） |
| 内核升级条目 | 版本、架构、包摘要与地址、最低来源版本、变更日志 | 内核发版 | 索引签名 | 升级提示与下载（P3） |

### 2.2 目录布局

```text
<root>/
├── channels/
│   ├── stable/
│   │   ├── manifest.json
│   │   ├── manifest.json.sig
│   │   └── index.json
│   └── night/…                 # 同构
├── blobs/sha256/<hex>          # 不可变内容：.wbpkg、compose、升级包、模板资源
└── schema/                     # 随产物发布的 JSON Schema，供镜像方与第三方校验
```

`blobs/` 全渠道共享，同一摘要只存一份；`channels/<name>/` 是唯一可变层。渠道名与客户端配置一一对应，新增渠道不改变已有路径。

### 2.3 条目字段（最小集）

字段名以 schema 为准，此处只表达语义。

| 对象 | 关键字段 |
|---|---|
| 插件 | `id` `version` `api` `channel` `pkg_sha256` `pkg_url` `size` `key_id` `requires` `summary` `published_at` `yanked` |
| Docker 应用 | `id` `version` `compose_sha256` `images[{ref,digest}]` `env[]` `ports[]` `volumes[]` `min_kernel` `summary` |
| 运行环境 | `kind` `version` `image` `image_digest` `eol` `api_min` `default` |
| 内核升级 | `version` `arch` `pkg_sha256` `pkg_url` `min_from` `changelog_url` `channel` |

`requires` 沿用能力面口径（[§4.1](sections/04-plugin-package.md)）；模板的 compose 仍受既有字段子集约束（[§8](sections/08-engineering.md)），不在清单里另开一套。

### 2.4 版本不可变、内容寻址、全量与增量

- **版本不可变**：同一 `(id, version)` 一经发布不得改写。修正发新版本；下架用 `yanked`，安全处置用 `revoked`。
- **内容寻址**：所有大对象以 `sha256` 存于 `blobs/`，URL 与内容一一对应；索引只引用摘要，镜像天然去重、可校验。
- **全量索引**：每渠道一份完整 `index.json`；新客户端只需两次 GET（manifest + index）即可工作，是全量与增量的**正确性基线**。
- **增量**：P0 不做增量索引；镜像同步在文件层天然增量（blob 只增不改）。P4 若索引规模需要，再评估 `deltas/<seq>.json`，但缺失增量必须能回退全量。

## 3. 格式与契约

### 3.1 JSON + JSON Schema

- 全部清单为 UTF-8 JSON；Schema 用 JSON Schema（Draft 2020-12），随产物放在 `schema/`。
- 每个对象有独立 `schema` 字符串（如 `skiff.server.index/v1`），按对象演进，不共用一个全局版本号。
- 未知字段必须忽略（前向兼容）；未知 `schema` **主版本**必须拒绝该对象，不得猜测解析后部分应用。

### 3.2 字段版本策略

| 变更 | 版本处理 | 客户端行为 |
|---|---|---|
| 新增可选字段 | 不改 `schema` | 旧客户端忽略，新客户端可选使用 |
| 新增必填字段、改语义、删字段 | 升主版本（`v1` → `v2`） | 旧客户端拒绝该对象并提示升级 |
| 枚举新增取值 | 不改版本 | 遇未知取值按「不可用」展示并给出原因，不得崩溃 |
| 字段弃用 | Schema 标 deprecated | 至少跨一个大版本才移除 |

### 3.3 签名粒度与算法

- 算法 **Ed25519**；签名对象是渠道 `manifest.json` 的**原始字节**，不做 JSON 规范化，前缀域 `SKIFF-SERVER-MANIFEST-v1\0`。
- `manifest.json.sig` 形如 `{alg, key_id, sig}`；`key_id` 沿用插件 manifest 的 SHA-256(公钥) 小写 hex 口径（[§4.1](sections/04-plugin-package.md)），可共用验签工具。
- 签名内容覆盖 `index.json` 的摘要、`seq`、`generated_at` 与撤销列表摘要——**一次验签即覆盖整棵树**。
- 条目级不重复签名：`.wbpkg` 已带作者签名，索引签名只声明「官方在某个版本索引中引用了这个摘要」，两个声明互不替代。
- 多密钥：`key_id` 指向客户端内置公钥集之一；轮换时先发布携带新旧公钥的客户端，再切换签名密钥。

### 3.4 摘要、回滚与重放防护

| 机制 | 规则 |
|---|---|
| 摘要 | 验签通过后校验 `index.json` 的 `sha256`，再解析 |
| 单调序号 | 每渠道每次发布 `seq + 1`；客户端持久化最大值，更旧的清单一律拒绝 |
| 同序不同摘要 | 视为镜像不一致或攻击：保留旧缓存，改拉其它镜像；仍不一致则告警并弃用该镜像 |
| 时间戳 | `generated_at` 只用于展示与缓存判断，**不作为安全依据**（客户端时钟不可信） |
| 过期 | 可选 `expires_at`；超期提示「数据过旧」，不阻断已缓存清单的只读展示 |

### 3.5 压缩与缓存头

| 资源 | Content-Type | Cache-Control | 校验 |
|---|---|---|---|
| `manifest.json` / `.sig` | `application/json` | `no-cache` | 服务端可选 ETag；客户端不依赖 |
| `index.json` | `application/json` | `no-cache` + ETag（镜像可自行加短 `max-age`） | 验签通过后按摘要比对 |
| `blobs/*` | `application/octet-stream` | `public, max-age=31536000, immutable` | 客户端按 sha256 校验 |
| `schema/*` | `application/schema+json` | 长缓存 | 随产物版本 |

- 压缩：manifest 不压缩；index 可由镜像以 `Content-Encoding: zstd` 或 `gzip` 传输（客户端 `Accept-Encoding`），**验签与摘要针对解压后的原始字节**；blob 本身已是压缩包，不二次压缩。
- ETag 建议写成 `"sha256:<hex>"`，条件请求（`If-None-Match` / 304）只是镜像的**可选优化**；客户端刷新节奏由自己的 TTL 决定（§11.2），不把 304 或任何缓存当正确性依赖——镜像每次都 200 全量返回也必须能正常工作。

### 3.6 最小示例

渠道清单（签名覆盖其原始字节）：

```json
{
  "schema": "skiff.server.manifest/v1",
  "channel": "stable",
  "seq": 142,
  "generated_at": "2026-11-09T12:00:00Z",
  "key_id": "…64 hex…",
  "index": { "path": "index.json", "sha256": "…", "size": 48120 }
}
```

索引片段（只保留理解结构所需字段）：

```json
{
  "schema": "skiff.server.index/v1",
  "plugins": [
    { "id": "com.skiff.files", "version": "0.1.0", "api": "1.2",
      "pkg_sha256": "…", "pkg_url": "blobs/sha256/…", "key_id": "…", "yanked": false }
  ],
  "runtimes": [
    { "kind": "php", "version": "8.5", "image": "php:8.5-fpm-alpine",
      "image_digest": "sha256:…", "eol": null, "default": true }
  ],
  "revoked": []
}
```

## 4. 分发与镜像

### 4.1 静态目录与 URL

- 一份产物 = `channels/` + `blobs/` + `schema/` 的纯目录；静态服务器、CDN、对象存储均可直接托管。
- 客户端只依赖一种拼接：`base_url + "/channels/" + channel + "/manifest.json"`；`pkg_url` 一律相对路径，由客户端基于同一 `base_url` 解析（防镜像拼接错误）。`skiff-server serve` 另提供 `base_url + "/" + channel + "/manifest.json"` 短别名，两者等价（§16.2）。
- 产物不带入口页与目录索引；`blobs/` 不开启目录列表（不列即不可枚举）。

### 4.2 HTTP 语义

- 只依赖 `GET` / `HEAD`。条件请求（`If-None-Match` / `If-Modified-Since` → 304）与断点续传（Range）都是镜像的**可选优化**：客户端不依赖它们，镜像不支持也必须能完成全部下载与刷新；`skiff-server serve` 已实现两者（§16.3），但那是服务端实现选择，不是客户端前提。
- 镜像须正确返回 `Content-Length`、`Content-Type`；`ETag` / `Last-Modified` 为可选（用于省流量）。**错误页不得以 200 返回 HTML 冒充 JSON**，客户端按 Content-Type 与解析结果判定镜像不可用。
- 允许重定向（CDN/对象存储常见），客户端限制跳数；是否允许跨域重定向作为配置项，默认仅同域。

### 4.3 镜像同步

| 方式 | 适用 | 约束 |
|---|---|---|
| `skiff-server sync` | 官方主站 → 镜像 | 内容寻址只补缺失 blob；`seq` 不回退；同步后自检（验签/摘要/seq），见 §14.3 |
| rsync / 整目录复制 | 自建镜像、内网 | 先 `blobs/` 后 `channels/`；`--delete` 只允许作用在 `channels/`，`blobs/` 只增不删 |
| 对象存储同步 | CDN / 云 | 同一顺序：blob → index → manifest |
| 单一 tar.zst 快照 | 离线交接、首次建镜像 | 快照只用于搬运，服务形态仍是解开后的目录 |

- **发布顺序与同步顺序相反**：官方先上传 blob、再 index、最后 manifest；镜像按同样顺序落地。这样任意时刻读者看到的要么是旧的完整版本，要么是新的完整版本，manifest 不会指向尚不存在的摘要。
- 镜像不签名、不重写内容；若组织需要自建「可信源」，它是**另一个镜像**（客户端显式配置），不得伪装成官方域名。

### 4.4 多镜像与故障切换

- 客户端持有**有序镜像列表**：默认值为内置内网镜像 `http://10.66.52.7:8080`（§11.1），可追加备源；用户可增删，内网可只保留一个。
- **只按优先级回落，不合并**：依次尝试，命中第一个「验签通过且 `seq` 不倒退」的清单即停止；不跨镜像拼接条目、不取各源最大值。配置里显式的 `[]` 表示不联网，只用本地快照。
- **切源保留本地快照**：换镜像列表不清空 `<data_dir>/catalog/`；新源首拉失败或不可达时继续展示上次成功快照并标「离线」，离线仍可只读展示。
- **策略（已实现）**：按顺序请求，单镜像超时（默认 10s，可配）后切换；连续失败进入指数退避
  （10s 起、每次翻倍、上限 5 分钟）。自动刷新（TTL 到期 / 页面加载）跳过仍在退避期的镜像；
  若所有镜像都在退避且已有缓存，直接展示「离线」快照、不再发请求；「刷新目录」按钮**绕过退避**
  立即重试，成功一次即清零该镜像的失败计数。退避状态在进程内，重启清零。
- 各镜像 `seq` 允许不同步：客户端取**验签通过**的清单，不要求镜像之间一致；`seq` 仍只与本地
  已见最大值比较（防回滚），不跨镜像比较。
- 镜像健康只决定「本次是否可用」，不改变信任：任何镜像返回的数据都必须通过签名与摘要校验。

### 4.5 离线与内网

- 发布包可携带一份只读快照（与 `official/` 随包携带同思路，见 [PLUGIN_CATALOG.md](PLUGIN_CATALOG.md) §5.4）：无网时商店展示快照内容并标注「离线快照」。
- 内网：整目录同步到内网静态服务器，客户端把镜像列表改为内网地址即可；不需要额外代理或常驻服务。
  镜像地址**允许 `http://` 与 `https://`**（见 §9 第 8 项）：清单已签名且 `seq` 单调，中间人无法伪造内容或回滚版本，HTTP 只损失传输保密性，不影响安装正确性。
- 若内网镜像仍用 HTTPS + 自签证书，则不再需要系统级信任：
  把签发该镜像证书的 CA 导出为 PEM，配置 `[catalog].ca_bundle`（或 `SKIFF_CATALOG_CA_BUNDLE`）即可。
  该 CA 只作为**额外信任锚**追加到内置根之上，证书链与主机名校验不放松，也没有 insecure 开关。
- CA 文件不存在 / 无法解析 / 不含证书 → **启动失败**（`catalog.ca_bundle 非法`），不会静默回退到内置根；
  生效后刷新成功/失败照旧落 `catalog.refresh`，启动日志只报「加载了几个额外根」，不输出证书内容。
- 完全断网：只能看到随包快照与已下载缓存；安装仍可用本地 `.wbpkg` 上传（[§4.5](sections/04-plugin-publish.md)）。

## 5. 发布流程

### 5.1 录入与审核

- 源数据是仓库内的清单文件（PR 提交），**产物由 CI 生成且不进 git**；审核者只 review 源文件。
- CI 校验：Schema、id 唯一、版本不可变（同 `(id, version)` 摘要不得变化）、`api` 兼容区间、`requires` 合法、`pkg_url` 为相对路径、`.wbpkg` 存在且验签通过、摘要匹配、模板 compose 在既有字段子集内（[§8](sections/08-engineering.md)）。
- 审核通过并合并后触发发布流水线；发布权限与审核权限可由同一维护者持有，但**只允许流水线写线上目录**。

### 5.2 版本发布

1. 生成 `index.json` → 计算摘要 → 生成 `manifest.json`（`seq` = 线上当前值 + 1）→ 签名。
2. 上传 blob → 上传 index → 上传 manifest（最后一步才对外可见新版本）。
3. 私钥只出现在签名步骤，位于受控环境（CI secret 或离线发布机），不落产物与日志。
4. `stable` 与 `night` 各自独立递增 `seq`，互不阻塞。

上述步骤已固化为 `skiff-server publish`（生成 → 签名 → 自检 → 落盘）与 `sync`（主站 → 镜像），
可执行命令与密钥托管口径见 §14。

### 5.3 撤回与吊销

| 手段 | 语义 | 客户端行为 |
|---|---|---|
| `yanked` | 条目保留但不可新装 | 拒绝安装与升级到该版本，已安装不受影响 |
| `revoked` | 摘要级黑名单（篡改、密钥泄漏、严重漏洞） | 拒绝下载与安装；已安装项提示处置建议，不自动卸载 |

撤回同样走 PR + CI + 签名发布，不允许手改线上产物；撤回会递增 `seq`，客户端下次拉取即生效。

### 5.4 公告与变更日志

- 每条目带 `changelog_url`；渠道级变更按仓库约定写入 `docs/changes/`，对外可镜像同一份。
- 客户端展示「有更新 / 已撤回」时只引用条目字段，不在客户端硬编码文案与链接。

### 5.5 如何避免人工改坏清单

| 风险 | 措施 |
|---|---|
| 手改线上索引 | 产物只由流水线生成；线上目录对发布身份只写、对镜像只读 |
| 版本被覆盖 | CI 断言「同 `(id, version)` 摘要不变」，变化即失败 |
| 漏签名或签错密钥 | 发布前先验签产物，失败即中止；缺私钥直接失败（与 [CI.md](CI.md) §4.3 的插件签名口径一致） |
| `seq` 回退 | 发布前读取线上 manifest 的 `seq`，生成值必须大于它；流水线串行 + 分支保护 |
| 条目字段漂移 | Schema 校验是必过门禁，反例（缺字段、错类型、未知主版本）必须有测试用例 |

## 6. Skiff 客户端接入

### 6.1 拉取时机与频率

- 触发：管理员手动刷新（主路径）、启动后延迟一次、每日一次（可关）；不做高频轮询。
- 失败后指数退避，且不影响面板其它功能；离线环境不重复重试到刷屏。
- 拉取由**内核**完成：仅允许访问配置的镜像主机，不接受任意 URL（避免变成通用 HTTP 代理）；插件 `net` 仍为 501，不参与商店拉取。

### 6.2 缓存与回退

| 层 | 内容 | 何时使用 |
|---|---|---|
| 内存 | 本次验签通过的清单 | 页面间复用 |
| 磁盘 | 上次成功清单与已下载 blob | 网络失败或验签失败 |
| 随包快照 | 发布包内置的只读快照 | 首次启动 / 长期离线 |

- 只有**验签通过**的内容才能落盘为「可信清单」；失败时继续使用上次成功清单并标注「数据可能过旧」，同时给出失败原因。
- 缓存按渠道隔离；切换渠道不覆盖另一渠道的缓存。

### 6.3 验签失败处理

- 签名、摘要、Schema、`seq` 回退任一失败 → 丢弃该镜像结果、标记该镜像本次不可用、写审计（镜像、时间、失败原因）。
- **不降级到无签名模式**；`allow_unsigned` 只作用于本地手动上传的 `.wbpkg`（开发配置），不适用于商店索引。
- 连续失败达到阈值时在商店页显式提示，不静默失败。

### 6.4 渠道

| 渠道 | 内容 | 面向 | 客户端默认 |
|---|---|---|---|
| `stable` | 审核通过的正式版本 | 生产 | 默认 |
| `night` | 预发布 / 每日构建，可能撤回 | 测试与尝鲜 | 需显式切换，UI 标注风险 |

渠道由客户端配置决定；条目自身也带 `channel`，两者不一致时以客户端配置为准并忽略不匹配条目，防止 stable 客户端误装 night 包。

### 6.5 镜像配置

- 配置项：渠道、有序镜像列表、超时与重试、是否允许跨域重定向。
- 默认列表随发布包内置（含官方主源）；内网部署可只保留内网源。
- 修改镜像**不改变信任根**：公钥内置在客户端，不能由配置覆盖。

### 6.6 商店安装链路

商店只补「发现 + 下载」，其余沿用现有安装管线：

```text
选择条目 → 下载 pkg_url → 校验 sha256 → 结构与签名校验（api/requires）
        → 信任判定（官方固定公钥 / 第三方 TOFU）→ 权限确认 → 解包 staging → 安装启用
```

- 摘要或验签任一失败即中止并清理临时文件，不留「部分安装」（[§4.3](sections/04-plugin-package.md)）。
- 权限确认仍是独立一道闸：商店不得预勾选、不得跳过；页面分工见 [PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md)。
- 升级比较沿用既有规则（低于或等于已装版本默认拒绝，[§4.5](sections/04-plugin-publish.md)）。

## 7. 安全

### 7.1 供应链风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| 索引签名私钥泄漏 | 可发布任意条目 | 离线/受控环境保管、发布 job 最小权限、轮换预案、`seq` 与审计可追溯 |
| 作者私钥泄漏 | 恶意 `.wbpkg` 冒名 | `revoked` 摘要黑名单；作者换钥视为新信任，客户端要求重新确认指纹 |
| id 仿冒（typosquatting） | 用户装错插件 | 反向域名 id + 索引 `verified` 标记 + UI 区分「官方 / 第三方」 |
| 镜像篡改 | 投毒包、降级 | 以「镜像不可信」为前提：签名清单 + 摘要 + 单调 `seq` |
| 运行环境 tag 漂移 | 镜像被替换 | 清单给 `image_digest`，P4 起按 digest 安装，tag 只用于展示 |
| 模板注入 | compose 越权端口/卷 | 模板受既有 Compose 子集约束，端口/卷/环境变量逐项展示由用户确认 |

### 7.2 密钥管理

- 索引签名密钥离线生成，仅发布步骤通过受控环境注入；不写入仓库、不进产物、不进日志。
- **职责分离**：索引签名密钥与插件发布密钥（[§4.3](sections/04-plugin-package.md)）不是同一把，泄漏影响面与轮换节奏不同。
- 轮换：先发布内置新旧公钥的客户端，再切换签名密钥；旧公钥保留到受支持客户端均可升级。
- 不承诺「静默自动换钥」；换钥是一次需要公告的发布事件。

### 7.3 吊销与应急

- 应急顺序：撤回受影响摘要 → 递增 `seq` 发布新清单 → 公告 → 客户端下次拉取生效。
- 不做在线 CRL/OCSP：吊销信息固定在签名清单内，代价是生效延迟取决于拉取频率，可接受。
- 已安装项只提示处置建议，不自动卸载、不自动回滚——是否行动由管理员决定。

### 7.4 镜像被篡改时的行为

- 签名、摘要或 `seq` 任一异常：拒绝该镜像本次数据、保留旧缓存、标记镜像不可用、写审计并在商店页提示；不自动改用其它未验证数据。
- 镜像即使返回验签通过的旧清单，也只能说明「这是官方发布过的旧版本」；是否更新由 `seq` 与用户策略决定，不能借旧清单强制回滚。

### 7.5 TOFU 与固定公钥

| 对象 | 方案 | 理由 |
|---|---|---|
| 商店索引 | **固定公钥**（客户端内置官方公钥集） | 官方是唯一签发者，用户不应参与该信任决策；公钥随发布包同源分发 |
| 内核升级 | **固定公钥** | 升级包影响整机，不接受首次使用即信任 |
| 第三方插件作者 | **TOFU + 用户确认指纹**（沿用 [§4.3](sections/04-plugin-package.md)） | 作者分散无法内置全部公钥；签名证明来源，权限确认控制能力 |
| Docker 应用模板 / 运行环境 | 索引签名 | 内容是声明式配置，官方审核签名即可，无需作者密钥 |

## 8. 分阶段推进

| 阶段 | 范围 | 验收口径 | 回滚方式 |
|---|---|---|---|
| **P0** 服务端产物 | Schema、生成器、CI 校验、静态产物树 | 见 §10（已完成） | 产物目录整体下线 |
| **P1** 客户端只读接入 | 拉取、验签、`seq` 防回滚、缓存、离线回退、商店页只读展示 | 见 §11（已完成） | 关闭网络拉取，回退本地缓存/随包快照 |
| **P2** 商店安装 | 条目下载 + 现有安装管线 + 撤回展示 | 见 §12（已完成） | 保留本地上传安装；商店入口可禁用 |
| **P3** 内核升级 | 升级清单、渠道、下载与摘要校验、版本提示 | 只读段已完成：摘要校验、渠道隔离、拒绝降级（§13）；升级执行仍走现有部署流程（[DEPLOY_TEST.md](DEPLOY_TEST.md)） | 保留旧版本包与数据备份，按部署文档手工回退 |
| **P4** 镜像生态与模板 | 多源切换与健康退避（已做）、增量索引（按需）、应用模板/运行环境服务端驱动 | 多镜像故障切换不阻塞页面；旧镜像不导致回滚；模板部署受 Compose 子集约束 | 客户端固定回官方主源；模板条目可整体撤回 |

阶段约束：P0 独立交付「服务端产物」；P1 起每阶段都不改变既有格式与信任根；每阶段的「回滚」都是配置或数据层面的操作，不需要回滚客户端代码。

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
| `deploy/skiff-server/schema/` | 8 份 JSON Schema：`channel` / `plugin` / `app` / `runtime` / `upgrade` / `index` / `manifest` / `signature` |
| `tools/skiff-server/` | 纯 Rust 实现（零原生 C 依赖，故不放进依赖 `zstd`/`rusqlite` 的 `xtask`）；根 workspace 成员 |
| `target/skiff-server/dist/` | `gen` 的输出目录（不入库）：`channels/<name>/{manifest.json,manifest.json.sig,index.json}` + `blobs/sha256/<hex>` + `schema/` |

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
| 镜像同步 | 已做（§14） | `sync` 按内容寻址只补缺失 blob、`seq` 单调、同步后自检并带步骤号报错 |
| 客户端接入 | P1 只读 + P2 插件安装 + P3 升级只读已完成 | 拉取/验签/缓存/回退/只读商店页见 §11；插件从目录安装见 §12；内核升级提示与下载见 §13；应用模板/运行环境安装、blob 本地缓存与内置公钥仍未接 |
| 增量索引 | 未做 | 按 §2.4 属 P4 |

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
| `timeout_secs` | `10` | 单次 HTTP 硬超时，上限 120 |
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
| `crates/web/src/handlers/store.rs` + `templates/store.html` | 只读商店页（登录即可访问） |
| `crates/web/src/menu.rs` | 仅 `enabled` 时追加「插件 → 商店」菜单项 |

### 11.5 已覆盖测试

`crates/app/src/catalog/tests.rs`：验签失败拒绝并保留旧快照、`seq` 回退拒绝、索引摘要不符拒绝、
本地 blob 大小/摘要不符拒绝、条件请求 304 复用缓存、离线回退、未签名默认拒绝且显式允许后放行、
多镜像按序回退、自动刷新跳过退避中的镜像、全部镜像退避时直接吃缓存不发请求、手动刷新绕过
退避并在镜像恢复后清零失败计数、TTL 内不重拉且强制刷新绕过 TTL、失败 5 分钟后才重试、
进程重启后未过一天的磁盘缓存不联网、默认关闭时不触网、离线提示「镜像源不可达，正在使用本地缓存」、
无缓存时给出「设置 → 商店目录」指引、四类拒绝/离线各自断言 `catalog.refresh`
审计落库、换钥后旧缓存
作废并忽略 ETag 强制重拉、换钥且离线时不展示旧条目且 `plugin_package` 返回 `None`。
`crates/web/src/handlers/store.rs` 单测覆盖商店页最少渲染
（状态提示、四列数据、空态、无安装入口）。`crates/app/src/catalog/fetch.rs` 端到端覆盖自定义
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
| 内核 API | 条目 `api` 主版本必须等于 `KERNEL_API`，次版本不得高于内核（当前 `1.2`） |
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
| `crates/web/templates/store.html` | 「操作」列（仅插件分区；撤回/告警条目不显示安装） |

### 12.5 已覆盖测试

- `crates/app/src/commands/store.rs`：整链路（含 `install_package` 收尾）、下载失败 / 摘要不符 /
  验签失败 / 能力面缺失四类拒绝、内核 API 不符、未配置公钥不触网、条目不存在、`api` 兼容矩阵；
  每类失败都断言 `.incoming` 无残留。暂存 TTL：23h 不删 / 25h 只删 `.wbpkg`/`.part`、
  `prepare` 前清掉 25h 遗留且不误删刚下载的包。`crates/core/src/bootstrap.rs`：启动时清理 25h
  遗留、保留未过期暂存。
- `crates/web/src/handlers/store.rs`：有公钥时渲染安装入口、无公钥/告警条目不渲染。
- `crates/web/tests/main_flow.rs`：viewer 403 且落 `store.install` deny 审计、未配置公钥 400 并落
  `no_key` 审计、目录不可用回商店页并落 `not_found` 审计且无暂存文件。

### 12.6 未完成

应用模板与运行环境安装、blob 本地缓存与淘汰、内置公钥（发布侧签名已接，见 §14）、
增量索引、商店内的升级/回滚操作。

## 13. P3 落地状态（内核升级只读段）

P3 只做「提示 + 下载 + 摘要校验」，**不执行升级**：不停服、不替换包、不跑数据迁移。
升级执行仍走 [DEPLOY_TEST.md](DEPLOY_TEST.md) 的手工流程；本阶段的产出只是把可信的升级包
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

- 入口：`GET /{prefix}/store/upgrades/{version}/download`，仅 `Admin`；与商店安装一致是
  只读跳转式 GET，不引入新表单或 CSRF 面。
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

## 15. 商店目录托管来源评估（已定案，2026-11-12）

> 15.1–15.4 是当时的评估记录，保留作背景；最终口径以 **§15.5** 为准。

### 15.1 现状问题

商店目录目前挂在临时公网隧道上，两轮验证里各挂过一次（502 后换域名），靠换域名兜底。
清单本身已签名、blob 内容寻址，**信任不依赖托管**；问题纯粹是**可用性与运维**：地址不稳定会让
客户端镜像列表反复失效，也会把「换域名」变成事实上的发布流程。

### 15.2 三种稳定来源对比

| 方案 | 成本 | 风险 | 适用 |
|---|---|---|---|
| **自有静态托管**（自有域名 + 反代/静态服务器 + 证书） | 域名 + 一台最小 VPS/既有机器；人力在于证书续期、安全基线、监控与备份 | 单点故障与带宽自担；证书过期、系统补丁、被扫被打都由自己负责；`blobs/` 增长需要自己扩容 | 已有常驻服务器、内网/合规要求数据自持、访问量可预测 |
| **对象存储 + 自定义域名**（静态网站托管 + CDN） | 按量计费（存储/请求/流量，具体价格以云商报价为准，未实测）＋域名；人力主要在首次配置与发布脚本 | 供应商锁定与计费波动；自定义域名绑定与证书由云商托管但配置错误仍会 502/403；跨境内网访问延迟需实测 | 当前阶段推荐：无需维护机器，天生多副本，天然支持按对象增量上传 |
| **内网 CA + 自签证书** | 无公网费用；需要一台内网静态服务器与一套 CA 签发/分发流程 | 客户端必须信任该 CA，否则 TLS 直接失败；跨组织分发根证书成本高；公网不可用 | 纯内网/离线环境；公网商店不适用 |

补充说明：

- 三种方案都**不改变**签名与摘要校验链路，也不需要在服务端加逻辑；`sync` 对三者同样适用。
- 临时隧道不是稳定来源：它适合演示与一次性验收，不适合作为客户端默认镜像。
- 客户端允许 `http://` 与 `https://`（§9 第 8 项）：信任来自签名 + 摘要 + 单调 `seq`，不来自传输层。若内网镜像用 HTTPS + 自签证书，配置 `[catalog].ca_bundle` 追加该 CA 即可，校验强度不变。

### 15.3 当时的建议（已被 §15.5 定案取代）

1. **主站**：对象存储 + 自定义域名（静态网站托管 + CDN），发布机用云商 CLI/API 按
   `blobs/ → schema/ → index → manifest → sig` 顺序上传。收益是机器与证书交给云商、天然多副本，
   代价是计费与供应商锁定。
2. **镜像**：保留至少一个不同厂商/不同地域的镜像（可用同一 `sync` 从主站拉取），避免单云故障
   直接打断商店；客户端镜像列表按序回退。
3. **内网**：独立部署，内网 CA + 自签证书 + 内网静态服务器；与公网主站解耦，不共享域名与证书。
4. **过渡**：在托管落地前，临时隧道地址只作为临时镜像写入测试配置，不作为发布文档的正式地址。

### 15.4 当时的待定项（已在 §15.5 收口）

| # | 待定 | 影响 |
|---|---|---|
| 1 | 域名归属与备案/解析由谁负责 | 决定客户端默认镜像地址与证书签发方式 |
| 2 | 主站选哪家对象存储、预算上限 | 决定上传脚本与计费模型 |
| 3 | 是否现在就建第二镜像（同 `sync`，不同厂商/地域） | 决定可用性等级与额外成本 |
| 4 | 内网镜像是否在本阶段交付 | 决定内网 CA 与证书分发是否进入本轮范围 |

> 以上是评估阶段的问题清单；域名、对象存储与第二镜像均已按 §15.5 后置，不作为一期交付项。

### 15.5 决策定案（2026-11-12）

> 托管形态已定案并落到配置默认值；索引驱动安装（决策二）仍维持「先不做安装」的判断。

#### 决策一：长期托管形态

**结论（已定案）：一期用内网静态服务（`skiff-server serve`，纯 HTTP）作为默认源与分发形态；对象存储 + 自定义域名（+CDN）后置，不作为一期交付项。** 默认镜像为 `http://10.66.52.7:8080`（§11.1）；客户端契约只依赖 **路径稳定 + 字节不变 + 验签 + `seq` 防回滚**，不依赖 TLS、ETag、Range 或任何缓存语义。

选择内网 `serve` 先行的理由：

1. **交付差距最小**：产物、`publish` / `sync`、`serve` 都已实现，缺的只是一个稳定地址；一期不需要域名、备案、证书续期、云账号与计费，改一行默认值即可开工。
2. **信任本来就不在托管上**：清单有 Ed25519 签名、索引有摘要、`seq` 单调防回滚，HTTP 只影响传输保密性，不影响「内容不可伪造、版本不可回滚」。因此先不上 TLS 不构成安全缺口。
3. **切换成本极低**：换成对象存储/CDN 时客户端只改 `mirrors` 列表，不改变 §16 契约、不改代码路径；`sync` 可直接从 `serve` 拉取做镜像。反过来先上云则会提前引入发布脚本、逐对象头与计费这些与一期目标无关的复杂度。

对象存储/CDN 后置的理由：

1. **当前没有真实外网用户**：现有使用者与测试机都在内网，公网可达性与地域覆盖没有需求支撑。
2. **带宽与可用性尚未成为瓶颈**：目录是低频小 JSON + 不可变 blob，内网链路足够；尚无实测数据证明需要多副本与 CDN。
3. **后置不欠债**：托管形态与信任链解耦，晚做只增加一次 `mirrors` 配置变更，不会产生格式或协议迁移。

**改判条件（满足任一即重新评估对象存储/CDN 或多镜像）**：

- 出现真实公网用户，内网地址不可达；
- 实测内网链路带宽或可用性成为瓶颈（拉取超时/失败率有数据支撑）；
- 需要多地域访问或可用性 SLA，单机 `serve` 无法满足。

在改判前保持现状：默认内网 HTTP、单源；域名与证书只作为可选增强（HTTPS + `ca_bundle`），不进入一期验收。

#### 决策二：索引驱动安装（模板 / 运行环境）

**结论：现在不做安装，先做「只读预检/提示」；等发布面 R3 与撤回链路更完整后再评估「可安装」。**

支持理由：

1. **信任链会被实质扩大**：现在远端清单只影响「展示」，安装仍走内核内置目录；一旦索引驱动安装，远端数据就影响本机装机行为，必须补齐摘要/签名校验、权限确认、审计归属、失败回滚与离线降级，任何一环缺失都是安全或可用性缺口。
2. **收益边界目前很窄**：官方模板/运行环境数量少、内核发版频率不低，「等一个内核版本」的痛点尚未被真实用户场景证实；相反，装错版本、升级回滚、已装版本比对的复杂度立刻出现。
3. **R1/R2e 的 `yanked`/`revoked` 只够「拒绝新装」，不够「装完回滚」**：已装实例的降级/替换、数据目录兼容、失败恢复都没有定义；把这些塞进 R2 收尾会破坏已冻结的分工。

反对理由：

1. **数据驱动的价值是真实的**：模板与运行环境本质是数据，让每次小改动都等内核发版，长期会拖慢目录更新节奏。
2. **客户端已有大半基础**：镜像多源、`ca_bundle`、摘要校验、签名校验、离线快照都已具备，补「下载 + 落盘」并非从零开始。
3. **越晚做迁移成本越高**：等目录条目变多、用户装机量上来后再改安装语义，兼容与回滚成本会更大。

如果只做最小一步：

- 客户端只做**只读预检与提示**：索引里有更新版本时在商店标「可升级」，并显示 `min_kernel` 兼容性结论；不下载、不执行、不改已装状态。这样既不扩大信任面，又能验证「提示」是否真的解决了等待痛点，为是否进入安装阶段提供依据。

> 决策一已落到配置默认值与 §11/§16 契约；决策二只记录判断，不产生实现计划。两项都不改变 §16 与 `/api/**` 的接口边界。

## 16. 静态服务契约（`skiff-server serve`）

> 定位：终态「开发者提交 → 管理员审核 → 生成静态产物 → **静态服务**」里**当前只做最后一环**。
> `serve` 把已生成的产物目录原样用 HTTP 提供，不做写入、鉴权、审核与上传；`/api/` 只保留命名空间。
> 它是**一期定案的默认源形态**（内网 HTTP，§15.5）；客户端对它没有特殊分支，
> 与对象存储/CDN/`python -m http.server` 完全同构，换托管只改 `mirrors`。
>
> **定位：测试与自建分发用，不是唯一的对外分发形态。** 正式对外分发由后续部署形态决定
> （Docker 镜像或 systemd 服务托管同一批静态产物）；更换托管方式不改变 §16.2/§16.3 的客户端契约。
>
> 本节是契约的**唯一来源**：实现（§14 的 `serve.rs`）、客户端（`crates/app/src/catalog`）与测试都以此为准。

### 16.1 启动形态与传输

```bash
skiff-server serve --dist <dir> [--listen 127.0.0.1:8443] \
    [--tls-cert <pem> --tls-key <pem> | --http] [--access-log [<path>]]
```

- `--dist` 必填，指向 `build` / `sync` 产出的目录根（内含 `channels/` 与 `blobs/`），启动时校验其存在且可读。
- **默认 https**，且不提供任何隐式降级：既没给 `--tls-cert/--tls-key`、也没带 `--http` → 启动失败并打印用法提示；
  只给其中一个证书参数 → 启动失败。
- `--http` 是**显式**开关，用于本机/内网分发（一期默认源就是内网 HTTP，§15.5）；
  启动仍打印既有警告 `警告：--http 仅测试用，无 TLS 与证书校验，不要用于任何对外地址`——
  这是对「不要裸奔到公网」的提醒，不改变内网 HTTP 的定案。`--http` 与证书参数互斥。
- 传输实现复用仓库既有 rustls/ring（经 `ureq::rustls`，版本与客户端一致），不引入 axum/hyper 等框架。
  线程模型：每连接一线程，处理一个请求后关闭（`Connection: close`），无 keep-alive、无 HTTP/2、无压缩。
- 只读：没有任何写入路由；进程不持有锁文件，不修改 `--dist` 内任何文件。
- **可选访问日志（默认关闭）**：`--access-log <path>` 追加写该文件（启动时验证可写，之后每行重新打开，
  兼容 logrotate）；裸写 `--access-log` 或 `--access-log -` 写 stderr（systemd 下即 journal）。
  不指定时**不产生任何额外输出或 IO**（不建文件、不写日志）。每请求一行：

  ```
  Wed, 11 Nov 2026 10:00:00 GMT GET /channels/stable/manifest.json?x=1 status=304 bytes=0 cache=inm range=none
  ```

  字段依次为：UTC 时间、方法、**原始 target（含查询串）**、状态码、实际写出的响应体字节数、
  `cache=none|inm|ims`（304 的命中来源）、`range=none|<a-b>|unsatisfiable`（实际生效的区间）。
  目标串中的控制字符替换为 `?`；**不记录请求头全文**，不会把凭据写进日志；**写日志失败被忽略**
  （不影响响应、不 panic）。该可选项**不改变任何协议语义**：不新增/修改请求头、状态码、304 判定与缓存头，
  只用于服务端取证（例如确认 304 确实由 `If-None-Match` 命中，而非客户端缓存推断）。

自签证书（**openssl 辅助说明**，不在程序内生成；`<IP>` 换成客户端访问用的地址）：

```bash
# 1) 自签 CA（客户端 [catalog].ca_bundle 指向 catalog-ca.pem）
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
  -keyout catalog-ca.key -out catalog-ca.pem -days 3650 -subj "/CN=skiff-catalog-test-ca" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"

# 2) 服务器证书（SAN 必须包含客户端实际访问的 IP/域名）
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
  -keyout server.key -out server.csr -subj "/CN=<IP>"
printf "subjectAltName=IP:<IP>\nextendedKeyUsage=serverAuth\n" > server.ext
openssl x509 -req -in server.csr -CA catalog-ca.pem -CAkey catalog-ca.key -CAcreateserial \
  -out server.pem -days 825 -extfile server.ext

skiff-server serve --dist ./dist --listen 0.0.0.0:8443 \
  --tls-cert server.pem --tls-key server.key
```

客户端侧：一期内网 HTTP 默认源为 `[catalog].mirrors = ["http://10.66.52.7:8080"]`，不需要证书。
若改用 HTTPS + 自签证书，则 `[catalog].mirrors = ["https://<IP>:8443"]` 且
`[catalog].ca_bundle = "/etc/skiff/catalog-ca.pem"`（语义见 §4.5；CA 只追加信任锚，校验不放松）。

**取二进制**：release 流水线与安装包同轮按架构产出 musl 静态裸二进制
`skiff-server-x86_64-unknown-linux-musl` / `skiff-server-aarch64-unknown-linux-musl`
（不在安装包内、不装进 `/usr/local/bin`），并列入 Release 级 `checksums.txt`。
测试机没有 Rust 工具链时直接下载运行：

```bash
sha256sum -c checksums.txt                       # 安装包与 skiff-server-<target> 一并校验
chmod +x skiff-server-x86_64-unknown-linux-musl
./skiff-server-x86_64-unknown-linux-musl serve --dist ./dist --listen 0.0.0.0:8443 \
  --tls-cert server.pem --tls-key server.key
```

该二进制只是把产物目录托管起来的一种方式；正式对外托管后续按部署形态决定（Docker 或 systemd），
替换掉它不影响客户端。

### 16.2 路径布局

| 请求路径 | 对应文件（相对 `--dist`） | 说明 |
| --- | --- | --- |
| `/channels/<channel>/manifest.json` | `channels/<channel>/manifest.json` | 规范路径，客户端现行拼接 |
| `/channels/<channel>/manifest.json.sig` | 同名 | 分离签名 |
| `/channels/<channel>/index.json` | 同名 | 索引 |
| `/<channel>/manifest.json` 等三件 | `channels/<channel>/…` | **短别名**，与规范路径等价（兼容「渠道目录 `/stable/`」写法） |
| `/blobs/sha256/<hex64>` | `blobs/sha256/<hex64>` | 内容寻址，`hex64` 必须是小写 64 位十六进制 |
| `/api/**` | — | 只属于 §17 的发布面（独立子命令 `skiff-server api`，R0/R1 已实施）；`serve` 不实现也不代理，**一律 404**，不属于本契约 |
| 其他（`/`、目录、`schema/`、`catalog/`、未知文件） | — | 404；不列目录、无入口页 |

- `<channel>` 只认磁盘上 `channels/<name>/manifest.json` 实际存在的目录（当前产物为 `stable` / `night`）；
  不存在的渠道名 → 404。
- **不支持 `cur` / `latest` 别名**：客户端已按渠道名固定路径；别名会让同一 URL 的 ETag/缓存语义随「当前最新」
  漂移，也无法与 `seq` 单调校验建立稳定对应。需要「跟随最新」时由运维改客户端 `channel`，或由发布流程
  同步复制产物，而不是让服务端维护可变指针。
- 三个 JSON 与签名必须同渠道同版本；服务端不校验其内部一致性（那是 `check`/`sync` 的职责），也不改写内容。

### 16.3 方法、条件请求与响应头

> 下表描述 `serve` 的**实现行为**。其中 `ETag` / `If-None-Match` / `If-Modified-Since` / `304` / `Range`
> 是**服务端可选优化**，不是客户端契约要求：客户端不作正确性依赖（§16.5），刷新节奏由自己的 TTL 决定
> （§11.2）。`serve` 的实现保留不动，其它静态托管不支持这些头也完全兼容。

| 项 | 规则 |
| --- | --- |
| 方法 | 仅 `GET` / `HEAD`；其他方法一律 `405` + `Allow: GET, HEAD`（先判方法，再看路径） |
| `ETag` | （可选优化）**强校验**，格式 `"sha256:<hex>"`；hex = 响应实体字节（200 时为整个文件；blob 时等于文件名 hex）的 SHA-256 |
| `If-None-Match` | （可选优化）与 `ETag` 做强比较（忽略可选 `W/` 前缀；`*` 匹配任意）；命中 → `304`，无 body、无 `Content-Length` |
| `If-Modified-Since` | （可选优化）仅在没有 `If-None-Match` 时生效；比较 `Last-Modified`（截断到秒），`mtime <= IMS` → `304` |
| `Last-Modified` | （可选优化）文件 mtime，格式 `Sun, 06 Nov 1994 08:49:37 GMT`；200/206/304 都带 |
| `Content-Type` | `manifest.json` / `.sig` / `index.json`：`application/json; charset=utf-8`；blob：`application/octet-stream`；错误体：`text/plain; charset=utf-8` |
| `Content-Length` | 200/206 必带（206 为区间长度）；`HEAD` 与 200 相同；304 不带 |
| `Range` | （可选优化）支持**单个** byte range：`bytes=a-b`、`bytes=a-`、`bytes=-n`；`206` + `Content-Range: bytes a-b/len`。多区间（含 `,`）或 `HEAD` 忽略 Range，按 200 返回整个实体 |
| `Cache-Control` | JSON 与 `.sig`：`no-cache`（每次校验 ETag）；blob：`public, max-age=31536000, immutable` |
| `Accept-Ranges` | `bytes`（200/206 都带；304 也带） |
| `Connection` | `close` |
| `Content-Encoding` | 不压缩、不协商 |

状态码与触发条件：

| 码 | 触发条件 |
| --- | --- |
| `200` | 命中白名单文件；无 Range 或 Range 被忽略 |
| `206` | 合法且可满足的单区间 Range |
| `304` | `If-None-Match` 命中，或（无 `If-None-Match` 时）`If-Modified-Since` 命中 |
| `404` | 路径不在白名单（含 `/api/**`、`schema/**`）、文件不存在、是目录、`<channel>` 不存在、blob 名不合法 |
| `405` | 方法不是 `GET`/`HEAD` |
| `416` | Range 语法非法或不可满足（如 `a > b`、`a >= len`、`-0`）；响应带 `Content-Range: bytes */<len>` |
| `500` | 读文件失败或其他内部错误；只返回固定短文本 `internal error` |

- `404` 与 `405` 的 body 分别为 `not found` / `method not allowed`，不回显请求路径。
- 条件请求只用于省流量：**客户端正确性不依赖 304 或任何缓存**，缓存丢失、镜像不支持条件请求、每次 200 全量返回都必须能正常工作；客户端自身的刷新间隔见 §11.2。

### 16.4 安全边界

- **路径穿越**：先按 `%XX` 做一次百分号解码，再拒绝 `..`、`.`、空段、`\`、NUL 与绝对路径；
  拼出的路径必须 `canonicalize` 后仍位于 `--dist` 根之内（符号链接指向根外同样 404）。
  解码后出现二级编码（如 `%252e`）按普通文件名处理，仍受上述规则约束。
- **只服务白名单的普通文件**：不是目录、不是设备/管道；未知文件名即使存在也不服务。
- **blob 名校验**：必须匹配 `^[0-9a-f]{64}$`；大写、长度不符、非 hex 一律 404，绝不落到文件系统探测。
- **目录列表关闭**：任何以 `/` 结尾、指向目录或不以白名单文件名结尾的请求都 404。
- 不返回 `Server` 版本头；错误页不含堆栈、绝对路径或文件系统信息。
- 启动时把 `--dist` 解析为绝对路径并 `canonicalize` 一次，后续所有比较基于该根。

### 16.5 版本与兼容

- **路径稳定**：`channels/<channel>/`、`blobs/sha256/` 一旦发布不改；新增渠道只是新增目录，不影响既有路径。
- **只增不删**：新增响应头、新增可选请求头、新增渠道都属于兼容变更；删除/改名路径、改变 `ETag` 语义、
  改变 304/416 判定属于破坏性变更，必须同步升级客户端与本节。
- **客户端依赖点**（少而硬）：`GET {base}/channels/{channel}/manifest.json` → 验签 → `index.json`
  （摘要/大小对齐）→ `blobs/sha256/<hex>`（sha256 与 size 校验）。除此之外不依赖本服务的任何行为。
- 本契约变更必须同时改：本节、`tools/skiff-server/src/serve.rs`、`crates/app/src/catalog` 与
  `docs/TEST_MATRIX.md` 的 STORE 条目；`serve` 的实现细节不是规范，本节才是。

### 16.6 测试覆盖

- `tools/skiff-server/tests/serve.rs`：真实 TCP 上覆盖条件请求 304、单区间 206 与 416、`404`（含 `/api/`、
  目录、未知文件）、`405`、`HEAD` 无 body、路径穿越与非法 blob 名拒绝、`--http` 明文与默认 https 的启动差异；
  访问日志另覆盖「一请求一行」与裸 `--access-log` 落 stderr。
- `serve.rs` 单测覆盖日志行本身：`status=304 bytes=0 cache=inm`、`status=206 … range=0-3`、`status=405`；
  未开启时不创建文件、不产生输出；启动后日志目录消失（写失败）时响应仍完整。
- TLS 链路：进程内生成自签 CA + 服务器证书（`rcgen`），`serve` 以 https 启动后用 ureq 指定该 CA 建连成功，
  未导入 CA 时失败（与 §4.5 的客户端口径一致）。
- `serve` 不改变产物：测试只读校验运行前后文件字节不变。

## 17. 发布面 `/api/**` 接口（R0/R1/R2a–R2e 已实施）

> 定位：终态链路「开发者提交 → 管理员审核 → 生成静态产物 → 静态分发」里，**分发面已由 §16 定稿并上机闭环**，
> 本节补**发布面**的接口边界。**R0/R1/R2a/R2b 已在 `tools/skiff-server` 落地**（`api`、`notify` 与 `release` 子命令），
> R2c/d/e 仍是草案。客户端不访问本节任何接口。
> 一句话原则：**客户端只读静态文件，不访问任何 `api`；API 只服务人与发布流水线，且永不持有清单签名私钥。**

### 17.1 与既有口径的关系（含一处措辞冲突）

| 既有条目 | 本节的关系 |
|---|---|
| §16 `serve` 只读静态、`/api/**` 一律 404 | 不变。API 是**独立子命令与独立进程**（`skiff-server api`），独立监听、默认关闭；`serve` 不实现、也不转发任何 API |
| §5.1 源数据 + PR/CI 审核 | 保留为离线/自托管路径。API 只是把「合并 PR」换成一个受控写入入口，落盘仍是同一种源数据格式（`deploy/skiff-server/catalog/`） |
| §14.1/§14.2 签名与 `publish` | 不变。API 不读 `SKIFF_SIGNING_KEY`、不签 manifest、不写 `--dist`；签名与产物生成仍只由 `publish` 在受控环境完成 |
| §0 / §1.2 / §1.4 的分发面表述 | 已对齐：§0「形态」把**现状**（静态分发、`serve` 只读、`/api/**` 一律 404）与**已落地的可选审核 API**（R0，默认关闭）分开写；§1.2/§1.4 明确客户端不依赖 `/api/**` |
| §15 托管未决 | 不受影响：API 与静态托管解耦，换托管不改变本节接口 |

### 17.2 角色、凭据与签名密钥归属

| 主体 | 凭据 | 能做的事 | 私钥位置 |
|---|---|---|---|
| 开发者 | Ed25519 作者密钥对，**与 `.wbpkg` manifest 的签名密钥是同一把** | 提交新版本、查看自己的提交、撤回待审提交 | 开发者本机，永不上传 |
| 审核者（管理员） | Ed25519 审核密钥对，只用于签 API 请求 | 查看待审、通过、驳回 | 审核者本机 |
| 发布流水线 | `SKIFF_SIGNING_KEY`（§14.1） | 生成并签名清单、自检、落盘 | CI secret 或离线发布机，API 进程不可见 |
| Skiff 客户端 | 无 | 只 GET 静态文件（§16.5） | 不适用 |

- **身份即公钥**：主体标识沿用 `key_id = sha256(公钥)` 小写 hex（§3.3 与 [§4.1](sections/04-plugin-package.md) 同一口径），服务端只存公钥；没有账号、密码、Cookie 与会话表。
- 注册是**手工配置**而非自助：作者公钥与命名空间前缀写入 `authors.json`，审核公钥写入 `reviewers.json`（均 0600，启动加载）；R0 不提供注册接口。
- **包签名绑定提交者**：上传包内 manifest 的 `key_id` 必须等于请求签名所用的 `key_id`，否则 403。「谁能提交这个包」由既有的包内签名回答，不引入第二套作者身份。
- **三把钥、三种权**：作者钥签包（不进服务端）、审核钥签请求（与包签名无关）、发布钥签清单（只在受控环境）。泄漏影响面与轮换节奏不同，不能合并（§7.2）。
- 吊销与轮换：
  - 作者钥：`authors.json` 标记 `disabled` → 新提交 403；已发布版本按需进 `revoked` 并递增 `seq`（§5.3）。换钥按新作者登记，已发布历史不从索引删除。
  - 审核钥：删白名单条目即可；已执行的审核动作以审计日志为准，不做追溯撤销。
  - 发布钥：按 §14.1 先发内置新公钥的客户端，再切 CI secret。

请求签名（每请求一次，无会话）：

```text
signed = "SKIFF-SERVER-API-v1\0" + method + "\n" + path + "\n" + query + "\n"
       + body_sha256 + "\n" + timestamp + "\n" + nonce
```

- 请求头：`X-Skiff-Key-Id`、`X-Skiff-Timestamp`（Unix 秒）、`X-Skiff-Nonce`（≥128 bit 随机）、`X-Skiff-Signature`（Ed25519 hex）。
- `body_sha256` 为请求体原始字节的 sha256（空体用空串摘要常量）；`query` 按原样参与签名，不重排。
- 时钟窗 ±300 秒（设计目标，待实测）；`nonce` 去重表按小时滚动落 `state/nonces/`，保留 ≥ 10 分钟，重启后仍能拦截窗口内重放。
- 签名域前缀与 §14.2 的 `SKIFF-SERVER-MANIFEST-v1\0` 分开，避免跨协议重放。

### 17.3 提交侧

提交内容（`multipart/form-data`，两个部分）：

**明确选择：只收签名 `.wbpkg` + 条目元数据**，不接受「清单 + 散装 bundle」。包格式、验签与安装链路已由 [§4.1](sections/04-plugin-package.md) 定稿，另开一种分发物会让客户端出现两条安装路径。

| 部分 | 内容 | 说明 |
|---|---|---|
| `entry` | 条目元数据 JSON：`summary`、`requires`、`changelog_url`、`license`、`source_url`、目标 `channel` | 与源数据条目同形；不填派生字段 |
| `package` | 签名 `.wbpkg` 原始字节 | 单包上限 64 MiB（设计目标，待实测） |

- **不采信客户端给的派生字段**：`pkg_sha256`、`pkg_url`、`size` 由服务端按字节派生（与 §10.1 `asset` 口径一致）；带上了必须与派生值一致，否则 422。
- 服务端校验顺序（任一失败即 422 或 409，且不落任何状态）：
  1. 包大小与 `sha256`；
  2. `.wbpkg` 结构与验签（复用 `skiff-plugin verify` 语义），`key_id` 必须等于请求者；
  3. manifest schema、`api`、反向域名 `id`、`requires` 合法、`(id, version)` 语义（[§4.1](sections/04-plugin-package.md)）；
  4. `id` 是否落在该 `key_id` 的命名空间前缀内（`authors.json`），否则 403；
  5. 版本不可变：同 `(id, version)` 已 `published`、或已在 `pending`/`approved` 且摘要不同 → 409。
- 落状态：`pending`（待审）。作者可在 `pending` 撤回为 `withdrawn`；进入 `approved` 之后不可撤回。
- 记录类型 `kind`：`plugin`（缺省）/`app`/`runtime`。插件记录不落该字段，旧 `state/submissions/*.json` 与旧响应字节不变（§17.12）。
- 幂等与重传：

| 情形 | 语义 |
|---|---|
| 同 `(id, version, sha256)` 重复提交 | 200 + 同一 `submission_id`，状态不变，不重复通知 |
| 同 `(id, version)` 不同 `sha256`，尚未发布 | 409，作者需撤回后重新提交 |
| 已 `published` 的 `(id, version)` | 409，版本不可变（§2.4） |
| 上传中断 | 不做分块续传，整体重传；服务端按摘要判定是否已有相同 blob |

**应用模板提交（R2c，`POST /api/v1/apps`）**：载荷同样是两个部分，但第二段是 compose 原文。

| 部分 | 内容 | 说明 |
|---|---|---|
| `entry` | `id`、`version`、`summary`、`min_kernel`、`images[{ref,digest}]`、`env[]`、`ports[]`、`volumes[]` | 派生字段（`asset`/`compose_sha256`/`compose_url`/`size`）一律由服务端生成，带上即 422（`deny_unknown_fields`） |
| `compose` | 严格子集内的 compose YAML 原文（UTF-8，≤ 1 MiB） | 校验复用 `skiff-compose`，与运行时/xtask 同一实现 |

- 信任模型按裁决 1：**API 请求签名 + 服务端摘要 + 审核**，不要求作者额外签名信封；服务端是提交时刻的信任边界。
- compose 原文落在 `state/submissions/<sid>/compose.yaml`；发布时按 `sha256` 复制成 `catalog/assets/<sha>.yaml` 并作为内容寻址 blob 进入产物（`compose_url = blobs/sha256/<sha>`）。
- `sid` 派生对应用加 `app` 域前缀（`app\0id\0version\0sha256`），与插件同 id 同版本不撞 id。
- 版本规则与插件一致：同 `(id, version)` 不同摘要、或已发布 → 409；同摘要重复提交 → 200 同一 `sid`。
- 审核/发布复用 `submissions/{sid}/approve|reject|withdraw` 与 `release`，事件 `subject.kind = "app"`；一次 `release` 只 +1 `seq`，插件与应用同批进入同一索引。
- **R2c 不做安装**：客户端只从签名索引展示 `apps[]`（商店「应用模板」分区），安装仍走内置目录，索引不驱动安装。

**运行环境提交（R2d，`POST /api/v1/runtimes`）**：JSON 载荷，无二进制资产，只做提交/审核/展示。

| 字段 | 说明 |
|---|---|
| `kind` | `php` / `node` / `rust` |
| `version` | 明文版本号（如 `8.5`） |
| `image` / `image_digest` | 镜像引用与 `sha256:` 摘要；**提交后不可改** |
| `eol` | 可选，`YYYY-MM-DD` |
| `api_min` | 兼容的内核 API 版本 |
| `default` | 提交时必须为 `false`；发布后只能通过元数据修订设置 |

- 身份是 `(kind, version, image_digest)`；同 `(kind, version)` 已有别的 digest、或已发布 → 409。
- **元数据修订**（独立动作）：`POST /api/v1/submissions/{sid}/revise`，body 只接受 `eol` / `default`（`deny_unknown_fields`，带 `image`/`image_digest` 一律 422），仅审核者可调用，只有 `published` 记录可修订。修订先落 `state/runtime_revisions/<id>.json` 并写审计 `action=revise`，由下一次 `release` 应用到源数据并递增 `seq`。
- **`default` 唯一性在 release 时校验**：同 kind 出现多个 `default=true` 时发布失败（`release.failed`），不会生成相互矛盾的索引。
- 运行环境只展示，不驱动安装：客户端从索引 `runtimes[]` 渲染版本/EOL/默认标记，安装仍走内置目录。

### 17.4 审核侧

```text
pending --approve--> approved --release ok--> published
   |                    |
   |reject              |release fail
   v                    v
rejected            approved（保留 last_error，可重试，不发布）
pending --withdraw（作者）--> withdrawn
published --yank/revoke（R1 走 API）--> 条目保留 + 递增 seq
```

- 审核动作只改状态并写入源数据，**不直接改 `--dist`**；产物仍由 `publish` 生成。
- **不得自我审核**：请求者 `key_id` 与提交者相同 → 403（R0 规则，避免单人闭环）。
- 驳回必须带 `reason_code`（`invalid_signature`、`schema`、`id_ownership`、`policy`、`other`）与可选说明；作者按新摘要重提同一 `(id, version)` 允许（尚未发布，不违反版本不可变）。
- `seq` 单调：**API 不预分配 `seq`**。发布 job 在受控环境读线上当前值后 +1（§14.2 的 `--prev` 语义不变），同渠道发布串行；并发审批后到者自然拿到后一个 `seq`。
- 只增不删：索引条目只追加；下架用 `yanked`（条目保留、拒绝新装），安全处置用 `revoked`（摘要黑名单），两者都递增 `seq`（§5.3）。R1 起 `yank` 与密钥吊销走 API：API 只写 `state/takedowns/`，由 `release` 把 `yanked`/`revoked[]` 写进源数据并递增 `seq`。
- **R2e 分 kind 撤包**：`POST /api/v1/plugins|apps|runtimes/{id}/versions/{version}/yank`，审核者调用，body 可带 `reason`；重复撤包 200 幂等，目标不存在 404。
  - 插件保持 R1 语义：源条目 `yanked=true` + `yank_reason`，索引 `plugins[]` 带标记，`revoked[]` 继续只表示插件密钥吊销。
  - 应用与运行环境没有作者密钥：撤包在打 `yanked` 标记的同时，把该版本登记进独立命名空间 `catalog/<ch>/revoked_apps.json` / `revoked_runtimes.json`，索引输出 `revoked_apps[]` / `revoked_runtimes[]`（应用记 compose 摘要、运行环境记镜像 digest 的 hex），`revoked[]` 不参与。
  - 撤包记录 `state/takedowns/*.json` 新增可选 `entry_kind`（缺省 `plugin`，旧记录无需迁移）；`release` 按 kind 写回对应源目录并回写提交的站内提示。
  - 兼容：`revoked_apps[]` / `revoked_runtimes[]` 为空时不落盘（`skip_serializing_if`），插件索引与旧客户端看到的字节不变；客户端忽略不认识的数组。
- 审计：提交、审核、发布结果追加到 `state/audit.jsonl`（JSON Lines：`key_id`、动作、`submission_id`、结果、时间）；这是发布面审计，与内核 `audit` 表无关。

### 17.5 产物生成

审核通过后的唯一写路径（与 PR 路径完全同构）：

```text
approved 提交 → 写入源数据（等价于合并 PR）→ 触发发布 job
             → publish：生成 index + manifest → 受控环境签名 → 内存严格自检
             → 落盘 staging → 按 §14.4 顺序上线（blobs → schema → index → manifest → sig）
             → published（回写提交状态与 seq）
```

- **签名位置：受控环境，不在 API 进程**。API 只产出待签名的源数据与候选索引；私钥仍只在 CI secret 或离线发布机（§14.1）。不新增「在线签名服务」，也不再造一把 API 专用清单密钥。
- **布局与 §16 完全一致**：产物由 `publish` 生成，必然是 `channels/<ch>/{manifest.json,manifest.json.sig,index.json}` + `blobs/sha256/<hex64>` + `schema/`（§16.2），不存在第二套布局。
- **失败不污染**：`publish` 在内存完成全部校验后才写输出目录（§14.2），失败时线上目录不变；上线顺序保证客户端读到的清单永远指向已存在的 blob（§14.4 第 5 步）。`manifest.json` 与 `.sig` 的极小不一致窗口按 §14.3 处理（客户端验签失败保留旧快照）。
- **发布幂等**：发布 job 以 `(channel, submission_id, sha256)` 为幂等键；重试前先检查源数据/线上索引是否已含该摘要，已含则只回写状态，不重复递增 `seq`。
- **没有内容回滚**：`seq` 单调意味着线上内容只能前进；发现坏版本只能 `yanked`/`revoked` 并发布更高 `seq`（§14.4 末条）。「换回旧字节」属于托管层操作，客户端会拒绝低 `seq`。

### 17.6 与 Skiff 客户端的接口边界

- **客户端不依赖 `/api/**`**，一个字节都不需要；依赖点仍是 §16.5 的三跳：`manifest.json` → `index.json` → `blobs/sha256/<hex>`。
- 吊销、yank、新版本提示全部在签名索引里（§2.3 条目字段与 `revoked`），客户端不查询在线服务即可完整工作；离线、内网、纯镜像部署与今天行为一致。
- `serve` 的 `/api/**` 保持 404（§16.2），避免分发节点意外暴露写入面；API 默认关闭、单独监听、单独部署。
- 未来若出现「客户端在线巡检」类需求，必须新开评审，不得把 API 塞进客户端现有拉取路径，也不得让客户端为拉目录而持有任何发布侧凭据。

### 17.7 阶段划分与最小可用集

本节阶段与 §8 的 P0–P4 是**两条独立轴**：§8 是分发与客户端，本节是发布面。为避免与既有编号冲突，本节用 R0/R1/R2。

**R0（最小可用，已实施）**：一名开发者能提交、一名管理员能审核、产物自动生成并被现有 Skiff 拉到。

| 方法 | 路径 | 主体 | 幂等 | 说明 |
|---|---|---|---|---|
| POST | `/api/v1/submissions` | 开发者 | 按 `(id, version, sha256)` | multipart：`entry` + `package`；返回 `submission_id` 与 `state=pending` |
| GET | `/api/v1/submissions?state=pending` | 审核者 | — | 待审列表；开发者调用只返回自己的 |
| GET | `/api/v1/submissions/{sid}` | 提交者或审核者 | — | 状态、派生字段、驳回原因、发布 `seq` |
| POST | `/api/v1/submissions/{sid}/approve` | 审核者 | 按 `sid` | 202；转 `approved` 并触发发布 job |
| POST | `/api/v1/submissions/{sid}/reject` | 审核者 | 按 `sid` | body 带 `reason_code` 与可选说明 |
| POST | `/api/v1/submissions/{sid}/withdraw` | 提交者 | 按 `sid` | 仅 `pending` 可撤回 |

R0 的运维侧（不新增 API）：`authors.json` 与 `reviewers.json` 手工维护；发布 job 用「扫描源数据变更」或「显式触发」二选一（实现时定，语义等价）；状态与审计落 `state/` 文件，不引入数据库（`tools/skiff-server` 刻意保持零原生 C 依赖，§10.1）。

**R1（已实施）**：撤包与密钥吊销 API 化、站内状态通知、审计查询。模板与运行环境仍留 R2。

| 方法 | 路径 | 主体 | 幂等 | 说明 |
|---|---|---|---|---|
| POST | `/api/v1/plugins/{id}/versions/{version}/yank` | 审核者 | 按 `(id, version)` | 202；body 可带 `reason`（1..=200 字符）；重复 200；未知版本/已撤回 404/409 |
| POST | `/api/v1/keys/{key_id}/revoke` | 审核者 | 按 `key_id` | 202；重复 200；未登记 key 404；吊销立即生效 |
| GET | `/api/v1/audit` | 作者或审核者 | — | 查询参数 `sid`/`key_id`/`action`/`result`/`since`/`until`/`limit`/`offset`；作者默认只看自己，显式查他人 403 |

**索引契约（不升 schema）**

- `plugins[].yanked` 与 `revoked[]` 是 `skiff.server.index/v1` 的既有字段；R1 只开始填充，schema 版本不变。
- R1 后续增量新增**可选**字段 `plugins[].yank_reason`（string，1..=200）：旧客户端忽略未知字段，旧索引不出现该字段，因此**仍不升 schema 版本**。有原因时客户端展示原因，无原因时保持通用文案。
- 旧版客户端读到未知字段一律忽略；读到 `yanked=true` 或 `revoked` 命中时，按已有拒绝语义给原因（不是解析失败）。R1 前的旧索引（`yanked=false`、`revoked=[]`、无 `yank_reason`）解析与安装行为完全不变。
- `seq` 单调与「已发布只增不删」继续成立：撤包是给既有条目打 `yanked` 标记并追加 `revoked[]`，**不物理删除**；每次生效都递增 `seq`，客户端防回滚不受影响。

**状态机与边界**

- `published → yanked` 由**审核钥**触发，不可逆；恢复只能发更高版本，不提供 `unyank`。
- 密钥吊销（`revoked`）按 `key_id` 一次性覆盖该 key 的历史版本：源条目 `yanked=true` 且追加 `revoked[]`；`revoked` 不可逆，也不能再审——该 key 之后既不能提交也不能审核，换钥按新作者重新登记。
- 已有安装者不自动卸载；`GET /api/v1/submissions/{sid}` 与内核插件页从索引的 `yank_reason` / `revoked[].reason` 读取撤包原因并提示（无原因时用通用文案）。
- 撤销/吊销只写 `state/` 记录，产物与签名仍由 `release` 在受控环境生成。

**通知的最小形态**

不引入 SMTP、webhook 或第三方依赖：撤包/吊销立即落 `state/audit.jsonl`，同时把状态回写到提交记录 `takedown` 并在插件页展示。通知失败只影响可见性，不影响撤包生效；同一 `(id, version)` 重复请求幂等，不重复产生记录。确需外推时再新增可重试的 webhook，R1 不做。

**审计查询**

- 数据源是 `state/audit.jsonl`，倒序返回；`limit` 默认 50、上限 200，`offset` 分页，返回 `total` 与 `next_offset`。
- 过滤：`sid`（匹配 `submission_id` 或 release 行的 `submission_ids[]` 任一元素，可串起 submit / approve / release）、`key_id`、`action`、`result`、`since`/`until`（UTC RFC3339）。
- 鉴权：审核者看全部；作者默认只看自己，显式查他人 `key_id` 一律 403。
- 返回字段只有 `at`/`key_id`/`action`/`submission_id`/`submission_ids`/`result`，**不含签名、包内容或请求体**。

**R2（分步实施）**：R2a webhook 事件通知、R2b 提交记录 `kind` 抽象、R2c 应用模板提交/审核/发布、R2d 运行环境提交与元数据修订、R2e 分 kind 撤包与独立吊销命名空间已实施（§17.11–§17.15）。裁决：apps 接受「API 请求签名 + 服务端摘要 + 审核」，不要求额外签名信封；compose 校验保持单一实现；runtimes 只做提交/审核/展示，不驱动安装；`eol`/`default` 允许独立元数据修订（image/digest 不变）；吊销用独立 `revoked_apps[]`/`revoked_runtimes[]`。组织级命名空间与发布 job 队列化仍不做。

统一错误体：`{"code": "...", "message": "...", "detail": {...}}`；`401` 签名缺失/无效/过期，`403` 密钥未登记、命名空间不匹配或自我审核，`409` 版本或摘要冲突，`413` 超限，`422` 校验失败，`500` 内部错误（不回显路径与堆栈）。

### 17.8 明确不做

- 账户体系（注册、登录、找回、邮箱验证）、多租户、组织与团队、细粒度权限；主体只有「已登记公钥」。
- 计费、付费内容、订阅、授权码。
- Web 控制台与前端页面；R0 只提供 HTTP 接口，管理界面后置。
- 客户端侧在线 API、遥测上报、在线 CRL/OCSP（吊销仍走签名索引，§7.3）。
- API 进程内签名、API 专用清单私钥、把 `SKIFF_SIGNING_KEY` 放进 API 环境。
- 包体 CDN/对象存储管理、评分评论举报等 UGC。

### 17.9 R0 落地说明（`tools/skiff-server`）

**进程与传输**

- 两个子命令：`skiff-server api`（六个端点）与 `skiff-server release`（发布 job，不在六端点内）。
- 不引 web 框架与数据库：沿用 `serve` 的 std 监听 + 每连接一线程；JSON 用 `serde_json`；状态是 `state/` 下的文件。
- HTTP 传输件（读请求头、写响应、TLS）抽到 `tools/skiff-server/src/http.rs`，`serve` 与 `api` 共用；`serve` 的路径白名单、条件请求、Range、访问日志**未改动**，`tests/serve.rs` 全绿。
- `api` 默认 `127.0.0.1:9443`，明文；TLS 由前置反代终止。非回环监听必须显式 `--allow-remote`，否则拒启。

**启动强校验（任一不满足即退出码 1）**

- `--state` 目录必须存在且 0700（Unix；Windows 由 ACL 兜底）。
- `--authors` / `--reviewers` 必须是普通文件且 0600，`schema` 固定，`key_id` 必须等于 `sha256(公钥)`。
- 运行期白名单每次请求重读：改文件即生效、无需重启；读取或解析失败按 503 失败关闭，绝不放行。

**请求与错误码**

- 签名域、四头、±300s 时间窗与 §17.2 一致；nonce 落 `state/nonces/<YYYYMMDDHH>`，保留 2 小时，重复即 401。
- 必须有 `Content-Length`（POST 缺失 → 411）；任何 `Transfer-Encoding`（含 chunked）→ 400；声明长度超 64 MiB → 413，**不读 body**。
- multipart 严格模式：boundary 1..=70 且字符集受限；每部分头部 ≤16 行、单行 ≤1024 B、总头部 ≤4096 B；只接受 `entry` 与 `package` 两个命名部分；**文件名只出现在头部，不参与任何路径拼接**。
- 401 签名缺失/无效/过期/重放；403 密钥未登记、命名空间不匹配、非审核钥调 approve/reject、自我审核、动别人的提交（统一 `forbidden` 体）；409 版本或摘要冲突；412 非法状态迁移与并发 approve 抢锁失败；422 包/元数据校验失败。
- 日志与审计（`state/audit.jsonl`）只记 `key_id`、动作、`sid`、结果与时间，**不记签名、不记包内容**。

**存储与状态机**

- `state/submissions/<sid>.json` + `<sid>/package.wbpkg`；`sid = sub-<sha256(id|version|sha256)[..24]>`，同一三元组天然幂等。
- `pending → approved | rejected | withdrawn`，`approved → published`；其余迁移 412；approve 用 `<sid>.lock` 独占，并发后到者 412。
- 被驳回/撤回后，同一摘要重提会把记录重新置为 `pending`（不新建）；已 `published` 的同版本一律 409。

**产物生成与私钥姿态**

```text
skiff-server api      → 只写 state/（提交、审核、审计），不读签名私钥
skiff-server release  → 写源数据 → 调既有 publish → 产出签名产物
```

- `release --staging-out <dir>`：只产出 staging，**不碰线上目录**，交发布机/CI 搬运（推荐）。
- `release --dist <dir> --swap`：显式换入线上目录，先备份旧目录、换入失败自动回滚。
- `seq` 基准：`--seq <n>` 由调用方给定，或从 `--prev`/`--dist` 的 manifest **只读**读取后 +1；缺省从 1 开始。
- 生产环境 `SKIFF_SIGNING_KEY` 只在离线发布机或 CI，分发主机只接收产物；**同机 `api` + `release --dist --swap` 仅测试姿态**，不得用于生产。
- `publish` 在内存完成全部校验后才写盘，失败时线上目录不变；`release` 失败也不回写 `published`。

**R0 测试**

- `tests/api.rs`：签名正例（提交→审核→发布出可拉取的 `manifest/index/blobs`）、篡改 body、错误域前缀、过期时间戳、重放 nonce、非审核钥 approve、自我审核、同摘要幂等、非法状态迁移、缺 `Content-Length`/`Transfer-Encoding`、超长 `Content-Length`、失败不污染 `--dist`、`serve` 对 `/api/**` 仍 404。
- 复用 `skiff-pkgfmt` 校验（与 `xtask verify-pkg` 同源），不另写验签。

### 17.10 R1 落地说明（`tools/skiff-server`）

- **端点与存储**：`yank` / `keys/{key_id}/revoke` / `audit` 三个新端点与 R0 六端点共用同一签名域与错误体。撤包记录落 `state/takedowns/<tak-…>.json`，密钥吊销落 `state/revoked_keys.json`；`api` 仍不读签名私钥、不写 `--dist`。
- **鉴权**：`authenticate` 在公钥一致性检查后、验签前查 `revoked_keys.json`，命中即 403；读取失败 503 失败关闭。密钥吊销即时生效，不等待发布。
- **发布 job**：`release` 同时处理 `approved` 提交与 `pending` 撤包记录。一次运行只递增一个 `seq`：先写源数据（新条目 / `yanked=true` + `yank_reason` / `revoked.json`），再 `publish`，最后回写 `published`、`released` 与提交记录的 `takedown`。没有待发布项时直接失败，不空转递增 `seq`。
- **并发**：yank 记录按 `(kind, id, version)` 稳定命名并用文件锁串行；重复 yank 返回 200 同一记录。发布 job 是 `pending → released` 的唯一写入者；并发到达的新 yank 留给下一次 `release`。
- **`revoked.json` 与签名覆盖**：`build` 读取 `<channel>/revoked.json` 填入 `index.revoked`；缺文件等价于空数组，产物逐字节一致；文件存在但 JSON 非法时构建直接失败。manifest 的 `index.sha256` 继续覆盖整份索引字节，因此吊销列表也被签名间接覆盖。
- **撤包原因**：`build` 把源条目的可选 `yank_reason` 原样带进索引；该字段不出现时输出与 R1 前逐字节一致。旧客户端忽略未知字段，因此不需要 schema 版本迁移。
- **客户端**：`skiff-app` 的索引模型对 `yank_reason` 用 `serde(default)`；`CatalogService::cached_takedown` 只读内存/本地缓存，不触网，有原因时返回原因，无原因时保持通用文案。安装侧仍在 `commands::store::prepare` 拒绝 `revoked` 与 `yanked` 并带出原因。
- **审计串联**：`release` 行携带 `submissions[]`；`GET /api/v1/audit` 解析为 `submission_ids[]` 并让 `?sid=` 命中任一元素，因此一个 sid 能同时追到 submit / approve / release。
- **R1 测试**：`tests/api.rs` 覆盖 yank 幂等/404/403、吊销后提交与审核 403、历史版本进入 `revoked[]`、审计分页/越权/sid 串联；`tests/p0.rs` 覆盖 `revoked.json` 缺省/非法/签名字节与 `yank_reason` 进入索引；`crates/app` 覆盖旧索引解析不变、新索引拒绝原因，以及「源数据 → 构建产物 → 客户端视图」的端到端用例。

### 17.11 R2a 落地说明：webhook 事件通知（`tools/skiff-server`）

- **形态**：业务侧只写 `state/outbox/pending/<event_id>.json`，`skiff-server notify` 独立进程负责投递；API 与 `release` 都不发起网络请求。`notify` 默认常驻轮询，`--once` 适合 systemd timer/CI。
- **事件集**：`submission.created`（`reopened` 带标记）、`submission.approved`、`submission.rejected`、`submission.withdrawn`、`submission.published`（带 `seq` 与 `submission_ids`）、`takedown.yanked`、`key.revoked`、`release.failed`。`duplicate` 不发；事件载荷不含包体、签名、请求体。
- **事件 id**：`evt-` + `sha256(type\0业务键)[..24]`。同一业务动作重复 emit 命中同一文件名，`create_new` 去重；`submission.published` 的业务键是 `channel:seq:release`。
- **订阅**：`state/webhooks.json` 手工维护，字段 `id/url/events[]/channels[]/enabled`；`events`/`channels` 为空表示全部。只允许 `https://`，默认拒绝回环/私有地址，内部目标需 `--allow-private`；本地联调才用 `--allow-http`。
- **投递**：at-least-once。2xx 成功；网络错误/超时/408/429/5xx 重试，其余 4xx 进 `dead/`；退避默认 5s/30s/2m/10m/1h/6h，最多 8 次，尊重 `Retry-After`。接收方按 `X-Skiff-Event-Id` 去重；不承诺事件顺序。
- **签名**：独立域 `SKIFF-SERVER-WEBHOOK-v1`，输入为 `域\0event_id\0timestamp\0sha256(body)`；私钥只在 `notify` 的 `SKIFF_WEBHOOK_SIGNING_KEY`，不复用 API 签名域。请求头：`X-Skiff-Event-Id`、`X-Skiff-Timestamp`、`X-Skiff-Signature: v1=<hex>`。
- **失败隔离**：outbox 写失败只记日志/返回 `Err` 给调用方忽略，业务响应不变；`notify` 与 API 是两个进程，投递失败不影响提交、审核、发布与撤包。
- **保留期**：pending 默认 30 天（超期转 dead）、dead 默认 90 天（超期删除），`NotifyOptions` 可配置；`notify.lock` 防止多实例并发投递，崩溃遗留锁超过 10 分钟视为过期。
- **R2a 测试**：`events` 覆盖确定性 id、重复 emit 去重、载荷不含敏感字段；`notify` 覆盖签名可验、5xx 重试后成功、4xx 不重试、超时重试并最终 dead、事件过滤；`tests/api.rs` 覆盖 outbox 不可写仍 201、`release.failed` 事件、submit/approve/release/yank 事件集合。

### 17.12 R2b 落地说明：提交记录 `kind` 抽象（`tools/skiff-server`）

- **模型**：`Submission` 新增 `kind: EntryKind`（`plugin` 缺省、`app`、`runtime`）。`#[serde(default)]` + `skip_serializing_if = "EntryKind::is_plugin"`：解析缺字段的旧记录得到 `plugin`，插件记录写盘与响应里都不出现 `kind`。
- **范围**：R2b 只做模型与兼容性，不新增接口。提交、审核、发布、撤包、事件、`sid` 派生、源条目与签名产物仍全按插件路径执行，字节不变。
- **后续用途**：R2c/R2d 按 `kind` 分别校验模板与运行环境，R2e 按 `kind` 撤包并写入 `revoked_apps[]` / `revoked_runtimes[]`；`revoked[]` 继续只表示插件。
- **R2b 测试**：`tests/api.rs::plugin_records_omit_kind_and_legacy_records_still_load` 断言插件提交响应与落盘记录都没有 `kind`、无 `kind` 的旧记录解析为 `plugin`、approve→release 后源条目与 `index.json` 都不出现 `kind`；其余 22 项 API 测试（签名、幂等、状态机、撤包、吊销、审计、事件）全绿，证明插件全链路行为未变。

### 17.13 R2c 落地说明：应用模板提交与发布（`tools/skiff-server`）

- **提交端点**：`POST /api/v1/apps`（`entry` + `compose`，见 §17.3）。作者身份仍由请求签名确立，`id` 必须落在 `authors.json` 的前缀命名空间内。
- **compose 校验单一实现**：直接依赖 `skiff-compose`（与 `skiff-docker`、`xtask` 同源），严格子集外的字段（如 `build:`）一律 422；服务端不另写解析器，也不靠测试向量对齐两份实现。
- **存储**：compose 原文 `state/submissions/<sid>/compose.yaml`；记录 `kind = "app"`、`app` 字段保存 `min_kernel`/`images`/`env`/`ports`/`volumes`；`api` 留空表示不适用。
- **发布**：`release` 把 compose 复制成 `catalog/assets/<sha>.yaml`，写源条目 `catalog/<channel>/apps/<id>-<version>.json`（`asset` 指向该文件），`build` 已有的 apps 分支据此生成 `index.apps[]` 与 `compose_sha256`/`compose_url`；`seq` 每次 `release` 只 +1，插件与应用同批发布共享同一个 `seq`。
- **客户端边界**：客户端继续只消费签名索引，`CatalogEntries.apps` 已展示「应用模板」分区；**不提供安装入口**，安装来源仍是内置目录，`serve` 与 §16 契约零变化。
- **失败隔离**：`submission.created/approved/published` 等事件仍走 best-effort outbox，写 outbox 失败不影响提交/审核/发布结果。
- **R2c 测试**（`tests/api.rs`）：
  - `app_submit_approve_release_writes_compose_blob_and_index`：201 → 202 → release 后 `index.apps[0]` 的 `compose_sha256`/`compose_url` 正确、blob 字节与提交原文一致、源条目 `asset` 指向 `assets/<sha>.yaml`；
  - `app_compose_outside_supported_subset_is_rejected`：`build:` 与非 UTF-8 均 422 且不落状态；
  - `same_app_version_with_different_compose_conflicts`：同摘要 200 同一 `sid`，同版本不同摘要 409；
  - `plugin_and_app_release_share_one_seq`：一次 `release` 同时发布插件与应用，`seq = 1`、`released.len() == 2`，索引两数组各 1 条。
  - 合计 `cargo test -p skiff-server`：34 单测 + 27 API + 4 CLI + 15 P0 + 7 serve + 6 sync = 93 项全绿。

### 17.14 R2d 落地说明：运行环境提交、审核与元数据修订（`tools/skiff-server`）

- **提交**：`POST /api/v1/runtimes`（JSON `entry`，见 §17.3）。作者 `require_author`、审核者 `require_reviewer`；提交时 `default` 必须为 `false`，镜像与 digest 落记录后不可改。
- **身份与版本**：`sid = sha256("runtime\0kind\0version\0image_digest")`；同 `(kind, version)` 不同 digest 或已发布 → 409；同一 digest 换了 `image` 引用 → 409。
- **发布**：`release` 写 `catalog/<channel>/runtimes/<kind>-<version>.json`（`RuntimeEntry`），由既有 `build` 生成 `index.runtimes[]`；`build` 同时兜底校验「同 kind 至多一个 `default`」，违反则发布失败并留 `release.failed` 事件。
- **元数据修订**：`POST /api/v1/submissions/{sid}/revise`（审核者、仅 `published`）。只接受 `eol`/`default`；写 `state/runtime_revisions/<id>.json`（`seq=null` 表示未应用）并审计 `revise`；下一次 `release` 校验源条目 digest 与修订一致后应用、递增 `seq` 并标记 `seq`。镜像与 digest 没有任何写入路径，因此不可改。
- **范围**：只做提交/审核/展示；客户端继续从签名索引 `runtimes[]` 展示，安装仍走内置目录，`serve` 与 §16 契约零变化。
- **R2d 测试**（`tests/api.rs`）：
  - `runtime_submit_approve_release_writes_index_entry`：201→202→release 后 `index.runtimes[0]` 的 `kind`/`version`/`image`/`image_digest`/`api_min`/`default` 正确，源条目落在 `catalog/stable/runtimes/php-8.5.json`；
  - `runtime_default_must_be_false_at_submission`：提交带 `default=true` → 422；
  - `runtime_revision_changes_eol_and_default_with_audit`：带 `image_digest` 的修订 422；`eol`+`default` 修订 202，下一次 release 生效，`audit.jsonl` 有 `action=revise`；
  - `runtime_revision_requires_reviewer_and_published_state`：未发布 412、作者 403；
  - `release_rejects_multiple_defaults_per_kind`：两个同 kind 条目都改 `default=true` 后 release 失败。
  - 合计 `cargo test -p skiff-server`：34 单测 + 32 API + 4 CLI + 15 P0 + 7 serve + 6 sync = 98 项全绿。

### 17.15 R2e 落地说明：分 kind 撤包与独立吊销命名空间（`tools/skiff-server`）

- **端点**：`POST /api/v1/plugins/{id}/versions/{v}/yank`、`POST /api/v1/apps/{id}/versions/{v}/yank`、`POST /api/v1/runtimes/{kind}/versions/{v}/yank`，共用 `takedown::yank` 单实现；审核者调用，幂等 200，目标不存在 404，已撤回/已在吊销列表 409。
- **记录**：`Takedown` 增加 `entry_kind`（缺省 `plugin` 且不落盘，旧 `state/takedowns/*.json` 无需迁移）；记录 id 对插件保持 R1 派生式不变，其它 kind 加类型域前缀，避免同 id/version 撞记录。
- **源数据**：`release` 按 kind 定位 `plugins/` / `apps/` / `runtimes/` 目录（运行环境的 id 字段是 `kind`），只改 `yanked` / `yank_reason`；非插件撤包同时追加 `revoked_apps.json` / `revoked_runtimes.json`（应用取 compose 摘要，运行环境取镜像 digest 的 hex）。
- **索引**：`IndexFile` 新增 `revoked_apps` / `revoked_runtimes`（空时省略），`apps[]` / `runtimes[]` 条目在撤包后带 `yanked` / `yank_reason`（未撤包不落字段）；`revoked[]` 语义与字节不变。
- **客户端提示**：`crates/app` 的索引模型解析新数组与 `yanked` 字段，商店「应用模板」「运行环境」分区的 `warn` 按「吊销命中 → 已撤回：原因」「yanked → 已下架：原因」「EOL」优先级展示；不新增安装入口，`serve` 与 §16 契约零变化。
- **R2e 测试**：
  - `yank_app_marks_source_and_revoked_apps_only`：应用撤包后索引 `apps[0].yanked`、`revoked_apps[0]`（含 compose 摘要）正确，`revoked[]` 为空，提交记录带站内提示；
  - `yank_runtime_marks_revoked_runtimes_only`：运行环境撤包写入 `revoked_runtimes[]`（digest hex），`revoked_apps[]` 不出现；
  - `yank_kind_endpoints_are_reviewer_only_and_idempotent`：作者 403、未知目标 404、重复撤包 200 同一 id；
  - `plugin_yank_keeps_revoked_namespaces_untouched`：插件撤包记录不含 `entry_kind`，索引不出现两个新数组；
  - `crates/app/src/catalog/tests.rs::app_and_runtime_takedown_use_their_own_namespaces`：客户端解析新数组并按命名空间给出提示。
  - 合计：`cargo test -p skiff-server` 102 项、`cargo test -p skiff-app catalog` 57 项全绿；`clippy -D warnings` 通过。


