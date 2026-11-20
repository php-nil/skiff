# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-20 14:17 覆盖升级 schema v21→v22（当前生效；HEAD `524f880`）**
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

> **2026-11-19 23:18 就地覆盖安装 `1e456cf`（当前生效；SSR 全删 + 插件 IPC 优化 + 壳页白名单回滚）**
> - 背景：测试机从干净系统（Docker 29.7.2，skiff 完全不存在）重新首装（Release run `44544002520`，head `b6a3e35`）→ 就地覆盖 `2024b81`（误加 docker-settings 壳页白名单）→ 就地覆盖 `1e456cf`（回滚白名单，恢复正确状态）。GitHub Release 资产经 `api.github.com` + `Accept: application/octet-stream` 直连下载（测试机 curl GitHub download 路径遇 SSL 重置）。
> - 资产：Release run `44551456436`（dispatch `ref=main`，head `1e456cf`，`publish=true`、`release_tag=v0.1.0`），success。`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **12,817,174 B** 资产 id `768167669`，sidecar sha id `768167660`。测试机 `sha256sum -c` OK。
> - 壳页白名单：`SHELL_PAGES`（`spa.rs`）15 条——sites / containers / images / networks / volumes / compose / runtimes / files / plugins / store / settings / audit / tasks / jobs。Docker 设置菜单的 href 是 `settings/docker`（settings 子路径，settings 已在白名单），独立的 `docker-settings` 路径前端不认（budding 路由无此项）。本轮误加后立即回滚。
> - 部署：`install.sh --no-docker --tls off --listen 127.0.0.1:18080` exit 0；`allow_reduced_sandbox = true`（无 landlock/cgroups）加于 `config.toml`；`skiff.service` + `skiff-http-fwd.service`（Python 行 pipe，`0.0.0.0:18081 → 127.0.0.1:18080`）均 active。
> - 入口：前缀 **`trpdbfmr`**（升级保留）；管理员 `admin` / `aSHKhyWPx6yPQ78k`；登录 URL `http://10.10.15.4:18081/trpdbfmr/login`。
> - accept.sh：`--group preflight,a,b,d0` → **PASS 32 / FAIL 0 / SKIP 3**（SKIP：PF4 装后端口占用、B0 明文 dev Cookie 无回传、D0b `--pkg-dir`）。
> - 浏览器验收（真实 budding 前端，无 console error）：
>   - 登录页 / Dashboard（概览统计、侧边栏 6 分组全展开）
>   - 容器页（空态表格 + 全部/仅异常 筛选 + 新建容器 按钮）
>   - 镜像页 / 网络页 / 卷页 / Compose 页
>   - Docker 设置（`/settings/docker`，从 Docker 菜单 → 设置 子项进入，H2 "Docker 设置" ✅）
>   - 插件（已安装 0 / 发现 / 可更新 三 tab）
>   - 设置 / 任务中心
> - 本轮文档禁令：AGENTS.md §4.3 增"测试机禁止 Rust 编译"硬禁令（本轮曾误向测试机 `rustup`，已立即 `rustup self uninstall -y && rm -rf ~/.cargo ~/.rustup`）。测试机只做安装验收，编译链路统一走 GitHub Release musl 静态资产。
> - Git commits（`b6a3e35 → 1e456cf`，三轮）：
>   - `b6a3e35` refactor: delete legacy SSR UI — routes / templates / menu deep links / htmx & xterm residuals
>   - `2024b81` fix(web): add docker-settings to SPA shell whitelist（误修复，立即回滚）
>   - `1e456cf` revert: docker-settings shell whitelist was unnecessary（菜单 href 走 settings/docker）

> **2026-11-19 15:10 就地覆盖编译部署 `b1234f8`（当前生效；登录页优化 + 文件管理弹窗 + 磁盘覆盖路径）**
> - 背景：两批改动——① 登录页视觉重写（分栏布局 / 渐变网格背景 / 品牌区 / 主题切换按钮）；② 文件管理「新建与上传」三个内联表单改为按钮触发 dialog（新建目录 / 上传文件 / 新建文件）；③ `SKIFF_EXTRA_STATIC_DIR` env var 磁盘覆盖 rust-embed 静态资源（测试机上装 Rust 工具链构建，绕开 CI Release token 问题）。CI Release workflow 需 `workflow_dispatch` 手动触发，开发机无 GitHub token，本次在测试机上用 rustup + musl-tools + 源码 tar 编译（4m25s 增量）。
> - 编译：测试机 `rustup 1.100.0` + `x86_64-linux-musl-gcc`，源码 tar 从开发机 SCP 到 `/root/skiff-src`，`cargo build --release --target x86_64-unknown-linux-musl`，exit 0（仅 `xtask` 有 unused `anyhow` warning）。
> - 部署：`systemctl stop skiff` → 备份旧二进制 → 新二进制 19MB 覆盖 `/usr/local/bin/skiff` → `systemctl start skiff` → healthz OK。
> - 热更新 drop-in：`/etc/systemd/system/skiff.service.d/extra-static.conf` 加 `Environment=SKIFF_EXTRA_STATIC_DIR=/www/skiff/static-override`（`ProtectSystem=full` + `ReadWritePaths=/www` 允许访问）。以后前端改 JS/CSS 直接 SCP 到 `/www/skiff/static-override/` 对应路径即可热更新，无需重编 Rust 二进制。
> - 浏览器验收（localhost → 代理 18081）：
>   - 登录页：分栏布局生效——左侧品牌区（Logo / Skiff·轻舟 / slogan / 三条卖点）、右侧表单卡片；背景多层渐变 + 网格纹理 + 柔光圆形装饰；右下角主题切换按钮；深浅主题切换正常。
>   - 文件管理：「操作」卡三按钮（新建目录 / 上传文件 / 新建文件）替换原「新建与上传」三内联表单，列表首屏不再被挤爆。
>   - 三 dialog 全弹：新建目录 dialog（width 480，输入框 + 取消/创建）、上传文件 dialog（width 560，文件选择器 + 关闭）、新建文件 dialog（width 640，文件名 + code 编辑器 + 取消/保存）。
>   - 磁盘覆盖：`curl -sI /static/skiff.css` 200 OK，ETag `"474d9fcf..."` 正确；curl header 安全响应头（nosniff / referrer-policy / frame-options / CSP）全部就位。
> - Git commit（`8fcca6b..b1234f8`，三个 commit）：
>   - `ab914db` 登录页 + 文件管理弹窗主改动（5 files, +517/-117）
>   - `8fcca6b` SHELL_VERSION bump 6→7（budding-boot.js / skiff.css 缓存键）
>   - `b1234f8` SKIFF_EXTRA_STATIC_DIR 磁盘覆盖路径（4 files, +55）
> - 测试机工具链：`rustup 1.100.0` / `cargo 1.100.0` / `rustc 1.100.0`，`rustup target add x86_64-unknown-linux-musl`，musl cross toolchain `/usr/bin/x86_64-linux-musl-gcc`（apt 装的 musl-tools）。crates.io 走 rsproxy-sparse 镜像，GitHub 不通。

> **2026-11-19 09:31 就地覆盖安装 `8f78221`（当前生效；库/配置/插件/运行环境全保留）**
> - 背景：本轮三项——新建 PHP 环境可选 apk 镜像源、软件源插件复查修复（编译错误 / `r.base` / 原生 schema）、测试机还原后重新部署。Release run `44435964314`（head `8f78221`）success（同提交 CI `44434687589` 全绿）；前一次首装为 Release run `44428042366`（head `f5267ec`），见 03-DEPLOY_LOG。
> - 机器状态：还原后为干净系统——无旧二进制/配置/库、18080/18081 空闲、Docker 29.7.2 在位、`/etc/skiff-baseline` 不存在；按 T1 口径（Docker 已装）首装后在同机覆盖升级。
> - 资产：`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **13,272,804 B** `f04eafe89684f69b3ca1dff4783738ed7938401a753ac695ac3977b51ff1dba7`（资产 id `765733618`；API `digest` / 本地 sha256 / 测试机 `sha256sum` 三方一致）。
> - 安装：`install.sh --no-docker --tls off --listen 127.0.0.1:18080` **exit 0**（首装 + 就地覆盖各一次）。`skiff.service` 与 `skiff-http-fwd.service`（`0.0.0.0:18081 → 127.0.0.1:18080`，脚本 `/root/iter/skiff-http-fwd.py`）均 active。
> - 二进制：`/usr/local/bin/skiff` **19,179,408 B** `e428451579aa8e198034898662683e43323114c5e52fad03a701a95e9d59bfc4`。
> - 库：`user_version=21`、`integrity_check=ok`；users 1 / plugins **8** / sites 0 / site_runtimes 1（`php-8.5` running）。配置 `/etc/skiff/config.toml` 已加 `allow_reduced_sandbox = true`（否则官方插件全被「要求完整沙箱」拒装），sha256 `59791959b24138b17a83c473433054c2ff9eec9ed752449b4d8becfe41234129`。权限 `711 /www/skiff`、`711 plugins/`、`600 skiff.db`、`711 /run/skiff` 保持。
> - 面板：入口前缀 **`zgvvx5xz`**；管理员 `admin` / `wGQcAXtSDKLDaxBs`（`/www/skiff/initial-admin.txt` 仍在，B 组在 TLS/prod 档跳过故未改密）。`/healthz` 18080 与 18081 均 **200**，登录页 **200**。
> - 插件：官方 8 个包（7 官方 + testkit）全部装成；`com.skiff.sources` 同版本内容随发布包自动更新（journal：`官方插件内容已随发布包更新（同版本重装）`）。
> - 软件源：apk 固定为 `https://mirrors.tuna.tsinghua.edu.cn/alpine`（`/www/runtime/cache/apk/.skiff-mirror` 带 `# skiff-fixed`）；容器 `/etc/apk/repositories` 为 tuna `v3.24/{main,community}`。
> - 运行环境：`php-8.5`（`php:8.5-fpm-alpine`，PHP 8.5.12）**10 s 装完**（固定 tuna 源），容器 `skiff-php-php-8.5` running；站点本轮未建。
> - 浏览器实测（真实资产无拦截）：概览页、运行环境新建弹窗（软件包源字段预选「清华 TUNA」）、软件源插件原生页（两表 5 行数据、`测速全部` 出真实 MB/s）均 **0 console error**。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此「某轮的资产 sha」只在该轮有效；对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表。

- **当前资产来源**：Release run `44435964314`（dispatch `ref=main`，head `8f78221`，`publish=true`、`release_tag=v0.1.0`），success。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`8f78221`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 13,272,804 B | `f04eafe89684f69b3ca1dff4783738ed7938401a753ac695ac3977b51ff1dba7` | `765733618` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | 111 B | （侧车，内容同上摘要） | `765733585` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 同轮 | 见同轮 `checksums.txt` | 同轮上传 |
| `skiff-server-{x86_64,aarch64}-unknown-linux-musl` | 同轮 | 见同轮 `checksums.txt` | 同轮上传 |

- **下载与核对**：开发机直接打 `api.github.com/repos/php-nil/skiff/releases/assets/<id>` + `Accept: application/octet-stream`（缓存 `%TEMP%\skiff-deploy\pkg2\`）；装机前本地 sha256 / API `digest` 一致，`scp` 到测试机后 `sha256sum` 再核对。
- **包内容**：`official/` 含 8 个官方插件包（`com.skiff.{cron,files,firewall,ftp,guard,monitor,sources,testkit}`）+ `public-key.txt`；aarch64 两件本轮未实机验证。
