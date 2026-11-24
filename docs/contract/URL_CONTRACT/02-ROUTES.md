# 现有路由清单与变更流程（§9–10）

> 上级索引：[URL_CONTRACT.md](../URL_CONTRACT.md) ｜ 拆自 `docs/contract/URL_CONTRACT.md` 原 §9–10（2026-11-16 整理）。

---

## 9. 现有路由清单（按资源）

> 只列规范路径；别名见 §7。`{p}` 为入口前缀。
> 「页面」列的路径**不是独立路由**——它们全部由壳页回落（§2.1）交付同一个 budding 壳页，
> 深链由前端路由自己解析；真正注册在 axum 上的只有「接口 / 长连接」列。

| 资源 | 页面（壳页回落深链） | 接口 / 长连接 |
| --- | --- | --- |
| 概览 | `/{p}`、`/{p}/`（302 到根） | `GET /ui/v1/overview`、`GET /ui/v1/menu` |
| 会话 | `/{p}/login`（公开，交付登录壳） | `GET`/`POST` `/ui/v1/auth/login`（未认证，签名 CSRF + 来源判定）、`GET /ui/v1/session`、`POST /ui/v1/session/logout` |
| 容器 | `/{p}/docker/containers`、`/{p}/docker/containers/{id}`、`/{p}/docker/containers/new`、`/{p}/docker/containers/{id}/terminal`、`/{p}/docker/containers/{id}/logs` | `GET /ui/v1/containers`、`GET /ui/v1/containers/{id}`、`GET /ui/v1/containers/{id}/logs?tail=`、`POST /ui/v1/containers`、`POST /ui/v1/containers/{id}/limits`、`POST /ui/v1/containers/{id}/{action}`；票据 `POST /{p}/containers/{id}/exec/ticket`；`GET /{p}/containers/{id}/exec`（WS）、`GET /{p}/containers/{id}/logs/stream`（SSE） |
| 镜像 | `/{p}/docker/images` | `GET /ui/v1/images`、`POST /ui/v1/images/pull`、`POST /ui/v1/images/{id}/remove` |
| 网络 | `/{p}/docker/networks` | `GET /ui/v1/networks`、`POST /ui/v1/networks`、`POST /ui/v1/networks/{id}/remove` |
| 卷 | `/{p}/docker/volumes` | `GET /ui/v1/volumes`、`POST /ui/v1/volumes`、`POST /ui/v1/volumes/{name}/remove` |
| Compose | `/{p}/docker/compose` | `GET /ui/v1/compose`、`POST /ui/v1/compose/validate`、`POST /ui/v1/compose/up`、`POST /ui/v1/compose/down`、`POST /ui/v1/compose/{project}/{action}` |
| 站点 | `/{p}/sites/list`、`/{p}/sites/new`、`/{p}/sites/{id}`、`/{p}/sites/web`、`/{p}/sites/web/logs` | `GET /ui/v1/sites`（`with_schema=true` 时附带页面 `schema`）、`GET /ui/v1/sites/{id}?tab=`（响应带 `data` + 服务端 `schema`）、`GET /ui/v1/sites/{id}/logs?part=&tail=&q=`；`POST /ui/v1/sites`、`POST /ui/v1/sites/batch`、`POST /ui/v1/sites/{id}/default`、`/domains`、`/config`、`/policy`、`/ssl`、`/backup`、`/delete`、`/ui/v1/sites/{id}/{action}`（站点路径统一用主键，主域名只作展示与删除确认） |
| Web 服务 | 无独立页（站点页内承载） | `GET /ui/v1/web`、`GET /ui/v1/web/install`、`GET /ui/v1/web/logs`、`POST /ui/v1/web/config`、`POST /ui/v1/web/params` |
| 运行环境 | `/{p}/sites/runtimes`、`/{p}/sites/runtimes/{id}`、`/{p}/sites/runtimes/{id}/logs` | 读：`GET /ui/v1/runtimes`、`/runtimes/catalog`、`/runtimes/images`、`/runtimes/{id}`、`/runtimes/{id}/logs?tail=`；写：`POST /ui/v1/runtimes`、`/runtimes/images`、`/runtimes/{id}/config`、`/config-file`、`/node-config`、`/port`、`/publish`、`/clone`、`/pecl`、`/probe`、`/extensions`、`/extensions/batch`、`/extensions/{key}/{op}`、`/action/{action}`、`/sites/{domain}/unbind` |
| 文件 | `/{p}/files`（壳页回落；形态门禁 `fs_capability` 挂在 `/ui/v1/files` 子路由） | JSON 全在 `/ui/v1/files/*`：`GET ?path=`、`/read`、`/download`；`POST /{write,mkdir,rename,remove,copy,chmod,chown,archive,extract,upload}`；`tree` 已下线（无消费方） |
| 回收站 | `/{p}/files/trash`（壳页回落；`FILE_TRASH_READ` 在数据端点判定） | `GET /ui/v1/files/trash/list`（`with_schema=true` 附页面骨架）、`POST /ui/v1/files/trash/{restore,purge,clear}` |
| 配额 | `/{p}/files/quotas`（壳页回落；`FILE_QUOTA_READ` 在数据端点判定） | `GET /ui/v1/files/quotas/list`（`with_schema=true` 附页面骨架）、`POST /ui/v1/files/quotas/{set,cancel,scan}` |
| 插件 | `/{p}/plugins/installed`、`/{p}/plugins/discover`、`/{p}/plugins/updates`、`/{p}/plugins/{id}`、`/{p}/plugins/{id}/{view}` | `GET /{p}/plugins/{id}/schema/{view}`（视图 schema JSON，`PluginManage` 权限）、`GET /{p}/plugins/{id}/ui/{path}`（包内资源）、`POST /{p}/plugins/{id}/rpc`；`{view}` 默认交壳页原生渲染，显式 `?legacy=1` 才是内核 iframe 工作台；JSON `GET /ui/v1/plugins`、`GET /ui/v1/plugins/{id}`、`POST /ui/v1/plugins/{id}/toggle`、`POST /ui/v1/plugins/{id}/delete`、`POST /ui/v1/plugins/confirm` |
| 商店 | `/{p}/plugins/store`、`/{p}/plugins/store/confirm` | `GET /ui/v1/store`、`POST /ui/v1/store/plugins/{id}/install`（返回确认数据）、`POST /ui/v1/store/upgrades/{version}/download`（落暂存）、`POST /ui/v1/plugins/confirm`（凭令牌最终安装） |
| 设置 | `/{p}/settings/security`、`/{p}/settings/menu`、`/{p}/settings/ui` | 读 `GET /ui/v1/settings`、`/settings/menu`、`/settings/docker`、`/settings/ui`；写 `POST /ui/v1/settings/menu`、`/settings/docker`、`/settings/ui`、`/settings/panel`、`/settings/password`、`/settings/totp/setup`、`/settings/totp/enable`、`/settings/keys`、`/settings/keys/{id}` |
| Docker 设置 | `/{p}/docker/settings` | 复用 `GET /ui/v1/settings/docker`、`POST /ui/v1/settings/docker` |
| 审计 | `/{p}/settings/audit`（budding 原生只读列表） | `GET /ui/v1/audit?q=&limit=&cursor=`（签名 keyset 游标）；`AuditRead` 权限不变，viewer 403 且菜单不可见 |
| 定时任务 | `/{p}/tasks`、`/{p}/tasks/{id}` | `GET /ui/v1/tasks`、`GET /ui/v1/tasks/{id}`、`POST /ui/v1/tasks/{id}/toggle`、`/run`、`/delete` |
| 任务中心 | `/{p}/jobs`、`/{p}/jobs/{id}`、`/{p}/jobs/{id}/logs` | `GET /ui/v1/jobs`、`GET /ui/v1/jobs/{id}`（含 `logs`）、`POST /ui/v1/jobs/{id}/cancel`、`/retry`；`GET /{p}/jobs/{id}/logs/stream`（SSE 尾随） |
| 壳页回落 | `GET /{p}` 与 `GET /{p}/{白名单前缀}[/{rest}]`（仅 `Accept: text/html`；白名单见 §2.1） | — |
| 静态资源 | — | `GET /{p}/static/{path}` |
| 图标 | — | `GET /favicon.ico`（无前缀、无鉴权；复用 `static/favicon.svg`，`image/svg+xml` + `public, max-age=86400`） |
| API v1 | — | `POST /api/v1/session`；`GET /api/v1/docker/containers`、`/docker/containers/{id}`、`/plugins`、`/plugins/ui`；`POST /api/v1/docker/containers/{id}/restart`（执行与审计统一走 `commands::docker::container_action`：成功 target=`name (short_id)`、detail=动词 `http_path`；失败 target=`name`、detail=错误串） |

## 10. 变更流程

1. 新路由先在本表登记资源、方法与动作，再写代码；不允许「先实现后补文档」。
2. 改已有路径必须同时提供兼容（§7）或明确下线记录，并更新前端路由表、静态资源与测试。
3. 代码侧守门：`crates/web/src/lib.rs` 的路由注册是唯一真相；`assets.rs` 校验静态资源 API 基址；
   `main_flow.rs` 校验旧路径跳转与壳页回落边界。
4. 文档侧引用：`MENU_CONTRACT` 只描述**菜单键 → URL**，URL 形状以本文为准。
