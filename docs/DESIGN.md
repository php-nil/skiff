# 设计方案：Skiff（轻舟）· Rust 极简 Linux 服务器管理面板

> 版本：**v1.0（MVP 实施稿）** ｜ 产品名：**Skiff（轻舟）** ｜ 交付形态：**宿主机 systemd 直装（主）**；1Panel 商店 Docker 包为可选受限形态
> 本方案 = **MVP 方案**，只覆盖三核心：**Web 管理服务 / Docker 管理 / 插件体系**。文件管理由内核持有管理员客户端（「工具 → 文件管理」，IO 全部经 `skiff-fs`）；防火墙、主机监控等其余宿主功能由插件层在宿主能力面（docker/identity/fs/net/ftp）实现（§1.4、§3.3）。
> 状态：**MVP 代码已落地**（M0–M4 主体完成；`landlock` / `cgroups` / `nftables` 与 `net` 收口尚未实现）。结构为 **16 个 crate + `xtask`**（`skiff-app` 承载 `AppState`；能力面实现在 `crates/plugin`，判定规则唯一来源在 `skiff-model`）。架构审查 P0–P2 与第六轮复审结论见 [ARCH_REVIEW.md](ARCH_REVIEW.md)（该轮基线为 14 crate；当前 16）；本地三关命令与 CI 口径见 [CI.md](CI.md)，测试用例与实测数字见 [TEST_MATRIX.md](TEST_MATRIX.md)，测试机部署与验收结论见 [TEST_ENV.md](TEST_ENV.md)。**Linux 专属能力中，setuid 降权 / Unix socket 已在 Linux CI 跑过单测，docker.sock 与 systemd 安装已在测试机验证；landlock / cgroups / nftables 尚未实现。** 目标读者：决策者。
> **文档结构**：本文件是索引与一页结论；正文按章节拆在 `docs/sections/` 下（13 个文件覆盖 §1–§12，§4 分两册）。**文件名前缀 = 章号**，章节号 `§x.y` 全局唯一、拆分不重编号，跨文件引用直接按编号跳转（见下方文档地图）。

---

## 0. 一页结论

| 维度 | 决策 | 理由 |
|---|---|---|
| **MVP 范围** | **三核心：Web 管理服务 + Docker 管理 + 插件体系** | 能登录、能管容器、能装插件且崩不了内核即 MVP；主机能力在插件层（§1.4） |
| 语言/运行时 | Rust 2021 + tokio（**显式** `worker_threads=2`） | 无 GC，单二进制；tokio 默认值是 CPU 数，不配就不是 2 |
| HTTP | `axum` + `tower` | 生态稳、体积可控 |
| 前端 | `askama` SSR + `htmx` + 懒加载 ESM（xterm.js） | 免 Node 构建链，首屏 < 100KB gzip |
| 存储 | `rusqlite`(bundled) + TOML | 单文件、可外部审计 |
| 扩展 | **进程外插件（宿主机原生）+ Unix Socket JSON-RPC**，ed25519 签名、权限逐条确认、UI 沙箱 iframe、降权；landlock/cgroups 属设计目标（未实现） | 崩溃隔离，越权收敛到 Broker；插件体系 = 开放能力面（§4） |
| 部署 | **主形态：systemd 直装宿主机**；可选：1Panel 商店 Docker 包（受限，能力面收缩为 docker/identity） | 插件必须落宿主才撑得起第二核心；容器内 fork 不出宿主进程 |
| 基础镜像 | **alpine:3.23**（仅可选 Docker 形态；3.20 已 EOL，禁用） | 排障（docker exec 进面板）需要 shell；跟随受支持分支（3.23 支持到 2027-11） |
| 资源目标 | 空闲 RSS ≤ 25MB，CPU < 0.3%，二进制 ≤ 14MB | **设计目标，待 M5 实测回填**（口径见 §7） |

**主结论**：MVP = 三核心跑通——**能登录、能管容器、能装插件且崩不了内核**。产品定位 = **极简服务器面板**：内核只做 Web 管理服务 / Docker 管理 / 插件体系，文件管理、防火墙、网站管理等宿主能力以插件形式开放（能力面 docker/identity/fs/net）。插件落宿主机（全家 native）——插件体系因此是真核心而非 docker 附庸；Docker 包降级为可选受限形态（§5.4）。

---

## 文档地图

文件编号 = 章节号（`04-` 前缀为 §4 的两册，按文件名排序即 `04-plugin-package` → `04-plugin-publish`）。

| 章节 | 文件 | 内容要点 |
|---|---|---|
| §1 需求评估 | [sections/01-requirements.md](sections/01-requirements.md) | 显式需求 → 验收项、隐含需求、三处需求冲突、分层边界与明确不做 |
| §2 技术选型 | [sections/02-tech-stack.md](sections/02-tech-stack.md) | 后端依赖表（含 R1 例外标注）、前端、扩展机制三方案对比 |
| §3 总体架构 | [sections/03-architecture.md](sections/03-architecture.md) | 分层图、重启容器请求路径、内核四项原子能力 |
| §4.1–4.3 扩展体系（一） | [sections/04-plugin-package.md](sections/04-plugin-package.md) | `.wbpkg` 包格式与签名编解码、manifest 字段、生命周期与资源兜底、信任模型 |
| §4.4–4.5 扩展体系（二） | [sections/04-plugin-publish.md](sections/04-plugin-publish.md) | 第三方发布交付物与 CLI、用户侧安装 / 升级 / 回滚 / 卸载 |
| §5 部署 | [sections/05-deploy.md](sections/05-deploy.md) | systemd 直装、数据与升级、与 1Panel/宝塔共存、1Panel Docker 包、路径与卷校验 |
| §6 安全 | [sections/06-security.md](sections/06-security.md) | 认证、传输与反代、暴露面与 RBAC、Docker Socket 三层控制、Compose 子集、插件安全 |
| §7 资源预算 | [sections/07-budget.md](sections/07-budget.md) | 资源目标表、手法、登录峰值口径（与空闲 RSS 目标的关系） |
| §8 工程结构 | [sections/08-engineering.md](sections/08-engineering.md) | crates 布局、核心 ↔ 代码对照、`xtask` 两类固化校验（另有独立 `.wbpkg` 校验子命令） |
| §9 里程碑 | [sections/09-milestones.md](sections/09-milestones.md) | M0–M5 交付物与通过标准、MVP 边界、9.1 架构核实门（原型挂里程碑） |
| §10 风险 | [sections/10-risks.md](sections/10-risks.md) | 21 条风险（带 架构/契约/实现/运营 层级）、10.1 架构支柱级 6 条原型、10.2 契约级 5 条 |
| §11 已拍板决策 | [sections/11-decisions.md](sections/11-decisions.md) | 10 条默认值（可推翻），不留阻塞实施的开放问题 |
| §12 待实测清单 | [sections/12-pending-tests.md](sections/12-pending-tests.md) | 17 条实测项 + 12.1 架构层缺口 5 条，逐项挂里程碑 |
| 部署与测试（非章节） | [DEPLOY_TEST.md](DEPLOY_TEST.md) | 两种部署形态的装机步骤、配置矩阵、7 组验收用例（A–G）、未实现清单、实测记录与排查表 |
| 持续集成（非章节） | [CI.md](CI.md) | 三个工作流的门槛口径、工具链锁定与 MSRV 依据、发布产物流向、实测结果与已知风险 |
| 架构审查（非章节） | [ARCH_REVIEW.md](ARCH_REVIEW.md) | 模块划分 / 职责边界 / 依赖关系 / 复用性四维审查；重复实现清单、已致缺陷、目标结构与 P0–P2 行动清单 |
| 官方插件集（非章节） | [PLUGIN_CATALOG.md](PLUGIN_CATALOG.md) | 竞品对照、官方插件清单与 5 个（guard/monitor/files/cron/ftp，已落地；原 nginx 已内核化为「网站」）、`plugins/` 第二 workspace、发布包携带 `official/` 与开箱即用交付线 |

### 按角色选读

| 你的目的 | 读这些 |
|---|---|
| 只做决策（约 15 分钟） | 本文 §0 → [§10 风险](sections/10-risks.md) → [§11 决策](sections/11-decisions.md) |
| 评审方案合理性 | [§1 需求](sections/01-requirements.md) → [§3 架构](sections/03-architecture.md) → [§4（一）](sections/04-plugin-package.md) → [§4（二）](sections/04-plugin-publish.md) → [§5 部署](sections/05-deploy.md) → [§6 安全](sections/06-security.md) |
| 动手写代码 | [§8 工程结构](sections/08-engineering.md) → [§9.1 架构核实门](sections/09-milestones.md) → [§3.3 原子能力](sections/03-architecture.md) → [§6.4 三层控制](sections/06-security.md) → [§4.2 生命周期](sections/04-plugin-package.md) → [§9 里程碑](sections/09-milestones.md) |
| 只关心上架 / 发布 | [§5.4 Docker 包](sections/05-deploy.md) → [§5.5 路径校验](sections/05-deploy.md) → [§4.4 发布](sections/04-plugin-publish.md) → [§9 M5](sections/09-milestones.md) |

---

## 文档约定

- **编号规则**：`§x.y` 为全局唯一编号，拆分到多文件后**不重编号**；文件名前缀号 = 章号，引用一律按编号，文件位置见上表。本文索引部分不占正文编号。
- **表格写法**：单元格内不出现裸 `|`（会被渲染器拆列）；取值枚举写成「A 或 B」或顿号分隔，必需的 `|` 用法放到表格外的段落里。
- **表格位置**：表格与代码块一律放在顶层，不缩进进列表项（部分渲染器不解析列表内的表格）。
- **代码 span 不跨行**：一行内闭合，跨行写法在多数渲染器里会退化成普通文本。
- **数字纪律**：未实测的数字与能力声明一律标注「设计目标 / 待实测」，M5 统一回填（§7、§12）。
- **单一事实来源**：同一约束只在一处下结论，其余处引用编号；发现冲突以编号靠前、且标注「拍板」的条目为准。

---

## 变更记录

| 日期 | 变更 |
|---|---|
| 2026-10-30 | 设计正文按章节拆分为 `docs/sections/` 13 个文件（覆盖 §1–§12，§4 分两册）；章节文件编号对齐章号，主文档只保留一页结论与索引。 |
| 2026-10-30 | §10 增加层级标注与「架构支柱级/契约级待核实」，§9 增加「架构核实门」，§12 增加「架构层缺口」；具体条目见正文。 |
| 2026-10-30～31 | MVP 代码落地、发布链路、GitHub CI/Release、P0/P1 收敛、两轮架构复审、上机就绪（0711 权限口径、TOCTOU 表述修正）等实现流水：详 [changes/2026-10-31.md](changes/2026-10-31.md)；审查结论见 [ARCH_REVIEW.md](ARCH_REVIEW.md)。 |
| 2026-11-01 | §11 决策 11（crate 拆分粒度维持现状）、决策 12（发布物去签名）；新增 [PLUGIN_CATALOG.md](PLUGIN_CATALOG.md)；官方插件集首批 4 个落地（`plugins/` 第二 workspace；ftp 于 2026-11-07 补入，现为 5 个）。详 [changes/2026-11-01.md](changes/2026-11-01.md)。 |
| 2026-11-02 | 架构审查第六轮结论：分层与判定层已稳，剩余缺陷同源于「授权 → 执行 → 留痕」没有唯一入口；文档校正官方插件 4 → 5。详 [changes/2026-11-02.md](changes/2026-11-02.md)。 |
| 2026-11-04 | **「网站」由插件内核化**：移除官方插件 `com.skiff.nginx`（`plugins/` 第二 workspace 成员、CI/打包清单、`docs/PLUGIN_CATALOG.md` / `MENU_CONTRACT.md` / `plugins/README.md` 描述同步），内核新增一级「网站」（网站列表 / 运行环境 / 设置）与 `sites` 表（schema v3）；`Docker::execute` 扩展 `images.pull` / `containers.create` / `containers.remove`，运行环境以容器形态跑 `nginx:alpine` + `php:8.5-fpm-alpine`，站点经 vhost 指向 PHP-FPM。详 `docs/changes/2026-11-04.md`。 |
| 2026-11-04 | **站点寄宿布局改为 `/www` + host 网络 + unix socket**：数据根迁 `/www/skiff`，站点 `/www/sites`、vhost `/www/nginx/conf.d`、日志 `/www/logs/nginx`、证书 `/www/certs`、PHP socket `/www/php`；nginx 换 OpenResty 固定 digest 并直接用宿主 80（去掉 8080 映射），PHP-FPM 只走 `unix:/www/php/php-fpm.sock`；站点日志落宿主并由内核轮转，容器日志 `local`+`max-size`。安装前探测 80/443 占用并拒绝半装。WAF 仅预留规则目录与模式字段（待定）。详 `docs/changes/2026-11-04.md`。 |
| 2026-11-05 | **站点树改「一站一目录」**：`/www/sites/<域名>/{index,log,ssl,conf}` 承载文档根、access/error 日志、证书与 vhost，备份单目录即备份整站；vhost 由公共 `10-skiff-sites.conf` 的 `include /www/sites/*/conf/*.conf;` 载入，默认站与兜底站仍在 `/www/nginx/conf.d`；**不做旧布局迁移**（启动不搬移/清理旧路径，测试期无正式库），站点目录浏览与 files 能力面根限定 `index/`。详 [changes/2026-11-05.md](changes/2026-11-05.md)。 |
| 2026-11-06 | 内核调度器契约与实现落地（任务模型、内存缓冲与四种回执、插件唤醒链路、内置任务）；站点停止态/运行态非法配置保存改为候选 `nginx -t` 校验。详 [changes/2026-11-06.md](changes/2026-11-06.md)。 |
| 2026-11-07 | 内核文件管理页第一步（`AdminFs` 接 `skiff-fs`，菜单退役与旧深链 301）；FTP 能力面与 `com.skiff.ftp` 上线（`MinUID=82`，schema v11）；内置任务删除在调度器/能力面/服务层硬拦截；PHP 池显式 `access.log = /dev/null`；站点高风险动作移入「管理」页签。详 [changes/2026-11-07.md](changes/2026-11-07.md)。 |
| 2026-11-07 | 文档体系收敛：新增 [TEST_ENV.md](TEST_ENV.md) 作为测试机访问信息唯一来源；门槛命令、测试台账、契约与实现说明去重。详 [changes/2026-11-07.md](changes/2026-11-07.md)。 |
| 2026-11-09 | 收尾清理：Node.js / Go 未实现，已从 `RUNTIME_KINDS` 与新建弹窗摘除；WAF 未实现，已移出范围，`sites.waf_mode` 与 `waf_rules_dir` 死配置及目录预留已删除。详 [changes/2026-11-09.md](changes/2026-11-09.md)。 |
