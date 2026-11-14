# Skiff 文档索引与阅读指南

> 本文是 `docs/` 的**唯一全量索引**：先看「一、30 秒路由」判断该读哪篇，再用「二、分类索引」查全文与状态。
> 分类只用于导航与筛选，**不代表物理目录调整**；现有文件名保持不变，避免断链。
> 最近核对：2026-11-14。新增文档必须在本文件登记，并在对应分类表补一行。

## 一、30 秒路由：我要做什么 → 先读哪篇

| 我要做什么 | 先读 | 读到什么程度 |
| --- | --- | --- |
| 任何改动动手前 | [AGENTS.md](../AGENTS.md) | 红线与交付纪律，必读 |
| 改 URL / handler / 表单动作 | [URL_CONTRACT.md](URL_CONTRACT.md) | 路径、动词、方法与副作用 |
| 改菜单 / 插件页 / iframe | [MENU_CONTRACT.md](MENU_CONTRACT.md) → [PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md) | 归属、声明、`view`、CSP |
| 改内核页面或文案 | [UI_CONTRACT.md](UI_CONTRACT.md) | 标题、提示、工具栏、空态、危险确认 |
| 改站点 / 运行环境 | [SITE_RUNTIME_CONTRACT.md](SITE_RUNTIME_CONTRACT.md) | 站点模型、建站类型、PHP/Node 边界 |
| 改 fs / 调度 / FTP / 防火墙能力面 | 对应 `KERNEL_*_CONTRACT.md` → `KERNEL_*_IMPL_NOTES.md` | 契约先读，实现差异后读 |
| 出包 / CI 红 / 发版 | [CI.md](CI.md) | §2 门槛、§4 发布、§6 本地复现 |
| 上测试机 / 验收 / 回滚 | [DEPLOY_TEST.md](DEPLOY_TEST.md) → [ITERATION.md](ITERATION.md) → [TEST_ENV.md](TEST_ENV.md) → [TEST_MATRIX.md](TEST_MATRIX.md) → [TEST_LEDGER.md](TEST_LEDGER.md) | 步骤 → 编排 → 坐标 → 用例 → 未闭环 |
| 改商店 / 分发 / `skiff-server` | [SKIFF_SERVER.md](SKIFF_SERVER.md) → [PLUGIN_CATALOG.md](PLUGIN_CATALOG.md) | 清单、签名、`serve` / `api` 契约 |
| 接 budding / SDUI | [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md) → [SDUI_HANDBOOK.md](SDUI_HANDBOOK.md) | 接入边界、迁移方法 |
| 想知道为什么这样设计 | [DESIGN.md](DESIGN.md) → [sections/](sections/) → [ARCH_REVIEW.md](ARCH_REVIEW.md) | 结论 → 论证 → 历史审查 |
| 查某天改了什么 | [changes/](changes/) | 只查历史，不作为规范 |

## 二、分类索引

状态取值：**现行**（按它实现/执行）、**设计稿**（未实施或部分实施）、**待实测**（数字或能力未经实测）、**历史存档**（只读，不更新）。括号内为最近核对日期。

### A. 设计总览（为什么做、范围、决策）

| 文档 | 什么时候读 | 不读的后果 | 状态 |
| --- | --- | --- | --- |
| [DESIGN.md](DESIGN.md) | 需要一页结论、范围边界与设计正文地图 | 容易把插件能力塞回内核 | 现行（2026-11-13） |
| [sections/01-requirements.md](sections/01-requirements.md) | 评审需求与验收项、确认「明确不做」 | 需求蔓延 | 现行（2026-11-13） |
| [sections/02-tech-stack.md](sections/02-tech-stack.md) | 换依赖、评估技术选型 | 引入与约束冲突的依赖 | 现行（2026-11-13） |
| [sections/03-architecture.md](sections/03-architecture.md) | 改分层、原子能力或架构目标 | 破坏内核与插件的边界 | 现行（2026-11-13） |
| [sections/04-plugin-package.md](sections/04-plugin-package.md) | 改 `.wbpkg`、manifest、生命周期、信任模型 | 包格式或验签链路漂移 | 现行（2026-11-13） |
| [sections/04-plugin-publish.md](sections/04-plugin-publish.md) | 改插件发布、安装/升级/回滚 | 交付物与 CLI 不一致 | 现行（2026-11-13） |
| [sections/05-deploy.md](sections/05-deploy.md) | 改部署形态、路径与卷 | 装机路径或共存策略出错 | 现行（2026-11-13） |
| [sections/06-security.md](sections/06-security.md) | 改认证、RBAC、Docker Socket、Compose 子集 | 安全边界被绕过 | 现行（2026-11-13） |
| [sections/07-budget.md](sections/07-budget.md) | 引用资源数字前 | 把设计目标当成实测值 | 待实测（2026-11-13） |
| [sections/08-engineering.md](sections/08-engineering.md) | 改 crate 布局、依赖方向、`xtask` 校验 | 分层红线被破坏 | 现行（2026-11-13） |
| [sections/09-milestones.md](sections/09-milestones.md) | 查 M0–M5 交付物与架构核实门 | 误判当前完成度 | 历史存档（2026-11-13） |
| [sections/10-risks.md](sections/10-risks.md) | 做架构评审或查设计期风险 | 重复踩已知风险 | 历史存档（2026-11-13） |
| [sections/11-decisions.md](sections/11-decisions.md) | 需要推翻或引用已拍板默认值 | 重复讨论已定事项 | 现行（2026-11-13） |
| [sections/12-pending-tests.md](sections/12-pending-tests.md) | 查设计期待实测清单 | 把未实测能力当事实 | 待实测（2026-11-13） |

### B. 契约（必须怎么做，实现的唯一依据）

| 文档 | 什么时候读 | 不读的后果 | 状态 |
| --- | --- | --- | --- |
| [URL_CONTRACT.md](URL_CONTRACT.md) | 新增/修改页面、接口、动作路径 | 路径与动词不一致，旧深链断裂 | 现行（2026-11-13） |
| [MENU_CONTRACT.md](MENU_CONTRACT.md) | 改菜单归属、声明、渲染、持久化 | 三级菜单与固定页槽位错位 | 现行（2026-11-13） |
| [PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md) | 改插件信息页/功能页、`view`、iframe | 插件页路由、CSP 或静态映射出错 | 现行（2026-11-13） |
| [UI_CONTRACT.md](UI_CONTRACT.md) | 新增或改动任何内核页面与文案 | 标题、提示、空态、断点不统一 | 现行（2026-11-13） |
| [SITE_RUNTIME_CONTRACT.md](SITE_RUNTIME_CONTRACT.md) | 改站点、运行环境、建站流程 | 站点模型或运行环境生命周期出错 | 现行（2026-11-13） |
| [KERNEL_FS_CONTRACT.md](KERNEL_FS_CONTRACT.md) | 改 fs 授权、操作面、回收站、配额 | 越权或路径语义漂移 | 现行（2026-11-13） |
| [KERNEL_SCHEDULER_CONTRACT.md](KERNEL_SCHEDULER_CONTRACT.md) | 改任务模型、调度、回执、插件唤醒 | 任务语义或重试/并发出错 | 现行（2026-11-13） |
| [KERNEL_FTP_CONTRACT.md](KERNEL_FTP_CONTRACT.md) | 改 ftp 能力面、账号、uid/gid | 权限与站点身份不一致 | 现行（2026-11-13） |
| [KERNEL_FIREWALL_CONTRACT.md](KERNEL_FIREWALL_CONTRACT.md) | 改 firewall 能力面、作用域、审计 | 越界改动宿主防火墙 | 现行（2026-11-13） |
| [SDUI_HANDBOOK.md](SDUI_HANDBOOK.md) | 做 SDUI 资源/动作/迁移 | 迁移阶段与回滚边界失控 | 设计稿（P0/P1 已落地，P2 未实施；2026-11-13） |
| [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md) | 接 budding、查 DTO 字段与错误码 | 前端契约与内核实现脱节 | 现行（状态以正文为准；2026-11-13） |
| [SKIFF_SERVER.md](SKIFF_SERVER.md) | 改商店清单、签名、`serve` / `api` 发布面 | 分发契约或 `seq` 防回滚被破坏 | 现行（2026-11-13） |

### C. 实现说明（代码实际怎么做、与契约差在哪）

| 文档 | 什么时候读 | 不读的后果 | 状态 |
| --- | --- | --- | --- |
| [KERNEL_FS_IMPL_NOTES.md](KERNEL_FS_IMPL_NOTES.md) | 改 `skiff-fs`，或契约与代码不一致 | 重复实现路径解析或授权 | 现行（2026-11-13） |
| [KERNEL_SCHEDULER_IMPL_NOTES.md](KERNEL_SCHEDULER_IMPL_NOTES.md) | 改 `skiff-sched`，或接 SQLite/Web | tick 语义与回执边界出错 | 现行（2026-11-13） |
| [KERNEL_UI_BOUNDARY_AUDIT.md](KERNEL_UI_BOUNDARY_AUDIT.md) | 改 SSR/`/ui/v1` handler、拆 `ui_api.rs`、判断能否复用命令层 | 重复实现、JSON 与 SSR 分叉、权限/审计口径不一致 | 现行（审计稿；2026-11-14） |

### D. 交付与验证（怎么出包、怎么装、怎么验、现在什么状态）

| 文档 | 什么时候读 | 不读的后果 | 状态 |
| --- | --- | --- | --- |
| [CI.md](CI.md) | 出包、发版、CI 红、改工作流 | 绕过门槛或发布错资产 | 现行（2026-11-13） |
| [DEV_ENV.md](DEV_ENV.md) | 本机编译/推送异常、闸门与 `PATH` 问题 | 绕过闸门或陷入认证/代理排查 | 现行（2026-11-13） |
| [DEPLOY_TEST.md](DEPLOY_TEST.md) | 装机、配置、上机验收 | 装错形态或漏验收项 | 现行（§9 为历史实测；2026-11-13） |
| [ITERATION.md](ITERATION.md) | 跑单轮迭代、回滚协同 | 轮次与基线用错 | 现行（§9 为历史实测；2026-11-13） |
| [TEST_ENV.md](TEST_ENV.md) | 查测试机地址、账号、当前部署坐标与最近验收 | 用过期入口前缀或旧资产 | 现行（唯一坐标来源；2026-11-13） |
| [TEST_LEDGER.md](TEST_LEDGER.md) | 查未闭环项与下一步 | 重复报已知问题或漏修 | 现行（2026-11-13） |
| [TEST_MATRIX.md](TEST_MATRIX.md) | 改功能域后做回归、上机验收 | 漏稳定编号用例 | 现行（2026-11-13） |

### E. 规划与外部协作

| 文档 | 什么时候读 | 不读的后果 | 状态 |
| --- | --- | --- | --- |
| [PLUGIN_CATALOG.md](PLUGIN_CATALOG.md) | 增删官方插件、查开箱即用交付线（官方插件 6 个 + testkit = 7 个签名包） | 插件清单与发布包口径不一致 | 现行（2026-11-13） |
| [BUDDING_UPSTREAM_BACKLOG.md](BUDDING_UPSTREAM_BACKLOG.md) | 需要向 budding 团队提能力缺口 | 重复提需求或误以为已实现 | 现行（对照 budding 1.8.0；2026-11-13） |
| [PLUGIN_UI_SCHEMA_ASSESSMENT.md](PLUGIN_UI_SCHEMA_ASSESSMENT.md) | 评估插件功能页去 iframe、改 Schema 原生渲染的可行性与工作量 | 误判插件页现状或重复评估 | 评估稿（2026-11-13） |
| [plugins/README.md](../plugins/README.md) | 改官方插件、第二 workspace、打包 | 根三关漏跑插件 workspace | 现行（2026-11-13） |

### F. 历史归档（只读，不更新，不参与路由）

| 文档 | 内容 | 状态 |
| --- | --- | --- |
| [ARCH_REVIEW.md](ARCH_REVIEW.md) | 六轮架构审查、P0–P2 行动清单；正文行号已随代码腐化 | 历史存档（2026-11-13） |
| [ui/README.md](ui/README.md) | 导航与站点运行环境的设计过程、可点预览说明 | 历史存档（2026-11-13） |
| [ui/preview.html](ui/preview.html)、[ui/redesign-preview.html](ui/redesign-preview.html)、[ui/menu-redesign.html](ui/menu-redesign.html)、[ui/files-manager-preview.html](ui/files-manager-preview.html)、[ui/site-runtime-preview.html](ui/site-runtime-preview.html) | 静态设计预览，数据为示意，不代表现行实现 | 历史存档（2026-11-13） |
| [changes/2026-10-31.md](changes/2026-10-31.md) | MVP 落地、发布链路、CI/Release、P0/P1、上机就绪 | 历史存档（2026-10-31） |
| [changes/2026-11-01.md](changes/2026-11-01.md) | 测试补强、Release 首跑修复、官方插件集落地、架构审查第五轮 | 历史存档（2026-11-01） |
| [changes/2026-11-02.md](changes/2026-11-02.md) | 架构审查第六轮、文档与注释一致性校正 | 历史存档（2026-11-02） |
| [changes/2026-11-03.md](changes/2026-11-03.md) | 迭代验证回路、首装/升级/重装轮、菜单契约 P1/P2 上机 | 历史存档（2026-11-03） |
| [changes/2026-11-04.md](changes/2026-11-04.md) | 网站内核化、`/www` 寄宿、UI 重构、交付链路固化 | 历史存档（2026-11-04） |
| [changes/2026-11-05.md](changes/2026-11-05.md) | 一站一目录、文件管理器 UI、运行环境 P1、多实例 PHP | 历史存档（2026-11-05） |
| [changes/2026-11-06.md](changes/2026-11-06.md) | 内核调度接线、cron 降级、运行环境元数据、musl 并发崩溃修复 | 历史存档（2026-11-06） |
| [changes/2026-11-07.md](changes/2026-11-07.md) | 文件管理第一步、FTP 能力面、容器删除入口、文档体系收敛 | 历史存档（2026-11-07） |
| [changes/2026-11-08.md](changes/2026-11-08.md) | 端口/前缀/TLS 可配置、文件管理第二步、Docker 三类入口、任务中心 | 历史存档（2026-11-08） |
| [changes/2026-11-09.md](changes/2026-11-09.md) | 收尾清理、Node.js 接入、SDUI 手册、DEV_ENV 登记 | 历史存档（2026-11-09） |
| [changes/2026-11-10.md](changes/2026-11-10.md) | 站点统一身份、防火墙插件、URL 契约、SDUI P0/P1、Skiff-server P0–P2 | 历史存档（2026-11-10） |
| [changes/2026-11-11.md](changes/2026-11-11.md) | PHP 双家族、配置写入加固、`serve` 与 R1/R2a 发布面 | 历史存档（2026-11-11） |
| [changes/2026-11-12.md](changes/2026-11-12.md) | R2b–R2e、托管形态定案、商店抓取超时修复 | 历史存档（2026-11-12） |

## 三、文档体系与命名

| 层级 | 位置 | 职责 |
| --- | --- | --- |
| L1 根规范 | `AGENTS.md` | 全局红线、交付与测试纪律、任务路由；不记录子项目细节 |
| L2 项目规范 | `crates/<name>/AGENTS.md` | crate 专属业务、配置、接口与开发说明（当前均未建） |
| L3 专项文档 | `docs/` | 跨 crate 的设计、契约、实现说明、交付与验证、规划与归档 |

- **命名**：`docs/sections/` 用「两位章号-小写中划线.md」；`docs/` 顶层用语义化短名（`UPPER_SNAKE_CONTRACT.md` / `*_IMPL_NOTES.md`）；变更记录用 `changes/YYYY-MM-DD.md`。
- **单一来源**：同一约束只在一处下结论，其它位置写相对路径引用，不复制正文。
- **子文档拆分**：crate 内复杂专题可放 `crates/<name>/docs/`，但须在该 crate 的 `AGENTS.md` 登记；新增 `docs/` 文档须在本文件登记。
- **历史内容**：失效的旧版文档不保留；仍有效的结论移入现行文档或归档，不直接删除。
- **数字纪律**：未实测的数字与能力声明标「设计目标 / 待实测」，不当作已验证事实。

## 四、改动时的文档同步责任

| 改动类型 | 必须同步 |
| --- | --- |
| 新增/修改 URL、动作、查询参数 | [URL_CONTRACT.md](URL_CONTRACT.md) §9、[TEST_MATRIX.md](TEST_MATRIX.md) |
| 新增/修改菜单、插件页 | [MENU_CONTRACT.md](MENU_CONTRACT.md)、[PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md) |
| 新增/修改页面与文案 | [UI_CONTRACT.md](UI_CONTRACT.md) |
| 改内核能力面（fs/调度/FTP/防火墙） | 对应 `KERNEL_*_CONTRACT.md`；实现差异写 `*_IMPL_NOTES.md` |
| 改站点/运行环境 | [SITE_RUNTIME_CONTRACT.md](SITE_RUNTIME_CONTRACT.md) |
| 改 CI、发布流程、工具链 | [CI.md](CI.md) |
| 每次测试机部署 | [TEST_ENV.md](TEST_ENV.md) §5–§7（地址 + 账号必须在汇报中交代） |
| 修复缺陷或改判定逻辑 | 对应测试用例 + [TEST_MATRIX.md](TEST_MATRIX.md)；未闭环写 [TEST_LEDGER.md](TEST_LEDGER.md) |
| 任何结构性变更 | `changes/YYYY-MM-DD.md`；[DESIGN.md](DESIGN.md) 只记结构性/结论性变化 |
