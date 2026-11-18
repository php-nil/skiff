# 目录落地、实施顺序与风险（§5–7）

> 上级索引：[PLUGIN_CATALOG.md](../PLUGIN_CATALOG.md) ｜ 拆自 `docs/plugin/PLUGIN_CATALOG.md` 原 §5–7（2026-11-16 整理，正文未改）。

---

## 5. 目录落地：放在哪里

### 5.1 方案对比

| 方案 | 优点 | 缺点 | 结论 |
|---|---|---|---|
| 顶层 `plugins/`（独立 workspace） | 插件是独立发布物；与内核边界清晰；CI 可单独跑；第三方可整目录对照 | 需要第二 workspace、独立 CI job 与独立 lock/target | **已采用（2026-11-01）** |
| `crates/plugins/*`（根 workspace 成员） | 复用现有三关 | 破坏「`crates/` = 内核 crate」的边界；插件测试/构建被内核发布节奏绑定 | 不推荐 |
| 独立仓库 `skiff-plugins` | 发版与 CI 最干净，社区贡献隔离 | SDK 未发布到 crates.io，跨仓库同步 SDK 变更成本高 | 中期 |
| `deploy/plugins/` | 无额外结构 | `deploy/` 只放部署物料，不含源码 | 不可 |

### 5.2 实际结构

```
plugins/
├── Cargo.toml              # 第二 workspace：members = common/guard/monitor/files/cron/firewall/ftp/testkit
├── Cargo.lock              # 必须入库（CI 与 pack.sh 都用 --locked）
├── pack.sh                 # 官方插件打包：build → stamp → pack → verify → official/
├── README.md               # 官方插件集约定（id、构建自检、打包、装机授权、多架构）
├── common/                 # skiff-plugin-common：共享小工具（unwrap_result / path / 状态读写 / base64）
├── guard/                  # com.skiff.guard
│   ├── Cargo.toml          # name = skiff-plugin-guard，[[bin]] name = "plugin"
│   ├── manifest.toml       # key_id = 64 个 0 的占位（打包时回填）
│   ├── src/main.rs
│   └── ui/                 # containers.html / events.html + app.js（只经 postMessage）
├── monitor/                # com.skiff.monitor
├── files/                  # com.skiff.files
├── cron/                   # com.skiff.cron（provides jobs.create/list/remove/run；consumes *）
├── firewall/               # com.skiff.firewall（firewall 能力面 + provides fail2ban.*）
├── ftp/                    # com.skiff.ftp（consumes com.skiff.firewall:fail2ban.*，可选登录防护）
└── testkit/                # com.skiff.testkit（架构验收：多视图/单视图/槽位/RPC/能力面）
```

约定：

- crate 名 `skiff-plugin-<x>`；包内可执行名固定 `plugin`（`skiff-plugin pack` 按 `bin/{arch}/plugin` 收）。
- id 用 `com.skiff.<x>`；版本独立 semver；`api = "1.0"`，内核并行兼容 N-1 主版本。
- 插件 UI 只经父页面 postMessage 桥取数（`skiff.rpc` / `skiff.rpc.result`），不携带面板凭据、不发同源请求；`[[ui]].label` 直接进主界面顶栏菜单，插件间切换用面板级消息 `skiff.open`（只改地址，不授予能力）。
- **第三方前端库必须 vendor 进 `ui/`**：插件 UI 的 CSP 是 `script-src 'self'; connect-src 'none'`，CDN 与内联脚本都加载不了（例：`plugins/files/ui/vendor/` 里的 highlight.js 11.9.0 + LICENSE）。
- **仓库里不含任何密钥的指纹**：manifest `key_id` 是 64 个 0 的占位，打包时才用 `skiff-plugin stamp --key` 在**临时目录**回填；私钥只在 CI secret 或离线发布机。

### 5.3 构建、签名与发布（已实现）

- 根 `Cargo.toml` 增加 `exclude = ["plugins"]`，显式声明第二 workspace（已完成）。
- 每个插件 crate 直接 `skiff-sdk = { path = "../../crates/sdk" }`；不设 `workspace.dependencies`（各有依赖，收敛收益不大）。
- `plugins/pack.sh`：`build`（`CARGO`/`--target` 交叉）→ 复制 `manifest.toml` + `ui/` + `migrations/` + `bin/<arch>/plugin` 到临时目录 → `stamp` 回填指纹 → `pack --arch` → `verify --arch` 自检 → 输出 `com.skiff.<name>-<version>.wbpkg` 与 `public-key.txt`。
- CI 只加**一个** job `plugins`（fmt / clippy `-D warnings` / test，只跑插件 workspace）；真打包冒烟放在 `pkg-chain`（临时密钥按 `pack.sh` 的 `PLUGINS` 清单打全量包并逐个验签）。
- 发布：`deploy/pack-release.sh --plugin-key <私钥>` 调 `pack.sh`，产物塞进发布包 `official/`；不给私钥且未 `--skip-plugins` 时**拒绝出包**（详见 [`docs/ops/CI.md`](../../ops/CI.md) §4）。
- 交叉打包需要 `skiff-plugin pack/verify --arch`（否则按打包机架构找载荷，arm64 包在 amd64 机上会被误判缺载荷）。
- 多架构是发布者责任：缺哪个架构，该架构上就拒装（§4.4）。

### 5.4 开箱即用的三条交付线（1/3 已落地）

1. **内置官方公钥**：命中内置指纹时显示「官方」徽章并免去手动导入（权限确认仍逐条）。**当前仍是缺口**：信任库只有 TOFU，发布包只带 `official/public-key.txt`，需要管理员手动导入。
2. **随包携带（已落地）**：发布包内有 `official/`（7 个已签名 `.wbpkg` + `public-key.txt`，含 testkit）；`install.sh` 复制到 `/www/skiff/official/`（0755），**不自动安装**。
3. **面板「官方插件」列表 + 一键安装**：从本地官方源读取、离线可用、安装仍走权限确认对话框。**未做**（现在只能 `skiff plugin install` 或面板上传）。

> 只把 `.wbpkg` 放 GitHub Release、让用户手动上传，不算「开箱即用」——第 3 条是下一步。

## 6. 实施顺序（含进度）

| 阶段 | 内容 | 状态 |
|---|---|---|
| 0 决策与目录 | 建 `plugins/` 第二 workspace + 三关门禁 | ✅ 2026-11-01（`exclude`、CI `plugins` job、`plugins/Cargo.lock`） |
| 1 能力补齐 | fs 宿主授权（`path:` grant、`resolve_data`/`resolve` 双分支、manifests 的 `resource_hints`、Broker 复验）；cron 运行时；interop 契约（manifest `[interop]`、`plugin:` 对象授权、`capability/interop/{call,catalog}`、审计） | ✅ fs / cron / interop 就绪；❌ docker exec、内置官方公钥仍未做 |
| 2 首批插件 | guard → monitor → files → cron → ftp → firewall（nginx 已内核化） | ✅ 代码 + UI，已编译验证（2026-11-10）；单测：guard 3 / monitor 13 / files 2 / cron 8 / ftp 3 / firewall 5 / common 7 |
| 3 分发 | 官方源目录 + 随包携带 | ✅ 随包携带（`official/` + `public-key.txt`）；❌ 面板内官方插件列表 |
| 4 第二批 | 备份与恢复、应用模板、数据库 | 未开始 |

> 官方插件（清单见 `plugins/pack.sh` 的 `PLUGINS`，含 testkit）的**真机验收**（安装、权限确认、启用、UI、RPC、越权、插件间调用、卸载）见 [TEST_LEDGER.md](../../test/TEST_LEDGER.md) 未闭环项；FTP 主链路已上机通过，卸载回收仍待复验；防火墙与 monitor 进程/网络本轮未上机。

> 能力补齐与插件开发并行时，先冻结 manifest 字段与协议契约再写插件，否则三份插件要返工。

## 7. 风险与未决

| 风险 | 说明 | 处理 |
|---|---|---|
| exec 白名单形同虚设 | 透传任意命令等于把容器 root 交给插件 | 命令模板 + 参数校验 + 输出/超时/并发上限 + 审计 |
| fs 授权越权 | 路径穿越、符号链接、TOCTOU | 已做：授权根目录粒度（`path:` grant）+ 插件侧预检 + 内核侧复验 + 目标存在时按 `canonicalize` 重判 + 越权 403 并审计；未做：全程 `O_NOFOLLOW` 持有句柄（判定与具体 IO 之间仍有窗口） |
| 大文件传输 | 插件 UI 单帧 1MiB 且 `connect-src 'none'` | 现状：256KiB 分页够文本编辑，**没有**二进制上传/下载；需要新增受限流式传输契约（未排期） |
| 资源预算 | 每个插件是独立进程 | 采集频率与保留点数设下限；数字待实测，不写承诺 |
| 信任与分发 | 无内置官方公钥、渠道未决策 | 已做随包携带（`official/` + `public-key.txt`）；内置公钥与面板官方列表未做；不动商店 |
| 插件间调用 | 一个插件拉着另一个插件跑，失败会顺链传播 | 四道闸 + 禁止自调 + busy 集合单层防环 + 15s 超时 + 逐次审计 `interop.call`；目标被停用时 409 |
| 插件 UI 静态资源 | opaque origin 下相对 `fetch` 会失败（§12） | UI 只经 postMessage，数据文件随包自包含 |

未决：商店/分发渠道（§1.4）；监控是否改 `resident`；文件管理器默认写权限还是默认只读；计划任务是否支持「按秒」触发（当前 5 段表达式最小粒度 1 分钟）。
