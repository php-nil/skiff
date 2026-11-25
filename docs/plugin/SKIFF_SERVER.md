# Skiff-server：中心化静态数据分发方案（索引）

> 状态：**静态分发已实施；发布面 R0–R2e 已实施、默认关闭** ｜ 日期：2026-11-09（最近核对 2026-11-13）
> 定位：官方维护的**中心化数据源**，最终打包成一份**静态资产目录**，供任何 Skiff 实例拉取或自行镜像——插件商店清单、Docker 应用模板、运行环境版本清单、内核升级元数据。
> 关联：[§1.4 分层边界](../sections/01-requirements.md)、[§4.1 包格式与信任](../sections/04-plugin-package.md)、[§4.4–4.5 发布与部署](../sections/04-plugin-publish.md)、[PLUGIN_CATALOG.md](PLUGIN_CATALOG.md)、[PLUGIN_UI_CONTRACT.md](../contract/PLUGIN_UI_CONTRACT.md)、[CI.md](../ops/CI.md)、[DEPLOY_TEST.md](../test/DEPLOY_TEST.md)、[SITE_RUNTIME_CONTRACT.md](../contract/SITE_RUNTIME_CONTRACT.md)。
> 本文只定协议、流程与阶段，不含实现代码；具体规模、频率与超时数字未实测，均标注为「设计目标 / 待实测」。
> **本文件是索引**：正文按 §编号拆在 [`SKIFF_SERVER/`](SKIFF_SERVER/)，编号沿用拆分前原文，跨册引用直接按编号跳转。

## 一页结论

- **数据分离**：商店清单（`index.json` + `schema/` + `blobs/`）由 `skiff-server` 生成并签名发布，Skiff 客户端只读；**客户端不依赖发布面 API**。
- **版本清单是例外**：`channels/<channel>/runtime-index/<kind>.json`（运行环境可选版本，`php` / `node` / `rust` 各一份）由同一个 `build` 产出但**不签名**、也不是客户端硬依赖——面板按固定路径取，取不到就用内置副本（PHP）或目录大版本；它只解决「可选版本不随面板发版滞后」，不进签名链。
- **身份即公钥**：作者钥签包、审核钥签请求、发布钥只在受控环境；`seq` 只增，撤包打 `yanked` / `revoked` 标记而非删条目。
- **落地状态**：P0–P3、发布面 R0 / R1 / R2a–R2e 均已实施（`tools/skiff-server`）；托管来源已定案（自建静态 + 镜像同步）。
- **校验口径**：`check`（清单合规）与 `serve`（静态分发）是唯一实现，本文只解释字段与流程。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–3 | [01-DESIGN.md](SKIFF_SERVER/01-DESIGN.md) | 一页结论、目标与非目标、数据模型与目录结构、清单格式与契约 |
| §4–5 | [02-DISTRIBUTE.md](SKIFF_SERVER/02-DISTRIBUTE.md) | 分发与镜像、发布流程 |
| §6–8 | [03-CLIENT_SECURITY.md](SKIFF_SERVER/03-CLIENT_SECURITY.md) | Skiff 客户端接入、安全、分阶段推进 |
| §9–10 | [04-ROLLOUT.md](SKIFF_SERVER/04-ROLLOUT.md) | 待定与建议、P0 落地状态 |
| §11–13 | [05-PHASES.md](SKIFF_SERVER/05-PHASES.md) | P1 客户端只读接入、P2 商店目录安装、P3 内核升级只读段 |
| §14–15 | [06-RELEASE_MIRROR.md](SKIFF_SERVER/06-RELEASE_MIRROR.md) | 签名发布与镜像同步、商店目录托管来源评估（已定案） |
| §16 | [07-SERVE.md](SKIFF_SERVER/07-SERVE.md) | 静态服务契约 `skiff-server serve`（TLS / ETag / 访问日志） |
| §17.1–17.8 | [08-API.md](SKIFF_SERVER/08-API.md) | 发布面 `/api/**`：角色与密钥归属、提交侧、审核侧、产物生成、最小集与不做项 |
| §17.9–17.15 | [09-API-ROLLOUT.md](SKIFF_SERVER/09-API-ROLLOUT.md) | R0–R2e 逐段落地说明 |

## 维护约定

- 清单字段的**唯一事实来源**是 `tools/skiff-server` 的 `check` 实现；本文与实现冲突时，先改实现再改本文，或就地登记差异。
- 落地说明只写「与设计稿差在哪、为什么」，不复述设计正文。
