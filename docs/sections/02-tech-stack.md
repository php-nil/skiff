# 2. 技术选型

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§2 ｜ 上一章：[§1 需求评估](01-requirements.md) ｜ 下一章：[§3 总体架构](03-architecture.md)

## 2.1 后端依赖

| 用途 | crate | 备注 |
|---|---|---|
| 运行时 | `tokio` | **显式配置** `worker_threads=2`（tokio 默认 = CPU 数） |
| Web | `axum` + `tower-http` | `tower-http` 只挂 `CatchPanicLayer`（panic → 500 + 审计）；gzip 是**构建期**由 `build.rs` 生成 `.gz` 变体（`flate2`），不走中间件 |
| 模板 | `askama` | 编译期，零运行时解析 |
| 存储 | `rusqlite`(bundled) | 静态 SQLite，约 +1MB |
| 密码 | `argon2` | argon2id(m=64MiB,t=3,p=4)（RFC 9106 第二推荐参数） |
| Docker | `bollard` | 走 socket，不 fork CLI；exec 终端走 hijack 流 |
| TLS | `rustls`(ring) + `axum-server` | feature `tls`；R1 例外清单见 §1.1。证书签发尚未引入 `rcgen` |
| DNS | `hickory-dns` | **未引入**（按需方案）；musl 下出现解析异常时再评估 |
| 插件 IPC | `hyper` over `UnixStream` + 自研 JSON-RPC | 不引 `tonic`；帧与路由常量在 `skiff-wire`，两端共用 |
| 插件签名 | `ed25519-dalek` + `sha2` | 包完整性与信任链（§4.3） |
| 插件包编解码 | `tar` + `zstd`（`zstd-sys`） | `.wbpkg`；R1 第三项 C 例外，双架构构建需纳入交叉编译验证 |
| 插件沙箱 | `libc`（setuid/setgroups/setrlimit） | 降权与 RLIMIT 已实现；**`landlock` 未实现**，隔离主力是「独立 UID + 目录权限 + 附加组」（§12、§8） |
| 防火墙 | netlink（nftables/iptables 适配） | **未实现**：`net` 能力 RPC 返回 501（§12） |
| 静态资源 | `rust-embed` | 前端打进二进制，`.gz` 变体由 `build.rs` 预生成 |

## 2.2 前端

`askama` + `htmx`（约 14KB gzip）+ 按需 `import()` 的 `xterm.js`（容器 exec 终端）。无 Node 构建链，第三方资源 vendored，`build.rs` 做 gzip 预压缩 + embed。

## 2.3 扩展机制选型

| 方案 | 隔离 | 常驻 | 语言 | 结论 |
|---|---|---|---|---|
| dlopen | ❌ 段错误带走主进程 | 0 | 仅 Rust | 排除 |
| wasmtime | ✅ | +20~50MB | 多 | 与 R2 冲突，排除 |
| **进程外 + UDS** | ✅ 崩溃隔离，权限经 Broker | **0（按需）** | 任意（运行时受支持矩阵约束） | **选定** |

进程外是唯一既保崩溃隔离又不背常驻内存的路线；代价是 IPC 与打包复杂度，由 §4 收口。「语言任意」指 IPC 层无关；实际可跑的插件运行时受沙箱 RLIMIT_AS 约束，支持矩阵以 M4 实测为准（§4.2）。
