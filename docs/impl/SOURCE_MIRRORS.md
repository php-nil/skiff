# 源管理：软件包源与容器镜像源

> 一句话：**清单是数据，测速是本机此刻的观测，选择权在用户**。三者分开存、分开展示，谁都不替谁下结论。
> 相关：[RUNTIME_PROVISION_SPEED.md](RUNTIME_PROVISION_SPEED.md)（安装耗时的分层解法）｜[HOST_EXEC_DESIGN.md](../plugin/HOST_EXEC_DESIGN.md)（插件能力边界）｜实现：`crates/app/src/runtime_catalog.rs`、`crates/app/src/docker_daemon.rs`

## 1. 前提：为什么"算出结论"这条路不成立

同一源在不同链路上能差三个数量级。测试机同网络同时段：清华 29.5 MB/s、`dl-cdn` 535 KB/s、阿里 271 KB/s——换一台机器排序可能整个反过来。因此：

- **任何速度数字都不进代码、不进文档结论位、不做跨机器宣称。** 云端共享测速结果同样无意义。
- 已落地的自动探测不属于"写死结论"：它在**用户容器内、装机那一刻**现测现选。但它有三个结构性缺陷——**隐式**（用户不知道被切了源）、**候选封闭**（内网/企业源进不来）、**只覆盖 apk**（镜像拉取没管）。
- 所以本方案把探测降级为"默认值的来源之一"，把**看得见 + 可覆盖 + 可自填**作为主路径。

## 2. 三层数据模型

| 层 | 内容 | 谁写 | 含速度 |
| --- | --- | --- | --- |
| 候选清单 | id、显示名、基址、地域/说明 | 内置表 + 用户自填 | **否** |
| 实测结果 | 源 id、`bytes`、`elapsed_ms`、错误、采样时刻、机器标识 | 用户点「测速」时 | 是，**只代表本机此刻** |
| 生效选择 | apk：`auto`（探测结果，24 h 过期）或 `fixed`（用户选的源根，不过期）；registry：有序列表 | 用户显式操作（首次安装的 `auto` 除外） | 否 |

两条硬约束：

- 实测结果落库带时间戳，超 24 h 标「已过期」，**不参与任何自动决策**（`auto` 的首次探测除外）。
- 用户一旦选过（缓存文件首行带 `# skiff-fixed`），任何探测都不再改它。这是"用户主导"的硬保证，不是 UI 提示：安装器读到标记就跳过 24 h 过期重测。

## 3. 测速口径

三条通用规则，缺一条就会重演 `APKINDEX` 的坑：

1. **必须下足量真实数据**，目标 ≥3 MB；不足 64 KB 才判「不可用」（量级等同错误页）。阈值不能定高：dl-cdn 从某条线路 8 秒只读到 245 KB，这是**有效的慢**，不是故障——按 512 KB 判会把它说成不可用，用户就失去了「为什么慢」的唯一线索。
2. **记录三元组** `bytes / elapsed_ms / 状态`，UI 同时显示采样字节数——「3 MB / 4 ms」用户一眼能看出是缓存命中而非带宽。
3. **单源硬上限 8 s，同时测 2 个**。串行测 5 个死源会把页面拖到 40 s；但 4 路并行实测有源挂在 `tls connection init failed: Resource temporarily unavailable`，单独重测却正常——并发制造的假故障比慢更坏，用户会以为源坏了。所以限 2 路，4 个候选最坏 2 轮。数字只用于**同一页横向比较**，UI 已写明「只代表此刻这条线路」。

### 3.1 apk

样本是 virt ISO 的前 3 MB（`Range: bytes=0-3145727`）。**难点是拿到这个文件名的 URL**，实测两条弯路：

- `/alpine/last-release` 在真实源上**已经不存在**（dl-cdn / 阿里 / 腾讯 / 中科大全部 404）。按它实现会得到「所有源都不可用」。
- 把版本写死也不行：镜像同步滞后时路径直接 404，而且跟着 Alpine 发版就得改代码。

正确入口是镜像自己发布的清单：`<base>/latest-stable/releases/<arch>/latest-releases.yaml`，从中取 `file:` 里 `alpine-virt*-<arch>.iso` 那一项（其余 `flavor` 是 minirootfs 等小文件，混进来就测不出吞吐）。只同步某个分支的内网镜像没有 `latest-stable`，退化为扫 `<base>/` 目录索引取最高 `vX.Y` 分支，再读该分支的 `latest-releases.yaml`。

容器内的自动探测不用这套：它直接读 `/etc/alpine-release` 与 `apk --print-arch`，是版本的第一手来源。

### 3.2 registry（**不能测 `/v2/`**）

`/v2/` 对匿名请求返回 401 或空 200，耗时是毫秒级，与吞吐无关；manifest 也只有几 KB。正确路径：

1. `GET /v2/library/alpine/manifests/<digest 或 tag>`，`Accept: application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json`。
2. 取 `layers[0].digest` 与 `size`。
3. `GET /v2/library/alpine/blobs/<digest>`，`Range: bytes=0-3145727`，测吞吐。
4. 遇 `WWW-Authenticate` 挑战取匿名 token 重试一次；响应体不是 JSON 判「不可用」（部分 mirror 对不存在的仓库回 HTML 200）。
5. **用 digest 而不是 tag 去拉 blob**：tag 会漂移，同一 tag 在不同 mirror 上可能命中不同层，字节数不同就不可比。但**不能把 digest 内置成常量**——各 mirror 同步进度不一，写死的 digest 在多数源上直接 404；实现按 tag 解析出 digest 再拉，`alpine:latest` 的层大小在数 MB 量级且稳定，可比性足够。

实现补充（2026-11-18 测试机验证口径时确认，属实现约束而非速度结论）：

- 401 挑战是**常态**，不是边缘情况：`docker.m.daocloud.io`、`docker.1ms.run` 对匿名 manifest 请求都回 401 + Bearer 挑战，必须走 token 流程；也有源（`dockerproxy.net`）直接放行。两条路径都得跑通。
- 公共 mirror 会**临时不可用**：验证时 `docker.xuanyuan.me`（正是测试机 `daemon.json` 里的第三顺位）8 秒内无响应。所以 UI 必须区分「未测 / 失败 / 慢」三态，且不能因为某个源失败就中止整页测速。
- 慢源不该判失败：8 秒上限内只读到 1 MB 也是有效观测（约 0.15 MiB/s），如实显示速率与采样字节数，比报「失败」有用。

## 4. 应用与回滚

### 4.1 apk 源（容器内，低风险）

- 落宿主 `<runtime>/cache/apk/.skiff-mirror`（就是安装器已经在读的那个缓存），内容为 `# skiff-fixed` + 一行**源根**（如 `https://mirrors.ustc.edu.cn/alpine`）。
- 内核启动时创建 `<web_root>/runtime/cache/apk`：它是 PHP 安装器的挂载源，也是插件 `path:` 授权的根——**根不存在时授权会被丢弃**，插件在首装运行环境前就无法写源缓存。
- **新建 PHP 环境的弹窗也能直接选源**（`GET /ui/v1/runtimes/catalog` 给候选与当前固定态，`POST /ui/v1/runtimes` 的 `apk_mirror` 字段）。命令层在安装作业入队前落盘，prelude 立即读到；非 PHP 类型不携带该字段，避免一次与安装无关的提交改写全局源。
- **不写分支、不写 arch**：分支由容器按自己的 `/etc/alpine-release` 拼成 `.../vX.Y/main` + `.../vX.Y/community`（**不带 arch**，否则拼成 `.../main/x86_64/x86_64` 全盘失败）。用户在输入框里粘了带分支或带 `/main` 的完整地址时，prelude 会先归一化再拼。
- 首行的 `# skiff-fixed` 是「别覆盖我」的标记：没有它，安装器 24 小时后会重新探测并把用户的选择改掉。
- 因此**应用这一步不联网、不需要版本发现**：内网镜像即使测不出速，也照样能选、能用。代价是「落地前先跑 `apk update`」这条校验做不到——真正的校验发生在下次装环境时（`apk update` 失败会在日志里直接暴露）。
- 回滚 = 删该文件，回到镜像默认源。页面「恢复自动探测」等价于清空（写空文件；安装器用 `-s` 判空，视为未固定）。已存在的环境不自动改；新建环境时生效。

### 4.2 容器镜像源（宿主 `daemon.json`，中风险）

**纠正此前的误判**：改 `registry-mirrors` **不需要重启 Docker，也不会重建或重启任何容器**。它属于 dockerd 支持 SIGHUP 热重载的键（同 `insecure-registries`、`debug`、`labels`、`runtimes`）；`data-root`、`storage-driver`、`log-driver`、`iptables` 才需要 restart。测试机 `CanReload=yes`，`ExecReload = kill -HUP $MAINPID`。

现行代码恰恰**因为**误判了这个风险，在有运行中容器时不敢重启 → 配置写完不生效，让用户自己找维护窗口手工 `restart`。这是当前实现的真实缺陷。

应用序列：

1. merge `registry-mirrors`，保留其它键 → JSON 校验 → 快照到 `<runtime>/backup/daemon.json.<ts>`（reload 不会丢文件，但快照是 reload 失败后唯一确定可回退的点）。
2. 写文件 → `systemctl reload docker` → `docker info --format '{{json .RegistryConfig.Mirrors}}'` 比对**内存态**是否生效 → 提交。
3. `CanReload=no`（unit 无 `ExecReload`）时才降级 restart，此时必须先预告「N 个运行中容器将被重启」并要求显式确认。
4. 失败回滚：写回快照再 reload 一次。dockerd 对非法配置本身会保留旧值继续跑，所以最坏是「没生效」，不是「拉不了镜像」。

**唯一的真实风险**是整文件覆盖丢掉 `data-root` / `log-driver` 等既有键。`merge_daemon_json` + 「读失败即中止」已防住，必须有测试钉住，不得简化成直接覆盖。

**预期管理写进 UI**：`registry-mirrors` 只代理 Docker Hub（`docker.io`），对 `ghcr.io` / `quay.io` / 私有 registry **无效**；mirror 不可用时 Docker 自动回退官方 Hub，所以配错源是「变慢」而非「拉不到」。

## 5. 能力面归属

落点是**插件**（`com.skiff.sources`，见 [`plugins/README.md`](../../plugins/README.md)），但**不是把 root 交给插件包**：插件不直接写 `/etc/docker`，而是经内核的 `fs` / `exec` 能力面代管——每条 `path:` / `bin:` 都是安装时逐条确认的对象授权，内核只代执行、不代决策，并留审计。

| 授权 | 用途 | 不勾的后果 |
| --- | --- | --- |
| `path:/www/runtime/cache/apk` | 写安装器源缓存 | apk 源只能测不能选 |
| `path:/etc/docker` | 读写 `daemon.json` | 镜像源只能看不能改 |
| `bin:/usr/bin/systemctl` | `reload docker` | 同上 |
| `bin:/usr/bin/docker` | `docker info` 校验内存态 | 无法确认是否生效，应用会回滚 |
| `network_egress: unrestricted` | 测速要连用户自填的任意域名 | 测速全失败 |

`bin:/usr/bin/docker` 不能省：`systemctl reload` 的退出码**不能证明** dockerd 接受了配置——SIGHUP 是 systemd 的 `kill` 发的，配置非法也返回 0。唯一可靠判据是 `docker info` 里的 `RegistryConfig.Mirrors`。

为什么不放进内核：测速是「本机此刻」的事，跟安装主链路没有共享状态；放进内核会让一次网络探测成为安装包的失败面。内核只保留它已经有的那部分（`docker_daemon.rs` 的 daemon.json 应用），插件负责测速与选择。

## 6. UI 形态

一页两卡（apk / registry），由**原生 schema** `ui/schema/main.json` 渲染；旧 `ui/main.html` 只在 `?legacy=1` 时保留，用于 iframe 沙箱/postMessage 桥的兼容验证，不再是面板入口（没有 schema 时壳页会退回 legacy，而 legacy 桥不带 nonce，RPC 永远无响应——表现为「读取中…」）。每表三列：名称、源根/地址、实测。规则：

- **三态分开**：「未测」是灰字，「不可用」带失败原因，「慢」照常显示速率。什么都不显示等于让用户猜。
- 每行的实测格必须同时显示**采样字节数与耗时**（`2.98 MB / 0.31 s`），否则缓存命中会被当成带宽。
- 每张表都能添加自定义地址；候选清单里没有的源不该被排除在外。
- 应用是显式动作：apk 行内「使用」固定该源、「恢复自动探测」清掉固定标记；registry 行内「仅用此源」或多选后「应用」。没有任何一步自动改配置。
- 没有逐行测速：每次探测都真实占用线路，「测速全部」按 2 路并发跑完全部候选即可，逐行点测更慢。

## 7. 用户场景

| 场景 | 行为 |
| --- | --- |
| 国内公网 | 大概率选中清华 / 中科大 / 阿里，几十秒装完 |
| 海外 | 多数时候 `dl-cdn` 最快，选它即可 |
| 内网 / 企业源 | 自填 URL，`# skiff-fixed` 后永不被探测覆盖；应用不联网 |
| 纯离线 | 不选任何源：安装器探测全失败后保持镜像默认源，不阻断安装 |
| registry 受限 | 选 mirror 或自填私有 registry；只对 `docker.io` 生效 |


## 8. 不做

- 不做跨机器测速排名，不做云端共享测速结果。
- 不因测速结果自动改 registry 顺序（顺序也由用户定）。
- 不在安装主链路里加第二次网络探测——已有一次，够用。

## 9. 未闭环

- Debian 家族 apt 源（`/etc/apt/sources.list.d`，签名机制与 apk 不同，需单独设计）。
- `ghcr.io` 类分层代理不在 `registry-mirrors` 语义内，需另设 pull-through 代理，未做。
- 镜像 CDN 的边缘节点选择有随机性，两次采样仍可能偏差 2 倍以上；UI 只做同页横向比较，不做绝对承诺。
