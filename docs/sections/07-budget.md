# 7. 资源预算

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§7 ｜ 上一章：[§6 安全](06-security.md) ｜ 下一章：[§8 工程结构](08-engineering.md)

| 项 | 目标 | 备注 |
|---|---|---|
| 二进制 | ≤ 14MB | `lto=fat`, `codegen-units=1`, `strip`, `opt-level=z` |
| 镜像 | ≤ 25MB | 仅可选 Docker 形态（alpine + musl） |
| 空闲 RSS | ≤ 25MB | tokio 2 worker + SQLite 页缓存 4MB |
| 单浏览会话 | ≤ 40MB | |
| 空闲 CPU | < 0.3% | |
| 冷启动 | < 200ms | |
| 首屏 | ≤ 100KB gzip | 不含按需 xterm |
| 插件常驻 | 0 | 空闲 300s 回收（`resident` 插件单独计入） |

手法：静态资源预 gzip；日志流/终端走 SSE/WS 不轮询。默认 `panic="unwind"`：`tower-http` 的 `CatchPanicLayer` 挂在路由最外层，handler panic → 错误页 500 + `web.panic` 审计，进程不死；`panic="abort"` 可再省体积但失去该性质，不采纳。默认不引 mimalloc，实测有瓶颈再加。

**登录峰值口径（必须声明，否则 M5 回填会误判）**：argon2id m=64MiB ⇒ 单次登录瞬时内存约 70MB+，远超空闲 RSS 目标，属预期。登录并发上限默认 2（配置项）；§7 上表全部指标按**闲置态**采样，登录峰值单独记录，不并入 p95。

> 全部为设计目标。M5 用 systemd cgroup 采样 30 分钟出 p50/p95 回填，偏差 > 30% 复审架构。
