# 桥接：登录、401/403、ACL、审计、表单与长任务（§5.1–5.6）

> 上级索引：[SDUI_BUDDING_INTEGRATION.md](../SDUI_BUDDING_INTEGRATION.md) ｜ 拆自 `docs/sdui/SDUI_BUDDING_INTEGRATION.md` 原 §5.1–5.6（2026-11-16 整理，正文未改）。

---

## 5. 登录、权限、审计、CSRF 的桥接

### 5.1 登录页与 CSRF（D3 定案：登录页也只用 budding）

**已实施（2026-11-12，第三批）**

- `GET /{p}/login` 返回 budding 登录壳（`id="skiff-login"` 挂载点），未认证直接 200，不经过 SPA 回落的匿名 303 分支；已登录访问仍 303 到 `/{p}`。壳只引用版本化 `budding-<ver>.min.js`、`budding-<ver>.min.css` 与 `budding-login.js`，不含内联脚本。
- 页面启动后调用 `GET /ui/v1/auth/login`（未认证、禁止缓存）：返回 `{ data: { csrf_token, entry_prefix, api_version } }`，并下发 `skiff_login` Cookie 作为兼容信号。
- 提交走 `POST /ui/v1/auth/login`（未认证）：JSON 体 `{ username, password, otp, csrf, next }`，CSRF 放 JSON 体 `csrf` 字段（与表单路径同一个签名 token，前端不额外造头）。
- CSRF：沿用现有签名 token（`skiff_identity::signed_token`，TTL 600 秒，密钥取持久化 `security.csrf_secret`）；**签名 token 仍是唯一权威**，Cookie 只在浏览器完全没有来源头时作兼容。失败沿用现有 `code` 字段：`auth.csrf_invalid`（伪造/过期，文案区分「已过期」与「校验失败」）、`auth.unauthenticated`（用户名/口令或动态码错误）。
- 来源判定：仍在内核 handler 读 `Sec-Fetch-Site` / `Origin` / `Referer`；跨站返回 403 与现有文案「跨站登录请求被拒绝」，budding 不参与判定。
- 限流：沿用 `skiff_app::auth::login` 的失败计数与并发闸门；超限返回 503（现有 `dependency.unavailable`）与「登录失败次数过多，请 N 秒后重试」。
- TOTP：现有实现是**单步表单**，没有第二步分支；`otp` 字段原样保留，错误文案仍为「用户名、口令或动态码不正确」，不用新建错误码。
- 失败呈现：服务端 JSON `error.message` 由启动脚本写入 `sessionStorage` 并重载登录页，页面内 budding `alert` 展示后即清除；错误文案不写 URL（避免可构造的伪造提示），用户名保留、口令清空——与旧页 POST 后重渲染的观感一致。
- 会话 Cookie：登录成功响应带 `{ data: { redirect_to, username, role } }` 与 `Set-Cookie`；Cookie 形态完全复用 `skiff_identity::session_cookie`（`HttpOnly; SameSite=Lax`，`Secure` 由可信代理解析后的 scheme 决定）。
- 跳转：`redirect_to` 由内核按与表单登录同一函数计算（首登强制改密 → `/{p}/settings?force=1`；站内 `next` 深链原样；否则 `/{p}`），前端只做 `location.assign`。
- 登出：`POST /ui/v1/session/logout`（会话 + CSRF 头），由内核清 Cookie。

**仍待实施 / 与早期设计稿的差异**

- 早期设计稿的 `GET /ui/v1/bootstrap`、`POST /ui/v1/session`、`x-csrf-token` 头与 `auth.csrf_expired` / `auth.cross_site` / `auth.rate_limited` / `auth.totp_*` 细分码**未采用**：当前实现复用既有 `auth.csrf_invalid` / `auth.unauthenticated` / `dependency.unavailable` 与现有中文文案，避免在登录链路上新造第二套错误码。后续如确需机器可判别 TOTP 缺省/错误，再单独提案。
- 登录成功响应暂不返回 `permissions/acl`（当前 `/{p}/login` 成功后由服务端页面加载，壳页会话接口 `GET /ui/v1/session` 提供角色与 CSRF）；接入第一张真实业务页时再按 §5.3 补 ACL 快照。
- **口令输入（已原生）**：budding 1.8.0 重打包起 `input-text` 透传原生 `type`（含 `password`），1.9.0 起 `autocomplete` 也同名透传（未配置时不输出属性）；登录页与设置页直接声明 `type: 'password'` / `autocomplete`，首帧即掩码，不再有渲染后改 DOM 的临时桥接。
- 登录只有两条：`GET /{p}/login` 交付登录壳、`POST /ui/v1/auth/login` 收 JSON。旧 askama 登录模板与表单 `POST /{p}/login` 已于 2026-11-19 删除，**没有服务端兜底提交路径**——前端产物加载失败时登录同样不可用（D3「不留双栈」的必然代价，回滚靠发布 tag）。

### 5.2 401 / 403

- `/ui/v1/*` 必须被 `AuthUser` 视为机器接口：未登录返回 401 JSON，**不得再 303 到登录页**；`entry_guard` 需与 `/api/v1` 并列豁免。
- 401：`code = unauthenticated`；前端 `platform.request` 适配器统一跳转 `/{p}/login?next=...`。
- 403：`code = forbidden`，可选 `required_permission`；前端展示 `outlet.content.forbidden` 或页面内提示。
- **budding 没有 401 全局拦截**，且注入 `platform.request` 后 `apiAllowlist` 失效；适配器必须自行实现「仅允许同源相对路径 + `/ui/v1` 前缀」的白名单，否则等于放开任意地址。
- 403 的现有语义可复用：路由 `permission` 不满足时菜单剔除 + 出口 403 占位且不拉 schema。
- 会话空闲超时（`verify_session` 返回空）按 401 处理；数据库不可用（`Unavailable`）按 503 处理，二者不得塌成同一个登录页。

### 5.3 ACL 快照与重新 embed

budding 的 `acl` 是快照，权限变化必须重新 `embed`。交接路径：登录成功响应带 `acl`，前端保存并在每次 `embed` 时传入；后续以 `/ui/v1/session` 的返回为准。

| 时机 | 动作 |
|---|---|
| 登录成功 / 登出 | 重建或销毁实例；登出后清空本地 acl |
| 收到 403 且 `required_permission` 与本地 acl 不一致 | 重新拉会话；仍 403 则展示无权限 |
| 会话空闲超时或指纹失效 | 跳登录，清空 acl |
| 菜单或权限配置被管理员修改 | 下次导航时刷新会话；当前页提示「刷新以应用」 |
| 入口前缀变更 | 内核 302 到新前缀；前端按新前缀重建并重新拉会话 |
| 前端组件目录升级（产物替换） | 整体重新 `embed`，不做局部热替换 |

前端 `permission` 只决定显隐；**任何写动作都必须由内核再次判定**，越权动作内核必须拒绝并留审计。

### 5.4 审计

- 业务审计保持在命令层（`crates/app/src/commands/`）；Web 层只负责授权入口、错误映射与响应。
- 拒绝路径审计留在传输层：CSRF 失败、未认证、无权限、入口前缀 404、panic。
- budding 的 `platform.report` 是**错误遥测**，`platform.request` 是可注入的请求钩子；两者**都不得作为业务审计来源**，最多作为补充日志。

### 5.5 表单字段错误回填

- budding 提交失败只 `report` + toast + 跑 `onError` 动作链，**不会自动回填字段错误**。
- 约束：内核失败返回 400 + `error.field`；前端用自定义动作把错误写入 `ctx.extras.form.errors[fieldCode]`。
- 字段码必须与节点 id 一致（§3.3），否则错误落不到控件。

### 5.6 长任务、流式与终端

| 能力 | 现状 | budding 接入方式 |
|---|---|---|
| 作业进度 | 作业中心 `progress` + SSE 日志 | 用现有轮询端点 + `source.autoRefresh`；SSE 暂不接 |
| 运行环境安装进度 | `GET /runtimes/jobs` 轮询 | 直接映射为轮询 |
| 容器日志 | 尾快照 + 刷新 | 壳页主路径为 `GET /ui/v1/containers/{id}/logs?tail=`（§5.20）；`GET /{p}/containers/{id}/logs/stream`（SSE）仍在，只作调试与超长日志跟随，不进壳页 |
| 终端 | 票据 + WS | 已落地：`terminal` Pack 内联 + 既有 exec 票据/WS（§5.19）；WS 真连待 Linux 测试机验收 |
| 文件上传 | 分片 `offset` 协议 | budding `file` 只支持单次 FormData POST；见 §6 |
