# Skiff 网站运行环境与建站契约（SITE_RUNTIME_CONTRACT）

> 状态：**设计契约 v1（2026-11-05）；P1 实现补充见 §11（2026-11-06）**。
> 2026-11-09：Node.js / Go **未实现**，已从 `runtime_catalog::RUNTIME_KINDS` 与新建弹窗摘除，
> 不再提供登记入口；本文相关章节保留为 P2 设计态说明。
> 基线：`crates/store/src/db.rs`（schema v15）、`crates/app/src/sites.rs`（OpenResty + 多实例 PHP-FPM
> 运行环境）、`crates/web/src/handlers/sites.rs`、`crates/web/templates/site_env.html`、
> `docs/MENU_CONTRACT.md`（菜单与深链）。
> 可点预览：[`docs/ui/site-runtime-preview.html`](./ui/site-runtime-preview.html)。

## 0. 五条结论

1. **站点分三类，先选类型再建站**：`runtime`（绑定运行环境）、`static`（只托管文件）、`proxy`
   （只转发上游）。`runtime` 站点必须绑定一个已就绪的 `RuntimeEnv`；`static` / `proxy` 不绑定运行环境。
2. **运行环境是“版本基线 + 运行实例”的业务对象**，不是 Docker 容器的别名。PHP 环境以**共享 FPM
   池 + 共享 socket** 承载多个站点；Node.js / Go 环境以**每站点独立进程 + 独立端口**承载多个站点，
   环境本身只提供版本、镜像与配置模板。
3. **单一事实源**：`site_runtimes` 表是运行环境的唯一记录，站点通过 `runtime_id` 引用；站点类型、
   kind、端口、上游、WebSocket 等字段落在 `sites` 表。旧的 `php_version` 字段在 schema v7 被
   `runtime_id` 取代，不再保留两份版本信息。
4. **运行环境页与 Docker 页分工明确**：运行环境页是业务视角（版本、绑定站点、专属配置、启停/重建），
   Docker 页只做原始容器管理。运行环境的**期望状态**存库、**实际状态**读 Docker / 进程探测；
   两页不维护第二份生命周期状态，出现外部改动时显示“状态漂移”并提供重建/同步入口。
5. **不做假实现**：已落地静态站、PHP / Rust 站、反向代理站；Node.js / Go **未实现**，已从运行环境
   目录与新建弹窗摘除（不显示、不可选、API 直接拒绝「未知运行环境类型」），不再落库占位行。
   PHP 扩展管理、抽屉页签、版本元数据与 Rust 运行环境在 2026-11-06 收口（见 §11）。

---

## 1. 领域模型

### 1.1 枚举与关系

```text
RuntimeKind  = php | node | go
SiteType     = runtime | static | proxy

RuntimeEnv 1 ─────< Site (SiteType = runtime)
Site (static / proxy)  不持有 RuntimeEnv
```

| 对象 | 字段 | 说明 |
| --- | --- | --- |
| `RuntimeEnv` | `id` | 稳定 slug：内置 `php-8.5`；新实例建议 `php-<name>`；全实例唯一，创建后不可改 |
| | `kind` | `php` / `node` / `go` |
| | `name` | 本地实例标识，规则 `^[a-z0-9][a-z0-9.-]{0,31}$`；同 kind 唯一；目录与容器名只由它推导 |
| | `version` | 镜像/上游版本（PHP `8.5`、Node `22`、Go `1.24`）；与 `name` 解耦，同版本可多实例 |
| | `image` / `image_digest` | 固定镜像引用与 digest；digest 是重建/校验依据 |
| | `desired_state` | `running` / `stopped`，用户意图 |
| | `last_status` | `installing` / `running` / `stopped` / `error`，最近一次探测结果（缓存，不是事实源） |
| | `socket_path` | 仅 PHP：FPM socket 绝对路径；vhost 只读该字段，禁止硬编码 |
| | `is_default` | 同一 kind 内最多一个默认版本；建站向导默认选中 |
| | `config_json` | PHP ini / FPM / Node-Go 模板等专属配置的规范化快照 |
| `Site` | `domain` | 主域名，唯一；别名仍存 `site_aliases` |
| | `site_type` | `runtime` / `static` / `proxy` |
| | `runtime_id` | `runtime` 站点必填；其余类型必须为空 |
| | `app_command` | 仅 Node/Go 站点：容器内启动命令（argv 形式存储，不经宿主 shell） |
| | `app_port` | 仅 Node/Go 站点：宿主 loopback 端口，全实例唯一 |
| | `proxy_target` | 仅 proxy：`http(s)://host[:port]` 上游 |
| | `proxy_websocket` | 仅 proxy：是否升级 WebSocket |
| | `index/ log/ ssl/ conf/` | 目录布局不变，见 §2 |

- `kind` 不冗余存在站点表：站点类型为 `runtime` 时，kind/version 由 `runtime_id` join 得到；
  列表页需要分型筛选时由查询层 join，不复制一份可变字段。
- 站点不能同时是 `runtime` 与 `proxy`；需要“PHP 站再做反代”时建 proxy 站指向它，或后续引入复合模式，
  P1/P2 均不提供“一站多模式”开关。

### 1.2 绑定约束（必须由服务层与 DB 双重校验）

1. `site_type = runtime` ⇒ `runtime_id` 非空且引用的环境存在、`kind` 属于 `php/node/go`。
2. `site_type = static` ⇒ `runtime_id` 为空，`app_command/app_port/proxy_target` 均为空。
3. `site_type = proxy` ⇒ `runtime_id` 为空，`proxy_target` 非空合法，`app_port/app_command` 为空。
4. 绑定前必须检查运行环境 `last_status = running` 且 `desired_state = running`：
   - PHP：`socket_path` 存在且可连接（stat 到 socket 文件，权限 `0660`）；
   - Node/Go：环境镜像可用（P2 才允许创建站点）。
5. 运行环境停止 / 异常时**禁止新建绑定站点**；已有站点保留记录，但列表显示“环境停止/环境异常”，
   不把站点状态误报为 running。
6. 禁止把 `runtime_id` 指向不同 kind 的站点：PHP 站点只能绑 PHP 环境，Node 站点只能绑 Node 环境，
   Go 同理。`kind` 由环境唯一决定，站点不单独覆盖。

### 1.3 状态与漂移

- 运行环境页每次打开**实时读** Docker / socket 探测结果，写回 `last_status` 仅用于概览与审计，
  不作为页面事实源。
- 站点有效状态 = 站点自身期望状态 ∩ 运行环境实际状态：
  - 站点 `stopped` → `已停止`；
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
  log/           runtime.log（内核操作）、build.log（重建/扩展构建）、fpm-error.log
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
  index/   静态/PHP 的文档根；Rust 的运行源码目录（Cargo.toml + src/）；Node/Go 的应用工作目录（入口文件或已编译二进制）
  log/     access.log / error.log；Node/Go 追加 app.log / app.err.log
  ssl/     cert.pem / key.pem（站点 HTTPS）
  conf/    <域名>.conf；Node/Go 追加 app.json（进程/端口/健康检查快照）
```

- **布局不变**：现有“一站一目录”保持不变，`config` 只新增 `app.json` 与日志文件，不迁移旧目录。
- `static`：`index/` 必须存在，根目录是 `<站点目录>/index`。
- `proxy`：不创建 `index/`；站点目录只含 `conf/ log/ ssl/`。目录页对 proxy 显示“反代站点无文档根”。
- `runtime`（PHP）：`index/` 是文档根；PHP-FPM 池与 socket 由环境提供，不由站点目录提供。
- `runtime`（Rust）：**`index/` 就是运行源码目录**。容器执行
  `cargo run --release --manifest-path /www/sites/<域名>/index/Cargo.toml`，构建产物写到运行环境
  `target/`，不回写站点目录。只有该环境**未绑定任何站点**时才回落到 `/www/runtime/rust/<name>/app`；
  新建 Rust 站点只在 `index/` 生成 `Cargo.toml` + `src/main.rs`，**不生成 `index.php`**。
  同一环境绑定多个站点时以最早绑定的站点为准（端口固定，只能跑一个进程）。
- `runtime`（Node/Go）：`index/` 是应用工作目录；`conf/app.json` 记录命令、端口、健康检查，
  由内核生成，站点详情只读展示，不提供任意 Docker 参数编辑。
- 删除站点：删除该站整目录 + 站点记录 + 别名 + 证书；**不删除运行环境**。Rust 站点的 `index/`
  即运行源码，删除站点就是删除源码（不可撤销，且不经回收站）。删除确认页必须展示将被删除的具体量：
  目录大小、文件数、关联定时任务数（`source = site:<域名>`）、绑定的运行环境（删除后仅解除该站点绑定），
  Rust 站点额外标注「将删除源码目录」；确认方式为输入域名，输入不匹配时提交按钮禁用，服务端同时校验
  `confirm` 字段。批量操作接口的 `action=delete` 一律拒绝（无 UI 入口，且会绕过输入确认），
  统一引导到单站确认页。删除后若该 Rust 环境仍绑定其它站点则以最早绑定者为准，否则回落 `app/` 兜底。删除运行环境：被站点引用时
  拒绝，并列出域名与“先改绑或删除站点”的动作。

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
| Node/Go 端口 | 每站点独立容器/进程 + 独立宿主 loopback 端口，范围 `30000–32767` |
| 端口分配 | 取范围内最小空闲且数据库未占用、宿主未监听的端口；创建/改端口时做 bind 探测，失败给出端口与占用进程 |
| 端口暴露 | Node/Go 容器使用 bridge 网络，端口映射固定 `127.0.0.1:<app_port>:<container_port>`；禁止 `0.0.0.0` 映射 |
| 端口释放 | 站点删除/改绑成功后释放；删除失败不提前释放 |
| 保留端口 | `80` / `443`（nginx）、`30000` 以下不分配；PHP 不占宿主端口，只走 socket |
| 健康检查 | Node/Go 站点创建后轮询 `http://127.0.0.1:<app_port><health_path>`（默认 `/`），1s 超时、最多 10 次；失败置 `error` 并保留启动日志，不无限重试 |

> `proxy` 的 `proxy_target` 不参与端口分配；允许指向本机其他端口，但保存时检测“目标等于本站域名/80/443”
> 形成回环的情况并拒绝。

---

## 3. 建站流程差异

三步向导固定为：**类型与运行时 → 站点信息 → 确认**。类型选择是第一步，不允许先填域名再猜类型。

| 类型 | 第一步 | 第二步字段 | 创建副作用 | 就绪条件 |
| --- | --- | --- | --- | --- |
| 运行环境 · PHP | 选“运行环境” → 选 PHP → 选 PHP 环境 | 主域名、别名、站点目录 | 生成 vhost（fastcgi_pass 指向 `socket_path`）、`index/log/ssl/conf` | PHP 环境 running + socket 可连接 |
| 运行环境 · Rust | 选“运行环境” → 选 Rust → 选 Rust 环境 | 主域名、别名、站点目录 | 构建并运行站点源码的站点容器 + vhost 反代 | 环境 running + 容器端口可访问 |
| 运行环境 · Node/Go | **2026-11-09 已从 `RUNTIME_KINDS` 与新建弹窗摘除**，向导不出现该类型 | — | —（P2 设计保留在 §4.3，未排期） | — |
| 静态网站 | 选“静态网站”，不选运行时 | 主域名、别名、站点目录 | 生成静态 vhost、`index/log/ssl/conf` | 无运行时依赖；目录可创建 |
| 反向代理 | 选“反向代理”，不选运行时 | 主域名、别名、上游 URL、WebSocket 开关 | 生成 proxy vhost、`log/ssl/conf`（不建 `index/`） | 上游 URL 合法；不要求上游当下可达，但保存时给出探测提示 |

### 3.1 向导校验

| 字段 | 规则 | 错误文案（示例） |
| --- | --- | --- |
| 主域名 | 小写字母/数字/中划线/点；1–253 字符；不得以 `-`/`.` 开头结尾；唯一（含别名占用） | `域名格式不正确` / `example.com 已被其他站点使用` |
| 别名 | 逗号/空格分隔；每个域名单独校验；不得与任何主域名/别名重复 | `别名 api.example.com 已是其他站点的域名` |
| 站点目录 | 默认 `/www/sites/<主域名>`；规范化后必须是 `/www/sites` 的一级子目录，拒绝 `..`、更深层、越界 | `站点目录必须是 /www/sites/<域名>` |
| PHP 运行时 | 必须 running；socket 必须可连接 | `PHP 运行环境未就绪，请先启动或重建` |
| Node/Go 运行时 | 已摘除；提交 `kind=node`/`kind=go` 直接拒绝 | `未知运行环境类型` |
| 启动命令 | P2；非空；不得包含宿主路径或凭据；按 argv 存储 | `启动命令不能为空` |
| 端口 | P2；整数；`30000–32767`；不与已有站点/保留端口冲突 | `端口 30081 已被站点 api.example.com 占用` |
| 上游 URL | 仅 `http://` / `https://`；无 userinfo；host 必填；端口 1–65535 | `上游地址必须以 http:// 或 https:// 开头` |
| WebSocket | 布尔；开启后 vhost 增加 `Upgrade` / `Connection` 头 | — |

### 3.2 交互口径（与预览对应）

- 类型卡显示“是否需要运行环境”和“创建后产出什么”，选中后立即显示对应的绑定提示：
  - PHP：`共享 PHP-FPM 池与 socket，一个环境可承载多个站点；修改环境配置会影响全部绑定站点。`
  - Node/Go：`每站点独立进程与端口；运行环境只提供版本与镜像。`
  - 静态：`不绑定运行环境，只创建文档根与 vhost。`
  - 反代：`不创建文档根，只生成转发配置；WebSocket 可选。`
- 运行环境卡显示：版本、状态、占用站点数、socket 或端口范围、默认标记。未运行环境不可选。
- 下一步按钮在前置条件不满足时禁用，并给出**原因**，不是点击后才报错。
- 确认页必须复述：类型、绑定环境、目录、端口/上游、WebSocket、HTTPS 是否后续配置。

---

## 4. 运行环境管理页

入口：`网站 → 运行环境`，稳定键 `core/sites/env`。它是业务视角，不是 Docker 容器列表的翻版。

### 4.1 列表

| 列 | 内容 |
| --- | --- |
| 类型 | `PHP`、`Rust`（Node.js / Go 已于 2026-11-09 摘除，不再出现在列表与向导） |
| 实例名 | 环境名；默认版本带「默认」标记 |
| 版本 | 镜像/上游版本号，来自版本目录（见 4.5） |
| 状态 | 运行中 / 已停止 / 异常 / 安装中（排队、拉镜像、创建容器、启动、失败）+ 进度 |
| 占用站点 | 绑定站点数 |
| 接入方式 | 只显示短标识：PHP `sock`、Rust 为容器端口；完整 socket/端口路径放 `title` 悬浮提示与抽屉里，不再单列长路径 |
| 操作 | 只保留一个「详情」入口；整行或按钮都打开右侧抽屉，不再并列「更多」 |

- 表格按内容自适应列宽并铺满容器，不固定撑宽某一列；接入方式列宽度只够放 `sock` / `:8080`。
- 空态：没有任何真实实例时显示「还没有运行环境」+「新建运行环境」主操作；迁移种入的 PHP 模板（8.5/8.4/8.3）只在新建弹窗里出现，不在列表占位。
- 顶部固定边界提示：`本页管理语言运行环境与绑定关系；Web 服务在「Web 服务」页单独管理，原始容器操作请到 Docker → 容器。`
- 主操作统一放右上角 `page-head .actions`（「新建运行环境」），不再下沉到列表头部。

### 4.2 PHP 环境详情

标签：`基础信息 / 专属配置 / 日志 / 关联站点`（页内标签，不新增菜单 L3）。

| 区块 | 内容 | P1 |
| --- | --- | --- |
| 基础信息 | 版本、镜像与 digest、socket 路径/权限、默认版本、启动时间、最近健康检查、绑定站点数 | 可读 |
| 扩展开关 | 基线全集按分类展示（数据库 / 图片 / 加密 / 缓存 / 其他），按 `(version, base_image)` 过滤；支持搜索、PECL 按名字安装 | P1 可增删：安装阶段装入，安装后在抽屉里开关并重建/重载（见 §11.4） |
| 安装任务 | 状态（排队/拉镜像/创建容器/启动/失败）、进度百分比、实时日志、失败原因与「重试安装」 | P1 异步执行，抽屉轮询刷新 |
| ini | `memory_limit`、`upload_max_filesize`、`post_max_size`、`max_execution_time` | P1 可改，保存后平滑 reload |
| FPM 进程池 | `pm`（dynamic/static）、`max_children`、`start_servers`、`min_spare_servers`、`max_spare_servers`、`max_requests` | P1 可改 |
| socket | 路径（只读）、权限 `0660`、属组 `65534`；不允许世界可写 | 只读 |
| 关联站点 | 域名、类型、状态、端口/socket；点击进站点详情 | 可读 |
| 日志 | `fpm-error.log`、运行环境操作日志、重建日志 | 可读、可筛选 |

- 配置保存流程：校验数值范围 → 写 `conf/` 临时文件 → `nginx -t` 与 FPM 配置检查 → 平滑 reload；
  任一步失败恢复原文件并提示失败行；**保存 ≠ 生效**必须显示“已保存，尚未生效/已生效”的状态。
- 重建流程：拉取固定 digest 镜像 → 生成配置 → 重建容器 → socket 就绪探测 → 成功后才把
  `desired_state` 置 running；失败保留旧容器与旧配置，明确报错。
- 停止 PHP 环境：所有绑定 PHP 站点显示“环境停止”，vhost 保留但请求返回 502/503 维护响应；
  恢复后自动回到运行中，不逐个改站点状态。

### 4.3 Node / Go 环境详情（P2 设计态）

- 基础信息：版本、镜像 digest、进程模型（每站点独立容器 + loopback 端口）、启动时间、关联站点数。
- 专属配置：版本只读；启动命令模板、健康检查路径、重试策略、端口分配范围；不提供任意 Docker 参数。
- 关联站点：域名、端口、健康状态、最近重启；点击进站点详情。
- 日志：环境操作日志 + 每个站点 `app.log` / `app.err.log` 的聚合入口。
- **当前实现**：Node / Go **未实现**，已从运行环境目录与新建弹窗摘除，列表/详情不存在入口；
  本节仅保留 P2 设计态说明。接入时必须先补齐安装、启动、端口分配与站点反代链路，再恢复目录项。

### 4.4 启停 / 重建 / 删除规则

| 动作 | 规则 |
| --- | --- |
| 启动 | 检查镜像 digest 与端口/ socket 可用；PHP 还需 socket 可连接 |
| 停止 | 运行环境 `desired_state = stopped`；绑定站点有效状态变为“环境停止”，不删站点 |
| 重建 | 同一 `id`、同一版本；生成新容器后原子切换；失败回滚旧容器/旧配置 |
| 删除 | **存在绑定站点时禁止**；列表给出域名与“先改绑或删除站点”的动作；删除默认版本前先切换默认 |
| 改版本 | 运行环境不可原地改版本；新建同 kind 新版本，站点逐个改绑（P2 提供批量改绑） |
| 改绑站点 | 目标环境必须同 kind 且 running；PHP 站点换环境后重新生成 vhost 的 socket；Node/Go 换环境后重建站点容器 |

### 4.5 新建运行环境与异步安装

- **新建弹窗**：类型下拉（PHP / Rust）→ 版本下拉（大版本下的小版本，如 `8.5 / 8.4 / 8.3`）
  → 实例名（默认 `php-8.5` 跟随类型与版本，可改；同 kind 内唯一，目标目录已存在直接拒绝）。
  不允许手填版本；Node / Go **未实现**，已从 `RUNTIME_KINDS` 与弹窗摘除，提交 `kind=node|go` 由后端
  按「未知运行环境类型」拒绝；Rust 可安装并建站。
- **版本目录单一来源**：`crates/app/src/runtime_catalog.rs` 的 `RUNTIME_KINDS` 同时驱动弹窗下拉、
  安装镜像选择与后端校验；新增版本只改这一处，模板不再硬编码版本列表。
- **扩展清单单一来源**：同一模块的 `PHP_EXTENSIONS` 同时驱动安装弹窗勾选、抽屉开关、
  `extensions` 字段校验与容器内执行脚本；默认集合为 `default_on` 项（pdo_mysql / mbstring 等）。
- **异步安装**：点击「创建并后台安装」立即返回（落库 + 入队），后台任务按
  `排队 → 拉镜像 → 创建容器 → 启动 → 完成/失败` 推进；列表与抽屉轮询
  `/sites/env/jobs`、`/sites/env/{id}/job` 获取状态、进度与日志。
- **安装日志**：任务持有最近 300 行日志，抽屉内 `data-job-log` 实时追加；失败保留原因，
  提供「重试安装」入口，不阻塞其它页面操作。
- **扩展变更**：抽屉提交 `/sites/env/{id}/extensions/{key}/install|uninstall` 或 `/pecl`，异步执行
  mlocati `install-php-extensions`（下载失败回退手写 apk 依赖 + `docker-php-ext-install` / `pecl install`），
  完成后重启容器、收紧 socket 权限并落库；容器不存在时提示先安装/启动。
- **持久化**：`site_runtimes.extensions`（schema v9）保存逗号分隔的启用集合，安装/扩展任务成功后回写。
- **PHP 池日志口径**：FPM 池显式 `access.log = /dev/null`，站点访问日志只由 Nginx 写；`error_log` 仍指 fd 2，由容器日志承接，不在站点目录重复落盘。

---

## 5. 与 Docker 容器页的边界

### 5.1 归属

| 维度 | 运行环境页 | Docker 页 |
| --- | --- | --- |
| 视角 | 业务：版本、绑定站点、专属配置、影响面 | 基础设施：原始容器、镜像、网络、卷 |
| 生命周期 | 期望状态 + 重建/删除业务动作 | 原始 start/stop/restart/remove/exec |
| 配置 | PHP ini / FPM / Node 模板 | 容器参数、挂载、环境变量 |
| 站点关系 | 展示“影响 N 个站点” | 展示“受管运行环境”徽章与返回链接 |

- 运行环境容器命名确定性：`skiff-rt-<id>`（如 `skiff-rt-php-8.5`）；标签
  `skiff.managed=runtime`、`skiff.runtime=<id>`。Node/Go 站点容器：`skiff-site-<slug>`，
  标签 `skiff.managed=site`、`skiff.site=<域名>`。
- Docker 页**不隐藏**这些容器：显示徽章“运行环境/站点”，操作前提示影响面；删除/停止等破坏性动作
  必须二次确认并说明“将影响 N 个站点，业务页可能出现状态漂移”。
- 运行环境页提供“在 Docker 中查看”深链：`/{p}/m/core/docker/containers?filter=<container>`；
  Docker 行提供“返回运行环境”深链：`/{p}/m/core/sites/env/<id>`。
- 两页共享同一套实际状态读取；Docker 页不写 `site_runtimes`，运行环境页不直接编辑原始容器参数。

### 5.2 禁止事项

- 不在运行环境页提供容器 exec、任意挂载、任意网络、Compose 编辑；这些只属于 Docker 页。
- 不在 Docker 页提供“给站点换运行环境”“改 PHP ini”等业务操作；避免两套配置互相覆盖。
- 不在两处各自维护“运行中/已停止”副本；以 Docker 实际状态 + DB 期望状态合成展示。

---

## 6. 路由、权限与审计

### 6.1 路由与深链

L2 保持 `docs/MENU_CONTRACT.md` 的内置项：网站列表 / 运行环境 / Nginx。运行环境详情是 L2 下的
子页面，不新增 L3（L3 只属于插件）。站点级设置页已移除：镜像加速源/前缀归 Docker 设置，
nginx/PHP 镜像标签归运行环境页的「镜像参数」。

| 页面 | 规范 URL | 旧路由别名 |
| --- | --- | --- |
| 网站列表 | `/{p}/m/core/sites/list` | `/{p}/sites` |
| 新建网站 | `/{p}/m/core/sites/new`（可带 `?type=runtime|static|proxy`） | `/{p}/sites/new` |
| 站点详情 | `/{p}/m/core/sites/list/<域名>` | `/{p}/sites/<域名>` |
| 运行环境列表 | `/{p}/m/core/sites/env` | `/{p}/sites/env` |
| 运行环境详情 | `/{p}/m/core/sites/env/<runtime-id>` | `/{p}/sites/env/<runtime-id>` |
| 运行环境标签页 | `/{p}/m/core/sites/env/<runtime-id>?tab=config|logs|sites` | 同上 |

- 站点级设置页已移除：`image_prefix` / `registry_mirror` 归 Docker 设置（`/{p}/m/core/docker/settings`），
  `nginx_image` / `php_image` 归运行环境页的「镜像参数」。

- `runtime-id` 只用 `[a-z0-9.-]`（如 `php-8.5`），不把镜像 tag、路径拼进 URL。
- 深链无 JS 可打开；切换标签只改 query 或服务端渲染，不依赖前端路由。
- 未知 runtime-id：404（不泄露存在性）；已删除运行环境的历史深链返回 404 并在列表页给出说明。

### 6.2 权限矩阵

P1 沿用现有站点权限，避免语义漂移；运行环境写操作同样仅 admin。

| 操作 | viewer | operator | admin |
| --- | --- | --- | --- |
| 查看站点列表/详情/日志 | ✓ | ✓ | ✓ |
| 查看运行环境列表/详情/日志 | ✓ | ✓ | ✓ |
| 新建/删除/启停站点 | — | — | ✓ |
| 修改站点配置 / SSL / 默认站 | — | — | ✓ |
| 安装/重建/启停/删除运行环境 | — | — | ✓ |
| 修改 PHP ini / FPM / 扩展 | — | — | ✓ |
| 建站向导读取可用运行环境 | ✓ | ✓ | ✓ |

- 所有写操作要求 CSRF；未授权用户直接访问 URL 返回 404（菜单不渲染死入口）。
- 若未来要让 operator 建站，必须单独修改 `skiff_model::Permission` 矩阵并补测试，不在本契约默认放开。

### 6.3 审计事件

统一使用 `sites.*` 命名；P1 把现有 `sites.env.install` / `sites.env.config` 改名为
`sites.runtime.install` / `sites.runtime.config`，**不做双写**（旧记录仅作历史文本，不参与语义查询）。

| 事件 | 触发 | 目标字段 |
| --- | --- | --- |
| `sites.runtime.install` | 首次安装运行环境 | runtime id |
| `sites.runtime.start` / `.stop` | 启停运行环境 | runtime id |
| `sites.runtime.rebuild` | 重建运行环境 | runtime id + 镜像 digest |
| `sites.runtime.config` | 保存 ini/FPM/模板 | runtime id + 变更键数（不记录完整配置） |
| `sites.runtime.delete` | 删除运行环境 | runtime id |
| `sites.create` | 新建站点 | 域名 + `site_type` + runtime id |
| `sites.delete` | 删除站点 | 域名 |
| `sites.start` / `sites.stop` | 启停站点 | 域名 |
| `sites.config.save` | 保存 vhost/站点配置 | 域名 |
| `sites.ssl.*` | 证书上传/自签/强制跳转/删除 | 域名 |
| `sites.default` | 切换默认站 | 域名 |
| `sites.batch.*` | 批量启停/删除 | 批量域名摘要 |
| `sites.rebind` | 站点改绑运行环境（P2） | 域名 + 旧/新 runtime id |

- 审计详情不记录私钥、口令、完整 ini 内容；失败动作记录 `fail` 与可读错误，不写入栈。

---

## 7. P1 / P2 范围

### P1（必须一次做完，且可验收）

1. **schema v8**：`site_runtimes` 以 `name` 为本地实例唯一标识（`UNIQUE(kind, name)`），
   `sites` 使用 `site_type/runtime_id/...`，删除 `php_version`（表重建，不做兼容双写）。
2. **三类站点**：静态站、PHP 站（绑定已就绪 PHP 实例）、反向代理站（含 WebSocket）完整建站、启停、删除、
   配置生成与日志。
3. **三步向导**：类型选择、运行时选择（列出所有已就绪 PHP / Rust 实例）、类型相关字段、校验与确认；Node/Go 未实现，不出现选项。
4. **运行环境页**：PHP / Rust 列表与右侧抽屉详情、基础信息、ini/FPM 配置、socket 只读、日志、关联站点、启停/重建/删除
   保护；列表与详情不常驻安装进度/镜像源；Node/Go 不显示目录项或信息卡。
5. **多版本 PHP**：同 kind 多实例并行，每实例独立 socket `/www/runtime/php/<name>/php-fpm.sock`；
   vhost 按站点 `runtime_id` 路由；停止实例时绑定站点明确 502/503 且可回环境页，不静默换实例。
6. **状态与边界**：运行环境停止导致绑定站点显示“环境停止”；删除被引用环境被拒绝；Docker 页徽章与双向深链。
7. **路由/权限/审计**：规范 URL 与旧路由别名；admin 写、viewer 读；`sites.runtime.*` 事件。
8. **测试**：§8 中 P1 项全绿；根 workspace 三关全绿；不部署测试机。

### P2（扩展，不阻塞 P1）

- Node.js / Go 运行环境安装、重建、启停；每站点独立容器 + loopback 端口 + 健康检查 + 应用日志。
- Go 只运行**用户已编译二进制**，面板不做 `go build`、不假装有构建链路；Node 依赖安装若做，必须有独立
  构建日志与失败回滚，不能静默假装成功。
- PHP 扩展开关与受控镜像重建（Dockerfile 在 `ext/`，构建日志可见）；站点级 php.ini / 独立 FPM 池。
- 运行环境漂移检测的“同步状态”、站点改绑向导、批量改绑。

### 非目标（P1/P2 都不做）

- 数据库创建、SSL 自动申请（Let’s Encrypt）、应用商店、一键源码部署、Compose 编辑。
- **WAF 拦截未实现**：`sites.waf_mode` 设置键与 `waf_rules_dir` 规则目录占位已在 2026-11-09 删除；
  要做需先定规则来源、开关位置与回滚口径。
- **SSL 保留现状**：自签证书、证书上传与强制 HTTPS 已实现，P1 不回退、不改入口；建站向导只在确认页
  说明「SSL 后续在站点 SSL 标签配置」，不内嵌申请流程。
- 在“静态站”上启动 PHP/Node；在“反代站”上托管本地文件；“一站多模式”复合站点。

---

## 8. 测试矩阵

| # | 场景 | 断言 | 层 | 阶段 |
| --- | --- | --- | --- | --- |
| T1 | 站点类型与运行时绑定 | `runtime` 缺 `runtime_id` 拒绝；`static`/`proxy` 带 `runtime_id` 拒绝 | 单测 | P1 |
| T2 | kind 一致性 | PHP 站点绑 Node 环境拒绝；Node 站点绑 PHP 环境拒绝 | 单测 | P1 |
| T3 | 运行环境未就绪 | 停止/异常环境不能新建绑定站点；错误含环境状态 | 单测/集成 | P1 |
| T4 | 共享 PHP socket | 两个 PHP 站点使用同一 `socket_path`；vhost 不硬编码路径 | 集成 | P1 |
| T5 | Node/Go 端口分配 | 端口唯一、范围 `30000–32767`、占用/监听冲突拒绝、删除后释放 | 单测 | P2 |
| T6 | 反向代理校验 | 非 http(s)、带 userinfo、缺 host、回环到自身均拒绝；WebSocket 头生成 | 单测/集成 | P1 |
| T7 | 静态站无运行时依赖 | 无任何运行环境也可创建、启停、访问 200 | 集成 | P1 |
| T8 | 删除被引用运行环境 | 有绑定站点时拒绝；错误列出域名；无绑定时删除成功且不删站点 | 单测/集成 | P1 |
| T9 | 停止运行环境的级联 | PHP 环境停止后站点显示“环境停止”，恢复后回到运行中；不修改站点自身期望状态 | 集成 | P1 |
| T10 | 默认版本 | 同 kind 至多一个默认；删除/切换默认规则；向导默认选中 | 单测 | P1 |
| T11 | 向导校验 | 域名格式/重复、别名冲突、目录越界、空字段、上游非法、端口冲突逐项报错 | 集成/浏览器 | P1 |
| T12 | 权限矩阵 | viewer/operator 无写操作；admin 可写；未授权 URL 404；CSRF 缺失拒绝 | 集成 | P1 |
| T13 | 审计事件 | 建站/启停/删除/环境配置/重建事件名与目标正确；无敏感内容 | 单测/集成 | P1 |
| T14 | 路由与深链 | 规范 URL 与旧别名均 200；runtime 详情/标签页；未知 id 404 | 集成 | P1 |
| T15 | Docker 边界 | 受管容器带徽章；Docker 操作后运行环境页显示漂移；双链正确 | 集成/浏览器 | P1 |
| T16 | 状态漂移 | 外部停止容器后页面显示“状态漂移”；“仅同步状态”不重启容器 | 集成 | P1 |
| T17 | 迁移 | v6→v8 幂等；旧站点映射到 `php-8.5`；默认站/SSL/别名不丢；`name` 回填 | 单测 | P1 |
| T18 | Node/Go 未实现 | P1 不出现“安装成功”或假端口；按钮禁用且有 P2 文案 | 集成/浏览器 | P1 |
| T19 | Node/Go 健康检查 | 健康检查超时置 error、保留日志、不无限重试；恢复后 running | 集成 | P2 |
| T20 | 多实例 PHP | 同 kind 不同 `name` 的 socket/容器互不覆盖；站点按 `runtime_id` 指向各自 socket；同版本多实例允许 | 单测/集成 | P1 |

浏览器验收至少覆盖：向导三类分支、校验报错、运行环境列表筛选、PHP 详情标签切换、配置保存失败回滚提示、
Node/Go P2 禁用态、站点列表窄屏无横向滚动。

---

## 9. 变更影响面

### 9.1 数据库

- 当前 `CURRENT_SCHEMA_VERSION = 8`；本契约目标 v8。
- `site_runtimes`：
  ```sql
  CREATE TABLE site_runtimes (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('php','node','go')),
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    image TEXT NOT NULL,
    image_digest TEXT NOT NULL DEFAULT '',
    desired_state TEXT NOT NULL DEFAULT 'running'
      CHECK(desired_state IN ('running','stopped')),
    last_status TEXT NOT NULL DEFAULT 'stopped'
      CHECK(last_status IN ('installing','running','stopped','error')),
    socket_path TEXT,
    is_default INTEGER NOT NULL DEFAULT 0,
    config_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(kind, name)
  );
  CREATE UNIQUE INDEX idx_runtime_default ON site_runtimes(kind) WHERE is_default = 1;
  ```
- `sites` 表重建：删除 `php_version`，新增 §1.1 字段；外键 `runtime_id REFERENCES site_runtimes(id) ON DELETE RESTRICT`
  （SQLite 需开启外键；服务层仍做“有绑定站点禁止删除”校验，不能只靠 FK）。
- 迁移策略：v8 重建 `site_runtimes`，按 `id` 回填 `name`；内置 `php-8.5` 的 socket 收敛为
  `/www/runtime/php/php-8.5/php-fpm.sock`。旧站点 `runtime_id` 保持不变，不做旧固定 socket 路径迁移。

### 9.2 文件与目录

- `/www/sites/<域名>/` 布局不变；运行环境目录为 `/www/runtime/<kind>/<name>/`。
- **vhost 必须从 DB `socket_path` 生成**；该字段只由实例 `name` 推导，不按版本号硬编码。
- 删除站点仍只删该站目录；删除运行环境不删站点目录。

### 9.3 菜单与导航

- `core/sites/list`、`core/sites/env`、`core/sites/web`（原 `core/sites/nginx`，测试期直接换键、不兼容旧链接）三项不变；运行环境详情不新增 L2/L3。
- 运行环境详情返回列表时保留筛选与当前 kind；深链落在 `env` 菜单当前态。
- 不改变顶部/侧栏布局与菜单偏好结构。

### 9.4 概览健康块

- 概览的服务健康块新增/扩展“网站运行环境”信号：运行环境异常或状态漂移时显示计数与受影响站点数；
  点击跳 `core/sites/env`。
- 无运行环境 / 无站点时显示明确空态，不写死数字；P1 不做 Node/Go 伪造指标。

### 9.5 现有代码与模板（仅登记影响，不在本轮修改）

| 位置 | 影响 |
| --- | --- |
| `crates/store/src/db.rs` / `db/sites.rs` | v7 迁移、`site_runtimes` CRUD、`sites` 字段重建 |
| `crates/app/src/sites.rs` | 按类型生成 vhost、端口分配、PHP socket 取 `runtime_id`、Node/Go 进程模型（P2） |
| `crates/web/src/handlers/sites.rs` | 向导三步、运行环境详情、校验、权限、审计改名 |
| `crates/web/templates/site_new.html` / `site_env.html` / `sites.html` / `site_detail.html` | 三类分支、环境列表与详情、站点类型列 |
| `crates/web/src/menu.rs` | 仅确认 `core/sites/*` 稳定键，不新增 L2 |
| `docs/DEPLOY_TEST.md` / `docs/TEST_LEDGER.md` | 目录、socket、端口、验收用例同步 |
| `docs/ui/site-runtime-preview.html` | 本契约的可点预览，不参与产品构建 |

### 9.6 1Panel 对照取舍

| 1Panel 能力 | Skiff 取舍 |
| --- | --- |
| 网站 → 运行环境（PHP/Node/Go 版本安装） | 采纳；但运行环境页只做业务视角，原始容器仍归 Docker |
| 建站先选“运行环境/静态/反代” | 采纳，作为三步向导第一步 |
| PHP 扩展、ini、FPM 进程池 | 采纳；P1 ini/FPM 可改，扩展只读，P2 受控重建 |
| Node/Go 项目一键部署、源码构建 | 不照抄；P1 不提供，P2 Go 只运行已编译二进制，不做 `go build` |
| 数据库创建、SSL 一键申请（Let’s Encrypt） | 不做；自签/上传证书保留现状；向导不放假开关 |
| 应用商店、Compose 编排 | 不做；避免与内核“三核心”边界冲突 |

---

## 10. 需要拍板的决策（本轮回复中给推荐）

### Q1. P1 是否必须包含 Node.js / Go 站点？

- 已定：**不包含，且已摘除**。2026-11-09 从 `RUNTIME_KINDS` 与新建弹窗移除，不再有「禁用态」占位；
  安装、进程、端口与健康检查等 P2 设计仍保留在 §4.3，未排期。
- 理由：没有构建/进程守护链路时做“能跑”的 Node/Go 必然假实现；先把 PHP / Rust / 静态 / 反代闭环做扎实。

### Q2. PHP socket 路径：固定 `/www/php/php-fpm.sock`，还是每实例独立路径？

- 已定：**每实例独立路径 `/www/runtime/php/<name>/php-fpm.sock`**，由实例 `name` 推导；
  内置 `php-8.5` 即 `/www/runtime/php/php-8.5/php-fpm.sock`。不同实例绝不共用 socket。
- 理由：路径只依赖 `name` 后，同版本多实例（`php-8.5` / `php-8.5-a`）天然隔离；vhost 按站点
  `runtime_id` 取对应实例 socket，不按版本号推导，也不再散落硬编码。

### Q3. Node/Go 的运行模型：每站点独立容器，还是每环境一个常驻进程？

- 推荐：**每站点独立容器 + 独立 loopback 端口 + 独立日志**；运行环境只提供镜像/版本/模板。
- 理由：站点级启停、日志、端口冲突与影响面清晰；符合内核 Docker 能力面，也避免一个进程崩溃拖垮同环境站点。

### Q4. 删除运行环境时存在绑定站点，如何处理？

- 推荐：**直接拒绝删除**，列出绑定域名并给“改绑/删除站点”入口；不提供隐式解绑或级联删除。
- 理由：运行环境是共享资源，隐式操作会一次影响多个站点；显式阻断更符合“保存 ≠ 生效、操作可追溯”的基调。

---

## 附：状态文案与空态

| 场景 | 文案 |
| --- | --- |
| 无运行环境 | `还没有运行环境。先安装 PHP 8.5，再创建需要运行时的网站。` |
| 环境停止 + 有绑定站点 | `环境已停止，3 个站点当前不可访问。启动环境后会恢复。` |
| 环境异常 | `环境异常：socket 不可连接。查看日志 / 重建环境` |
| Node/Go（已摘除） | 列表与向导不再出现该类型；直接提交 `kind=node`/`kind=go` 返回 `未知运行环境类型` |
| proxy 无文档根 | `反向代理站点不创建文档根，只生成转发配置。` |
| 无绑定站点 | `该环境暂无站点。去网站列表新建。` |

---

## 11. P1 实现补充（2026-11-06）

> 本节记录与 §1–§10 设计契约的实现差异与落地口径；与上文冲突时以本节为准。

### 11.1 版本显示：镜像元数据 + `image_id` 内存缓存

- **数据来源**：列表用的 `list_containers` 已返回 `ImageID`；`ContainerRow.image_id` 随行模型返回，
  渲染路径不再调用 `docker exec`，也不再为每个实例额外查一次容器。
- **缓存**：进程内 `HashMap<image_id, (Option<String>, Instant)>`。未命中时只对**该镜像**执行
  1 次 `docker image inspect`，从 `Config.Env` 读 `PHP_VERSION` / `NODE_VERSION` / `GOLANG_VERSION`。
  同一 `image_id` 的多个实例共享一次结果；正缓存**不设 TTL**（image_id 不可变），
  失败做 **60 秒负缓存**，避免每页重复打 Docker。Rust 官方镜像无稳定版本变量，回退目标版本并标注。
- **显示**：
  - 已知 → 主显真实补丁号（如 `8.5.12`），副行 `目标 8.5 · php:8.5-fpm-alpine`；
  - 容器缺失/未安装 → `8.5` +「未安装」；
  - 容器存在但元数据读不到 → `8.5` +「版本未读取」；
  - **容器停止但容器仍存在时照常显示镜像真实版本**（已知事实，不因未运行回退）。
- **唯一保留的 exec**：抽屉「实测版本」按钮，用户主动点击才执行 `php -r 'echo PHP_VERSION;'`
  （2 秒超时），显示结果与时间戳，并注明「仅代表当前运行中的容器」。
- **digest 落库**：安装/重建拉完镜像后从 `ImageInspect.repo_digests` 取 manifest digest 写入
  `site_runtimes.image_digest`；默认仍按 tag 重建，digest 只作复现/回滚信息与详情展示。
  登记镜像与容器实际 `image_id` 对应镜像不一致时，抽屉提示「登记镜像与容器镜像不一致」，只提示不自动修。

### 11.2 镜像来源结论与 1Panel 对比

- **`php:8.5-fpm-alpine` 的来源**：仓库内唯一默认值是 `crates/app/src/sites.rs` 的
  `DEFAULT_PHP_IMAGE = "php:8.5-fpm-alpine"`（`runtime_catalog::image_for` 对非默认版本拼
  `php:<ver>-fpm-alpine`）。它是 **Docker Hub 官方 `php` 镜像的 alpine/fpm 变体**
  （docker-library/php 维护），**不是** 1Panel 的镜像，也不是本项目自建镜像。
- **1Panel 对照**：

| 维度 | 1Panel 运行环境 | Skiff 当前 |
| --- | --- | --- |
| 镜像来源 | 自建镜像（PHP 为 `1panel/php` 系列 / `1panel-php-fpm:<patch>`），按面板版本统一发布 | Docker Hub 官方 `php:<ver>-fpm-alpine`，默认版本可由「镜像参数」覆盖 |
| tag 规则 | 自建 tag 跟补丁号（如 `8.3.24`），可复现 | 浮动 tag（`8.5-fpm-alpine`），实际补丁号由拉取时的镜像元数据回填 |
| 运行形态 | 每站点/每环境独立容器，常驻端口 + FPM，站点经端口反代 | 每实例独立容器 + **unix socket**，nginx 直接 `fastcgi_pass`，不暴露 9000 |
| 扩展安装 | 面板预置扩展清单 + 图形化安装，镜像内置编译依赖 | 源码内置全集 + PECL 按名字安装，mlocati 解析依赖，容器内编译 |
| alpine/debian | 两种基础镜像都提供；默认 alpine | 默认 alpine；`base_image_of` 按镜像名识别，debian 已按 glibc 处理 |

- **alpine 与 debian 的差异**：alpine 用 musl、镜像体积小、启动与内存占用低；debian 用 glibc、
  体积大但二进制兼容性好（Oracle Instant Client、商业 loader、部分预编译扩展只发 glibc 版）。
- **结论与取舍**：
  - **继续以 alpine 为默认**：体积/内存与官方维护成本更优，PHP 扩展可用 mlocati 在 alpine 上解析 apk 依赖；
  - 代价：扩展编译依赖 apk、遇过 `oniguruma`/PECL 编译坑，`oci8` / `pdo_oci` / `ioncube_loader` / `sg16`
    在 alpine **明确不可用**（UI 灰显 + 原因），需要这些扩展时改用 debian 基础镜像；
  - **不擅自改默认镜像**：默认仍为 `php:8.5-fpm-alpine`；是否提供 debian 变体作为可选项留待后续。
- **镜像源与固定 tag**：
  - `image_prefix` / 加速源已存在（Docker 设置页 `DockerSettingsPage.image_prefix`），不新增配置；
  - 浮动 tag 建议**拉取时记录 digest 并落库**，重建默认仍按 tag；是否改成「按 digest 锁定」暂不动，
    因为当前无法保证 digest 在扩展编译/重建路径上始终可用。

### 11.3 Rust 运行环境（P1）

- 目录：`runtime_catalog::RUNTIME_KINDS` 新增 `rust`，版本白名单 `1.99 / 1.98 / 1.97 / 1.83`，
  镜像 `rust:<ver>-slim`（如 `rust:1.99-slim`）；`default_name` 为 `rust-<ver>`，同 kind 唯一、目录已存在拒绝。
  - 1.99.0 于 2026-10-01 发布、1.99.1 于 2026-10-15；保留 1.83 作为旧版本可选，避免只留最新一条。
  - 白名单与 `image_for` 必须同步：`image_for("rust", "1.99") == "rust:1.99-slim"`，非白名单版本拒绝。
  - **镜像可拉取性待实测**：本轮开发机无 Docker 且不能直连 Docker Hub，`docker pull rust:1.99-slim`
    需并入上机清单验证；若某 tag 实际不可拉，回退到下一个白名单版本并同步改测试。
- 运行模型（独立进程 + 固定端口）：
  - 端口区间 **9100–9199**，创建时取区间内最小未占用端口并落 `site_runtimes.port`；冲突直接拒绝；
  - 容器 `skiff-rust-<name>`，`network_mode=host`，挂载 `/www`；
  - 容器入口固定为 `sh /www/runtime/rust/<name>/entry.sh`，日志重定向到
    `/www/runtime/rust/<name>/log/server.log`；`entry.sh` 运行时读取宿主标记
    `/www/runtime/rust/<name>/site`：
    - 有标记（环境已绑定站点）→ `cargo run --release --manifest-path <标记指向的站点 index>/Cargo.toml`；
    - 无标记（未绑定任何站点）→ 回落 `/www/runtime/rust/<name>/app/Cargo.toml`；
  - `CARGO_TARGET_DIR=/www/runtime/rust/<name>/target`，构建产物不写站点目录；
  - 安装任务写最小 std HTTP 工程（无第三方依赖），启动后 `wait_tcp_port` 等端口监听，失败回传日志尾部；
  - 目录结构：`/www/runtime/rust/<name>/{entry.sh,site,app/{Cargo.toml,src/main.rs},log/server.log,target/}`。
    站点目录与 `app/` 不复用同一份脚手架：站点源码只存在于站点 `index/`，`app/` 仅作无绑定兜底。
  - 绑定/解绑/删除站点后内核同步 `site` 标记；标记变化且容器在运行时重启容器，停止的容器下次启动自然生效。
- 站点绑定：站点类型 `runtime`，vhost 生成 `proxy_pass http://127.0.0.1:<port>`（不是 `fastcgi_pass`）；
  解绑入口在抽屉「站点」页签，删除环境仍受「存在绑定站点即拒绝」约束。
- Node / Go 保持 P2 现状，不在本轮改动。

### 11.4 详情抽屉与扩展管理

- **抽屉宽度**：`width: min(840px, 92vw)`，`max-width: 840px`；`max-width: 720px` 时整宽 `100vw`（窄屏）；
  内容区在 `min-width: 980px` 时两栏，窄屏单栏。头部与底部操作固定，中间内容滚动。
- **页签**：`基础信息 / 扩展 / 配置 / 日志 / 站点`，沿用 `.workbench-tabs`，抽屉内切换不整页刷新。
- **扩展矩阵（只做 delta）**：
  - 基线 `PHP_EXTENSIONS` 全集按分类（数据库 / 图片 / 加密 / 缓存 / 其他）展示，支持搜索与分类过滤；
  - 版本 delta 用 `since / core_until / pecl_only_since`：8.4 起 `imap` / `oci8` / `pdo_oci` / `pspell`
    移出核心，8.5 延续；8.3 仍可选 `imap`；
  - 基础镜像 delta：alpine 上 `oci8` / `pdo_oci` / 商业 loader 为 `Unsupported`，`rdkafka` 可用；
  - `extensions_for(version, base_image)` 同时供 UI 与后端校验；**UI 只返回可用项**，不可用项
    直接不显示（不灰显、不占位、不带原因文案），原因保留在后端错误里；依赖差异项提示
    「将临时安装 X/Y」；后端错误码 `EXT_NOT_IN_VERSION` / `EXT_BASE_IMAGE_UNSUPPORTED` /
    `EXT_PECL_NOT_READY`，文案带版本与替代建议。
  - **builtin 修正**：`mbstring`、`sodium`、`sqlite3`、`pdo_sqlite` 标为镜像内置（官方
    `php:*-fpm-alpine` 已编译并启用），不再触发 `oniguruma-dev` 安装与编译，也不重复 enable。
- **依赖解析**：使用随包分发的固定版本 mlocati/docker-php-extension-installer（版本/基座感知，
  安装后自动清理）。脚本落在 `<web_root>/skiff/tools/install-php-extensions`，容器内固定读取
  `/www/skiff/tools/install-php-extensions`，**运行时默认不访问 GitHub**；本地副本缺失时打印明确
  错误并回退固定版本下载，仍失败再回退原手写依赖表。依赖名修正 `ldap-dev → openldap-dev`、
  `tidy → tidyhtml-dev`、`snmp → net-snmp-dev`。
- **热装与重建**：`hot=true` 的扩展在运行中的容器内安装/卸载后重启容器即可；
  `hot=false` 或镜像层依赖变化（如新增系统库、需要改基础镜像）必须重建容器。
  编译很重的 PECL（`grpc` / `swoole` / `mongodb`）允许安装，但 UI 标注预计耗时，失败回传真实 stderr。
- **1Panel 对照**：1Panel 的运行环境页是「版本 + 容器」入口，扩展在 PHP 配置页按清单勾选安装，
  依赖由自建镜像预置；Skiff 保留同样的「清单 + 分类 + 安装日志」信息架构，但复用官方镜像与
  mlocati 动态解析依赖，不维护自建镜像，因此新增/升级扩展不需要重新发布面板镜像。

### 11.5 验收矩阵（本轮）

| 用例 | 口径 |
| --- | --- |
| 版本显示 | 已知显示补丁号；容器缺失 → 目标 + 未安装；元数据缺失 → 目标 + 版本未读取；停止但存在 → 仍显示真实版本 |
| 缓存 | 同 image_id 只 inspect 一次；image_id 变化重新 inspect；失败 60 秒负缓存；列表渲染路径 0 次 exec |
| Rust | 创建 → 安装异步任务 → 端口监听 → 站点绑定 `runtime` → 反代返回内容；无 Docker 时验证失败链路与渲染 |
| 抽屉 | 宽度 `min(840px, 92vw)`；五页签可切换；扩展搜索/分类过滤 |
| 扩展矩阵 | 8.3 imap 可选、8.4/8.5 不可用；alpine oci8 不可用；mbstring/sodium 不产生编译步骤；PECL 失败回传真 stderr |

## 12. 运行环境拆分、扩展多选与安装器本地化（2026-11-06）

### 12.1 取消「基础环境」

- **模型**：运行环境列表只列真实语言实例（`php` / `rust`，以及用户显式创建、尚未实现的 `node` / `go` 登记行），
  各自安装、各自状态、各自删除，互不连带。**Web 服务（OpenResty）不属于运行环境列表**，归 `/$P/sites/web` 页管理。
  站点创建只做两类独立校验：
  1. Web 服务：`docker_available && nginx 容器存在 && running`，否则提示「装 Web 服务」；
  2. 语言运行环境：所选实例存在且运行中，否则提示「装对应语言运行环境」。
  不再存在「基础环境就绪」的合并判断。
- **种子与实例**：`site_runtimes.origin ∈ {instance, seed}`（schema v13）。迁移种入、从未安装且无站点引用的
  PHP 行标记为 `seed`，列表通过 `RuntimeView::is_listed` 过滤（种子只在已安装或被站点引用时显示）；
  同类型同名新建时会接管无引用的种子行。种子仍可按 id 直达详情页，模板入口只出现在新建流程。
- **Web 服务页**：容器名 `skiff-web-nginx`，状态与运维入口（安装/启停/重启/重建/删除）在
  `/$P/sites/web?tab=status`；`install`/`rebuild` 只拉取 OpenResty、重建 vhost、启动 nginx，
  不触碰任何 PHP/Node/Go/Rust 容器。运行环境页不再有合成 `web` 行与行内 Web 动作。
- **删除的耦合**：`add_runtime` 弹窗内的「安装基础环境（OpenResty + 默认 PHP 8.5）」次要表单、
  `/sites/env/install` 路由、`base_install_href` 上下文、`run_site_env_install` 里的 PHP 池配置 /
  PHP 镜像拉取 / PHP 容器创建与 socket 收紧，全部移除。
- **schema**：`site_runtimes` 无「基础环境」标记；v13 新增 `origin`（`instance` / `seed`），`is_default` 仍是 per-kind 默认实例。
- **vhost**：站点 vhost 生成只依赖 Web 服务存在 + 所选语言实例的 socket（PHP）或端口（Rust）；
  任一缺失各自报错（`PHP 实例未登记 socket` / `Rust 实例未分配端口` / `Web 服务未安装`）。

### 12.2 扩展多选与二次确认

- **展示**：`extensions_for(version, base_image)` 只返回可用项；不可用项不渲染（不灰显、不占位）。
  三版本 × 两基座的可显示项数量见下文实测；50 项全集与 `since/core_until/pecl_only_since` 矩阵不变。
- **多选安装/卸载**：抽屉扩展页签是一张表单，`name=ext` 多值 + `op=install|uninstall`；
  新建弹窗同样多选，随环境创建任务一次提交。
- **二次确认**：前端拦截提交，列出本次扩展名、是否重启容器、是否触发编译；`extension_is_heavy`
  的重项标注「预计 2–5 分钟（需编译）」。确认后才 POST。
- **后端形态**：`POST /sites/env/{id}/extensions-batch`，字段 `op` + 多个 `ext`；服务端一次
  `apply_extension_selection` 计算目标集合，复用 `extensions_task` 串行执行，**日志合流、只重启一次容器**；
  任务记录 `ext_keys: Vec<String>`，轮询 JSON 同步为 `ext_keys`。
- **PECL**：按名字安装同样走二次确认；批量与单扩展都使用同一任务执行器。

### 12.3 mlocati 安装器本地化

- **资产**：`deploy/vendor/install-php-extensions`，**官方 2.13.2 release 完整文件**（非仓库
  master 副本；`IPE_VERSION=2.13.2`，不会进入 `sleep 10` 分支）。来源 URL、
  SHA-256 `27fa8d80bd4e22feb9ddcadff1ca40c8f3e8ea942eb2dd8f446ed1afd2f2f0ab` 记录在
  `runtime_catalog` 常量中。`include_str!` 编进二进制，Docker 形态也能落盘。
- **落盘**：安装 PHP 环境或执行扩展任务前写入 `<web_root>/skiff/tools/install-php-extensions`（0755）；
  PHP 容器整目录挂载 `web_root`，容器内固定路径 `/www/skiff/tools/install-php-extensions`。
- **脚本行为**：优先 `cp` 本地副本并校验 `IPE_VERSION` 与常量一致，打印版本与 sha256；
  本地缺失/版本不符时打印明确错误并回退固定版本 URL；执行安装器时显式 `CI=true`。
- **回退链**：主路径（安装器）**执行失败**也会进入手写依赖表，输出「主路径失败，已回退」；
  两者都失败时保留两段输出，退出码取回退路径（编译阶段的具体失败码）。
- **批量**：安装一次 `docker exec` 把多个包名交给安装器；回退按 union 运行库 + union 编译依赖
  → 逐项编译（分段标记）→ 末尾一次清理。卸载把多个 ini 删除合并为一次 `docker exec`。
- **其它性能项**：PECL 编译设置 `MAKE="make -j$(nproc)"`；拉镜像前先 `inspect`，已存在且与
  登记 digest 一致时跳过 `ImagePull`；安装时若有扩展，跳过第一次 socket 收紧，重启后统一
  `chmod`。
- **打包/安装**：`pack-release.sh` 强制要求并复制 `vendor/install-php-extensions`；`install.sh`
  复制到 `/www/skiff/tools/`；`release.yml` 校验包内存在、可执行且含 `IPE_VERSION`；`checksums.txt`
  自动覆盖。`xtask verify-pkg` 只校验插件 `.wbpkg` 结构与摘要，不涉及发布包文件清单，无需改动。

### 12.4 验收矩阵（本轮）

| 用例 | 口径 |
| --- | --- |
| Web 行独立 | 列表首行固定 `web`；安装/删除不影响任一 PHP/Node/Go/Rust 行状态 |
| 基础环境移除 | 页面无「基础环境」文案与 `runtime-base-install`；无 `/sites/env/install` 路由 |
| 扩展不可用项 | PHP 8.5 抽屉不出现 `imap`；无「当前不可用」灰显文案 |
| 多选 + 确认 | 勾选多项 → 确认弹窗列出名称/重启/编译与重项耗时 → 单个 `extensions-batch` 请求 |
| 批量后端 | 一个任务串行执行，`RuntimeJob.ext_keys` 为多值，日志合流，只重启一次 |
| 安装器版本 | 随包脚本 `IPE_VERSION=2.13.2`（非 master），sha256 与常量一致 |
| 安装器回退 | 主路径执行失败输出「主路径失败，已回退」并进入 legacy；两者失败时两段日志保留 |
| 发布包 | `vendor/install-php-extensions` 存在、可执行、含 `IPE_VERSION`，并被 checksums 覆盖 |

## 13. PHP 配置文件直接编辑（2026-11-08）

### 13.1 文件面

| 文件 | 宿主路径 | 容器路径 | 来源/权威 |
| --- | --- | --- | --- |
| 主配置 | `<runtime>/php/php.ini` | `/usr/local/etc/php/php.ini` | 宿主新建（默认仅注释），用户直接编辑 |
| FPM 池 | `<runtime>/www.conf` | `/usr/local/etc/php-fpm.d/www.conf` | 「PHP 参数」表单渲染；表单字段是权威 |
| 扩展 ini | `<runtime>/php/conf.d/*.ini` | `/usr/local/etc/php/conf.d-skiff/*.ini` | 宿主托管，PHP 在镜像 `conf.d` 之后扫描 |

- 镜像/扩展安装器写入容器内 `conf.d` 的 ini 不在宿主侧，**不列进编辑列表**；需要覆盖时在
  `php/conf.d/` 新建同名语义的 ini，加载顺序保证它在镜像 ini 之后。
- 所有保存经 `AdminFs`（`skiff-fs`）原子写并写审计，Web/业务层禁止 `std::fs::write`。
- 页面「PHP 参数」写的是 FPM 池的 `php_admin_value`，优先级高于 `php.ini` 同名项；
  保存参数表单会重写 `www.conf`，直接编辑的自定义行会丢失。`php.ini` 里的同名项仅对其他
  SAPI/未覆盖指令生效。

### 13.2 保存流程

1. 写候选文件到 `<runtime>/php/*.candidate`（同在 `/www` 内，容器可见）；
2. 容器内校验：`www.conf` 用临时主配置 `include` 候选后执行 `php-fpm -t -y`；`php.ini` /
   扩展 ini 执行 `php -c <候选> -r 'echo "skiff-ok";'`。**只拦语法错误与启动失败**
   （退出码非 0；非池配置还要求输出 `skiff-ok`），`PHP Warning` 不阻止保存，但原文收进界面提示；
3. 校验通过才原子写正式文件，然后重启对应 PHP 容器；
4. 重启后复核容器仍为 `running`（Docker restart 返回成功不代表 FPM 没被坏配置带崩）；
   重启报错或容器未能保持运行即视为失败：回滚原内容（原文件不存在则移入回收站）、
   再次尝试重启，并返回回滚原因。
5. 存量实例：容器创建于本功能之前（宿主缺 `<runtime>/php/.config-mounted` 标记）时，
   页面提示「编辑不生效」并给出直达重建入口；重建只重装容器与挂载，不动站点目录与源码。
   旧 Rust 站点的 `index.php` 不自动删除。

### 13.3 生效值

- `?ini=1`（「刷新生效值」按钮）才执行容器内 `php -i`，页面渲染路径不执行容器命令；
- 并排展示 `php.ini` 文件值、`php -i` 本地值、来源判断（`php.ini` / FPM 池覆盖 / 其他 conf.d）；
- 读取失败在面板内回显，不写入 URL。

### 13.4 验收

| 用例 | 口径 |
| --- | --- |
| 非 PHP 分流 | Rust/Node/Go 详情抽屉无「扩展」页签与 PHP 配置/文件区块；直达 URL 同样不出现 |
| Rust 站点默认文件 | 新建后站点 `index/` 只有 `Cargo.toml` + `src/main.rs`，无 `index.php`；静态站为 `index.html`，反代无文件 |
| 校验失败 | 写入非法 ini → 返回 400 类错误并提示原因，磁盘文件与 FPM 状态不变 |
| Warning 不阻断 | 写入只触发 `PHP Warning` 的配置 → 保存成功，界面提示条回显 warning 原文 |
| 存量实例重建 | 缺 `.config-mounted` 的容器 → 配置页签出现「编辑不生效」提示与直达重建入口 |
| 重启失败回滚 | 模拟重启失败 → 文件恢复原内容并再次重载，页面提示已回滚 |
| 生效值来源 | `php.ini` 设 `memory_limit=256M` 而表单设 512M → 生效值 512M、来源标为 FPM 池覆盖 |
