# 路由权限、P1/P2 范围（§6–7）

> 上级索引：[SITE_RUNTIME_CONTRACT.md](../SITE_RUNTIME_CONTRACT.md) ｜ 拆自 `docs/contract/SITE_RUNTIME_CONTRACT.md` 原 §6–7（2026-11-16 整理，正文未改）。

---

## 6. 路由、权限与审计

### 6.1 路由与深链

L2 保持 `docs/contract/MENU_CONTRACT.md` 的内置项：网站列表 / 运行环境 / Nginx。运行环境详情是 L2 下的
子页面，不新增 L3（L3 只属于插件）。站点级设置页已移除：镜像加速源/前缀归 Docker 设置，
nginx/PHP 镜像标签归运行环境页的「镜像参数」。

| 页面 | 规范 URL |
| --- | --- |
| 网站列表 | `/{p}/sites/list` |
| 新建网站 | `/{p}/sites/new`（可带 `?type=runtime|static|proxy`；壳页原生表单） |
| 站点详情 | `/{p}/sites/<id>`（站点主键，不复用主域名） |
| 运行环境列表 | `/{p}/sites/runtimes` |
| 运行环境详情 | `/{p}/sites/runtimes/<runtime-id>` |
| 运行环境标签页 | 壳页内标签：`?tab=config|logs|sites` |

- 页面只有壳页：`/{p}/sites*` 均为 budding 页，数据走 `/ui/v1/*`；旧深链已删除（见 `docs/contract/URL_CONTRACT/01-RULES.md` §7）。

- 站点级设置页已移除：`image_prefix` / `registry_mirror` 归 Docker 设置（`/{p}/docker/settings`），
  `nginx_image` / `php_image` 归运行环境页的「镜像参数」。URL 规则统一见 [`URL_CONTRACT.md`](../URL_CONTRACT.md)。

- `runtime-id` 只用 `[a-z0-9.-]`（如 `php-8.5`），不把镜像 tag、路径拼进 URL。
- 深链必须能直接打开（交付 budding 壳页）；标签切换只改 query，不减损可分享性。
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
3. **三步向导**：类型选择、运行时选择（列出所有已就绪 PHP / Node.js / Rust 实例）、类型相关字段、校验与确认；Go 未实现，不出现选项。
4. **运行环境页**：PHP / Node.js / Rust 列表与右侧抽屉详情、基础信息、PHP ini/FPM、Node 启动命令/健康检查/环境变量、日志、关联站点、启停/重建/删除
   保护；列表与详情不常驻安装进度/镜像源；Go 不显示目录项或信息卡。
5. **多版本 PHP**：同 kind 多实例并行，每实例独立 socket `/www/runtime/php/<name>/php-fpm.sock`；
   vhost 按站点 `runtime_id` 路由；停止实例时绑定站点明确 502/503 且可回环境页，不静默换实例。
6. **状态与边界**：运行环境停止导致绑定站点显示“环境停止”；删除被引用环境被拒绝；Docker 页徽章与双向深链。
7. **路由/权限/审计**：规范 URL 与旧路由别名；admin 写、viewer 读；`sites.runtime.*` 事件。
8. **测试**：§8 中 P1 项全绿；根 workspace 门禁全绿（本地 `fmt` + `clippy`，用例以 CI 结果为准）；不部署测试机。

### P2（扩展，不阻塞 P1）

- Go 运行环境安装、重建、启停；只运行**用户已编译二进制**，面板不做 `go build`、不假装有构建链路。
- 每站点独立 Node 进程/容器（当前为每环境单进程 + 固定端口）、站点级依赖构建日志与失败回滚、
  站点级健康状态与最近重启。
- PHP 扩展开关与受控镜像重建（Dockerfile 在 `ext/`，构建日志可见）；站点级 php.ini / 独立 FPM 池。
- 运行环境漂移检测的“同步状态”、站点改绑向导、批量改绑。

### 非目标（P1/P2 都不做）

- 数据库创建、SSL 自动申请（Let’s Encrypt）、应用商店、一键源码部署、Compose 编辑。
- **WAF 未实现，已移出范围**：`sites.waf_mode` 设置键与 `waf_rules_dir` 规则目录占位已在 2026-11-09 删除；
  该项不再作为待定功能，如需恢复须重新立项（规则来源、开关位置与回滚口径均无设计）。
- **SSL 保留现状**：自签证书、证书上传与强制 HTTPS 已实现，P1 不回退、不改入口；建站向导只在确认页
  说明「SSL 后续在站点 SSL 标签配置」，不内嵌申请流程。
- 在“静态站”上启动 PHP/Node；在“反代站”上托管本地文件；“一站多模式”复合站点。

---
