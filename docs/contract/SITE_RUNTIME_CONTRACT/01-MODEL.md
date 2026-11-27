# 站点领域模型、数据与目录（§0–2）

> 上级索引：[SITE_RUNTIME_CONTRACT.md](../SITE_RUNTIME_CONTRACT.md) ｜ 拆自 `docs/contract/SITE_RUNTIME_CONTRACT.md` 原 §0–2（2026-11-16 整理，正文未改）。

---

> 状态：**设计契约 v1（2026-11-05）；P1 实现补充见 §11（2026-11-06）；Node.js 接入见 §4.3（2026-11-09）**。
> 2026-11-09：Node.js 已在内核接入（运行环境目录、安装/启停/重建、站点绑定与反代）；
> Go 仍未实现，已从 `runtime_catalog::RUNTIME_KINDS` 与新建弹窗摘除，不再提供登记入口。
> 基线：`crates/store/src/db/migrations/schema.rs`（schema v21）、`crates/app/src/sites.rs`（OpenResty + 多实例 PHP-FPM
> 与 Node.js / Rust 进程运行环境）、`crates/app/src/sites/network.rs`（站点专用网络与静态地址）、
> `crates/app/src/runtime_catalog.rs`、
> `crates/web/src/handlers/sites.rs`（运行环境写接口）、`crates/web/src/handlers/ui/runtimes.rs`（壳页读模型）、
> `docs/contract/MENU_CONTRACT.md`（菜单与深链）。
> 2026-11-15：运行环境改为**专用 bridge + 内核静态容器地址**，宿主不再出现运行环境端口；
> 站点 vhost 只反代到容器地址；对外发布是显式开关（`site_runtimes.publish_port`，默认关闭）。
> 可点预览：[`docs/archive/ui/site-runtime-preview.html`](../../archive/ui/site-runtime-preview.html)。

## 0. 五条结论

1. **站点分三类，先选类型再建站**：`runtime`（绑定运行环境）、`static`（只托管文件）、`proxy`
   （只转发上游）。`runtime` 站点必须绑定一个已就绪的 `RuntimeEnv`；`static` / `proxy` 不绑定运行环境。
2. **运行环境是“版本基线 + 运行实例”的业务对象**，不是 Docker 容器的别名。PHP 环境以**共享 FPM
   池 + 共享 socket** 承载多个站点；Node.js / Rust 环境以**单进程常驻 + 容器内固定端口 + 反代**承载站点，
   环境本身提供版本、镜像、启动命令与端口。运行环境容器统一接入专用 bridge（`skiff-sites`），
   **不占宿主端口**；站点按域名由 nginx 直接反代到容器静态地址。Node.js / Go 的「每站点独立进程」
   拆分作为后续演进，不在本轮。
3. **单一事实源**：`site_runtimes` 表是运行环境的唯一记录，站点通过 `runtime_id` 引用；站点类型、
   kind、端口、上游、WebSocket 等字段落在 `sites` 表。旧的 `php_version` 字段在 schema v7 被
   `runtime_id` 取代，不再保留两份版本信息。
4. **运行环境页与 Docker 页分工明确**：运行环境页是业务视角（版本、绑定站点、专属配置、启停/重建），
   Docker 页只做原始容器管理。运行环境的**期望状态**存库、**实际状态**读 Docker / 进程探测；
   两页不维护第二份生命周期状态，出现外部改动时显示“状态漂移”并提供重建/同步入口。
5. **不做假实现**：已落地静态站、PHP / Node.js / Rust 站、反向代理站；Go **未实现**，已从运行环境
   目录与新建弹窗摘除（不显示、不可选、API 直接拒绝「未知运行环境类型」），不再落库占位行。
   PHP 扩展管理、抽屉页签、版本元数据与 Rust 运行环境在 2026-11-06 收口（见 §11）。

---

## 1. 领域模型

### 1.1 枚举与关系

```text
RuntimeKind  = php | node | rust   （go 未接入，不登记）
SiteType     = runtime | static | proxy

RuntimeEnv 1 ─────< Site (SiteType = runtime)
Site (static / proxy)  不持有 RuntimeEnv
```

| 对象 | 字段 | 说明 |
| --- | --- | --- |
| `RuntimeEnv` | `id` | 稳定 slug：内置 `php-8.5`；新实例建议 `php-<name>`；全实例唯一，创建后不可改 |
| | `kind` | `php` / `node` / `rust`（`go` 未接入，不登记） |
| | `name` | 本地实例标识，规则 `^[a-z0-9][a-z0-9.-]{0,31}$`；同 kind 唯一；目录与容器名只由它推导 |
| | `version` | 镜像/上游版本（PHP `8.5`、Node `22`、Rust `1.83`）；与 `name` 解耦，同版本可多实例 |
| | `image` / `image_digest` | 固定镜像引用与 digest；digest 是重建/校验依据 |
| | `desired_state` | `running` / `stopped`，用户意图 |
| | `last_status` | `installing` / `running` / `stopped` / `error`，最近一次探测结果（缓存，不是事实源） |
| | `socket_path` | 仅 PHP：FPM socket 绝对路径；vhost 只读该字段，禁止硬编码 |
| | `port` | 仅 Node.js / Rust：**容器内**固定监听端口（Node `9200–9299`、Rust `9100–9199`）；vhost 反代只读该字段，与宿主端口无关 |
| | `container_ip` | 仅 Node.js / Rust：容器在专用网络上的静态地址，落在**实际选中的站点网段**静态池内（首选时 `172.18.16.1`–`172.18.31.254`）；vhost 只允许指向它 |
| | `publish_port` | 显式对外发布宿主端口；`NULL` = 不发布（默认）。非空时 Docker 映射 `宿主端口 → container_ip:port` |
| | `is_default` | 同一 kind 内最多一个默认版本；建站向导默认选中 |
| | `config_json` | PHP ini / FPM 与 Node 启动命令 / `NODE_ENV` / 健康检查 / 额外环境变量的规范化快照 |
| `Site` | `domain` | 主域名，唯一；别名仍存 `site_aliases` |
| | `site_type` | `runtime` / `static` / `proxy` |
| | `runtime_id` | `runtime` 站点必填；其余类型必须为空 |
| | `proxy_target` | 仅 proxy：`http(s)://host[:port]` 上游 |
| | `proxy_websocket` | 仅 proxy：是否升级 WebSocket |
| | `rewrite` | 伪静态预设键：`none` / `wordpress` / `laravel` / `thinkphp`；只对 PHP 站点生效，vhost 由内核按白名单生成 |
| | `limit_rate_kbps` | nginx `limit_rate` 出口限速（KB/s，0 = 不限制），对所有站点类型生效 |
| | `index/ log/ ssl/ conf/` | 目录布局不变，见 §2 |

- `kind` 不冗余存在站点表：站点类型为 `runtime` 时，kind/version 由 `runtime_id` join 得到；
  列表页需要分型筛选时由查询层 join，不复制一份可变字段。
- **Node.js 与 PHP 的核心差异**：Node 不走 FastCGI，而是「启动命令 + 监听端口 + nginx 反代」。
  启动命令、`NODE_ENV`、健康检查路径与额外环境变量属于**运行环境**（`config_json`），
  端口属于**运行环境**（`port`）；站点只做域名反代，**不决定工作目录**（固定为环境 `app/`）。
- 站点不能同时是 `runtime` 与 `proxy`；需要“PHP 站再做反代”时建 proxy 站指向它，或后续引入复合模式，
  P2 均不提供“一站多模式”开关。

### 1.2 绑定约束（必须由服务层与 DB 双重校验）

1. `site_type = runtime` ⇒ `runtime_id` 非空且引用的环境存在、`kind` 属于 `php/node/rust`。
2. `site_type = static` ⇒ `runtime_id` 为空，`proxy_target` 为空。
3. `site_type = proxy` ⇒ `runtime_id` 为空，`proxy_target` 非空合法。
4. 绑定前必须检查运行环境 `last_status = running` 且 `desired_state = running`：
   - PHP：`socket_path` 存在且可连接（stat 到 socket 文件，权限 `0660`）；
   - Node.js / Rust：`container_ip` 与 `port` 已登记，容器在专用网络上 running，且容器实际地址
     等于登记地址；Node 还要求 `http://<container_ip>:<port><health_path>` 返回 2xx/3xx，Rust
     要求 TCP 可达。容器处于 `restarting` / `StartedAt` 在等待窗口内变化（crash-loop）一律判
     未就绪；容器实际地址与登记地址不一致时终态失败并提示重建，**绝不按实际地址改写站点**；
   - Go：未实现，禁止创建绑定站点。
5. 运行环境停止 / 异常时**禁止新建绑定站点**；已有站点保留记录，但列表显示“环境停止/环境异常”，
   不把站点状态误报为 running。
6. 禁止把 `runtime_id` 指向不同 kind 的站点：PHP 站点只能绑 PHP 环境，Node 站点只能绑 Node 环境，
   Go 同理。`kind` 由环境唯一决定，站点不单独覆盖。
7. `runtime` 站点的反代端口只从环境 `port` 读取，站点表不存端口字段，服务端不接受站点自定义端口；
   多个 `runtime` 站点可绑定同一环境，即多个域名反代到同一应用进程。

### 1.3 状态与漂移

- 运行环境页每次打开**实时读** Docker / socket 探测结果，写回 `last_status` 仅用于概览与审计，
  不作为页面事实源。
- 站点有效状态 = 站点自身期望状态 ∩ 运行环境实际状态 ∩ 巡检结论：
  - 站点 `stopped` → `已停止`；
  - 巡检判定环境不可用（容器未运行 / 地址不一致 / 探测失败）→ `维护中`，站点详情给出原因与恢复路径；
  - 站点 `running` 但环境 `stopped` / `error` → `环境停止` / `环境异常`（可点击跳到环境详情）；
  - 站点 `running` 且环境 `running` → `运行中`。
- Docker 页对运行环境容器做停止/删除等操作后，不修改 `site_runtimes.desired_state`；运行环境页检测到
  实际状态与期望状态不一致时显示 **状态漂移** 徽章，并提供「重建环境」「仅同步状态」两个明确动作。
  选择“仅同步状态”只更新 `last_status`，不擅自重启容器。

---

## 2. 数据与目录

### 2.1 运行环境目录

```text
/www/runtime/<kind>/<name>/
  php-fpm.sock   PHP：FPM unix socket（权限 0660，属主 root、属组 65534）
  www.conf       PHP：FPM pool（宿主托管，可在「配置 → 配置文件」直接编辑）
  php/php.ini    PHP：宿主托管主配置（挂载为容器 /usr/local/etc/php/php.ini）
  php/conf.d/    PHP：宿主托管扩展 ini（挂载为 conf.d-skiff，扫在镜像 conf.d 之后）
  entry.sh       Node.js / Rust：入口脚本（进入固定 app/、首次装依赖、exec 启动命令）
  app/           Node.js / Rust：唯一代码目录（与站点绑定无关，可整体替换/克隆）
  log/           runtime.log（内核操作）、build.log（重建/扩展构建）、fpm-error.log、server.log（进程输出）
  data/          运行环境自身数据（session/临时目录），不放站点代码
```

- `name` 是本地实例标识（规则 `^[a-z0-9][a-z0-9.-]{0,31}$`），路径与容器名只从 `name` 推导；
  `version` 只表示镜像/上游版本，允许 `name=php-8.5-a` + `version=8.5`。容器名 `skiff-<kind>-<name>`。
- `/www/runtime` 由内核创建，权限 `0711`（可穿越不可列）；socket 权限 `0660`，属主 `root`、
  属组固定 `65534`（nobody/nogroup），**禁止 0666 兜底**。
- 运行环境配置只写自己目录；站点目录不写运行环境级配置，避免“改一个站点影响全部站点”的隐式耦合。
- 删除运行环境只删除该环境目录与记录，不触碰 `/www/sites/*` 与数据库备份。**存在绑定站点时禁止删除**
  （见 §4.4），不做隐式解绑。

### 2.2 站点目录

```text
/www/sites/<域名>/
  index/   静态/PHP 的文档根；Node.js / Rust 站点仅占位（代码在环境 app/）
  log/     access.log / error.log
  ssl/     cert.pem / key.pem（站点 HTTPS）
  conf/    <域名>.conf
```

- **布局不变**：现有“一站一目录”保持不变，不为 Node.js 新增 `app.json`（命令/端口/健康检查属于运行环境，见 §4.3）。
- `static`：`index/` 必须存在，根目录是 `<站点目录>/index`。
- `proxy`：不创建 `index/`；站点目录只含 `conf/ log/ ssl/`。目录页对 proxy 显示“反代站点无文档根”。
- `runtime`（PHP）：`index/` 是文档根；PHP-FPM 池与 socket 由环境提供，不由站点目录提供。
  池模板的 `user/group` 固定为统一身份 `skiff-web`（201000:201000），不再使用镜像自带的
  `www-data`；debian 系镜像的 `session.save_path` 与 `opcache.file_cache` 在池模板里显式指到
  `/tmp`（容器初始化脚本会建好 `skiff-web` 可写的 `/tmp/skiff-opcache`）。
- **站点内容统一身份**：`index/` 子树归 `skiff-web:skiff-web`（目录 2775、文件 0664）。
  建站落盘即对齐；FTP 账号绑定同一 uid；面板文件管理写完归还属主；插件经 `fs` 写宿主
  `sites/<domain>/index/` 后同样归还（唯一例外，见 `docs/contract/KERNEL_FS_CONTRACT.md` §8.1）。
  `/www`、数据根与会话目录仍是 0711 可穿越不可列；只改 `index/` 子树属主。
- **Node.js / Rust 例外**：代码目录 `/www/runtime/<kind>/<name>/app` 仍归 root，站点
  `index/` 只是占位；FTP v1 不服务这两类站点。
- **容器身份约束**：不启用容器级 `--user`、`userns-remap`，也不挂载宿主 `/etc/passwd` 片段。
  PHP 容器保持镜像默认入口（root 启动），经只读挂载的 `php-account-init.sh` 在容器内幂等
  建号后 `exec php-fpm`，由 root master 建 socket 再按池降权；`listen.owner/group` 与 `0660` 不变。
  **组名口径**：模板固定写 `listen.group = nobody`，但 Debian 系 gid 65534 的组名是 `nogroup`，
  直接 `getgrnam("nobody")` 会让 FPM 起不来。初始化脚本不按镜像家族猜组名，而是在 `exec php-fpm`
  前保证 `nobody` 解析到 65534：已存在则校验 gid，缺失则 `groupadd -o`（Alpine 用 `addgroup`）
  建同 gid 别名，最后兜底直写 `/etc/group`；自建镜像同样适用。池模板与 golden 不变。
- `runtime`（Node.js / Rust）：**方案 A —— 一个环境 = 一个应用进程 = 一个固定代码目录**。
  容器进入 `/www/runtime/<kind>/<name>/app` 执行启动命令（Node 默认 `npm start`，Rust
  `cargo run --release`），`node_modules/` / `target/` 也在该目录；新建环境时若 `app/` 缺失
  才写最小脚手架（已存在则原样保留，重建/克隆不覆盖用户代码）。
  绑定站点只在 vhost 里 `proxy_pass http://<container_ip>:<环境端口>`，**不改工作目录、不写站点
  `index/`**；多个站点绑同一环境即多个域名反代到同一进程。
- 删除站点：删除该站整目录 + 站点记录 + 别名 + 证书；**不删除运行环境与其代码目录**。
  删除确认页展示目录大小、文件数、关联定时任务数与绑定的运行环境（删除后仅解除该站点绑定）；
  确认方式为输入域名，输入不匹配时提交按钮禁用，服务端同时校验 `confirm` 字段。批量操作的
  `action=delete` 一律拒绝（无 UI 入口，且会绕过输入确认），统一引导到单站确认页。
  删除运行环境：被站点引用时拒绝，并列出域名与“先改绑或删除站点”的动作。

### 2.3 多版本、端口与 socket 分配

| 规则 | 结论 |
| --- | --- |
| 同一 kind 多实例并行 | 允许；唯一约束 `(kind, name)`，`name` 是本地实例标识；每个 kind 最多一个 `is_default` |
| 同一版本多实例 | 允许：`name` 不同即可（如 `php-8.5` 与 `php-8.5-a` 都可用 `version=8.5`） |
| 实例名 | 规则 `^[a-z0-9][a-z0-9.-]{0,31}$`；安装时确定，同 kind 内不得重名；目标目录已存在直接拒绝，不覆盖、不自动加后缀 |
| 安装并发 | 同一 `(kind, name)` 安装期内互斥，第二个请求返回“该环境正在安装” |
| PHP 默认实例 | 建站向导默认选中；删除默认环境前必须先切换默认，存在绑定站点时直接拒绝 |
| PHP socket | 每个 PHP 实例独立 socket `/www/runtime/<kind>/<name>/php-fpm.sock`；vhost 按站点 `runtime_id` 取该实例 socket，不按版本号推导 |
| PHP 共享池 | 同一实例所有 PHP 站点共享一个 FPM master / socket / 池；P1 不做站点级池，站点级 php.ini 覆盖留 P2 |
| Node.js 端口 | 每个 Node.js 环境一个**容器内**固定端口，范围 `9200–9299`；端口属于环境，不属于站点 |
| 端口分配 | 建环境时取区间内最小未占用端口（DB 已用即跳过），区间耗尽拒绝创建；不在站点创建时分配 |
| 端口变更 | 环境详情可改端口：必须在 kind 区间内且未被其它环境占用；有站点绑定时先列出受影响站点并要求二次确认，确认后重写全部相关 vhost 并重建容器 |
| 端口与站点 | 站点的反代端口**只从环境读取**（`site_runtimes.port`），站点表不含端口字段，不接受用户自定义端口 |
| 运行环境网络 | 统一接入专用 bridge `skiff-sites`（首选 `172.18.0.0/16`，动态池 `172.18.0.0/24`）；不发布端口、不使用 host 网络，宿主上不存在运行环境端口；首选与既有 Docker 网络/宿主网卡重叠时按固定候选序列回退（见 §2.3.1），全部候选不可用时显式报错，不改宿主网络、不接管他人网络 |
| 容器地址 | 静态地址池落在**实际选中**的网段内（首选时 `172.18.16.1`–`172.18.31.254`），分配结果落 `site_runtimes.container_ip`；重建复用同一地址 |
| 端口暴露 | 默认不公开：站点由 nginx 反代 `http://<container_ip>:<port>`；仅当 `publish_port` 非空时 Docker 映射 `宿主端口 → 容器端口`（显式对外发布） |
| 端口释放 | 环境删除后释放（含 `publish_port`）；删除失败不提前释放 |
| 保留端口 | `80` / `443` 归 nginx；对外发布端口必须 ≥1024 且未被其它环境发布；PHP 不占宿主端口，只走 socket |
| 健康检查 | 容器 running + 容器实际地址 == 登记地址 + 可达性（Node 轮询 `http://<container_ip>:<port><health_path>`，Rust 只要求 TCP）；安装 180 秒、启动 20 秒上限；crash-loop / 已退出 / 地址变化为终态，2 次确认后失败 |
| 维护态 | 巡检每 3 秒评估被运行中站点引用的环境；不可用时把站点 vhost 改成 503 维护页（`Retry-After: 5`），恢复后自动写回，不需人工改配置 |

> `proxy` 的 `proxy_target` 不参与端口分配；允许指向本机其他端口，但保存时检测“目标等于本站域名/80/443”
> 形成回环的情况并拒绝。

#### 2.3.1 容器网络模型与对外发布

与负责人给定的容器模型一致：

1. 每个运行环境容器有专用网络上的一个地址（如 `<container_ip>`），应用只在容器内监听自己的端口；
2. 只服务内网的环境（如数据库类）无需任何宿主动作，直接对接 `<container_ip>:<port>`；
3. 与 nginx 对接的站点（Node.js / Rust 运行环境站）由 nginx 把域名 TCP 转发到 `<container_ip>:<port>`；
4. 直接对外服务是**显式可选项**：在运行环境详情「对外发布」填写宿主端口，才映射 `8081 → <container_ip>:8080`。

- **网段选择与冲突**：首选母网段 `172.18.0.0/16`（网关 `172.18.0.1`，与 1Panel 对齐），Docker 动态池
  `172.18.0.0/24`，站点静态池 `172.18.16.1–172.18.31.254`。`ensure_site_network` 创建前先枚举宿主机上
  **所有既有 Docker 网络网段**与**宿主网卡 IPv4 地址**（`ip -4 -o addr show`），按固定候选序列
  `172.18.0.0/16`、`172.19.0.0/16` … `172.31.0.0/16` 取第一个不重叠（含嵌套）的网段：
  - 首选可用 → 使用首选，`sites.network_fallback` 置空；
  - 首选被占用 → 回退并把占用者写进 `sites.network_fallback`（形如「首选 `172.18.0.0/16` 被占用：
    Docker 网络 `bridge`（`172.18.0.1/16`）；当前使用 `172.19.0.0/16`」），运行环境详情页、`/ui/v1/runtimes/{id}`
    的 `network` 与内核日志（`warn`）都明确展示，不静默；
  - 实际网段落 `sites.network_cidr`；同名网络 `skiff-sites` 已存在时以其实测网段为准，与本库记录不一致时
    按实测纠正并留 `warn`；已存在但网段无法识别 → 显式报错；
  - 候选序列全部被占用 → 显式报错并列出候选段，不随意挑段、不改宿主 Docker 配置、不删除或接管他人网络。
- **稳定分配**：静态池由内核独占（Docker 动态池限制在实际网段的 `X.Y.0.0/24`），站点的池划分、地址分配、
  冲突校验、vhost 与巡检全部基于**实际选中的网段**；`ensure_runtime_ip` 复用已登记地址，
  分配时把其它环境的登记地址一律视为已占用；池耗尽时拒绝创建并报错。
- **重建后的地址变化**：重建先复用原地址；地址变化（外部改动/冲突）在健康判定中是**终态失败**，
  站点进入维护态并提示重建，绝不把站点改指实际地址（避免流量落到别的容器）。
- **上游形态**：vhost 只出现 `proxy_pass http://<container_ip>:<port>`；PHP 保持 `fastcgi_pass unix:<socket>`，
  两者都不出现 `127.0.0.1:<端口>`。
- **对外发布开关**：`site_runtimes.publish_port`，`NULL` 为默认不公开；UI 入口在运行环境详情「对外发布」
  （`POST /{prefix}/runtimes/{id}/publish`，留空即取消），保存后重建容器生效。
- **迁移**：旧实例（host 网络、`container_ip` 为空）不自动接管；站点在巡检中进入维护态，
  在运行环境页「重建」后获得容器地址即可恢复。测试阶段不做数据迁移脚本。

---
