# 4. 扩展体系（二）发布与部署

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§4.4–4.5 ｜ 上一章：[§4 扩展体系（一）](04-plugin-package.md) ｜ 下一章：[§5 部署](05-deploy.md)

## 4.4 第三方如何发布

**本期只定交付物，不定渠道**：第三方发布物是**签名后的 `.wbpkg`**——可以走 GitHub Release / 自建下载页直接分发，将来若出现插件商店（自建或第三方），同一交付物直接上架，无需返工。**渠道是未决策项**（§1.4），不在本期范围。

配套工具链（`crates/sdk` 内的 `skiff-plugin` CLI，独立 semver 发布）：

```
skiff-plugin new my-plugin                  # 脚手架：manifest + src + ui 骨架
skiff-plugin build --arch amd64,arm64       # 交叉编译出 bin/{arch}/plugin
skiff-plugin pack                           # 哈希清单 → 私钥签名（包内原始字节）→ my-plugin-0.1.0.wbpkg
skiff-plugin verify ./my-plugin-0.1.0.wbpkg # 结构/签名/manifest 自检（装前自查）
```

- **SDK 独立 semver**（与内核版本解耦），窄接口文档 `docs/PLUGIN_API.md` 只声明兼容的插件 API 区间；破坏性变更升 API 主版本，内核并行兼容 **N-1 API 主版本**。
- **官方示范插件（Nginx 管理，经 docker exec 操作目标容器）即范本**：脚手架、签名、打包、权限声明全链路可对照抄；目标容器须由 admin 显式授予对象 grant 与 exec 动词，不能凭插件标签自行认领。
- 对外发布（无论什么渠道）必须附两样东西：**权限清单** + **签名指纹**（供用户导入公钥时核对）。
- 多架构是发布者的责任：`bin/` 下缺哪个架构，该架构上就拒装（不静默降级）。

## 4.5 用户侧如何部署

**UI（主路径）**：插件页 → 上传 `.wbpkg` → 内核校验（结构 / 签名 / `api` 兼容 / `requires` 能力可用）→ **权限确认对话框**（权限逐条列出，可部分拒绝；拒绝项从 granted 集剔除并以 `SKIFF_GRANTED_PERMS` 注入启动环境，插件可查询以便降级适配）→ 安装 → 启用。对话框明示：**权限确认防误用不防恶意**——防恶意靠签名信任 + 降权沙箱，装插件 = 信任作者。**全程不出网，离线可用**。

**CLI（批量 / 无人值守）**：服务器本地执行 `skiff plugin install ./foo.wbpkg`，校验流程与 UI 完全一致；`--yes` 跳过交互确认，但权限**默认全拒**，需显式 `--grant` 指定允许的 docker / identity / fs / net 动词；管理既有 Docker 对象还须重复指定 `--resource <type:id>`，默认不含任何外部对象。

**升级与恢复**：
1. 上传新版本，完成签名、同源密钥、API 与权限差异校验；低于或等于已装版本默认拒绝，只有 admin 的灾难恢复入口可装旧版。
2. 停止旧进程并暂停该插件调度，按 §5.2 做一致性数据快照；`/var/lib/skiff/plugins/{id}/versions/` 保留 N-1 版。
3. 内核以**新版插件的独立 UID + 完整沙箱**启动 migration 模式，将 `migrations/` 作为输入，调用带截止时间的 `migrate(from_version, to_version)` RPC；**内核从不以 root 直接执行包内迁移脚本**。迁移须幂等，只允许修改本插件 `data/`；对外部 Docker/网络资源的副作用必须由插件自行记录补偿步骤。
4. 迁移成功才原子切换 active 版本并启用。迁移或健康检查失败则终止整个插件 cgroup、恢复升级前数据快照并切回旧代码；若旧代码与数据 schema 不兼容，则保持插件停用并提示执行备份恢复，**不伪报“已自动回滚”**。外部资源补偿失败单独告警并保留审计记录。

**卸载**：停进程 + 删版本目录；`cache/`/`run/` 即删；插件数据目录（`/var/lib/skiff/plugins/{id}/data`）默认保留，勾选“同时删除数据”才清。
