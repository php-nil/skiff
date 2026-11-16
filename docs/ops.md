# 工程环境与流水线索引

> 正文在 [`ops/`](ops/)。

## 门槛架构（共性）

- 链路：**本地三关**（`fmt` → `clippy` → `test`）→ CI 全量（`ci.yml`）→ 发布物（`release.yml`）；`plugins/` 是第二 workspace，三关单独跑（`--manifest-path plugins/Cargo.toml`）。
- **命令与判定口径的唯一来源是 [CI.md](ops/CI.md) §2、§6**；根 `AGENTS.md` 的「常用命令」只是速查，不另立标准。
- 编译与 git 写操作共用全局串行令牌（拿不到即以退出码 2 拒绝，不排队）；退出码 2/3/4 语义、内存准入与本机编译限制见 [DEV_ENV.md](ops/DEV_ENV.md)。

## 子文档

| 子文档 | 管什么 | 什么时候读 |
| --- | --- | --- |
| [CI.md](ops/CI.md) | 四个 workflow、门槛命令、发布产物流向 | 出包 / CI 红 / 发版前 |
| [DEV_ENV.md](ops/DEV_ENV.md) | 闸门链路、退出码、内存准入、`PATH` 包装、本机编译限制 | 编译报错 / 换机器 / 退出码非 0 |
