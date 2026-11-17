# 验收结论：当前轮与最近两轮（§8）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §8 最近三轮 + 历史轮次台账（其余轮次已精简）（2026-11-16 整理）。

---

## 8. 最近一次验收结论（截至 2026-11-17 13:00 `ab70234` Release 资产就地覆盖安装 + 三级菜单验收）

> 2026-11-16 07:53 的原地重装（budding `1.8.0-r7`）是叠加复验，未另起轮次；其结论记在 [02-DEPLOY_STATE.md](02-DEPLOY_STATE.md) §5 的「验收」项。

> 本节只保留验收断言、实测数字与限制，是「测试机上发生了什么」的唯一来源；各轮的代码改动、根因与提交链见 [changes/2026-11-10.md](../../changes/2026-11-10.md)、[changes/2026-11-09.md](../../changes/2026-11-09.md)、[changes/2026-11-15.md](../../changes/2026-11-15.md)，用例编号与预期结果见 [TEST_MATRIX.md](../TEST_MATRIX.md)，未闭环项见 [TEST_LEDGER.md](../TEST_LEDGER.md)。

**2026-11-17 13:00–13:05 `ab70234` 资产就地覆盖安装 + 三级菜单验收：横向菜单与站点/运行环境子视图已上机生效，0 FAIL**

- **安装（通过，升级路径）**：资产 12,144,387 B `8c53a1eb…5580`（本地 sha256 / 侧车 / `checksums.txt` 三方一致，测试机 `sha256sum -c` OK）→ `sh install.sh --no-docker --tls off --listen 127.0.0.1:18080` **exit 0**。新二进制 19,851,024 B `73a34d77…`；库 `user_version=21`、`integrity_check=ok` 未变；配置 sha `5bfb00d9…16af8` 未变；权限 `711/711` 保持；前缀 `cb5mgdk7` 未变；7 个官方插件仍在；`/healthz` 18080 与 18081 均 200。
- **三级菜单（D17 修正口径）**：内容区顶部不再有路径条/面包屑（`.bud-admin__crumbs` 与 `.skiff-path` 均已不存在），改为页面容器内、标题之下的横向菜单；7 页真实部署截图断言如下（1440×900，无 console error）——
  - `/sites/web`：`状态 / 配置 / 日志 / 模块`，当前项「状态」，页面标题「Web 服务」。
  - `/sites/a.com`：`概览 / 域名 / 配置 / 证书 / 备份 / 日志`，当前项「概览」；`/sites/a.com/ssl` 当前项「证书」且只渲染证书块。
  - `/runtimes/php-8.5/config`：`基础信息 / 配置 / 扩展 / 站点 / 管理 / 日志`，当前项「配置」，只渲染运行参数与配置文件。
  - `/plugins/com.skiff.monitor/history`：`总览 / 历史采样 / 进程 / 网络`，当前项「历史采样」——插件多视图由 `/ui/v1/menu` 的 `views` 驱动，页内按钮行已不再出现。
  - `/sites`、`/plugins/com.skiff.files/main`：单视图/列表页**不出现**三级菜单（整条不渲染，不占位）。
  - 样式：菜单底色与内容区一致（`--v-theme-surface`），不随侧栏米色；当前项主色文字 + 2px 底部强调轨。
- **侧栏高亮**：`/plugins/com.skiff.files/main` 只高亮「网站 › 站点文件」，上一轮记录的「站点文件 + 插件 › 已安装」双高亮现象**已消失**（插件路由 `activeMenu` 改指插件自身地址）。
- **未覆盖（限制）**：accept.sh 全组本轮未复跑——服务端接口无变更（改动集中在 `budding-boot.js` / `skiff-shell.css` / `spa.rs` 壳页样式版本 / files 插件 schema 标题），沿用 2026-11-17 00:06 轮全组结论（29+14+4 PASS，0 FAIL）。`com.skiff.testkit` 的视图仍是 HTML/iframe，壳内点其子视图会回落到旧 SSR 工作台（该页自带横向 L3 标签栏），原生渲染需给它补随包 schema，未在本轮范围。
- **现场**：站点 `a.com`（php-8.5 运行中）、`skiff-web-nginx` running；`/root/iter/` 新增 `rel-ab70234/`。

**2026-11-17 08:14–08:20 `fc43f3c` 资产就地覆盖安装 + UI 定向验收：壳层 chrome（D17）与四页精修已上机生效，0 FAIL**

- **安装（通过，升级路径）**：资产 12,143,038 B `fd782a0…cd7f`（本地 sha256 / 侧车 / `checksums.txt` / API digest 四方一致，测试机 `sha256sum -c` OK）→ `sh install.sh --no-docker --tls off --listen 127.0.0.1:18080` **exit 0**。新二进制 19,855,120 B `2477dcd9…8b212`；库 `user_version=21`、`integrity_check=ok` 未变；配置 sha `5bfb00d9…16af8` 未变；权限 `711/711/600/711/660` 保持；前缀 `cb5mgdk7` 未变；7 个官方插件仍全部 `installed + enabled`；`/healthz` 18080 与 18081 均 200。
- **UI 定向验收（浏览器 8 页截图，全部通过）**：真实部署资源直连截图（`admin` / `SkiffPass2026x` 登录，1440×900，无 console error）——
  - **壳层 chrome（D17 核心）**：右侧顶部通栏（品牌/账号/退出登录）已消失；内容区顶部内核三级路径条生效（`网站 › 网站列表`、`网站 › Web 服务 › 状态/配置/日志/模块` 各级带下拉切同级）；页面大标题去重生效。
  - **网站列表**：工具条（共 N 站 + 批量启停/新建站点）在表格上方，行操作「详情 / 启动/停止」，时间本地化，无「没有更多了」浮字。
  - **运行环境**：工具条（共 N 个 + 新建运行环境），行操作「详情 / 日志」，镜像参数表单正常。
  - **Web 服务四子视图**：状态（运行状态/安装进度/容器运维，安装分步日志逐行显示）、配置（主配置/性能参数）、日志（筛选表单 + 读取范围/状态码分布）、模块（nginx -V）各自独立成页，互不混入。
  - **站点文件**（files 新 schema 随包生效）：工具行（刷新；返回上级按条件隐藏）、子目录/文件两卡片、站点根目录选择器。
  - **设置**：首卡「当前账号」显示 `admin（admin）` + 「退出登录」按钮 + 提示文案（承接原顶栏账号入口）。
- **未覆盖（限制）**：accept.sh 全组本轮未复跑——服务端接口无变更（本轮改动集中在 `budding-boot.js` / `skiff-shell.css` / `spa.rs` 静态资产与 files 插件 schema），沿用 2026-11-17 00:06 轮全组结论（29+14+4 PASS，0 FAIL）；B4 限流、TOTP、FTP/防火墙等同沿用历史结论。观察项 1 个：files 页左侧菜单「网站 › 站点文件」与「插件 › 已安装」双高亮（menuTrail 匹配问题，非本轮引入，见 changes 记录）。
- **现场**：站点 `a.com`（php-8.5 运行中，上轮 UI 迭代验收所建）、`skiff-web-nginx` running（openresty/1.31.8.1，安装完成 05:55:03）；`/root/iter/` 保留 `rel-fd782a0/`、`accept.sh`、`admin-pw.txt`、`skiff-http-fwd.py`。

**2026-11-17 00:06–00:35 `2847580` 资产就地覆盖安装 + 全组验收：Docker 409 语义修正已上机确认，0 FAIL**

- **安装（通过，升级路径）**：安装前基线 `0d82bc95…581b` / `0.1.0` / `config.toml` 备份到 `/root/iter/config-before-2847580.toml` → 资产 12,130,261 B `f90676b4…6894`（本地 sha256 / 侧车 / `checksums.txt` / API digest 四方一致，测试机 `sha256sum -c` OK）→ `sh install.sh --no-docker --tls off --listen 127.0.0.1:18080` **exit 0**。新二进制 19,822,352 B `5e4aa297…f2b5`；配置与备份 **`diff` 无差异**；库 `user_version=21`、`integrity_check=ok`；权限 `711/711/600/711/660` 全部保持；前缀 `cb5mgdk7` **未变**；7 个官方插件仍全部 `installed + enabled`；`/healthz` 18080 与 18081 均 200。
- **accept.sh preflight + A（通过）**：**29 PASS / 0 FAIL / 3 SKIP**（`--install-exit 0 --pkg-dir …`）。含 A4a `/healthz` 200、A5b/c 未登录 303 跳登录、A5d 裸 `/` 404、A6a–g 权限位、A9a/b 重启自恢复且前缀不变、A10a `kill -9` 后 systemd 2 s 拉起、A11 `--help`/`version` 不落文件、A8a/b `doctor` 无 ERROR。SKIP：PF3（未传 `--now`）、PF4（装后阶段不适用）、A6h（`initial-admin.txt` 已按设计删除）。
- **认证（通过，脚本补 `--password` 后）**：首跑整组 `SKIP B0`——覆盖安装后 `initial-admin.txt` 已被产品删除且口令链只认该文件；给 `accept.sh` 加 `--password` 后复跑 **14 PASS / 0 FAIL / 1 SKIP**（B4 未开 `--with-rate-limit`）：B1/B1a 登录页与 `csrf_token`(len=119)、B2b/B2b1 错误口令 401「用户名或口令不正确」、B2/B2a 登录 200 且 `redirect_to=/cb5mgdk7`、B5 错 CSRF 403、B3a 设置数据 200、B3c 改密 200、B3d `initial-admin.txt` 已删、B3e 旧会话 303 失效、B3f 新会话 200。改法见 [changes/2026-11-16.md](../../changes/2026-11-16.md)。
- **D0（通过）**：`--group d0 --pkg-dir …` → **4 PASS / 0 FAIL / 0 SKIP**：`docker 29.7.2` 可用、`docker-bootstrap.sh --check-only` 退出 0 且 `daemon.json` 摘要前后一致（`8a26aea5…`）、已装不重装。
- **本轮改动定向回归（重点：Docker 409）**：
  - **409 语义（通过，本轮核心）**：`POST /ui/v1/networks {"name":"regress409-net"}` 首次 **200** `{"created":true}`；同名第二次 **409** `{"error":{"code":"domain.conflict","status":409,"message":"network with name regress409-net already exists",…}}`——状态码、错误码、文案三者一致，`retryable=false`，不再出现「文案说已存在、错误码说 `shape.invalid`」的混合态。测后删除该网络，`docker network ls` 仅 `bridge/host/none/skiff-sites`。
  - **卷重名**：`POST /ui/v1/volumes` 同名二次为 **200**（命令层先查后建、幂等），不是 409 分支——两处语义差异属设计，不另报缺陷；测后删除，`docker volume ls` 为空。
  - **读面冒烟（通过）**：`/ui/v1/{sites,runtimes,web,containers,plugins,tasks,jobs,audit,menu,files,networks,images,volumes,settings,session}` 全 **200**；壳页 `/{p}/{sites,settings,plugins,containers,tasks}`（`Accept: text/html`）全 **200**，`/{p}/` 带尾斜杠 307 归一到 `/{p}`（axum 常规行为）。
- **口令处置**：B3c 把 admin 口令轮换为未知值后，按「停服 → 清 `sessions`/`users` → 启服由 bootstrap 重建 admin 并写 `initial-admin.txt` → AUTH-07 改回」恢复为 `SkiffPass2026x`，并重写 `/root/iter/admin-pw.txt`（14 B / 0600）；改密成功后产品自动删除 `initial-admin.txt`（已确认）。重建后的 admin 仍 `role=admin`、`user_version=21` 不变。
- **未覆盖（限制）**：插件能力面（`skiff_docker` 经 kernel socket 回 409 给插件）与容器名冲突分支未单独构造——两者与网络创建共用 `cmd_err` / `api_err`，由 `skiff-docker` 与 `skiff-app` 单测覆盖（`cmd_err_routes_409_to_conflict_and_rest_to_bad_request` 等）；B4 限流、TOTP、建站/删站、FTP/防火墙、文件配额与回收站沿用 2026-11-15 结论，本轮未复跑。
- **清理**：`regress409-net` 网络、`regress409-vol` 卷已删；`/root/iter/` 保留 `rel-2847580/`、`accept.sh`、`regress409.sh`、`admin-pw.txt`、`skiff-http-fwd.py`、`config-before-2847580.toml`。

## 历史轮次台账（2026-11-16 精简）

> 下表只留「日期 + 版本 + 轮次标题」，每轮完整的逐项断言与实测数字已随本轮整理删除；需要复盘时看当轮 `docs/changes/YYYY-MM-DD.md`、对应提交与 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标。
> 现行口径：只保留最近三轮完整结论（上文），旧轮次不再作为判定依据。

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
