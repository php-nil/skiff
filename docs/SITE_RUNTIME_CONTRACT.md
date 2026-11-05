# Skiff 网站运行环境与建站契约（SITE_RUNTIME_CONTRACT）

> 状态：**设计契约 v1（2026-11-05）**。本轮只定模型、信息架构与验收口径，不改 Rust 代码、不改
> `crates/app/src/sites.rs`、不部署测试机。
> 基线：`crates/store/src/db.rs`（schema v8）、`crates/app/src/sites.rs`（OpenResty + 多实例 PHP-FPM
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
5. **P1 不做假实现**：P1 落地静态站、PHP 站、反向代理站与 PHP 运行环境管理；Node.js / Go 只落模型、
   路由占位与“P2 未实现”提示，不显示安装成功、不伪造进程、不伪造端口监听。扩展开关、多 PHP 版本、
   站点级 php.ini 覆盖、Go 构建链路均留 P2。

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
  www.conf       PHP：FPM pool（内核渲染，不手改）
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
  index/   静态/PHP 的文档根；Node/Go 的应用工作目录（入口文件或已编译二进制）
  log/     access.log / error.log；Node/Go 追加 app.log / app.err.log
  ssl/     cert.pem / key.pem（站点 HTTPS）
  conf/    <域名>.conf；Node/Go 追加 app.json（进程/端口/健康检查快照）
```

- **布局不变**：现有“一站一目录”保持不变，`config` 只新增 `app.json` 与日志文件，不迁移旧目录。
- `static`：`index/` 必须存在，根目录是 `<站点目录>/index`。
- `proxy`：不创建 `index/`；站点目录只含 `conf/ log/ ssl/`。目录页对 proxy 显示“反代站点无文档根”。
- `runtime`（PHP）：`index/` 是文档根；PHP-FPM 池与 socket 由环境提供，不由站点目录提供。
- `runtime`（Node/Go）：`index/` 是应用工作目录；`conf/app.json` 记录命令、端口、健康检查，
  由内核生成，站点详情只读展示，不提供任意 Docker 参数编辑。
- 删除站点：删除该站整目录 + 站点记录 + 别名 + 证书；**不删除运行环境**。删除运行环境：被站点引用时
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
| 运行环境 · Node/Go | 选 kind；P1 仅显示“P2 未实现”，不可提交 | （P2）域名、工作目录、启动命令、端口、健康检查路径 | （P2）站点容器 + vhost 反代 + `app.json` | 环境 running + 端口 bind 成功 + 健康检查通过 |
| 静态网站 | 选“静态网站”，不选运行时 | 主域名、别名、站点目录 | 生成静态 vhost、`index/log/ssl/conf` | 无运行时依赖；目录可创建 |
| 反向代理 | 选“反向代理”，不选运行时 | 主域名、别名、上游 URL、WebSocket 开关 | 生成 proxy vhost、`log/ssl/conf`（不建 `index/`） | 上游 URL 合法；不要求上游当下可达，但保存时给出探测提示 |

### 3.1 向导校验

| 字段 | 规则 | 错误文案（示例） |
| --- | --- | --- |
| 主域名 | 小写字母/数字/中划线/点；1–253 字符；不得以 `-`/`.` 开头结尾；唯一（含别名占用） | `域名格式不正确` / `example.com 已被其他站点使用` |
| 别名 | 逗号/空格分隔；每个域名单独校验；不得与任何主域名/别名重复 | `别名 api.example.com 已是其他站点的域名` |
| 站点目录 | 默认 `/www/sites/<主域名>`；规范化后必须是 `/www/sites` 的一级子目录，拒绝 `..`、更深层、越界 | `站点目录必须是 /www/sites/<域名>` |
| PHP 运行时 | 必须 running；socket 必须可连接 | `PHP 运行环境未就绪，请先启动或重建` |
| Node/Go 运行时 | P2；P1 选择后提交按钮禁用并说明 | `Node.js 运行环境将在 P2 接入，当前不会创建站点` |
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
| 类型 | `PHP`、`Node.js`、`Go` |
| 实例名 | 环境名；默认版本带「默认」标记 |
| 版本 | 镜像/上游版本号，来自版本目录（见 4.5） |
| 状态 | 运行中 / 已停止 / 异常 / 安装中（排队、拉镜像、创建容器、启动、失败）+ 进度 |
| 占用站点 | 绑定站点数 |
| 接入方式 | 只显示短标识：PHP `sock`、Node/Go `P2`；完整 socket/端口路径放 `title` 悬浮提示与抽屉里，不再单列长路径 |
| 操作 | 只保留一个「详情」入口；整行或按钮都打开右侧抽屉，不再并列「更多」 |

- 表格按内容自适应列宽并铺满容器，不固定撑宽某一列；接入方式列宽度只够放 `sock` / `:8080`。
- 空态：未安装任何环境时显示「新建运行环境」主操作 + “Node.js / Go 将在 P2 接入”说明，不显示空表。
- 顶部固定边界提示：`本页管理运行环境与绑定关系；原始容器操作请到 Docker → 容器。`
- 标题栏不再直出「安装/重新安装基础环境」按钮：该入口并入「新建运行环境」弹窗的次要表单，
  并标注重建会重装 nginx 容器与默认 PHP 实例、已有站点配置保留但会短暂中断。

### 4.2 PHP 环境详情

标签：`基础信息 / 专属配置 / 日志 / 关联站点`（页内标签，不新增菜单 L3）。

| 区块 | 内容 | P1 |
| --- | --- | --- |
| 基础信息 | 版本、镜像与 digest、socket 路径/权限、默认版本、启动时间、最近健康检查、绑定站点数 | 可读 |
| 扩展开关 | opcache / curl / pdo_mysql / mysqli / gd / zip / bcmath / intl / redis；显示已启用/未启用 | P1 可增删：安装阶段装入，安装后在抽屉里开关并重建/重载 |
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
- **P1 呈现**：列表/详情可见模型与说明，但安装、启动、配置保存、重建按钮均禁用并标注“P2”；不展示伪造的
  运行中状态或端口监听结果。

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

- **新建弹窗**：类型下拉（PHP / Node.js / Go）→ 版本下拉（大版本下的小版本，如 `8.5 / 8.4 / 8.3`）
  → 实例名（默认 `php-8.5` 跟随类型与版本，可改；同 kind 内唯一，目标目录已存在直接拒绝）。
  不允许手填版本；Node / Go 在 P1 只登记不安装，提交后在列表显示「P2 未实现」。
- **版本目录单一来源**：`crates/app/src/runtime_catalog.rs` 的 `RUNTIME_KINDS` 同时驱动弹窗下拉、
  安装镜像选择与后端校验；新增版本只改这一处，模板不再硬编码版本列表。
- **扩展清单单一来源**：同一模块的 `PHP_EXTENSIONS` 同时驱动安装弹窗勾选、抽屉开关、
  `extensions` 字段校验与容器内执行脚本；未勾选时为默认集合（opcache / curl / pdo_mysql / mysqli / zip / bcmath）。
- **异步安装**：点击「创建并后台安装」立即返回（落库 + 入队），后台任务按
  `排队 → 拉镜像 → 创建容器 → 启动 → 完成/失败` 推进；列表与抽屉轮询
  `/sites/env/jobs`、`/sites/env/{id}/job` 获取状态、进度与日志。
- **安装日志**：任务持有最近 300 行日志，抽屉内 `data-job-log` 实时追加；失败保留原因，
  提供「重试安装」入口，不阻塞其它页面操作。
- **扩展变更**：抽屉勾选后提交 `/sites/env/{id}/extensions`，异步执行
  `docker-php-ext-install/enable/disable`（redis 走 pecl），完成后重启容器、收紧 socket 权限并落库；
  容器不存在时改走完整安装并重放扩展集合。
- **持久化**：`site_runtimes.extensions`（schema v9）保存逗号分隔的启用集合，安装/扩展任务成功后回写。

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

L2 保持 `docs/MENU_CONTRACT.md` 的三个内置项：网站列表 / 运行环境 / 设置。运行环境详情是 L2 下的
子页面，不新增 L3（L3 只属于插件）。

| 页面 | 规范 URL | 旧路由别名 |
| --- | --- | --- |
| 网站列表 | `/{p}/m/core/sites/list` | `/{p}/sites` |
| 新建网站 | `/{p}/m/core/sites/new`（可带 `?type=runtime|static|proxy`） | `/{p}/sites/new` |
| 站点详情 | `/{p}/m/core/sites/list/<域名>` | `/{p}/sites/<域名>` |
| 运行环境列表 | `/{p}/m/core/sites/env` | `/{p}/sites/env` |
| 运行环境详情 | `/{p}/m/core/sites/env/<runtime-id>` | `/{p}/sites/env/<runtime-id>` |
| 运行环境标签页 | `/{p}/m/core/sites/env/<runtime-id>?tab=config|logs|sites` | 同上 |
| 网站设置 | `/{p}/m/core/sites/settings` | `/{p}/sites/settings` |

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
3. **三步向导**：类型选择、运行时选择（列出所有已就绪 PHP 实例）、类型相关字段、校验与确认；Node/Go 卡片禁用并说明 P2。
4. **运行环境页**：PHP 列表与右侧抽屉详情、基础信息、ini/FPM 配置、socket 只读、日志、关联站点、启停/重建/删除
   保护；列表与详情不常驻安装进度/镜像源；Node/Go 只展示“P2 未实现”信息卡。
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

- 数据库创建、SSL 自动申请（Let’s Encrypt）、WAF 规则拦截、应用商店、一键源码部署、Compose 编辑。
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

- `core/sites/list`、`core/sites/env`、`core/sites/settings` 三项不变；运行环境详情不新增 L2/L3。
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

- 推荐：**不包含**。P1 只落模型、列表说明与禁用态；P2 再实现安装、进程、端口与健康检查。
- 理由：没有构建/进程守护链路时做“能跑”的 Node/Go 必然假实现；先把 PHP/静态/反代闭环做扎实。

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
| Node/Go P1 | `Node.js / Go 运行环境将在 P2 接入；当前不会创建站点或伪造进程。` |
| proxy 无文档根 | `反向代理站点不创建文档根，只生成转发配置。` |
| 无绑定站点 | `该环境暂无站点。去网站列表新建。` |
