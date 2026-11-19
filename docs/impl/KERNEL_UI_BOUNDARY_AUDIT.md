# 内核业务层 ↔ UI 层边界审计（SSR + `/ui/v1` · 未闭环）

> 状态：只读审计（2026-11-14）；**批次 0（拆 `ui_api.rs` → `handlers/ui/*` 及 0.2–0.8 各项）已落地**（见 `docs/changes/2026-11-14.md`）。本文件只保留**未闭环问题、待决策批次与未验证项**；已处理条目与拆分方案已删（原文行号对应拆分前代码，仅存 git 历史）。
> 范围：`crates/web`（SSR handler + `/ui/v1` JSON handler）、`crates/app` 命令层、`crates/plugin`/`crates/pkgfmt` 的插件 UI 边界、`crates/web/static` 的壳页与静态资源。
> 相关契约：[DESIGN.md](../DESIGN.md)（分层与依赖方向）、[URL_CONTRACT.md](../contract/URL_CONTRACT.md)、[SDUI_BUDDING_INTEGRATION.md](../sdui/SDUI_BUDDING_INTEGRATION.md)、[PLUGIN_UI_CONTRACT.md](../contract/PLUGIN_UI_CONTRACT.md)、[MENU_CONTRACT.md](../contract/MENU_CONTRACT.md)。

## 1. 依赖方向现状

**crate 级依赖不违规**：`web` 允许依赖 `app/model/store/identity/docker/plugin/pkgfmt`（`docs/sections/08-engineering.md`）。越层只发生在调用面，审计时发现的两处「绕过命令层」（`/api/v1` 重启直调 `Docker::execute`、`/ui/v1` 文件写直调 `AdminFs`）已在批次 0.6 / 0.5 收口；web 内无手写 SQL。

## 2. 未闭环问题

> 每条只留结论与影响；现象级证据见 git 历史（2026-11-14 审计原稿）。

1. **（S1，行装配已处理 2026-11-19）JSON DTO 直接依赖 SSR 展示模型**：`/ui/v1` 曾以 `templates::RuntimeRow` 等展示结构为输入；现已改为消费 `app/src/views/*` 读模型，模板与 DTO 各自映射。读路径编排仍见条目 2。
2. **（S1）读路径两套编排**：站点列表、运行环境详情 SSR 与 `/ui/v1` 各写一遍查询/聚合；写路径已复用命令层。→ 批次 1.2。
3. **（S2）审计口径缺口**：TOTP 启用 JSON 写审计、SSR 不写；公钥操作未记录真实来源 ip。→ 批次 2.3。
4. **（S2）RPC 错误靠字符串猜内核状态码**：`PluginError` 仅 `Rejected` 映射 400、其余 500，与 RPC 路径字符串解析出的 503 语义不对称。→ 批次 1.3。
5. **（S2）DTO 冻结没有编译期/测试期约束**：字段冻结靠用例与文档约定。→ 批次 1.1。
6. **（S3）同一日志查询在 SSR 与 JSON 的参数名不同**（`part`/`tail`/`q` vs 表单名）。→ 收尾时统一或在契约里写明映射。
7. **（S3）schema 运行期校验失败返回 500**：应为 400/409 + 稳定错误码。→ 批次 2.1。
8. **（S2，2026-11-19）`SiteError → AppError` 映射 4 份且口径不一**：`handlers/sites.rs::map_err`（`Field(_,inner)`→Internal）、`ui/mod.rs::site_write_err`（→BadRequest）、`ui/mod.rs::site_api_error`（递归 inner）、`jobs.rs::retry` 内联（→Internal）——同一领域错误三条 HTTP 结论。→ 批次 2.4（先定 `Field` 错误的正确语义，再在 `error.rs` 收一个 `From`）。
9. **（S3，2026-11-19）web 内小重复**：插件确认页装配两份（`plugins.rs` vs `store.rs`）、公钥导入两份（`settings.rs` vs `ui/settings.rs`）、运行环境健康计数两份（`dashboard.rs` vs `ui/overview.rs`）、`ui/containers.rs` 手写与 `ui/mod.rs::paginate` 逐行相同的 keyset 算法、`LoginBody` 两份、运行环境安装文案两份。→ 批次 2.4 顺带。
10. **（S3，2026-11-19）视图层残留判定**：`store.rs::load_view` 内联「已装版本高于目录可装版本则拒绝降级」（后端同判两处）→ 移 `skiff_app::catalog` 读模型；`labels.rs` 自述「唯一映射」但状态中文散在 `ui/mod.rs`/`sites.rs`/`jobs.rs`（补收或改自述）。

## 3. 待决策批次（需要先定契约）

| 批次 | 目标 | 需要定的契约 |
| --- | --- | --- |
| 1.1 | DTO 冻结机制（golden JSON / key-set 快照 / 可选 JSON Schema） | 冻结粒度、新增字段是否必须升 `api_version` |
| 1.2 | 抽 `app` 单一读取入口：站点列表查询、运行环境详情聚合、settings 账户安全命令 | `skiff-app` 是否接受「读模型」职责；跨 crate API 命名 |
| 1.3 | RPC 错误结构化（code + status） | `skiff-plugin`/`skiff-wire` 协议面变更 |
| 2.1 | 错误码细分：schema 校验失败改 400/409 并给稳定码 | 与商店安装侧统一 |
| 2.3 | 审计来源补全（TOTP、公钥操作真实 ip） | 审计内容变化需在 TEST_MATRIX 补断言 |
| 2.4 | 错误映射与小重复收敛（未闭环 8/9/10）：`error.rs` 收 `From<SiteError>`、确认页/公钥导入/健康计数合单 | `Field` 错误的 HTTP 语义（400 vs 500）；合单后文案归属 |

## 4. 未验证项

1. **budding pack 漏文件时的前端行为**：`budding-boot.js` 有 `try/catch`，实际是静默降级还是控制台报错，未实测。
2. **`KERNEL_API` 版本门覆盖面**：任务门与商店安装走 `api_compatible`；直接 `.wbpkg` 上传安装没有 API 版本校验，是否有启动期兜底未验证。
3. **两套站点列表是否存在排序差异**：SSR 依赖 `list_sites` 返回顺序，JSON 按 `(created_at, id)` keyset；当前应一致，未写测试锁定。
