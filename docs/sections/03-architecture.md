# 3. 总体架构

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§3 ｜ 上一章：[§2 技术选型](02-tech-stack.md) ｜ 下一章：[§4 扩展体系（一）](04-plugin-package.md)

## 3.1 分层

```
浏览器 (askama + htmx + 懒加载 ESM)
        │ HTTPS / WS / SSE
skiff-core (单进程 · systemd · 宿主机)
  中间件(axum layer，由内到外): 请求超时(非流式路由) → 可信代理解析 → 会话 → 安全入口 → CatchPanic
  handler 内: CSRF → RBAC(policy) → 审计；登录限流(同 IP 5 次/5 分钟)
  应用上下文: AppState/WebConfig（db · docker · supervisor · policy · 限流表 · exec 票据）
                                              ← 装配层（crates/app）
  api::v1 │ ui(SSR) │ static(embed)             ← 核心① Web 管理服务（crates/web）
  Capability Broker (鉴权/审计/配额；能力面 docker/identity/fs/net) ← 服务端在 core/src/kernel/，能力面实现在 crates/plugin
  Plugin Supervisor (签名/拉起/退避/熔断/回收/降权/沙箱) ← 核心③ 插件体系（crates/plugin）
  docker (bollard/三层策略/路径校验/Compose/exec/日志) ← 核心② Docker 管理（crates/docker）
        │ Unix Socket /run/skiff/ + JSON-RPC 2.0
插件进程 (宿主机原生, 语言受支持矩阵约束, 按需启动, 空闲 300s 回收)  ← 核心③ 第三方侧（crates/sdk 出包）
```

## 3.2 请求路径（重启容器）

`POST /api/v1/docker/containers/{id}/restart` → 中间件（会话/安全入口）→ handler 内 `policy.check` + 审计 → `Docker::execute` → `bollard` → Engine API → `202` → htmx 局部替换状态徽章。

## 3.3 内核原子能力（MVP）

| 能力 | 内容 |
|---|---|
| `docker` | 容器/镜像/网络/卷/Compose/日志流/exec 终端（白名单见 §6） |
| `identity` | 用户/会话/TOTP/API Token/RBAC |
| `fs` | 路径白名单内文件读/写/列表/元数据（白名单 = 插件自身目录 + 用户授权目录，Broker 逐次校验） |
| `net` | 主机防火墙端口规则（放行/拒绝；nftables 适配，探测 firewalld/ufw；按 UID owner-match 收口插件出网） |

> 内核之外一律插件。这是硬约束。文件管理器/防火墙管理/网站管理/主机监控等是**插件层产物**（能力面已开放，`requires` 点单），内核不内置任何一项。官方示范插件（Nginx 管理）走 `docker` 能力；fs/net 类官方插件排在 MVP 之后。
