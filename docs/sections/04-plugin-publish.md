# 4. 扩展体系（二）发布与部署

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§4.4–4.5 ｜ 上一章：[§4 扩展体系（一）](04-plugin-package.md) ｜ 下一章：[§5 部署](05-deploy.md)

## 4.4 第三方如何发布

**本期只定交付物，不定渠道**：第三方发布物是**签名后的 `.wbpkg`**——可以走 GitHub Release / 自建下载页直接分发，将来若出现插件商店（自建或第三方），同一交付物直接上架，无需返工。**渠道是未决策项**（§1.4），不在本期范围。

配套工具链（`crates/sdk` 内的 `skiff-plugin` CLI，独立 semver 发布）：

```
skiff-plugin new my-plugin                    # 脚手架：manifest + src + ui 骨架（自带开发密钥）
skiff-plugin build --dir my-plugin            # 本机架构出 bin/<arch>/plugin；交叉编译先自行 --target
skiff-plugin pack --dir my-plugin --arch amd64
                                              # 哈希清单 → 私钥签名（包内原始字节）→ my-plugin-0.1.0.wbpkg
                                              # --arch 指定要校验的载荷（交叉打包必需）
skiff-plugin verify ./my-plugin-0.1.0.wbpkg   # 结构/签名/manifest 自检（装前自查），同样支持 --arch
skiff-plugin stamp --dir my-plugin --key k    # 用私钥派生的指纹回填 manifest.key_id
```

- **`stamp` 的用途**：源码里的 `key_id` 只是占位，打包前用发布私钥回填指纹，让「仓库不含任何一把密钥的指纹」与「`pack` 的指纹一致性校验」同时成立。官方插件走这条（`plugins/pack.sh` 在**临时目录**回填，不弄脏工作树）。
- **SDK 独立 semver**（与内核版本解耦），窄接口以 `skiff-sdk` 的公开 API 为准，只声明兼容的插件 API 区间；破坏性变更升 API 主版本，内核并行兼容 **N-1 API 主版本**。
- **官方示范插件是 `plugins/` 下的 6 个**（`guard` / `monitor` / `files` / `cron` / `ftp` / `firewall`，见 [`plugins/README.md`](../../plugins/README.md)）：脚手架、签名、打包、权限声明、分页读写、cron tick、interop provides 全链路可对照抄。原 `nginx` 插件已内核化为「网站」，不再作为插件示范；`guard` 走的是「`containers.restart` + `fs`」，**没有**用 `containers.exec`（exec 属 M3 门禁）；目标容器仍须由 admin 显式授予 `container:<NAME>` grant，插件不能凭标签自行认领。
- 对外发布（无论什么渠道）必须附两样东西：**权限清单** + **签名指纹**（供用户导入公钥时核对）。
- 多架构是发布者的责任：`bin/` 下缺哪个架构，该架构上就拒装（不静默降级）。
- 发布包自带官方插件：`deploy/pack-release.sh --plugin-key <私钥>` 会把 7 个已签名 `.wbpkg`（6 个官方插件 + testkit）放进 `official/`，不给私钥且未 `--skip-plugins` 时拒绝出包（[`docs/CI.md`](../CI.md) §4）。

## 4.5 用户侧如何部署

**UI（主路径）**：插件页 → 上传 `.wbpkg` → 内核校验（结构 / 签名 / `api` 兼容 / `requires` 能力可用）→ **权限确认对话框**（权限逐条列出，可部分拒绝；拒绝项从 granted 集剔除并以 `SKIFF_GRANTED_PERMS` 注入启动环境，插件可查询以便降级适配）→ 安装 → 启用。对话框明示：**权限确认防误用不防恶意**——防恶意靠签名信任 + 降权沙箱，装插件 = 信任作者。**全程不出网，离线可用**。

**装好之后怎么用**：每个已启用且有 `[[ui]]` 的插件会以自己的 `label` 出现在主界面顶栏（插件菜单），点一下直接进插件界面，不必每次经「插件」列表页中转；顶栏当前项高亮。插件界面之间也能互跳：插件发父页面消息 `skiff.open`（如 NGINX 站点页的「在文件管理里打开该目录」），面板按白名单只改地址，定位路径能否读仍由目标插件自己的 `path:` 授权根判定（口径见 [§4.2「UI 托管 / 插件间跳转」](04-plugin-package.md)）。

**CLI（批量 / 无人值守）**：服务器本地执行 `skiff plugin install ./foo.wbpkg`，校验流程与 UI 完全一致；`--yes` 跳过交互确认，但权限**默认全拒**，需显式 `--grant` 指定允许的 docker / identity / fs / net 动词；管理既有 Docker 对象还须重复指定 `--resource <type:id>`，默认不含任何外部对象。

**升级与恢复**：
1. 上传新版本，完成签名、同源密钥、API 与权限差异校验；低于或等于已装版本默认拒绝，只有 admin 的灾难恢复入口可装旧版。
2. 停止旧进程并暂停该插件调度，按 §5.2 做一致性数据快照；`/www/skiff/plugins/{id}/versions/` 保留 N-1 版。
3. 内核以**新版插件的独立 UID + 降级沙箱**启动 migration 模式（landlock/cgroups 未实现），将 `migrations/` 作为输入，调用带截止时间的 `migrate(from_version, to_version)` RPC；**内核从不以 root 直接执行包内迁移脚本**。迁移须幂等，只允许修改本插件 `data/`；对外部 Docker/网络资源的副作用必须由插件自行记录补偿步骤。
4. 迁移成功才原子切换 active 版本并启用。迁移或健康检查失败则终止插件进程（cgroup 进程树清理未实现）、恢复升级前数据快照并切回旧代码；若旧代码与数据 schema 不兼容，则保持插件停用并提示执行备份恢复，**不伪报“已自动回滚”**。外部资源补偿失败单独告警并保留审计记录。

**卸载**：停进程 + 删版本目录；`cache/`/`run/` 即删；插件数据目录（`/www/skiff/plugins/{id}/data`）默认保留，勾选“同时删除数据”才清。
