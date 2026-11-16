# 插件与分发索引

> 正文在 [`plugin/`](plugin/)；插件工程（第二 workspace、开发/签名/打包）在 [`../plugins/README.md`](../plugins/README.md)。

## 体系概览（共性）

- **官方插件集**：6 个官方插件 + testkit（guard / monitor / files / cron / firewall / ftp），随发布包 `official/` 交付、不自动安装；id、构建与打包约定见 `plugins/README.md`。
- **信任链**：`.wbpkg` = manifest + 载荷 + Ed25519 签名；首次导入公钥走 TOFU 并展示指纹，权限逐条确认（设计正文见 [DESIGN.md](DESIGN.md) §4）。
- **分发**：商店清单（`index.json` + blobs）由 `skiff-server` 生成、签名、发布；`check`（清单合规）与 `serve`（静态分发）是唯一实现，文档只解释字段含义与发布流程。
- 插件 UI 交付方向是 Schema 原生渲染（见 [SDUI_BUDDING_INTEGRATION.md](sdui/SDUI_BUDDING_INTEGRATION.md)）；iframe 形态只对旧 SSR 页有效。

## 子文档

| 子文档 | 管什么 |
| --- | --- |
| [PLUGIN_CATALOG.md](plugin/PLUGIN_CATALOG.md) | 官方插件 6 个 + testkit、开箱即用交付线（分 3 册） |
| [SKIFF_SERVER.md](plugin/SKIFF_SERVER.md) | 商店清单模型、Ed25519 签名与 `seq` 防回滚、`serve` / `api` 发布面（分 9 册） |
