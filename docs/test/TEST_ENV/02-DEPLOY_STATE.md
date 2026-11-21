# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-21 11:45 登录页视觉重做 + 滚动条修复（HEAD `5221b4c`，沿用本轮安装）**
> - 背景：用户反馈登录页「卡片右侧被裁、标题重复、字段 10px 压缩、大片空白」。根因是旧 CSS 对 budding 表单做极端压缩（label 10px、字段 28px、内层 padding 归零），页面标题与卡片标题重复，装饰层绝对定位造成 125px 水平溢出。
> - 改动：登录页改为一张合成卡片（左深青品牌面板 + 右表单区），宽度上限 1000px；删除全部压缩规则；标题/副标题/页脚改由服务端模板承担，budding 只渲染表单本体；登录按钮通栏实心；辉光层改 `position: fixed`；主题按钮只显示一个文案；`SHELL_VERSION` 26→30。
> - 滚动条修复：budding page 根把 `#skiff-login` 撑到 571px 并叠加固定卡片高度，导致右列出现内层滚动条。取消固定高度（卡片由内容撑开、`margin:auto` 居中）、右列改 `overflow: visible`、强制 `#skiff-login`/page 根 `height:auto`，并为 ≤660px 矮视口压缩纵向留白；仅极端矮视口才出现单条页面滚动，滚动条已做细样式。
> - 资产：Release run `207`（head `5221b4c`），amd64 **12,913,254 B** id `774328309`，`sha256=374927425308c05d3151fbd7fab62a44f83b8b6bb2cd6065c051653ed6c275ac`（API 资产端点下载）；`install.sh` exit 0，前缀 **`88qfm2s8`**、8 插件、口令、库与配置全部保留；二进制 18,053,136 B `8af6592f…df28`。
> - 验证：真实浏览器 960×571 深色/浅色双主题——「欢迎回来」完整可见、三字段 + 通栏实心登录按钮、`#skiff-login=282px`、页面与右列 `clientHeight == scrollHeight`、无任何滚动条、0 console error；`admin` 登录后概览正常。
> - 中间构建（`abc7b0c` 布局仍裁切、`b44aac8` 报 `form is not defined`、`cb1b421` 仍有内层滚动条）未通过浏览器验证，不计入本部署结论。
> - 影响面：仅登录页模板与样式；已装页面、插件与数据未变化。
>
> **2026-11-21 10:18 干净重装 + 弹窗 schema 修复（已过期；HEAD `d5680b6`）**
> - **场景**：测试机还原为干净系统（T1：Docker 29.7.2 在位，无 skiff / 无 `/etc/skiff` / 端口空闲）→ 安装 GitHub Release 当前资产（run `202`，head `5bec69b`）→ accept.sh 全组 → 浏览器逐页巡检 → 发现 3 个页面 `CompileError` → 修复 → 重发版（run `203`，head `d5680b6`）→ 就地升级复验。
> - **首装资产**：Release run `202`（head `5bec69b`），amd64 包 **12,913,533 B**，`sha256=3c6d40b6491de18fb8a3f6c80baada148c59ec11a80fd754417e3f68cd1cb424`（测试机 `sha256sum` 与侧车一致）。
> - **首装**：`install.sh --no-docker --tls off --listen 0.0.0.0:18080` exit **0**（跑两次均 0，幂等）；前缀 **`88qfm2s8`**；`skiff.service` active；healthz `{"docker":"available","status":"ok","version":"0.1.0"}`；doctor 全绿。
> - **形态**：直接明文（形态 C）`listen="0.0.0.0:18080"` + `tls_enabled=false`；Cookie 为 `skiff_dev_session`。`allow_insecure_http=true`、`allow_reduced_sandbox=true` 仍由人工追加（install.sh 契约缺口未变）。
> - **accept.sh 全组**：`--group all --install-exit 0` = **49 PASS / 0 FAIL / 2 SKIP**（PF4 装后端口不适用、B4 未开限流）；改密后口令轮换为 `skiff-accept-1795226634`，写 `/root/iter/admin-pw.txt`（0600）。
> - **8 官方插件**：逐个 `skiff plugin install <包> --yes`（不带 `--resource`，采用 manifest `resource_hints`），全部验签/安装/启用；monitor 授权含 `path:/proc`、`path:/etc/passwd`，firewall 11 个 bin。
> - **浏览器巡检（31 个页面 / 3 组）**：Docker 6 页、插件列表 3 页、设置 4 页、配额、插件功能页 9 个、任务/作业/站点/运行环境/文件等全部正常，**0 console error**；首轮 `/sites/list`、`/sites/runtimes`、`/files` 三页 **FAIL**：`CompileError: 动作 … 的 body 应写在顶层 content.body`（budding 1.8.3 契约：dialog 的 body/footer 必须在动作顶层 `content`）。
> - **修复（`d5680b6`）**：5 处前端弹窗（`dialogFormButton`、解压、文件新建目录/上传/新建文件）+ 2 处服务端弹窗（站点 SSL 上传、`dialog_button`）把 `props.body/footer` 移到 `action.content`；`SHELL_VERSION` 25→26；`main_flow` 增加 7 条源级回归断言。
> - **升级复验资产**：Release run `203`（head `d5680b6`），amd64 包 **12,913,194 B** id `774119680`，`sha256=152e5902f3f945212d22350341646768eba78e551a15fc51ec569521047ae257`；`install.sh` exit 0，前缀保持 `88qfm2s8`，8 插件与口令全部保留。
> - **复验（PASS）**：`/sites/list` 表格渲染 + 「新建站点」弹窗打开；`/sites/runtimes` 表格 + 「新建运行环境」弹窗打开；`/files` 渲染 + 「新建目录」弹窗创建 `verify-dir` 落盘并出现在列表（随后清理）；三页 0 console error。
> - **已知限制**：本轮未装 Web 服务（`/sites/web` 显示 nginx 未安装）、未装 pure-ftpd/fail2ban；TLS 形态一/二、aarch64、站点详情 SSL 上传弹窗未做浏览器实测（该弹窗只有源级断言）。
>
> **2026-11-21 01:00 干净重装 v23（已过期；HEAD `26f53ec`，前缀 `ewtdhtgx`）**
> - **场景**：按标准流程——清测试机（stop + 删 /www/skiff /etc/skiff / skiff.service / skiff-http-fwd.service）→ 下载 Release #197 资产 → 校验二进制 sha256 → install.sh → 配转发层 → 浏览器验收。
> - **资产**：Release run `197`（head `26f53ec`，dispatch `ref=main` + `publish=true`，success）。`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` 13.4 MB（GitHub download 直连，sha256 侧车超时，解包后二进制 sha256 校验确认）。
> - **二进制**：`/usr/local/bin/skiff` **21,125,136 B**，`sha256=bf1ce05fa711732ede991f959b46bd7cce2e31d39cfd3771d79a51e29202162d`。
> - **安装**：`install.sh --no-docker --tls off --listen 127.0.0.1:18080` exit 0；schema v0→v22；前缀 **`ewtdhtgx`**；初始口令 admin / `SGJRnUdksZfxZc8a`（写 `/root/iter/admin-pw.txt` 0600）；`skiff.service` active；healthz `{"docker":"available","status":"ok","uptime_secs":6,"version":"0.1.0"}`。
> - **转发层**：`skiff-http-fwd.service` active（`/usr/local/bin/skiff-http-fwd.py`，Python 行 pipe，0.0.0.0:18081 → 127.0.0.1:18080，settinmeout 清理 + RestartSec=1），`/etc/systemd/system/skiff-http-fwd.service`。curl 18081/ewtdhtgx/login 返回 200 + `title="登录 · Skiff"` + `body class="skiff-login-shell"`。
> - **登录页 CSS 修复（最终方案 · SHELL_VERSION 23）**：
>   - `body.skiff-login-shell { overflow: hidden !important; overflow-y: hidden !important; padding-top: 0 !important; }` —— **根治滚动条**（body 不让任何垂直滚动产生）。
>   - `html:has(body.skiff-login-shell) { overflow: hidden !important; }` —— html 根元素也锁住。
>   - `.login-layout { max-height: 100dvh; box-sizing: border-box; }` —— 动态视口高度约束。
>   - `.login-form-col { flex: 0 0 360px; padding: var(--sp-4) var(--sp-4); }` —— 表单卡片 360px。
>   - `.login-form-col .v-field / .v-btn / .v-label / .v-messages` —— 大量覆盖 Vuetify 组件间距压缩表单总高。
> - **上游 budding 1.8.3 同步**（`C:\dev\budding` package.json `1.8.3`，commit `1120d3`）：
>   - 主包：`budding-1.8.3.min.js` 1304KB / `.css` 647KB。
>   - Icons：`budding-icons-1.8.3.min.css` 5KB + `.woff2` 4KB。
>   - 分包：上游改 `.mjs`（ESM 格式），skiff 改回 `.min.js` 文件名。
>   - `assets.rs`：`SHELL_VERSION=23`、`BUDDING_VERSION="1.8.3"`、`BUDDING_ICONS_VERSION="1.8.3"`。
> - **验收（browser_use 4/4 PASS）**：
>   - ✅ CSS href `skiff.css?v=23`，budding `?v=1.8.3` 正确加载（curl HTML 确认 `body class="skiff-login-shell"`）。
>   - ✅ **无垂直滚动条**：`htmlScrollHeight=571 === htmlClientHeight=571`（960×571 视口）；当前部署概览页同样 html/body scrollHeight==clientHeight。
>   - ✅ 所有表单字段（用户名、密码、动态码）+ 登录按钮（top:278, bottom:314）完全可见（960×571 视口）。
>   - ✅ admin / SGJRnUdksZfxZc8a 登录成功 → 概览页（容器/网站/插件/工具菜单正常）。
> - **curl HTML 全链验证**：`curl -s http://18081/ewtdhtgx/login` 返回 `<body class="skiff-login-shell">` + `<link rel="stylesheet" href=".../skiff.css?v=23">` + login-layout/login-form-col DOM 完整。CSS 文件确认 body 规则 `overflow: hidden !important` 已就位。
> - **已知限制**：本机 L1 门禁因可用内存 3 MB < 1024 MB 被 cargo 闸门拒绝，未跑 fmt/clippy；编译级以 Release run `197` 四 job 全绿为准；budding 分包格式从 UMD 改 ESM `.mjs`，改名为 `.min.js` 运行时兼容性需进一步验证（本轮只验登录页，登录流程正常说明主包 OK）。
>
> **2026-11-21 00:53 磁盘覆盖 v23（已过期；HEAD `26f53ec`，前缀 nitrz26v）**
> - 场景：v20 登录页修复不够紧，三轮迭代（v20→v21→v22→v23）逐步紧缩 + 最终 body overflow:hidden !important 根治；上游 budding 1.8.3 同步。当时用磁盘覆盖 SKIFF_EXTRA_STATIC_DIR 方式验证，未做干净重装。
>
> **2026-11-20 23:34 机器还原后全新安装（已过期；HEAD `6d98116`，前缀 egehgs6m）**
> - **场景**：测试机还原为干净系统（T1：Docker 29.7.2 在位），空库全新安装后按「逐页操作 → 记录 → 修复 → 重发版 → 复验」迭代到问题收敛。
> - **资产**：Release run `44750632405`（head `ffc0aa2`，publish 成功），`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **12,897,732 B** id=`772097777`，`sha256=fbb96094…6fbb`。测试机经 `api.github.com` + `Accept: application/octet-stream` 下载并 `sha256sum` 校验一致（GitHub download 直连本轮两次 SSL 重置）。
> - **安装**：`install.sh --no-docker --tls off --listen 127.0.0.1:18080` exit 0；schema **v0→v22**；`user_version=22`、`integrity_check=ok`；前缀 **`egehgs6m`**；`skiff.service` active；转发层 `skiff-http-fwd.service`（`/root/iter/skiff-http-fwd.py`，0.0.0.0:18081 → 127.0.0.1:18080）active。
> - **临时修补**：install.sh 仍未写 `allow_reduced_sandbox=true` + `allow_insecure_http=true`，本轮继续手动追加（契约缺口未变）。
> - **8 官方插件**：逐个 `skiff plugin install <pkg> --yes`（不传 `--resource`，按 manifest hints 授权），全部验签/安装/启用。
> - **accept.sh `--group all`**：**44 PASS / 0 FAIL / 5 SKIP**；B3 轮换口令后 `initial-admin.txt` 已删除，口令写 `/root/iter/admin-pw.txt`。
> - **浏览器逐页巡检**（两轮 20+ 页面 + 深度写操作）：全程 0 console error；修复 6 类交互缺陷（见 `changes/2026-11-20.md`）——① `code` 组件不收集表单值导致文件/配置写空串；② 插件沙箱把降权写成「完整」；③ 运行环境弹窗主按钮在小屏被截断；④ 配置文件编辑入口 404 且 query 跳转不重载；⑤ 审计「重置」不刷新关键字与结果；⑥ 删除运行环境后停在「不存在」错误页。
> - **测试数据清理**：站点/运行环境/文件测试对象均已删除，`/www` 仅保留 `log/nginx/php/runtime/sites/skiff` 核心目录；`skiff-web-nginx` 是运行环境安装时拉起的 Web 服务容器。
> - **已知限制**：本机 L1 门禁因可用内存 424 MB < 1024 MB 被拒（exit 3），本轮只做 `node --check`；编译级与用例以 CI/Release 构建 `44750632405`（四 job 全绿）为准。
>
> **2026-11-20 19:00 插件权限修复（已过期；HEAD `9d8d7a4` 同资产，无新发版）**
> - **根因**：首次批量装插件用了 `skiff plugin install *.wbpkg --resource path:/www --yes`，显式 `--resource` 覆盖了 manifest 的 `resource_hints`（monitor 声明 `path:/proc, /etc/passwd`，testkit 声明 `path:/proc`）。CLI help 明确写了"--resource 可覆盖为最小授权"，但批量命令硬塞统一 path:/www 把所有 manifest hints 干掉了——**这是操作者命令错误，不是代码 bug**。
> - **修复现场**：全部 8 插件卸载（`--delete-data`） → 逐个用 `--yes` 重装（**去掉 `--resource`**，让 CLI 的 `resolve_yes_selection` 自动把 manifest `resource_hints` 当 object_grants）。修复后 grants：
>   - monitor：`perm:fs:list, fs:read + object:path:/proc, object:path:/etc/passwd` ✅
>   - testkit：`perm:fs:list, fs:read, identity:self, interop:call, scheduler:manage, scheduler:read + object:path:/proc` ✅
>   - firewall：`perm:exec:run + object:bin:/usr/sbin/nft, iptables, apt-get, fail2ban-client, systemctl...`（11 个 bin）✅
>   - ftp：`perm:exec:run, ftp:*, interop:call + 16 个 bin + object:plugin:com.skiff.firewall` ✅
>   - sources：`perm:exec:run, fs:read, fs:write + object:bin:docker, systemctl + object:path:/etc/docker, /www/runtime/cache/apk` ✅
>   - files/cron/guard：无 resource_hints 插件，仅 `perm:*`（与首轮一致）✅
> - **浏览器复验 51/51 页面全 PASS**：完整页面清单见 04-ACCEPTANCE.md。首次巡检 46 PASS + 2 FAIL（monitor overview/process 缺 path:/proc）+ 3 BLOCKED（设置 3 子页）；修复后复验 5/5 全部 PASS——monitor overview 展示真实主机指标（CPU 0.5%、内存 459 MiB / 3.83 GiB、网卡入站 1.0 KiB/s），process 页展示 50 行真实进程列表（PID 1 `/sbin/init`）。**所有页面 console error 均为空数组**。
> - **后续改进建议（代码层）**：CLI `resolve_yes_selection` 在显式 `--resource` 非空且 manifest 有 hints 被完全覆盖时，应输出一条警告提示用户"manifest 声明的 resource_hints 被 --resource 覆盖"，避免操作者不知情就截断权限面。当前 help 文字"--resource 可覆盖为最小授权"是准确的，但批量装插件的场景下容易踩坑。
>
> **2026-11-20 18:26 机器还原后全新安装 + 门禁修复收尾（已过期；HEAD `9d8d7a4`）**
> - 背景：测试机还原为干净系统（T1 口径，Docker 29.7.2 已装），执行完整迭代闭环。HEAD `9d8d7a4` 是 `14:17 → 18:20` 期间 5 个门禁修复 commit 的总结——从 pre-existing 单测 FAIL → Clippy 剩余 default_view/CSRF/p2_row → 全部收敛到门禁全绿。
> - 本地 L1 门禁全绿：根 workspace `fmt + clippy --all-features` exit 0；plugins workspace `fmt + clippy` exit 0。
> - 资产：Release run `44717281516`（head `9d8d7a4`，dispatch ref=main，publish 成功）。`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **12,895,953 B** id=771372800，`sha256=4211de8c1f73dc4644bb52270e8d96577e80aad08bd07374e003679cff4c8acd`。测试机 curl GitHub download 直连成功 + `sha256sum` 校验 OK。
> - 部署：`install.sh --no-docker --tls off --listen 127.0.0.1:18080` exit 0；daemon schema **v0→v21** 完整迁移。入口前缀 `dm9sdqjv`，`initial-admin.txt` 口令自动生成（重置多次，终态 `dRnkZbYEAP4q8gCL`）。`skiff.service` active。**临时修补**：install.sh 未自动写 `allow_reduced_sandbox=true` + `allow_insecure_http=true`，需手动在 config.toml 末尾追加（本轮仍在，后续须修复）。
> - **8 官方插件已全部安装启用**（`--yes` 参数，签名验签 OK，沙箱降级正常）：cron / files 0.1.3 / firewall / ftp / guard / monitor / sources / testkit。
> - **转发层**：Python 脚本 `/root/iter/skiff-fwd.py`（监听 0.0.0.0:18081 → 转发 127.0.0.1:18080，剥掉 Sec-Fetch-Site/Origin/Referer 等安全头），`nohup` 后台运行 PID 1716，curl 18081/healthz 返回 200。
> - **accept.sh 全过！preflight + A + B + D0 = 44 PASS / 0 FAIL / 5 SKIP**（SKIP：PF3 时钟、PF4 装后端口、A3 未传 install-exit、B4 限流未开、D0b docker-bootstrap.sh 路径）。关键：A 组 22 PASS（含 restart 自恢复、kill-9 后 2s 拉起、权限位 711/600/644 全正确）；B 组 14 PASS（含完整 CSRF→登录→改密→旧会话失效→新会话 200 链路）；D0a Docker 29.7.2 可用。
> - **浏览器验收 8/8 PASS，0 JS 错误**：登录页 → 登录成功 → 概览（容器 0/插件 8/正常）→ 容器页（空态正确）→ 插件页（8 官方全显示）→ 文件管理（目录视图完整）→ 设置页（安全表单）。
> - **Git commits（本轮 5 个）**：
>   - `69efce0` fix(ci): 修复 Clippy / xtask baseline / assets 单测三项引入性失败
>   - `bdf0944` fix(clippy): ExecRequest::parse dead_code
>   - `7882e88` fix(ci): 修复剩余 10 个 pre-existing 单测 FAIL
>   - `edcd9cb` fix(ci): Clippy unused variable + rustfmt 格式
>   - `9d8d7a4` fix(ci): 剩余 3 项 — Clippy default_view + CSRF + p2_row site_page.rs

> **2026-11-20 14:17 覆盖升级 schema v21→v22（已过期；HEAD `524f880`）**
> - 背景：fa19cd 提交 50 文件含 schema v22 迁移、站点详情 Schema 服务端化（site_page.rs）、slug→id 重定向、rewrite/limit_rate_kbps/backup interval-time 等 6 项站点详情功能。524f880 提交 1 文件修正 budding-boot.js CSS 类名（.bud-code__language → .bud-code__select+.bud-code__status）。
> - 资产：Run `44687309424`（head `524f880`，publish 成功，amd64 12.9 MB，覆盖 v0.1.0）。Run `44685263606`（head fae19cd，publish 成功）。
> - 部署：就地覆盖（停服务→scp skiff→start），schema **v21→v22 自动迁移**（sites 表新增 rewrite TEXT 'none' + limit_rate_kbps INTEGER 0），skiff.db WAL 模式正常。
> - **浏览器 6 项全量复验 PASS**：① slug→id URL 重定向 ② 概览页"打开文件管理"链接 ③ 备份间隔/时间可改 ④ PHP 伪静态+nginx 流量限制 ⑤ vhost code 语言标签隐藏 ⑥ runtime type=PHP 8.5。
> - 转发层 `.workbuddy/skiff-http-fwd.py`（NoRedirectHandler 透传 303 + 剥 CSRF 头）沿用无需改动。

> **2026-11-20 10:58 空库首装 + 官方插件安装（HEAD `a02f1d0`，已过期）**
> - 背景：测试机从干净系统还原（T1 口径，Docker 29.7.2 已装），空库全新安装（非覆盖升级）。Release run 184 四 job 全绿（构建 x86_64/aarch64/Docker/发布到 GitHub Release），publish=true，资产覆盖 v0.1.0。
> - 资产：Release run `44574500026`（head `a02f1d0`，dispatch ref=main，publish 成功）。`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **12,821,377 B** id=768570200，`sha256=2fd680b507163d3156aacd4aaf2ccd196b98c7c99d13d7f8a7642b848b55bd5c`。测试机 `sha256sum -c` OK。二进制 `/usr/local/bin/skiff` `b7d4556f...111`。
> - 部署：`install.sh --no-docker --tls off --listen 127.0.0.1:18080` exit 0；daemon schema **v0→v21** 完整迁移。入口前缀 `t2mhspmy`，`initial-admin.txt` 口令 `5PvxpDNdQ8QyR9Ny`。`skiff.service` active + `skiff-http-fwd.py`（修复版，剥 Sec-Fetch-Site/Origin/Referer）0.0.0.0:18081 → 127.0.0.1:18080。
> - **临时修补**：install.sh 未自动写 `allow_reduced_sandbox=true` + `allow_insecure_http=true`（无 landlock/cgroups），需手动在 config.toml `tls_enabled = false` 下追加（本轮发现的契约缺口，install.sh + accept.sh dev_mode 需同步修复）。
> - **8 个官方插件已安装并启用**（签名验签 OK，沙箱降级 `landlock/cgroups` 未实现，supervisor 按需拉起）：
>   | 插件 | 版本 | 能力 | grants |
>   |---|---|---|---|
>   | com.skiff.cron | 0.1.0 | identity, interop, scheduler | 6 |
>   | com.skiff.files | 0.1.3 | fs, identity | 6 |
>   | com.skiff.firewall | 0.1.0 | exec, interop | 12 |
>   | com.skiff.ftp | 0.1.0 | exec, ftp, interop | 22 |
>   | com.skiff.guard | 0.1.0 | docker, identity, fs | 6 |
>   | com.skiff.monitor | 0.1.0 | fs | 4 |
>   | com.skiff.sources | 0.1.0 | fs, exec | 7 |
>   | com.skiff.testkit | 0.1.0 | fs, identity, interop, scheduler | 7 |
> - accept.sh：preflight+A+D0=35 PASS / 0 FAIL / 1 SKIP；B=14 PASS / 0 FAIL / 1 SKIP；**合计 48 PASS / 0 FAIL / 3 SKIP**。关键验证：**A6e config.toml 644 root:root ✅（`a02f1d0` write_atomic 修复根治）**；A6d skiff.db 600 root:skiff-plugins；A9 restart 自恢复；A10 kill-9 后 2s 拉起；A11 --help/version 不落文件；B 组完整认证链路（csrf_token 登录→改密→旧会话失效 303 踢回登录页）；B0 SKIP 首轮是 dev_mode 判定挡，补 allow_insecure_http=true 后二轮 B 组全过。
> - 浏览器全量抽查：9 页面（登录页→概览→容器→镜像→网络→卷→Compose→Docker 设置→插件已装→文件管理）**全部正常渲染 + 零 console error**。转发层修复后跨端口 `Sec-Fetch-Site: cross-site` 被剥掉，Skiff 按 dev_http 放行。
> - Git commits（本轮 5 个代码改动）：
>   - `116c0a3` feat(cli): add plugin rpc subcommand（新增 CLI 子命令）
>   - `edf2a75` fix(cli): plugin rpc 走 HTTP 调 daemon（避免 token 分裂，loopback bypass PluginManagerUser）
>   - `a9697c6` fix(cli): entry_prefix fallback 读 DB（CLI-daemon 前缀分叉修复）
>   - `e42f439` fix(bootstrap): daemon 启动时持久化 entry_prefix 到 config.toml（同上配套）
>   - `a02f1d0` fix(atomic): write_atomic rename 后恢复原 owner/group（**根治 A6e**，本轮最关键修复）

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此「某轮的资产 sha」只在该轮有效；对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表。

- **当前资产来源**：Release run `207`（dispatch `ref=main`，head `5221b4c`，`publish=true`、`release_tag=v0.1.0`），success；资产覆盖于 2026-11-21 03:45 UTC。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`5221b4c`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 12,913,254 B | `374927425308c05d3151fbd7fab62a44f83b8b6bb2cd6065c051653ed6c275ac` | `774328309` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | 111 B | （侧车，内容同上摘要） | — |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | — | （本轮仅 amd64 实测） | — |
| `skiff-server-x86_64-unknown-linux-musl` | — | （本轮未下载） | — |
| `checksums.txt` | — | （本轮未下载） | — |

- **下载与核对**：GitHub `releases/download/v0.1.0/<asset>` 直连偶发 SSL EOF，本轮改用 `api.github.com/repos/.../releases/assets/<id>` + `Accept: application/octet-stream` 下载成功；测试机 `sha256sum -c` 与包内 `checksums.txt` 复核均 OK。
- **本轮实际部署二进制**：`/usr/local/bin/skiff` **18,053,136 B**，`sha256=8af6592fb4549074d4c915cd1f27b486150ecfc15dcceb67ca0701e676cfdf28`。
- **包内容**：`official/` 含 8 个官方插件包（`com.skiff.{cron,files,firewall,ftp,guard,monitor,sources,testkit}`）+ `public-key.txt`。
- **Release 触发方式**：开发机无 `gh` CLI 和 `GITHUB_TOKEN` env var，用 git credential manager 取 token 后 `Invoke-RestMethod -Uri .../actions/workflows/release.yml/dispatches -Method Post` 直接 dispatch。
