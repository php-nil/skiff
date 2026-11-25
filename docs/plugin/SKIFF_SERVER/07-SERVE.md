# 静态服务契约 `skiff-server serve`（§16）

> 上级索引：[SKIFF_SERVER.md](../SKIFF_SERVER.md) ｜ 拆自 `docs/plugin/SKIFF_SERVER.md` 原 §16（2026-11-16 整理，正文未改）。

---

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
| `/channels/<channel>/runtime-index/<kind>.json` | 同名 | 版本清单（面板按固定路径取；`<kind>` 白名单只有 `php` / `node` / `rust`，其它文件名一律 404） |
| `/<channel>/manifest.json` 等三件 | `channels/<channel>/…` | **短别名**，与规范路径等价（兼容「渠道目录 `/stable/`」写法） |
| `/<channel>/runtime-index/<kind>.json` | `channels/<channel>/runtime-index/<kind>.json` | 版本清单的短别名 |
| `/blobs/sha256/<hex64>` | `blobs/sha256/<hex64>` | 内容寻址，`hex64` 必须是小写 64 位十六进制 |
| `/api/**` | — | 只属于 §17 的发布面（独立子命令 `skiff-server api`，R0/R1 已实施）；`serve` 不实现也不代理，**一律 404**，不属于本契约 |
| 其他（`/`、目录、`schema/`、`catalog/`、未知文件） | — | 404；不列目录、无入口页 |

- `<channel>` 只认磁盘上 `channels/<name>/manifest.json` 实际存在的目录（当前产物为 `stable` / `night`）；
  不存在的渠道名 → 404。
- **不支持 `cur` / `latest` 别名**：客户端已按渠道名固定路径；别名会让同一 URL 的 ETag/缓存语义随「当前最新」
  漂移，也无法与 `seq` 单调校验建立稳定对应。需要「跟随最新」时由运维改客户端 `channel`，或由发布流程
  同步复制产物，而不是让服务端维护可变指针。
- `manifest.json` / `manifest.json.sig` / `index.json` 必须同渠道同版本；`runtime-index/<kind>.json` 由同渠道 `runtimes/` 条目按 kind 生成，**不参与签名**。`serve` 不校验、也不改写其内容（生成时已由 `build` 按 `runtime-index.schema.json` 校验；`check` 只统计 `index.runtimes` 条目数，不读这几份文件；`sync` 的自检会校验它同步的那几份）。
- `skiff-server sync` 在步骤 5/6 一并同步这三份清单：主站有哪几份就留哪几份，字节与主站一致（不重新序列化）。主站返回 404/410 视为「该类未发布」，**删掉镜像本地残留**，否则下游会长期对外发一份过期清单；其它状态码与网络错误仍是失败，不会被当成「主站撤了这份文件」。

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
- **版本清单不是硬依赖**：`channels/<channel>/runtime-index/<kind>.json` 取不到（未发布/404/网络不通）时
  面板回退**内置副本（PHP）或目录大版本（Node.js / Rust）**，只提示来源与原因，不阻断新建运行环境——所以它不进签名链，也不算客户端依赖点。
- 本契约变更必须同时改：本节、`tools/skiff-server/src/serve.rs`、`crates/app/src/runtime_index.rs`（路径拼接与解析）、
  `crates/app/src/catalog`（镜像基址来源）与 `docs/test/TEST_MATRIX.md` 的 STORE 条目；`serve` 的实现细节不是规范，本节才是。

### 16.6 测试覆盖

- `tools/skiff-server/tests/serve.rs`：真实 TCP 上覆盖条件请求 304、单区间 206 与 416、`404`（含 `/api/`、
  目录、未知文件）、`405`、`HEAD` 无 body、路径穿越与非法 blob 名拒绝、`--http` 明文与默认 https 的启动差异；
  另覆盖版本清单的两个路径（`/channels/<ch>/runtime-index/<kind>.json` 与其短别名）200、白名单外文件名
  （`other.json` / `php.json.bak`）与磁盘缺失的 `rust.json` 均 404；访问日志再覆盖「一请求一行」与裸 `--access-log` 落 stderr。
- `serve.rs` 单测覆盖日志行本身：`status=304 bytes=0 cache=inm`、`status=206 … range=0-3`、`status=405`；
  未开启时不创建文件、不产生输出；启动后日志目录消失（写失败）时响应仍完整。
- TLS 链路：进程内生成自签 CA + 服务器证书（`rcgen`），`serve` 以 https 启动后用 ureq 指定该 CA 建连成功，
  未导入 CA 时失败（与 §4.5 的客户端口径一致）。
- `serve` 不改变产物：测试只读校验运行前后文件字节不变。
