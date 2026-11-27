# 验收结论：当前轮与最近两轮（§8）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §8 最近三轮 + 历史轮次台账（其余轮次已精简）（2026-11-16 整理）。

---

## 8. 最近一次验收结论（截至 2026-11-27 13:10：还原后干净首装 + 一次就地升级）

**2026-11-27 干净首装 + 就地升级（当前包 `3fbf8e2` / 12,552,580 B / `baac26fb…`）：三组回归 106·32·20 全绿，浏览器 44 路由 0 FAIL**

- **前置与安装（通过）**：测试机还原为干净系统（无 `/etc/skiff`、无 `/www/skiff`、18080 空闲、Docker 29.7.2 在位、时钟已同步）。preflight **7 PASS / 1 SKIP**（PF3 需 `--now`）→ `sh install.sh --listen 0.0.0.0:18080 --tls off --no-start` exit **0** → 追加 `allow_insecure_http`、`allow_reduced_sandbox`、`[catalog]` → `systemctl start skiff`。`skiff.service` active、`/healthz` 200、前缀 **`gu9uhtu3`**、`skiff doctor` 全 ok、`official/` 7 包 + `public-key.txt`。
- **升级（通过）**：`cda4553` → `3fbf8e2` 一次 `sh install.sh --no-start`（不传 `--listen/--tls`），exit 0、`/etc/skiff/config.toml` **逐字节相同**（`sha256=cc50e3b2…`）、前缀不变、7 插件与 2 容器全保留、`/healthz` 200。
- **能力面（通过）**：7 个官方插件（cron / firewall / ftp / guard / monitor / sources / testkit）全部「已验签」，各自独立 UID 运行、`firewall.status` / `sources.status` / `guard.status` / `cron.jobs.list` RPC 均返回结果；`php-8.5`（`php:8.5-fpm-alpine`，`8.5.13`）running；OpenResty 已装；三站点 `matrix.local`（PHP）/ `matrix-static.local`（静态）/ `matrix-proxy.local`（反代，上游 18081，`X-Upstream: 18081` 透传）均 200。
- **回归（通过）**：`round.sh` **106**、`round_ext.sh` **32**、`round_ext2.sh` **20**，全 0 FAIL；`accept.sh` preflight+A+D0 **34 PASS / 0 FAIL / 3 SKIP**，B 组（`--group b --password`）**15 PASS / 0 FAIL / 1 SKIP**（SKIP 为未开限流 / 装后阶段端口断言 / 首装口令文件已消耗）。
- **浏览器（通过）**：44 条路由（菜单枚举 + 站点详情子视图 + 插件功能页）**44 PASS / 0 FAIL**，0 CompileError / 0 pageerror；`/sites/runtimes/none` 作为负向用例断言「运行环境不存在：none」落地页。
- **本批修掉的缺陷**：站点生命周期误用「重启容器」——增删别名、建站、站点启停、默认站切换、删站都调 `restart_nginx()`，实测加别名后首个请求 `Failed to connect`、`StartedAt` 每次变化，**其它站点**也在窗口内不可服务。改为 `activate_nginx()`（reload 优先、回退重启）；建站补 `nginx -t`；别名写入-校验-生效收进同一把锁。复验：上述五类动作全程 `StartedAt` 不变、其它站点首个请求 200。
- **已知行为（非缺陷）**：reload 生效为异步收敛（0.2–0.5 s），期间旧 worker 按旧配置应答（新别名 404、停站仍 200），用例按「2 秒内收敛」判；保存 PHP 运行参数 = 重启 PHP 容器（同环境站点约 3.2 秒 503）；`proxy_target` 必须带 `http://`；CLI 装插件后 manifest 任务要到下次启动才种入。
- **未做**：aarch64 仍未装机实测（只有 qemu 冒烟）；MIRROR-08/09 篡改与异钥反例、防火墙 fail2ban M 组本轮未重跑（上一轮已闭环）；nginx **主配置**保存仍走重启（本轮有意不动，见 `docs/changes/2026-11-27.md` 四）。

**2026-11-26 干净首装 + 三次就地升级（当前包 `a915b6e` / 12,542,260 B / `c038e313…`）：三组回归 106·30·20 全绿，浏览器 44 路由 0 FAIL**

- **前置与安装（通过）**：测试机还原为干净系统（无 `/etc/skiff`、无 `/www/skiff`、18080 空闲、Docker 29.7.2 在位）。preflight **8/8 PASS** → `sh install.sh --listen 0.0.0.0:18080 --tls off --no-start` exit **0** → 追加 `allow_insecure_http` 与 `[catalog]` → `systemctl restart skiff`。`skiff.service` enabled+active、`/healthz` 200、前缀 **`5e3yz2yr`**、`skiff doctor` 六项 ok、`official/` 7 包 + `public-key.txt`。
- **升级（通过）**：`070ff1d` → `7810675` → `a915b6e` 三次都是 `sh install.sh --no-start`（不传 `--listen/--tls`），exit 0、`/etc/skiff/config.toml` **逐字节相同**、前缀不变、7 插件与容器全保留、`/healthz` 200。
- **能力面（通过）**：7 个官方插件（cron / firewall / ftp / guard / monitor / sources / testkit）全部「已验签」；`php-8.5`（`php:8.5.13-fpm-alpine`）running；OpenResty 已装；三站点 `matrix.local`（PHP）/ `matrix-static.local`（静态）/ `matrix-proxy.local`（反代，上游 18081）均 200，PHP 页 `PHP Version 8.5.13`。
- **回归（通过）**：`accept.sh` **47 PASS / 0 FAIL / 5 SKIP**（SKIP 全部是本次调用不适用：未传 `--now` / `--install-exit`、装后阶段端口占用、已改密机器无 `initial-admin.txt`、未开限流）；`round.sh` **106**、`round_ext.sh` **30**、`round_ext2.sh` **20**，全 0 FAIL。
- **浏览器（通过）**：44 条路由（菜单枚举 + 站点详情子视图 + 插件功能页）**44 PASS / 0 FAIL**，0 CompileError；`/sites/runtimes/none` 作为负向用例断言「运行环境不存在：none」落地页。
- **回收站（通过，Playwright 实点）**：删除 → 页面 0 error → 点「恢复」→ 条目退出回收站且文件回原路径；构造「目标已存在」→ 点「恢复」→ 弹覆盖确认 → 覆盖成功；另以 API 验「彻底删除」条目与丢弃目录一并消失。
- **自建镜像（通过）**：三类清单 `source=registry`、`failed=false`（PHP 13 / Node 9 / Rust 1）。中途开发机 `serve` 随会话退出导致 `Connection refused`，测试机按设计降级（PHP→`builtin`、Node/Rust→`catalog`），重启后 `?refresh=true` 回到 `registry`——降级与恢复都符合预期。
- **本批修掉的缺陷**：① 面板只改 `gzip on/off` 时不提交 `gzip_types`，内核渲出 `gzip_types ;`，`nginx -t` 失败并回滚保存；② 回收站「恢复」把壳对象当动作数组元素，整页 `CompileError`；③ PHP 配置保存文案写「重载 FPM」而实际重启容器（约 3.2 秒 503），且真实错误页被套上「该功能正在适配中」。
- **已知行为（非缺陷）**：保存 PHP 运行参数 = 重启容器 → 同环境站点约 3.2 秒 503（采样 150 次、非 200 共 29 次），已在页面提示写明；`crates/docker/src/client.rs` 的自定义信号白名单决定不做 `USR2` 热重载（改动属策略决策，见 [TEST_LEDGER.md](../TEST_LEDGER.md)）。
- **镜像侧（通过）**：MIRROR-08 篡改（等长改版本号）→「校验失败：索引摘要与 manifest 不符」；MIRROR-08 异钥 →「验签失败: 清单 key_id 与固定公钥不匹配」；两次都 `can_install=false` 且保留上次成功快照。MIRROR-09 `runtimes --channel stable` 4.3 秒、全「未变」、内置副本无变化。临时 8081/8082 服务与临时 dist 已清理，客户端配置按备份原样恢复。
- **未做**：aarch64 仍未装机实测（只有 qemu 冒烟）；`com.example.hello` 占位包（镜像里故意放的那个）依旧按设计被拒装；长时间观测类（闲置 RSS / 并发登录）不进每轮。

**2026-11-25 20:55 就地升级到 `a0f27c8` 包（12,538,544 B / `2d0e29b7…`）：MIRROR-01~09 全部通过，含面板侧三类清单**

- **前置**：11-24 后半到 11-25 的积压改动先提交（`7918bac` / `7e3146f`），CI 一跑露出 9 项 `main_flow` 过期断言（写操作改走 `writePost`、`routes` 新增 `data.subviews`、C12 删 `handlers::files::page`、页头主操作迁 `page.content.actions`、配额测试落在受保护目录）。修完（`f194751` + `a0f27c8`）CI run `45616188710` **全绿**。
- **升级**：`7e3146f` 包 → `a0f27c8` 包两跳都是 `sh install.sh --no-start`（不传 `--listen/--tls`），exit 0、`config.toml` 与升级前逐字节相同、`official/` 7 包、前缀 `8au3xewy`、`/healthz` 200。
- **清单（通过）**：三类 `/ui/v1/runtimes/versions` 均 `source=registry`、`failed=false`、`generated_at=2026-11-25T10:07:00Z`；PHP 13 / Node 9 / Rust 1，每分支 3 条；弹窗三个下拉共 23 项、无「已 EOL」、`8.6.0（最新）`；提示行给出 `http://10.66.52.7:8080/channels/stable/runtime-index/{php,node,rust}.json` 固定路径。
- **篡改与伪造（通过）**：改 `index.json` 一字节（保持长度）→「校验失败：索引摘要与 manifest 不符：声明 2921ccf4…，实得 48f24d21…」；换异钥产物 →「验签失败: 清单 key_id 与固定公钥不匹配（清单 c0d75726…，公钥 707cede3…）」。两次都 `can_install=false` 且保留上次成功快照。
- **本批修掉的缺陷**：① 清单 `eol` 是日期，面板按「非空即 EOL」解析，导致 13 个 PHP 版本全标「已 EOL」（含刚发的 `8.6.0`）；② `skiff-server <子命令> --help` 报「参数 --help 缺少取值」。
- **未做**：`accept.sh` 全量、浏览器巡检、`sync` 到独立镜像站、清单签名（二期）。
## 历史轮次台账（2026-11-16 精简）

> 下表只留「日期 + 版本 + 轮次标题」，每轮完整的逐项断言与实测数字已随本轮整理删除；需要复盘时看当轮 `docs/changes/YYYY-MM-DD.md`、对应提交与 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标。
> 现行口径：只保留最近三轮完整结论（上文），旧轮次不再作为判定依据。

- 2026-11-17 00:06–00:35 `2847580` 资产就地覆盖安装 + 全组验收：Docker 409 语义修正上机确认，0 FAIL（preflight+A 29 PASS、认证 14 PASS、D0 4 PASS）
- 2026-11-16 23:22–23:55 `280345e` 全新安装 + 定向回归：安装/认证/授权/审计通过，0 FAIL（accept.sh preflight+A 32 PASS、B 组 13 PASS）
- 2026-11-15 23:04–23:57 `v0.1.0` 全新安装 + 全量回归：本机可执行批次全部通过
- 2026-11-15 22:33–23:05 `704b890` 站点网络模型验收：八项全部通过
- 2026-11-15 21:12–21:30 `e5df734` 站点网络模型验收：第 1 项失败停手（专用网络与宿主 docker0 网段冲突），第 7 项被动复现通过
- 2026-11-15 15:35–15:52 `3b5c433` Node 端口冲突 / crash-loop 复验：五项全部通过
- 2026-11-15 14:43–14:47 `fb7412d` PHP / Node 站点全链路验收：PHP 与 Node 正常路径通过，第 3 项 Node 端口冲突失败
- 2026-11-15 13:47–14:05 `fb7412d`（镜像源预算/取消还原、Node 健康 20s、现网 `nginx -t` 10s）验收：四项全部通过
- 2026-11-15 12:34–13:10 `d025258`（内核健壮性 C 批：临时单元超时清理 / 审计兜底 / Docker 重启超时与就绪预算 / `nginx -t` 统一通道）验收：第 1、2、4、5 项通过，第 3 项不通过
- 2026-11-15 08:10–08:35 `7946e93`（界面预设/三层配色 + 列表页内弹窗新建）验收：全部通过
- 2026-11-15 01:58–02:18 `7eeb0fd`（文件管理「改属主」请求体类型修复）验收：五项全部通过
- 2026-11-15 01:20–01:35 `194d6a1`（Docker 资源动作与设置保存键分离）验收：第 3 项失败停手
- 2026-11-15 00:39–00:50 `53b06bf`（P0/P1/P2 交互改造 + budding r5 + Web 安装进度持久化）验收：第 2 项失败停手
- 2026-11-14 19:26–19:55 `56e5790`（主题 CSS 变量 + 菜单精简 + Web 服务安装/运维）验收
- 2026-11-14 16:09–16:18 `10deadf`（查询态页内跳转重渲染 + 列表场景空态）验收
- 2026-11-14 14:39–14:47 `008ecff`（budding 1.8.0-r4 + 概览/容器列表精修）验收
- 2026-11-14 08:30–08:33 `16cbe9a`（作业日志路径按 `Accept` 分流）验收
- 2026-11-14 07:46–07:51 `c535343`（budding 1.8.0-r3 三处缺陷修复）验收
- 2026-11-13 21:37–22:05 `91813f1`（budding 1.8.0-r3）验收
- 2026-11-13 20:55–21:06 `1abcf4d` 四项修复验收
- 2026-11-13 20:11–20:20 files 原生页与概览系统状态卡补验（`3d00543`）
- 2026-11-13 19:53–20:00 插件功能页原生化验收（`4da21cd`）
- 2026-11-13 20:15–20:40 转发层 10 s 超时修复与复验（18081 → 18080）
- 2026-11-13 18:55–20:10 compose down 断连诊断 + 商店真实安装链路（镜像站恢复）
- 2026-11-13 18:20–19:00 写操作验收（Docker / Compose / 运行环境 / 商店 / 文件 / Web 页，`b7771c3`）
- 2026-11-13 14:46–16:05 终端 / 审计 / 日志验收（`4c1a1aa`）
- 2026-11-13 09:52–10:00 复验 `281dc9d`（上轮两项不通过）
- 2026-11-13 09:18–09:30 admin 外壳 + 插件标签修复验收（22 页 + 外壳专项，`d06bdab`）
- 2026-11-13 08:17–08:25 budding 迁移页浏览器巡检（22 页，`51914a6`）
- 2026-11-12 15:44–15:58 商店抓取静默丢包超时修复（`7e322a3`）
- 2026-11-11 21:38–21:48 R0 发布面 API 端到端验收（`189d581`，`tools/skiff-server`）
- 2026-11-11 23:03–23:13 R1 撤包 / 吊销 / 审计查询验收（`7d4a9b4`，`tools/skiff-server`）
- 2026-11-11 23:07–23:18 R1 修复复验（`f3b6f89`）
- 2026-11-11 21:20–21:30 宿主 `pure-ftpd` / `fail2ban` 补装与 FTP / fail2ban 用例复验（未重装 Skiff，库/配置未变）
- 2026-11-11 21:10–21:18 测试机快照还原后全量重建验收（形态三）
- 2026-11-12 08:41–08:48 §15 维护路径 + 站点文件 0.1.3 + R2a/b/c/d 发布面（`13a054c`）
- 2026-11-12 09:25–09:41 R2e 分 kind 撤包 + 维护任务失败文案（`87a636e`）
- 2026-11-12 10:31–11:05 商店安装选择器（`085bc18`）
- 2026-11-12 12:47–13:10 SDUI 解耦八条上机回归（`0eeabbe`）
- 2026-11-12 14:58–15:25 商店目录 TTL 与降级（`a988085`）
