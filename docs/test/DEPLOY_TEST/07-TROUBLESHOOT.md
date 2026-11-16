# 故障排查与未实现清单（§7–8）

> 上级索引：[DEPLOY_TEST.md](../DEPLOY_TEST.md) ｜ 拆自 `docs/test/DEPLOY_TEST.md` 原 §7–8（2026-11-16 整理，正文未改）。

---

## 7. 故障排查

| 现象 | 可能原因 | 处置 |
|---|---|---|
| 启动即退出，日志提示“非 loopback 且未配置 TLS” | 监听 `0.0.0.0` 但没配 TLS | 配 `tls_cert/tls_key`，或改 loopback + 反代 + `trusted_proxies` |
| 启动即退出，提示 `SchemaTooNew` | 换了旧版本二进制但库已迁移 | 用备份回滚数据库（§2.6） |
| 页面 404，但服务是活着的 | 没带安全入口前缀 | 用日志里的 `/xxxxxxxx/` 前缀 |
| 登录一直 401 | 初始口令已改、或 `initial-admin.txt` 被手动删 | 用 `skiff token create` + API，或重装并保留数据后由 admin 重置 |
| 容器页显示“Docker 不可用” | socket 不存在 / 权限不足 / 非 Linux | 查 `skiff doctor`；确认 unit 里 root 运行 |
| **插件装上了，一启动就停用，插件侧报 EACCES** | 数据根/运行目录缺 other-x（被改成 0700），或 `kernel.sock` 属组不是 `skiff-plugins` | 按 §2.10 查；数据根应为 0711，socket 应为 `660 root:skiff-plugins` |
| **插件日志报“UID 未映射到任何插件”** | 插件进程 uid 与 `sha256(id)%1000+200000` 不一致（手工 chown 过目录、或换了用户跑） | 别手工改插件目录属主；重装插件恢复 |
| **重启后插件突然写不进自己的 `run/`** | `clean_run_dirs` 重建目录后属主本应归还，若仍失败说明不是 root 起的服务 | 确认 `systemctl` 里是 `User=root`；或 `ls -ld .../run` 看属主 |
| **装包报“未导入公钥”** | 信任公钥没导入 | 面板「设置 → 信任公钥」导入 key_id + 公钥 hex |
| **装包报“已安装版本高于包内版本”** | 包版本低于已装版本 | 改 manifest 的 `version`；同版本可重装，降级需先卸载（保留 data）再装 |
| **`pack` 报“指纹不一致”** | `--key` 用的私钥与 manifest.key_id 不是一对 | 用 `new` 生成的那把 `<dir>/skiff-signing.key`，或改 manifest.key_id |
| **`skiff-plugin new` 后 `build` 找不到 skiff-sdk** | SDK 未发布到 crates.io，脚手架写成了版本依赖 | 加 `--sdk-path <仓库>/crates/sdk` 重新生成，或手改 Cargo.toml 为 path 依赖 |
| **编译报 `x86_64-linux-gnu-gcc not found`** | 缺 C 工具链 | `apt install build-essential pkg-config` |
| **systemd 起不来，手跑却正常** | `ExecStart` 路径与实际安装前缀不一致（用了 `--prefix`），或 `ProtectSystem=full` 让某路径只读 | `systemctl cat skiff` 核对路径；需要写 `/etc` 的功能改到 `/var/lib` 或调 unit |
| 反代后 Cookie 不生效 | scheme 判定为非 HTTPS，`__Host-` 前缀被浏览器拒收 | 确认 `trusted_proxies` 命中且反代传 `X-Forwarded-Proto: https` |
| 表单提交 403 | CSRF 令牌与当前会话不匹配（多标签/会话续期） | 刷新页面重取令牌 |

## 8. 未实现 / 未验证清单（不得当事实引用）

| 项 | 现状 | 影响 | 对应 §12 |
|---|---|---|---|
| **插件安装后的代码目录可执行位** | **已修并上机复验**：后续 7 个官方插件长期 enabled、可正常拉起（见 [TEST_ENV/02-DEPLOY_STATE.md](../TEST_ENV/02-DEPLOY_STATE.md) §5） | 修前官方插件装完即 `Permission denied`（`versions/{ver}` 0700、`bin/plugin` 0644） | §4.2 |
| **Docker 容器生命周期 / 资源限制 / Compose 运行时** | **已实现并上机**：容器/镜像/网络/卷/Compose 的创建、启停、删除入口已上线；资源限制与 Compose 运行时细节以 [TEST_MATRIX.md](../TEST_MATRIX.md) H 章为准 | 不得把未覆盖的边界说成「全量已验证」 | §5.3、§12 |
| **HSTS 响应头** | **未实现**：`crates/` 内无 `Strict-Transport-Security` 下发，实测响应头也没有 | 浏览器首次仍可被诱导到同端口明文地址（该地址不返回页面）；不得声称已启用 HSTS | §12 |
| landlock 隔离 | **未实现** | 插件隔离主力是“独立 UID + 目录权限 + 附加组” | §12 待实测清单 |
| cgroups v2 限额（内存/PID/CPU） | **未实现** | 只有 `setrlimit`（AS 256MB / NOFILE 256 / NPROC / FSIZE / CORE） | §12 |
| nftables 出网收口（net 能力） | **未实现**，kernel RPC 返回 501 | 插件出网不受控；`net` 能力插件不可用 | §12 待实测清单 |
| project quota | **未实现** | 磁盘只有软限额 | §12 |
| **bind 挂载的 TOCTOU 窗口** | `canonicalize` 校验；`open_dir_nofollow` 已实现但**未接入** | 校验与 Docker 实际挂载之间有窗口（D8 会通过）。**不得声称已消除** | §5.5 |
| **Linux 目标的完整编译** | **已由 CI 覆盖**：`test` 与 musl 构建 job 在 Linux 全量编译（含 `plugins/`）；Windows 本地仍只跑无 C 依赖子集 | 以 CI 结果为准，本机不再承担 Linux 全量类型检查 | §12 |
| **备份的一致性** | `skiff backup` 不停插件：库 `VACUUM INTO` 一致，插件 `data/` 直接复制可能抓到写一半的文件 | 需要强一致时先停服再备（§2.5） | §12 |
| **插件 UID 只有 1000 个槽位** | 哈希取模；撞车**装前拒绝**，但没有换槽/动态分配 | 装到几十个插件后可能出现「装不上」 | §4.2 |
| 运行时支持矩阵（Go/JVM/.NET 在 AS 256MB 下） | **未实测** | 决定哪些运行时能跑插件 | §12 |
| 全部资源数字（RSS/体积/CPU） | **设计目标** | M5 回填 | §7、§12 |
| 备份恢复演练 | **未做** | “备份成功”不等于可恢复 | §12 |
| 与 1Panel/宝塔共存实测 | **未做** | 仅设计上不冲突 | §12 |
| **Linux 发行包的二进制体积** | **未实测** | 本机只产出 Windows PE（6.65 MB）；Linux x86_64 的实测体积与 `≤14MB` 目标的对照要在目标机上补 | §7、§12 |
| 发布包（Linux）实跑 | **已装机实测**：当前测试机运行 `7e322a3` 的 Release 覆盖包（见 [TEST_ENV/02-DEPLOY_STATE.md](../TEST_ENV/02-DEPLOY_STATE.md) §5）；打包链路同时由 CI 覆盖 | 以 [TEST_ENV.md](../TEST_ENV.md) 的部署记录为准 | §5.1 |
| 多架构发布包（arm64） | CI 已出包，**装机未实测** | `CARGO=cross sh deploy/pack-release.sh --target aarch64-...` 已在 run 41270406760 跑通；目标机实跑待补 | §12 |
| 1Panel 商店上架 | 包结构就绪，**未上架** | `xtask check-1panel` 只校验变量对齐 | §5.4 |
| 插件配置的管理界面 | **未实现**（`config_schema` 渲染表单没有） | 只能用 `skiff plugin config <id> <key> <value>` 写；插件侧 `Plugin::config()` 与内核 `capability/config` 路由已通 | §5.2 |

**写文档/对外宣讲时的口径**：上表任意一项都不能说成“已支持”。要么写“已实现”，要么写“设计目标/未实现”。
