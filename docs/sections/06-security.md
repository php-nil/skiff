# 6. 安全

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§6 ｜ 上一章：[§5 部署](05-deploy.md) ｜ 下一章：[§7 资源预算](07-budget.md)

## 6.1 认证

argon2id(m=64MiB,t=3,p=4)；会话与 API Token 均为 32 字节随机、服务端只存哈希；会话 Cookie 使用 `__Host-skiff_session; Path=/; HttpOnly; SameSite=Strict; Secure`（无 Domain），支持空闲/绝对过期与改密后撤销；TOTP + 备份码；API Token 带 scope（`docker:read` / `docker:write` / `plugins:read` / `*`）。token 的有效权限 = 所属用户角色的权限 **∩** scope：`docker:write` 蕴含 `docker:read`，空 scope 集等于不能访问 `/api/v1` 的任何受控端点；解析是严格的，未知 scope 直接 401 并留审计，不会静默降级成「空权限」或「全权限」。会话认证不受 scope 收窄，角色限制仍由 §6.3 的 RBAC 负责。

## 6.2 传输与反代

非 loopback 管理流量默认只接受 HTTPS（内置 TLS 或可信反代终止）。纯 HTTP 仅限显式 `allow_insecure_http=true` 的 loopback 开发模式，此时改用非 `__Host-` 前缀的 `skiff_dev_session`、关闭 Secure 并持续标红，不能作为远程生产配置。`trusted_proxies` 使用 CIDR 白名单；只有直连 peer 命中时才接收 `X-Forwarded-*`，否则忽略并清理客户端伪造头，真实 IP 与 scheme 才能进入登录限流、Cookie 和审计逻辑。WebSocket 校验 `Origin` 与面板预期 origin 完全一致，并使用短期单连票据绑定会话和权限（票据由 `POST /{prefix}/containers/{id}/exec/ticket` 在 HTTP 侧签发：30 秒有效、用一次即废、绑定 session id 与容器 id，内存只存哈希；WS 握手必须带 `?ticket=`，**仅有 Cookie 不再足够**）；所有状态变更只走 POST/非 GET，WS/SSE 断开必须取消对应 Docker 流。

## 6.3 暴露面与 RBAC

**暴露面**：随机 8 位安全入口前缀（非该前缀返回 404 而非 403，只作隐蔽入口、不替代认证）；登录失败单 IP 5 次/5min 退避 + 封禁；全局失败计数；安全响应头（CSP/XFO/Referrer）；上传流式落盘 + 体积上限 + 文件名净化；非流式请求 30 秒处理超时（返回 504 错误页并取消下游调用），SSE 日志与 WS 终端不设超时——它们的生命周期由连接本身决定。

**RBAC**：admin / operator / viewer；插件安装、对象 grant、宿主路径授权和 Docker 删除/exec 等危险操作仅 admin，需二次确认或重新认证。

## 6.4 Docker Socket：动词 + 对象 + 参数三层控制

**诚实前提**：`docker.sock` 等价宿主 root。不引 socket-proxy（用户不接受额外组件）⇒ 内核侧采用**动词 + 资源对象 + 参数字段三层控制**。内核只用 bollard 的强类型方法，不对 UI 或插件提供任意 Engine HTTP 路径转发。

### 6.4.1 第一层：动词白名单

其余操作一律 403 + 审计。按里程碑放开：

- **基础档（M2 起）**：`GET /containers/json`、`GET /containers/{id}/json`、`GET /containers/{id}/logs`、`GET /containers/{id}/stats`；`GET /images/json`、`GET /networks`、`GET /volumes`、`GET /info`、`GET /version`；`POST /containers/{id}/start`、`POST /containers/{id}/stop`、`POST /containers/{id}/restart`、`POST /containers/{id}/kill`；`POST /containers/{id}/exec` + `POST /exec/{id}/start`（hijack 流，容器终端必需）+ `GET /exec/{id}/json`。
- **生命周期档（M3，Compose 所需）**：`POST /containers/create`、`POST /images/create`（仅 `fromImage` 拉取）、`POST /networks/create`、`POST /volumes/create`；`DELETE /containers/{id}`、`DELETE /images/{id}`、`DELETE /networks/{id}`、`DELETE /volumes/{id}`。

### 6.4.2 第二层：资源对象授权

动词相同也不代表能碰同一对象：

- Skiff 创建的 Compose 项目与容器统一打 `io.skiff.project` / `io.skiff.owner` 标签，保留标签只由内核写入，用户/插件提交同名标签一律拒绝；插件默认只能操作 `io.skiff.owner=<plugin-id>` 的资源，**只有动词、没有对象 grant 同样 403**。
- 插件要管理既有容器或他人创建的项目，必须由 admin 将明确的对象 ID/项目加入该插件 grant；授权记录可撤销，撤销即时生效。`exec`、删除、kill 是独立敏感动词，不因 read/list 授权自动获得。
- Skiff 自身容器、内核数据卷和带有 `io.skiff.protected=true` 的资源对插件恒拒绝；1Panel/宝塔资源默认不出现在插件可写集合。UI admin 管理外部资源须显式选择，危险操作重新认证，避免“同机共存”变成隐式接管。

### 6.4.3 第三层：参数字段白名单

在面板 API 层强制——Docker Engine 本身没有这类策略，未列字段拒绝而非忽略：

- MVP 容器创建只开放镜像、命令/入口、环境变量、标签、端口发布、重启策略、健康检查、非 host 网络附件、资源限额和受控卷挂载；拒绝 `Privileged`、`CapAdd`、`Devices`/`DeviceCgroupRules`、`SecurityOpt`、`VolumesFrom`、自定义 `Runtime`、host 网络/PID/IPC/user namespace，以及未列入白名单的 HostConfig 字段。
- **native 形态**：绑定挂载仅允许 admin 配置的宿主授权根；插件不能借一次调用自行扩根，超出范围只能由 admin 在授权界面新增。**受限 Docker 形态**：宿主绑定挂载一律拒绝，仅允许受控 named volume（§5.5）。
- MVP 新建 named volume 只允许默认 `local` driver 且 `driver_opts` 为空；引用既有卷前检查 driver/options，`o=bind`、`type=none`、`device=<host-path>` 等宿主绑定语义一律按绑定挂载处理，在受限形态拒绝，在 native 形态重新走路径授权。
- `networks/create` MVP 只允许默认 `bridge` 网络；拒绝 `host`/`none`/`macvlan`/`ipvlan`/`overlay`、自定义 driver、自定义 IPAM 与未列 options，外部网络引用须走对象 grant。
- 所有宿主路径一律经 §5.5 校验后才下传；对象授权与参数校验均在调用 bollard 前完成并记审计。

> **UI 与文档必须写明**：装了本面板 = 授予其宿主 root 等价能力。这不是缺陷，是这类产品的定义；上述策略用于约束面板与插件的可见操作，不改变 socket 本身的权限本质。

## 6.5 Compose MVP 子集

支持顶层 `services` / `volumes` / `networks`，可选 `version`（仅接受并忽略，保证常见 compose 文件可导入）；服务字段限 `image`、`command`、`entrypoint`、`environment`、`ports`、`restart`、`depends_on`、`healthcheck`、`volumes`、`networks`、`labels`；不支持 `build`、`extends`、`profiles`、`env_file`、`secrets`、`configs` 和未列字段，遇到即明确报错而非忽略。Compose 不调用 Engine `/build`，镜像只走 `fromImage` 拉取。项目名规范化后写入 `io.skiff.project`，up/down/restart 只作用于同项目对象；部分失败必须报告每个对象状态，不把“部分成功”判成整体成功。

## 6.6 插件安全与 fs / net 能力

**插件安全**：独立进程 + 每插件降权 UID/GID；landlock/cgroups v2 **尚未实现**（§4.2、§10.1），当前只有降权 + `setrlimit`。UI 沙箱 iframe 隔离；ed25519 签名 + 哈希清单校验（§4.3），未签名默认拒装；权限逐条确认 + Broker 逐次校验；不做自定义 seccomp。**宿主插件属于高信任原生代码**，签名与沙箱用于来源认证、权限约束与减灾，不消除恶意面或未知逃逸风险——文档明示“装插件 = 信任作者”。

**fs / net 能力（随 M4）**：fs 已实现 `read/write/list/mkdir/remove/rename`（无独立 `stat`），以路径白名单强制（插件自身目录 + 用户授权目录）；net 尚未实现——`net.apply` 一律返回 501，nftables 适配与按 UID owner-match 出网收口属 M4 硬门槛（§12.1），危险操作（全开/全关）二次确认待该能力落地时一并实现。
