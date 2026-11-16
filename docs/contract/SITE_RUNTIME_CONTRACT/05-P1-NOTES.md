# P1–P3 实现补充（§11–13）

> 上级索引：[SITE_RUNTIME_CONTRACT.md](../SITE_RUNTIME_CONTRACT.md) ｜ 拆自 `docs/contract/SITE_RUNTIME_CONTRACT.md` 原 §11–13（2026-11-16 整理，正文未改）。

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
  `site_runtimes.image_digest`；该值参与「本地已有→跳过拉取 / 与登记值不一致→按 digest 重拉 /
  新环境回填」判定（`ensure_runtime_image`），不只是展示。
  **实现口径**：`ImageInfo::primary_digest_for(image)` 按镜像引用的**仓库名**匹配
  `repo_digests`（归一化 `docker.io` / `index.docker.io` 别名与 Docker Hub `library/` 前缀），
  命中不到才退回首个 `@sha256:` 条目。Docker 的 `RepoDigests` 条目本身不含 tag，因此无法按
  tag 精确匹配；同一本地镜像打多个仓库时按仓库名取对应 digest，避免取错仓库导致误判漂移。
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
  - 端口区间 **9100–9199**（容器内监听），创建时取区间内最小未占用端口并落 `site_runtimes.port`；冲突直接拒绝；
  - 容器 `skiff-rust-<name>`，`network_mode=host`，挂载 `/www`；
  - 容器入口固定为 `sh /www/runtime/rust/<name>/entry.sh`，日志重定向到
    `/www/runtime/rust/<name>/log/server.log`；`entry.sh` 始终在固定目录运行：
    `cargo run --release --manifest-path /www/runtime/rust/<name>/app/Cargo.toml`；
  - 端口同样属于环境（`9100–9199`），站点只反代；改端口与 Node.js 走同一套确认与重建流程；
  - `CARGO_TARGET_DIR=/www/runtime/rust/<name>/target`，构建产物不写站点目录；
  - 安装任务写最小 std HTTP 工程（无第三方依赖），启动后 `wait_tcp_port` 等端口监听，失败回传日志尾部；
  - 目录结构：`/www/runtime/rust/<name>/{entry.sh,app/{Cargo.toml,src/main.rs},log/server.log,target/}`。
    `app/` 是唯一代码目录；站点目录不写脚手架，绑定站点不改变工作目录。
  - 脚手架只在文件缺失时写入，已存在的用户代码在重建/克隆时原样保留。
- 站点绑定：站点类型 `runtime`，vhost 生成 `proxy_pass http://<container_ip>:<port>`（不是 `fastcgi_pass`）；
  解绑入口在抽屉「站点」页签，删除环境仍受「存在绑定站点即拒绝」约束。
- Node.js 已接入，见 §11.6；Go 仍未接入，保持不出现目录项与安装入口。

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
- **ini 扫描目录**：容器 `PHP_INI_SCAN_DIR` 显式为
  `/usr/local/etc/php/conf.d:/usr/local/etc/php/conf.d-skiff`，两个目录各扫一次。
  **不得带前导冒号**——PHP 的前导冒号表示「沿用编译期默认扫描目录」，而官方镜像默认目录就是
  `conf.d`，会重复扫描同一目录、扩展 ini 加载两次（`Module already loaded`），
  `install-php-extensions` 的 `checkModuleWorking` 因此判失败；不依赖「沿用默认」语义。
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
| Node.js | 创建 → 安装异步任务 → HTTP 健康检查 → 站点绑定 `runtime` → vhost 反代返回内容；配置变更触发重建；无 Docker 时验证失败链路与渲染 |

### 11.6 Node.js 运行环境（2026-11-09）

- **目录与镜像**：`RUNTIME_KINDS` 新增 `node`，版本白名单 `24 / 22 / 20`，镜像 `node:<ver>-alpine`；
  `default_name` 为 `node-<ver>`；`NODE_VERSION` 作为镜像真实版本来源。
- **与 PHP 的差异**：不建 FPM 池、不生成 socket、不走 `fastcgi_pass`；一个环境一个常驻进程、
  一个容器内固定端口（`9200–9299`），站点 vhost 用 `proxy_pass http://<container_ip>:<port>`，
  容器在专用 bridge 上持有静态地址，宿主不发布端口。
- **目录**：`/www/runtime/node/<name>/{entry.sh,app/{package.json,server.js},log/server.log}`；
  `app/` 是唯一代码目录，**不随站点绑定切换**，也没有 `site` 标记。
- **专属配置**（`config_json`）：`command`（默认 `npm start`，argv 切分）、`health_path`（默认 `/`）、
  `node_env`、`env`。保存后配置有变化即触发重建；`PORT` / `NODE_ENV` / `HOST` 由内核注入。
- **健康检查**：安装/启动/重建后以 HTTP GET 轮询，最多 180 秒；失败置 `error` 并回传 `server.log` 尾部。
  详情页可「检测应用健康」，把容器 running 与应用就绪分开呈现。
- **站点目录**：Node 站点不写 `package.json` / `server.js`，也不生成 `index.php`；只建
  `index/log/ssl/conf` 与反代 vhost。应用代码放环境 `app/`（已存在则重建不覆盖）。
- **共享与克隆**：多站点共享一个进程/端口，停启重建改端口须确认影响面；删除环境被拒绝；
  克隆复制配置与代码（跳过 `node_modules` / `target`）并分配新端口。
- **未做**：站点级独立进程/容器、站点级日志与最近重启（P2，见 §7）。

## 12. 运行环境拆分、扩展多选与安装器本地化（2026-11-06）

### 12.1 取消「基础环境」

- **模型**：运行环境列表只列真实语言实例（`php` / `node` / `rust`，以及尚未实现的 `go` 不允许登记），
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
  不触碰任何 PHP/Node.js/Rust 容器。运行环境页不再有合成 `web` 行与行内 Web 动作。
- **安装进度（2026-11-14）**：`install`/`rebuild` 提交任务中心 `web.install` 作业（目标
  `web_env/web`，`cancellable=false`），步骤固定为检查环境 → 准备目录/主配置 → 拉镜像 →
  重建 vhost → 创建并启动容器 → 写默认站点；每一步同步写 `jobs.steps_json`（`pending/running/ok/failed`
  + 时间戳 + 错误摘要）。没有历史时进度为 `idle`；失败/中断记录保留到任务中心清理窗口。
  进程重启时 `JobService::new` 把所有遗留 `queued/running` 作业结算为 `failed/interrupted`，
  其中 `running` 步骤改 `failed`，页面不会再永久显示「安装中」。重试通过任务详情或
  「重新安装」创建新作业，不在原作业上续跑。
- **删除的耦合**：`add_runtime` 弹窗内的「安装基础环境（OpenResty + 默认 PHP 8.5）」次要表单、
  `/sites/env/install` 路由、`base_install_href` 上下文、`run_site_env_install` 里的 PHP 池配置 /
  PHP 镜像拉取 / PHP 容器创建与 socket 收紧，全部移除。
- **schema**：`site_runtimes` 无「基础环境」标记；v13 新增 `origin`（`instance` / `seed`），`is_default` 仍是 per-kind 默认实例。
- **vhost**：站点 vhost 生成只依赖 Web 服务存在 + 所选语言实例的 socket（PHP）或端口（Node.js / Rust）；
  任一缺失各自报错（`PHP 实例未登记 socket` / `Node.js / Rust 实例未分配端口` / `Web 服务未安装`）。

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
| Web 行独立 | 列表首行固定 `web`；安装/删除不影响任一 PHP/Node.js/Rust 行状态 |
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
   旧 Node.js / Rust 站点的 `index.php` 不自动删除。

### 13.3 生效值

- `?ini=1`（「刷新生效值」按钮）才执行容器内 `php -i`，页面渲染路径不执行容器命令；
- 并排展示 `php.ini` 文件值、`php -i` 本地值、来源判断（`php.ini` / FPM 池覆盖 / 其他 conf.d）；
- 读取失败在面板内回显，不写入 URL。

### 13.4 验收

| 用例 | 口径 |
| --- | --- |
| 非 PHP 分流 | Node.js / Rust / Go 详情抽屉无「扩展」页签与 PHP 配置/文件区块；直达 URL 同样不出现；Node.js 显示启动命令/健康检查/环境变量表单 |
| Node.js 站点默认文件 | 新建后站点 `index/` 只有 `package.json` + `server.js`，无 `index.php`；vhost 为 `proxy_pass` |
| Rust 站点默认文件 | 新建后站点 `index/` 只有 `Cargo.toml` + `src/main.rs`，无 `index.php`；静态站为 `index.html`，反代无文件 |
| 校验失败 | 写入非法 ini → 返回 400 类错误并提示原因，磁盘文件与 FPM 状态不变 |
| Warning 不阻断 | 写入只触发 `PHP Warning` 的配置 → 保存成功，界面提示条回显 warning 原文 |
| 存量实例重建 | 缺 `.config-mounted` 的容器 → 配置页签出现「编辑不生效」提示与直达重建入口 |
| 重启失败回滚 | 模拟重启失败 → 文件恢复原内容并再次重载，页面提示已回滚 |
| 生效值来源 | `php.ini` 设 `memory_limit=256M` 而表单设 512M → 生效值 512M、来源标为 FPM 池覆盖 |
