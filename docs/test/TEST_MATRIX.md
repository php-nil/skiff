# Skiff 测试矩阵（按内核功能域 · 索引）

> 定位：**回归清单**。改一处代码后，按章首「重跑触发」挑一章，按稳定编号逐项执行即可。
> 本文只写当前实现应通过的验收项，不记历史、不记未闭环问题；未闭环问题归 [TEST_LEDGER.md](TEST_LEDGER.md)。
> 所有预期值均按当前路由、模板与契约整理；无法从代码确认的项标「待核对」并写明原因。
> **本文件是索引**：用例按功能域拆在 [`TEST_MATRIX/`](TEST_MATRIX/)，**用例编号（AUTH-xx / SITE-xx …）跨册不变**，按编号检索即可。

## 使用约定（先读）

`$B` 基址、`$P` 入口前缀、`$J` CookieJar、执行档位（`[本机]` / `[本机+上机]` / `[上机]`）与登录命令统一见 [`TEST_MATRIX/00-USAGE.md`](TEST_MATRIX/00-USAGE.md)；坐标取值只在 [`TEST_ENV.md`](TEST_ENV.md)。

## 分册地图

| 域 | 文件 | 覆盖编号 |
| --- | --- | --- |
| 约定与总览 | [TEST_MATRIX/00-USAGE.md](TEST_MATRIX/00-USAGE.md) | 使用约定、总览表 |
| A 身份与会话 / B 菜单与导航 / C 插件体系 | [TEST_MATRIX/01-IDENTITY_MENU_PLUGIN.md](TEST_MATRIX/01-IDENTITY_MENU_PLUGIN.md) | AUTH-xx、MENU-xx、PLUGIN-xx |
| D 网站与运行环境 / E Web 服务 | [TEST_MATRIX/02-SITE_WEB.md](TEST_MATRIX/02-SITE_WEB.md) | SITE-xx、WEB-xx |
| F 文件系统原语 / G 调度器 | [TEST_MATRIX/03-FS_SCHEDULER.md](TEST_MATRIX/03-FS_SCHEDULER.md) | FS-xx、TASK-xx |
| H Docker / I 部署与升级 | [TEST_MATRIX/04-DOCKER_DEPLOY.md](TEST_MATRIX/04-DOCKER_DEPLOY.md) | DOCKER-xx、DEPLOY-xx |
| J 站点压测 / K FTP / L 后台端口与 TLS | [TEST_MATRIX/05-PERF_FTP_PANEL.md](TEST_MATRIX/05-PERF_FTP_PANEL.md) | PERF-xx、FTP-xx、PANEL-xx |
| M 防火墙与进程网络 / N 任务中心作业 | [TEST_MATRIX/06-FIREWALL_JOBS.md](TEST_MATRIX/06-FIREWALL_JOBS.md) | FW-xx、JOB-xx |
| O 商店目录 / P 发布面 API / 附 | [TEST_MATRIX/07-STORE_API.md](TEST_MATRIX/07-STORE_API.md) | STORE-xx、API-xx、待核对项结论 |

## 维护约定

- 新增用例只追加编号，不复用已删除编号；跨域用例放「主域」册内，其余处引用编号。
- 用例的预期值以契约为准（`docs/contract/`），实现与契约不一致时先改契约或先修实现，别在矩阵里留第三种口径。
