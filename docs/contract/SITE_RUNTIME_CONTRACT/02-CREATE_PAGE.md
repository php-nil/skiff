# 建站流程差异与运行环境管理页（§3–5）

> 上级索引：[SITE_RUNTIME_CONTRACT.md](../SITE_RUNTIME_CONTRACT.md) ｜ 拆自 `docs/contract/SITE_RUNTIME_CONTRACT.md` 原 §3–5（2026-11-16 整理，正文未改）。

---

## 3. 建站流程差异

三步向导固定为：**类型与运行时 → 站点信息 → 确认**。类型选择是第一步，不允许先填域名再猜类型。

| 类型 | 第一步 | 第二步字段 | 创建副作用 | 就绪条件 |
| --- | --- | --- | --- | --- |
| 运行环境 · PHP | 选“运行环境” → 选 PHP → 选 PHP 环境 | 主域名、别名、站点目录 | 生成 vhost（fastcgi_pass 指向 `socket_path`）、`index/log/ssl/conf` | PHP 环境 running + socket 可连接 |
| 运行环境 · Node.js | 选“运行环境” → 选 Node.js → 选 Node 环境 | 主域名、别名、站点目录 | 生成反代 vhost（`proxy_pass http://<container_ip>:<环境端口>`）、`index/log/ssl/conf`；站点目录不写代码，环境已有应用则直接可用 | Node 环境 running + 容器地址一致 + HTTP 健康检查通过 |
| 运行环境 · Rust | 选“运行环境” → 选 Rust → 选 Rust 环境 | 主域名、别名、站点目录 | 生成反代 vhost（`proxy_pass http://<container_ip>:<环境端口>`）、`index/log/ssl/conf`；代码固定在环境 `app/` | 环境 running + 容器地址可达 |
| 运行环境 · Go | **未实现，已从 `RUNTIME_KINDS` 与新建弹窗摘除**，向导不出现该类型 | — | — | — |
| 静态网站 | 选“静态网站”，不选运行时 | 主域名、别名、站点目录 | 生成静态 vhost、`index/log/ssl/conf` | 无运行时依赖；目录可创建 |
| 反向代理 | 选“反向代理”，不选运行时 | 主域名、别名、上游 URL、WebSocket 开关 | 生成 proxy vhost、`log/ssl/conf`（不建 `index/`） | 上游 URL 合法；不要求上游当下可达，但保存时给出探测提示 |

### 3.1 向导校验

| 字段 | 规则 | 错误文案（示例） |
| --- | --- | --- |
| 主域名 | 小写字母/数字/中划线/点；1–253 字符；不得以 `-`/`.` 开头结尾；唯一（含别名占用） | `域名格式不正确` / `example.com 已被其他站点使用` |
| 别名 | 逗号/空格分隔；每个域名单独校验；不得与任何主域名/别名重复 | `别名 api.example.com 已是其他站点的域名` |
| 站点目录 | 默认 `/www/sites/<主域名>`；规范化后必须是 `/www/sites` 的一级子目录，拒绝 `..`、更深层、越界 | `站点目录必须是 /www/sites/<域名>` |
| PHP 运行时 | 必须 running；socket 必须可连接 | `PHP 运行环境未就绪，请先启动或重建` |
| Node.js 运行时 | 必须 running；端口已分配且 HTTP 健康检查通过 | `Node.js 运行环境未就绪，请先启动或重建` |
| Go 运行时 | 未实现；提交 `kind=go` 直接拒绝 | `未知运行环境类型` |
| 启动命令 | 属于运行环境配置（非建站表单）；非空、按 argv 切分、拒绝 shell 元字符与宿主凭据 | `启动命令不能为空` / `启动命令包含不支持的字符` |
| 端口 | 属于运行环境（`9200–9299`）；建站表单不填端口，由环境固定 | — |
| 上游 URL | 仅 `http://` / `https://`；无 userinfo；host 必填；端口 1–65535 | `上游地址必须以 http:// 或 https:// 开头` |
| WebSocket | 布尔；开启后 vhost 增加 `Upgrade` / `Connection` 头 | — |

### 3.2 交互口径（与预览对应）

- 类型卡显示“是否需要运行环境”和“创建后产出什么”，选中后立即显示对应的绑定提示：
  - PHP：`共享 PHP-FPM 池与 socket，一个环境可承载多个站点；修改环境配置会影响全部绑定站点。`
  - Node.js：`单进程常驻 + 容器内端口 + 域名反代；启动命令、环境变量与代码目录都属于环境，站点只是域名入口。`
  - 静态：`不绑定运行环境，只创建文档根与 vhost。`
  - 反代：`不创建文档根，只生成转发配置；WebSocket 可选。`
- 运行环境卡显示：版本、状态、占用站点数、socket 或容器内端口范围、默认标记。未运行环境不可选。
- 下一步按钮在前置条件不满足时禁用，并给出**原因**，不是点击后才报错。
- 确认页必须复述：类型、绑定环境、目录、端口/上游、WebSocket、HTTPS 是否后续配置。

---

## 4. 运行环境管理页

入口：`网站 → 运行环境`，稳定键 `core/sites/env`。它是业务视角，不是 Docker 容器列表的翻版。

### 4.1 列表

| 列 | 内容 |
| --- | --- |
| 类型 | `PHP`、`Node.js`、`Rust`（Go 未实现，不出现在列表与向导） |
| 实例名 | 环境名；默认版本带「默认」标记 |
| 版本 | 镜像/上游版本号：容器可读时显示**真实版本**（PHP 取 `PHP_VERSION`、Node.js 取 `NODE_VERSION`，含补丁号如 `8.5.13` / `24.4.0`，副行为「目标 + 镜像」）；Rust 无稳定变量，回退目标版本并标「版本未读取」；未安装显示目标 +「未安装」。可选版本见 4.5 |
| 状态 | 运行中 / 已停止 / 异常 / 安装中（排队、拉镜像、创建容器、启动、失败）+ 进度 |
| 占用站点 | 绑定站点数 |
| 接入方式 | 只显示短标识：PHP `sock`、Node.js / Rust `:容器端口`；完整容器地址（`<ip>:<port>`，含是否对外发布）放 `title` 悬浮提示与详情页，不再单列长路径 |
| 操作 | 只保留一个「详情」入口；整行或按钮都跳详情页 `/{p}/sites/runtimes/<id>`，不再并列「更多」 |

- 表格按内容自适应列宽并铺满容器，不固定撑宽某一列；接入方式列宽度只够放 `sock` / `:8080`（完整容器地址在悬浮提示里）。
- 空态：没有任何真实实例时显示「还没有运行环境」+「新建运行环境」主操作；迁移种入的 PHP 模板（8.5/8.4/8.3）只在新建弹窗里出现，不在列表占位。
- 顶部固定边界提示：`本页管理语言运行环境与绑定关系；Web 服务在「Web 服务」页单独管理，原始容器操作请到 Docker → 容器。`
- 主操作统一放右上角 `page-head .actions`（「新建运行环境」），不再下沉到列表头部。

### 4.2 PHP 环境详情

子视图：`基础信息 / 配置 / 扩展 / 站点 / 管理 / 日志`，地址为
`/{p}/sites/runtimes/<id>/{,config,extensions,sites,manage,logs}`（壳页横向菜单，不在菜单里新增入口）。

| 区块 | 内容 | P1 |
| --- | --- | --- |
| 基础信息 | 版本、镜像与 digest、socket 路径/权限、默认版本、启动时间、最近健康检查、绑定站点数 | 可读 |
| 扩展开关 | 基线全集按分类展示（数据库 / 图片 / 加密 / 缓存 / 其他），按 `(version, base_image)` 过滤；支持搜索、PECL 按名字安装 | P1 可增删：安装阶段装入，安装后在详情页「扩展」子视图开关并重建/重载（见 §11.4） |
| 安装任务 | 状态（排队/拉镜像/创建容器/启动/失败）、进度百分比、实时日志、失败原因与「重试安装」 | P1 异步执行，列表与详情页轮询刷新 |
| ini | `memory_limit`、`upload_max_filesize`、`post_max_size`、`max_execution_time` | P1 可改，保存后平滑 reload |
| FPM 进程池 | `pm`（dynamic/static）、`max_children`、`start_servers`、`min_spare_servers`、`max_spare_servers`、`max_requests` | P1 可改 |
| socket | 路径（只读）、权限 `0660`、属组 `65534`；不允许世界可写 | 只读 |
| 关联站点 | 域名、类型、状态、容器端口/socket；点击进站点详情 | 可读 |
| 日志 | `fpm-error.log`、运行环境操作日志、重建日志 | 可读、可筛选 |

- 配置保存流程：校验数值范围 → 写 `conf/` 临时文件 → `nginx -t` 与 FPM 配置检查 → 平滑 reload；
  任一步失败恢复原文件并提示失败行；**保存 ≠ 生效**必须显示“已保存，尚未生效/已生效”的状态。
- 重建流程：拉取固定 digest 镜像 → 生成配置 → 重建容器 → socket 就绪探测 → 成功后才把
  `desired_state` 置 running；失败保留旧容器与旧配置，明确报错。
- 停止 PHP 环境：所有绑定 PHP 站点显示“环境停止”，vhost 保留但请求返回 502/503 维护响应；
  恢复后自动回到运行中，不逐个改站点状态。

### 4.3 Node.js 环境详情（2026-11-09 已接入）

- **运行模型**：一个环境 = 一个常驻 Node 进程 = 一个容器内固定端口 = 一个固定代码目录
  `/www/runtime/node/<name>/app`；不走 FastCGI，不共享 socket，**不随站点绑定切换目录**。
  容器接入专用 bridge 并持有静态地址，应用监听 `0.0.0.0`；宿主不发布端口。
  多个站点绑同一环境时，只是多个域名反代到同一容器地址（环境详情与站点详情都提示影响面）。
- 基础信息：版本、镜像与 digest、容器地址、工作目录、对外发布状态、关联站点数、按需“检测应用健康”。
- 专属配置（`基础信息 / 专属配置 / 日志 / 关联站点` 页内标签）：
  - 启动命令（默认 `npm start`，按空白切分为 argv，拒绝 `;`、`|`、`$`、反引号等 shell 元字符）；
  - 健康检查路径（默认 `/`，必须 `/` 开头）；
  - `NODE_ENV`（`production` / `development` / `test`）；
  - 额外环境变量（每行 `KEY=VALUE`，大写键名；`PORT` / `NODE_ENV` / `HOST` 由内核注入，不可覆盖）；
  - 端口（可改，必须在 `9200–9299` 且未被其它环境占用；有站点绑定时先确认影响面）、
    工作目录（固定只读，不存在“跟随站点”的切换）；
  - 对外发布（**默认关闭**）：填写宿主端口（≥1024）后重建容器，宿主端口转发到容器内端口；
    留空即取消。发布与站点绑定无关，站点始终走容器地址。
- **共享保护**：有站点绑定时，停止/重启/重建/改端口都先列出受影响域名并要求二次确认；
  删除环境直接拒绝，必须先改绑或删除站点。「克隆环境」复制配置与代码目录（跳过
  `node_modules` / `target`）、自动分配新端口，用于并跑第二个应用。
- **保存流程**：校验 → 写 `config_json` → 若配置有变化，触发一次重建任务（命令与环境变量在容器
  `command`/`Env` 里，只有重建才生效）。保存成功但重建未启动时必须如实提示，不假装已生效。
- 安装/启动/重建流程：写 `entry.sh` 与兜底 `app/` → 拉取固定 digest 镜像 → 确保专用网络与静态地址 →
  创建/启动容器 → 轮询「容器状态 + 地址一致 + HTTP 健康检查」（最多 180 秒）→ 成功后才写
  `last_status = running`；失败保留 `log/server.log` 尾部作为原因，不无限重试。
- 日志：`log/server.log`（应用 stdout/stderr）、运行环境操作日志与重建日志；不另建每站点 `app.log`。
- 依赖安装：入口脚本在 `node_modules/` 不存在时执行 `npm ci`（有 lockfile）或
  `npm install --omit=dev`；失败原因直接进 `server.log`。

### 4.4 启停 / 重建 / 删除规则

| 动作 | 规则 |
| --- | --- |
| 启动 | 检查镜像 digest 与端口/socket 可用（PHP socket 可连接；进程型环境容器地址已登记） |
| 停止 | 运行环境 `desired_state = stopped`；绑定站点有效状态变为“环境停止”，不删站点 |
| 重建 | 同一 `id`、同一版本；生成新容器后原子切换；失败回滚旧容器/旧配置 |
| 删除 | **存在绑定站点时禁止**；列表给出域名与“先改绑或删除站点”的动作；删除默认版本前先切换默认 |
| 改版本 | 运行环境不可原地改版本；新建同 kind 新版本，站点逐个改绑（P2 提供批量改绑） |
| 改绑站点 | 目标环境必须同 kind 且 running；PHP 站点换环境后重新生成 vhost 的 socket；Node.js / Rust 换环境后重建 vhost 的容器地址并按新环境重启进程 |

### 4.5 新建运行环境与异步安装

- **新建弹窗**：类型下拉（PHP / Node.js / Rust）→ 版本 → 实例名（默认跟随类型与版本，
  如 `php-8.5` / `node-24`，可改；同 kind 内唯一，目标目录已存在直接拒绝）。
  - 三类都有**版本下拉**，字段名按类型生成 `version_<kind>`（`version_php` / `version_node` /
    `version_rust`），显示条件 `visibleOn: "${kind === '<kind>'}"`；选项来自镜像服务的清单
    （PHP 如 `8.6.0 / 8.5.13 / 8.5.12 …`，Node.js 如 `26.13.1 / 24.22.3 …`），按版本降序、
    **首个未 EOL 的标「最新」**，预选顺序为「清单 `default`（且未 EOL）> 首个未 EOL」。
  - 某类型清单取不到时**退回目录大版本**（`26 / 24 / 22 / 20`、`1.100 / 1.99 …`）并在提示里写明来源，
    下拉不会空。
  - 「自定义版本」输入框（`version_custom`）对所有类型可见，填了优先于下拉——清单外版本、
    镜像服务不可达时的唯一出路。
  - 提交值取 `version_custom > version_<kind>`，为空时后端用该类型目录首项。
  - 校验：**大版本必须在 `RUNTIME_KINDS` 目录内**，补丁号按点号边界归入大版本——
    PHP `8.5.13` → `8.5`、Node.js `24.22.3` → `24`、Rust `1.99.1` → `1.99` 都通过；
    `9.0.1`（大版本不在目录）、`8.5.13-beta`（非纯数字）、`8.5.13.1`（超过三段）、
    `8.55.0`（不是 `8.5` 的补丁）拒绝。Go **未实现**，已从目录与弹窗摘除。
- **版本清单来源**：目录只维护「支持哪些大版本」，具体版本号由镜像服务提供，避免发版后
  面板里的可选版本永远慢一拍。
  - **路径固定、不可配**：`<镜像基址>/channels/<channel>/runtime-index/<kind>.json`，
    三个类型各一份（`php.json` / `node.json` / `rust.json`）。基址取
    `[catalog].mirrors` **首项**（默认 `http://10.66.52.7:8080`，一期只走 HTTP），渠道取
    `[catalog].channel`（默认 `stable`）；`mirrors = []` 表示不联网，只用面板内置副本/目录大版本。
    取数与 `[catalog].enabled` **无关**：商店目录默认关闭也照取清单（清单不签名，不走签名链）。
    清单落在渠道目录下而不是根目录，是因为服务端产物里 `channels/<name>/` 是唯一的可变层
    （[SKIFF_SERVER/01-DESIGN.md §2.2](../../plugin/SKIFF_SERVER/01-DESIGN.md)）。
  - **格式 `skiff.server.runtime-index/v1`**（schema 随产物发在
    `deploy/skiff-server/schema/runtime-index.schema.json`）：
    `{"schema","kind","channel","generated_at","versions":[{"version","eol?","default?"}]}`。
    由 `skiff-server build` 按 `kind` 分组、从该渠道 `runtimes/` 条目生成（版本数值降序、
    跳过 `yanked`）；某类型没有条目就不产出该文件，面板取到 404 回退。
    条目由 `skiff-server runtimes` 从上游刷新，**每个发布分支默认保留最近三个版本**
    （PHP `8.5` → `8.5.13 / 8.5.12 / 8.5.11`；Node.js major 跨 minor 往前数；
    `--per-branch 1` 可只留最新），老站点升级时才有降级余量。
    直连不到 `docker.io` 的机器用 `--proxy <url>` 出网（该命令**不读** `HTTPS_PROXY`，
    代理清单见 `docs/test/ITERATION/02-ISSUES_ENV.md`）。
    旧形态（裸数组、`{"versions":[…]}`、`{"tags":[…]}`、`{"items":[…]}`、
    `{"results":[{"name":…}]}`）继续兼容，只认 1–3 段纯数字版本（`24` 是合法的 Node LTS 版本）。
  - **内置副本只对 PHP 存在**：`crates/app/assets/runtime-index/php.json` 是服务端产物的
    **快照**（发版时用产物覆盖，两边同一份字节）。Node.js / Rust 不给内置副本——它们的可选
    版本是 LTS/stable 大版本，目录即权威；且内置副本的 `generated_at` 一旦比产物新，回滚
    保护会把服务端清单**永久**判成「更旧」。
  - **防回滚**：清单的 `generated_at` 早于内置副本即拒绝采用并提示原因——镜像同步卡住时
    「刷新一次反而少几版」比不刷新更坏（只对有内置副本的类型生效）。
  - **回填顺序（硬要求）**：先发布服务端数据、再回填面板内置副本，即内置副本的
    `generated_at` 必须 **≤** 镜像服务的清单。顺序反了远端会被永久判成「更旧」而拒用，
    页面上只看到「已忽略」，功能等于失效。日常刷新直接用 `skiff-server runtimes`
    （写条目 → 生成产物 → 回填内置副本一次完成，见 `SKIFF_SERVER/06-RELEASE_MIRROR.md` §14.4），
    收尾再跑
    `skiff-server check --dist <产物> --builtin-index crates/app/assets/runtime-index`
    校验字节一致（产物有多个渠道时加 `--channel`），不一致即失败——手写这份文件迟早写错。
  - **信任边界**：该文件不签名、内网只走 HTTP。能挡陈旧镜像回滚，挡不住伪造；要防伪造得把
    它的 sha256 写进已签名的 `index.json`（二期，未做）。
  - 缓存：成功 10 分钟、失败 60 秒负缓存（避免每次开弹窗都等一次超时）；「服务端未发布」
    （HTTP 404/410）按成功缓存，不是故障。每大版本最多列 4 个版本；`eol` 版本标「已 EOL」
    并排到最后，`default` 决定预选项。
  - 接口：`GET /ui/v1/runtimes/versions?kind=php[&refresh=true]`（`kind` 另支持 `node` / `rust`；Admin，未知类型 400）。
    页面右上角「刷新版本清单」依次刷三类，成功后整页重载让弹窗拿到新选项。
- **软件源（apk，仅 PHP 生效）**：弹窗提供源下拉（默认「自动」= 容器内实测后择快；其余为
  `runtime_catalog::APK_MIRRORS` 候选）。目录接口
  `GET /ui/v1/runtimes/catalog` 同时返回候选与**当前生效值**（`apk_mirror_current.fixed/base`），
  弹窗据此预选已固定的源，避免用户只是新建环境却把源改回自动。提交字段 `apk_mirror`：
  缺省/`null` = 不改动；`""`/`auto` = 恢复自动探测；候选 key 或源根地址 = 固定为该源。
  命令层在安装作业入队**之前**落盘，容器内 prelude 立即读到；非 PHP 类型不应携带该字段。
- **版本目录单一来源**：`crates/app/src/runtime_catalog.rs` 的 `RUNTIME_KINDS` 维护**大版本**
  （2026-11 对齐上游：PHP `8.6 / 8.5 / 8.4 / 8.3 / 8.2`、Node.js `26 / 24 / 22 / 20`、
  Rust `1.100 / 1.99 / 1.98 / 1.97 / 1.83`），
  同时驱动校验、默认实例名与镜像名推导；补丁号不写入目录，走上面的清单。镜像名
  `image_for(kind, version, php_image, image_prefix)`：默认版本用设置里的完整镜像名（已含前缀），
  其余版本按 `php:<version>-fpm-alpine` 生成并**统一拼镜像前缀**。
- **扩展清单单一来源**：同一模块的 `PHP_EXTENSIONS` 同时驱动安装弹窗勾选、详情页扩展开关、
  `extensions` 字段校验与容器内执行脚本；默认集合为 `default_on` 项（pdo_mysql / mbstring 等）。
- **异步安装**：点击「创建并后台安装」立即返回（落库 + 入队），后台任务按
  `排队 → 拉镜像 → 创建容器 → 启动 → 完成/失败` 推进；**状态与进度跟随列表接口每行的 `job` 字段**
  下发（`RuntimeItem.job`：状态 / 进度 / 起止时间），没有单独的安装进度端点。
- **安装日志**：任务持有最近 300 行日志；详情页「日志」子视图按需取尾部
  （`GET /ui/v1/runtimes/{id}/logs?tail=`）+「刷新」，不建页内实时流。失败保留原因；
  重试走任务中心（`POST /ui/v1/jobs/{id}/retry`），运行环境页不另设重试按钮。
- **扩展变更**：详情页提交 `/runtimes/{id}/extensions/{key}/install|uninstall` 或 `/pecl`，异步执行
  mlocati `install-php-extensions`（下载失败回退手写依赖 + `docker-php-ext-install` / `pecl install`），
  完成后重启容器、收紧 socket 权限并落库；容器不存在时提示先安装/启动。
- **两家族可用性（Debian / Alpine）**：扩展安装脚本在**容器内**用 `command -v apk` / `apt-get`
  判定包管理器后分支，不在宿主侧按镜像标签猜（自建镜像、换基础镜像同样成立）。
  Alpine 保持「运行库显式安装 + `.skiff-build` 虚拟构建包 + 编译后清理」；
  Debian 走 `apt-get install --no-install-recommends` 装映射后的 `-dev` 包（运行库随依赖保留，
  不做 `apk del` 式清理）。`/etc/apk/config`、`apk add/del` 等 Alpine 专属路径只出现在
  `apk` 分支内；两边都没有包管理器时脚本明确报错退出，不盲写 Alpine 路径。
  官方 `php:*-fpm`（Debian）与 `php:*-fpm-alpine` 都必须能装扩展并成功重启 FPM。
- **持久化**：`site_runtimes.extensions`（schema v9）保存逗号分隔的启用集合，安装/扩展任务成功后回写。
- **PHP 池日志口径**：FPM 池显式 `access.log = /dev/null`，站点访问日志只由 Nginx 写；`error_log` 仍指 fd 2，由容器日志承接，不在站点目录重复落盘。

---

## 5. 与 Docker 容器页的边界

### 5.1 归属

| 维度 | 运行环境页 | Docker 页 |
| --- | --- | --- |
| 视角 | 业务：版本、绑定站点、专属配置、影响面 | 基础设施：原始容器、镜像、网络、卷 |
| 生命周期 | 期望状态 + 重建/删除业务动作 | 原始 start/stop/restart/remove/exec |
| 配置 | PHP ini / FPM / Node 启动命令与环境变量 | 容器参数、挂载、环境变量 |
| 站点关系 | 展示“影响 N 个站点” | 展示“受管运行环境”徽章与返回链接 |

- 运行环境容器命名确定性：`skiff-<kind>-<name>`（如 `skiff-node-web`、`skiff-php-8.5`）；
  Node.js / Rust 的代码固定在环境 `app/`，
  站点只做域名反代，不额外创建每站点容器。
- Docker 页**不隐藏**这些容器：显示徽章“运行环境/站点”，操作前提示影响面；删除/停止等破坏性动作
  必须二次确认并说明“将影响 N 个站点，业务页可能出现状态漂移”。
- 运行环境页提供“在 Docker 中查看”深链：`/{p}/docker/containers?filter=<container>`；
  Docker 行提供“返回运行环境”深链：`/{p}/sites/runtimes/<id>`。
- 两页共享同一套实际状态读取；Docker 页不写 `site_runtimes`，运行环境页不直接编辑原始容器参数。

### 5.2 禁止事项

- 不在运行环境页提供容器 exec、任意挂载、任意网络、Compose 编辑；这些只属于 Docker 页。
- 不在 Docker 页提供“给站点换运行环境”“改 PHP ini”等业务操作；避免两套配置互相覆盖。
- 不在两处各自维护“运行中/已停止”副本；以 Docker 实际状态 + DB 期望状态合成展示。

---
