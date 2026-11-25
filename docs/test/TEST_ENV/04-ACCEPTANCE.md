# 验收结论：当前轮与最近两轮（§8）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §8 最近三轮 + 历史轮次台账（其余轮次已精简）（2026-11-16 整理）。

---

## 8. 最近一次验收结论（截至 2026-11-25 19:30：本机当服务端 + 测试机安装对接轮）

**2026-11-25 19:18 干净重装：安装链路通过，商店目录对接通过，运行环境清单面板侧未覆盖（旧包无端点）**

- **前置**：测试机现场（11-23 首装 + 两轮升级）整体备份到 `/root/iter/backup-20261125/`（`config.toml.bak`、`skiff-data.tgz` 4.1 MB、`sites-old/`、`php-old/`、`runtime-old/`），再 `printf 'YES\n' | sh uninstall.sh --purge`。卸载后 `/etc/skiff`、`/www/skiff`、`/usr/local/bin/skiff*`、unit 全无，`/www` 仅剩 `log`、`nginx`。
- **服务端（开发机）**：`publish` 签名产物（`seq=1`、`key_id=707cede3…ac2`、`index.json` 8,303 B、23 条运行环境、三类清单）→ `check --require-signature --builtin-index crates/app/assets/runtime-index` 报「校验通过 + 内置副本一致：1 份」→ `serve --dist … --listen 0.0.0.0:8080 --http --access-log .workbuddy/server-access.log`。本机自检：三件套 + 三类清单 200、`/api/x` 404、`If-None-Match` 304；测试机 `curl` 均 200（60 ms）。
- **安装（通过）**：`sha256sum -c` 通过（`c905859d…9451b2`）→ `sh install.sh --listen 0.0.0.0:18080 --tls off --no-start` exit **0** → 追加 `allow_insecure_http` 与 `[catalog]` → `systemctl restart skiff`。`skiff.service` enabled+active、`/healthz` 200、前缀 **`8au3xewy`**、`skiff doctor` 六项 ok、`official/` 8 个包 + `public-key.txt` 在位、自动建出 `php-8.5` 运行环境（Docker 29.7.2）。
- **登录**：`GET /ui/v1/auth/login` 取 `csrf_token` → `POST /ui/v1/auth/login` JSON **200**（`redirect_to=/8au3xewy`）；旧表单路径 `POST /{p}/login` 已返回 405，登录命令要按 JSON 端点写。
- **目录对接（通过）**：`GET /ui/v1/store?with_schema=true` →
  - `message` = 「已从 http://10.66.52.7:8080 更新到 seq 1」，`fetched_at=2026-11-25T11:19:38Z`，`key_configured=true`、`allow_unsigned=false`；
  - `sections` 三类齐全：插件 1、应用模板 1、运行环境 **23**（PHP 13 / Node 9 / Rust 1，`8.5.13/12/11`、`24.22.3/2/1`、`26.13.1/13.0/12.1` 等每分支 3 条）；
  - 镜像侧访问日志同一时刻出现 `manifest.json` / `manifest.json.sig` / `index.json` 三条 GET。
- **条件请求与降级（通过）**：再次 `refresh=1` → 「目录未更新（304），展示 … 的本地缓存」，`fetched_at` 不变；停掉开发机 `serve` 后 → 「镜像源不可达，正在使用本地缓存；… Connection refused (os error 111)」，条目仍在；恢复 `serve` 后 → 回到 304，条目未丢。
- **未覆盖 / 限制**：
  - **运行环境清单面板侧**：该包无 `GET /ui/v1/runtimes/versions`（请求被 `/ui/v1/runtimes/{id}` 吃掉，返回「运行环境不存在：versions」），三类清单的「面板取到并展示」要在含 2026-11-25 改动的**新包**上重跑 MIRROR-06；服务端侧三类清单 200 已验。
  - `accept.sh` 全量未跑（本轮只做 `doctor` + 手工断言），A6 权限位全表、B 组改密链路待下一轮随新包补。
  - MIRROR-08（篡改/伪造被拒）、MIRROR-09（`runtimes` 命令回归）本轮未执行。

**2026-11-24 11:24 干净首装 `c841074`：accept.sh 48 PASS / 0 FAIL / 4 SKIP，权限位全对，B 组完整链路通过**

- **前置**：测试机还原为干净 T0（uptime 5 min，无 `/etc/skiff`、无 `/www/skiff`、无 `/root/iter`，18080 空闲，Docker 29.7.2 在位，时钟 UTC 已同步，磁盘 114 GB 可用）。L1 门禁 `cargo fmt --all --check` + `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` 均 exit 0。
- **上游同步**：budding 1.8.6 → 1.8.7；替换 `budding-1.8.6.min.{js,css}` / `budding-code-1.8.6.min.js` / `budding-terminal-1.8.6.min.js` / `budding-icons-1.8.6.min.css` / `.woff2` 为同名 1.8.7 版本。
- **适配**：页面主操作从 `form.actions` 迁移到 `page.content.actions`（容器/镜像/网络/卷/文件/站点/作业/任务/插件/商店等 12+ 页面）；`SUBVIEWS` 从两套 `views`/`menu_entries` 合一为单一 `subviews` 字段（保留 `nav`/`entries`/`icon`/`badge`/`path`）；`writePost` 模板动作统一（POST 表单提交模板）。
- **首装（通过）**：Release run `45317927914`（head `c841074`，`publish=true`），amd64 包 12,923,388 B `sha256=c905859d…9451b2`（侧车 + 包内 `checksums.txt` 双重校验）。`sh install.sh` exit **0**；前缀 **`gk5y33tq`**；`skiff.service` active；`/healthz` 200。
- **形态三配置**（install.sh 只写 loopback + `allow_insecure_http`，以下人工追加）：
  - `listen = "0.0.0.0:18080"`（默认 loopback → 外网直连）
  - `tls_enabled = false`（直接明文监听必需；缺省启动会因非 loopback + 无 TLS 被拒）
  - `allow_insecure_http = true`（让 accept 判 dev 档、跑通 B 组 Cookie 断言）
  - `allow_reduced_sandbox = true`（插件沙箱降级开关）
  - `entry_prefix = "gk5y33tq"`（硬编码）
- **accept.sh（通过）**：`--group all --password W8UdMRcTHTCbgKVb --pkg-dir /root/iter/skiff-night/skiff-0.1.0-x86_64-unknown-linux-musl` → **48 PASS / 0 FAIL / 4 SKIP**（PF3 未传 `--now` 时钟偏差、PF4 装后端口、A3 未传 `--install-exit`、B4 未开限流）。
- **权限位（A6 全对）**：`/www/skiff` 711 root:root、`plugins` 711、`backup` 700、`skiff.db` 600 root:skiff-plugins、`/etc/skiff/config.toml` 644、`/run/skiff` 711 root:skiff-plugins、`kernel.sock` 660 root:skiff-plugins、`initial-admin.txt` 600 root:skiff-plugins。
- **B 组认证**：CSRF → 登录 200 + `redirect_to=/gk5y33tq`（默认不强制改密）→ 错误口令 401「用户名或口令不正确」→ 改密成功 + `initial-admin.txt` 删除 + 旧会话 303 → 新会话 200；改密后口令回写 `/root/iter/admin-pw.txt`（0600）。
- **重启自恢复**（A9/A10）：`systemctl restart skiff` 后 3 s 内 healthz 200；`kill -9` PID 后 2 s systemd 拉起新进程。
- **D0 Docker**：Docker 29.7.2 已装可用 → `--check-only` exit 0 且不改动 `daemon.json` → 已装不重装。
- **未覆盖（限制）**：本轮跳过浏览器巡检（accept.sh 全绿 + 之前多轮已覆盖核心交互）；TLS 形态一/二、aarch64、PHP/Node 运行环境安装与扩展、FTP/fail2ban 实装、真实备份计划、8 个官方插件实装（本轮未调 install.sh `--yes`）、`--check-only --fix` / `--install` 未测。

**2026-11-23 20:30 干净首装 `9f43aee` → 修复 `5a48b83` → `fe4baa4` / 断言 `9c1be6c`：accept.sh 三轮 49 PASS / 0 FAIL / 3 SKIP，50 实路由 0 console error**

- **前置**：测试机还原为 T1（无 `/etc/skiff`、无 `/www/skiff`、无 `/root/iter`，18080 空闲，Docker 29.7.2 在位，时钟已同步，磁盘 112 G 可用）。
- **上游同步**：budding 1.8.5（`a3a3c8d`）→ 1.8.6。主包 1,027,351 B / CSS 574,157 B / icons 4,645 B + woff2 3,992 B / code pack 90,331 B / terminal pack 395,014 B（CSS、code、terminal、woff2 与 1.8.5 逐字节相同）；`BUDDING_VERSION` = `1.8.6`，`SHELL_VERSION` 43→44→45。
- **适配**：表单全面改 `props.submitMode: "event"`（`schema_kit::form` / `form_body` / `search_form` 与 22 处调用点，`DialogForm.url` 字段删除）；`crates/pkgfmt` 删掉 `inject_form_submit_stub`、改为「带 `onEvent.submit` 的 form 必须声明 `submitMode: event`」；删除宿主 K3（dialogRegistry + 克隆重开）与 K4（插件 schema 注入 no-op `api`），改成提交动作打 `inLayer`；`confirm` 的 `level` 改为 `color`。
- **首装（通过）**：`install.sh --listen 0.0.0.0:18080 --tls off` exit **0**；前缀 **`28bwmusa`**；库 `user_version=22`、`integrity_check=ok`；`/healthz` 200。人工追加 `allow_insecure_http`（让 accept 判 dev 档、跑通 B 组）与 `allow_reduced_sandbox`（插件沙箱降级开关）。
- **缺陷 1（已修）**：插件拒装删源包——8 个官方插件因「要求完整沙箱」全拒，`/www/skiff/official/*.wbpkg` 同时被清空。`commands/plugin.rs::install` 改为「装成功 + 路径在 `plugins/.incoming` 内」才 `remove_kernel`；新增用例 `rejected_install_keeps_the_source_package`。复验：装完 8 个官方插件后官方目录仍 9 个文件；`/tmp/bad-0.1.0.wbpkg` 拒装后原样保留。
- **缺陷 2（已修）**：弹窗提交失败静默——`formError` 只认 `ctx.extras.form`，而弹窗提交按钮在图层 `footer`（与正文是兄弟子树）恒取不到控制器，失败只剩 2 秒 toast。改为按 budding 目标解析规则再查 `extras['budding:layer-forms']` 登记表（`formOf(ctx)`）。复验：同名建目录弹窗内出现「新建目录失败：目标已存在」，输入值保留、层不关；新建站点在 Web 服务未就绪时就地显示 503 原因。
- **accept.sh（通过）**：首装 `9f43aee` **50 PASS / 0 FAIL / 2 SKIP**；中间包 `5a48b83` 与最终包 `9c1be6c` 各 **49 PASS / 0 FAIL / 3 SKIP**（PF4 装后端口、A6h 已改密机无 `initial-admin.txt`、B4 未开限流）。权限位 711/711/700/600/644/711/660 全对；restart 自恢复、kill -9 后拉起；B 组完整 CSRF→登录→错误口令 401→改密 200→`initial-admin.txt` 删除→旧会话 303→新会话 200。
- **浏览器（通过）**：`/ui/v1/routes` 展开 69 条（19 条为 `:id` 模板路由，命中「不存在」属预期），**实路由 50/50 全 200、0 CompileError、0 JS 异常**；关键交互 15 项全过（同名建目录失败原地回填、空提交字段级错误、新建站点成功关层并刷新列表、站点详情 6 子视图、6 个插件功能页、主题页 `--v-theme-primary=23,169,142`）。
- **环境补充**：本轮装了 OpenResty 容器（`skiff-web-nginx`，安装作业 100% 完成），站点创建因此可跑通；8 个官方插件（cron / files 0.1.3 / firewall / ftp / guard / monitor / sources / testkit，API 1.3）全部 installed + enabled。
- **未覆盖（限制）**：TLS 形态一/二、aarch64、PHP/Node 运行环境安装与扩展、FTP/fail2ban 实装、真实备份计划、≤880px 布局与浅/深主题逐页比对；镜像/网络/卷的实际创建删除未做浏览器实测。
**2026-11-22 22:47 干净首装 `e3066e6` → 修复 `5fa12e2` → 补收口 `76412df`：accept.sh 三轮 49 PASS / 0 FAIL / 3 SKIP**

- **前置**：测试机还原为 T1（Docker 29.7.2 在位，无 skiff / 无 `/etc/skiff`，18080 空闲）。
- **上游同步**：budding 1.8.4（`1121d5`）→ 1.8.5（`a3a3c8d`）。主包 1,022,104 B / CSS 574,157 B / icons 82 字形 / code pack 90,331 B / terminal pack 395,014 B；`BUDDING_VERSION` = `1.8.5`，`SHELL_VERSION` 41→42。
- **适配**：图层页脚可直接 `submit` / `reset` 本层唯一表单；壳页弹窗注册表改为保存原始 `content`，失败重开复用正文/页脚并回填输入；`confirm` 危险语义字段 `level` 统一为 `color`。
- **新建入口弹窗化**：容器新建（列表弹窗 + 深链页共用字段，删除 `docker-container-new.json`）、编排「新建 / 编辑编排」、镜像拉取、网络/卷创建、插件 cron/firewall/ftp 的新建/重置密码；表单保存/重置统一页脚。
- **插件 schema**：白名单新增 `dialog` / `reset` / `close` 与页脚 `submit`；`dialog` 仅允许上游图层参数和 `body` / `footer`；校验后为缺失 `api` 的 form 注入 `{ sendOn: false }` no-op 配置。
- **安装（通过）**：首装 `install.sh --listen 0.0.0.0:18080 --tls off` exit 0，前缀 **`dwkaf8si`**，healthz 200；人工追加 `allow_insecure_http` / `allow_reduced_sandbox`。
- **accept.sh（通过）**：首装与升级后均 **49 PASS / 0 FAIL / 3 SKIP**（PF4 装后端口、A6h 改密机无 initial-admin、B4 未开限流）；权限位、restart/kill-9 自恢复、CSRF→登录→改密→旧会话失效、Docker 29.7.2 全部通过。
- **浏览器（通过）**：概览正常；`/sites/list` 新建站点弹窗底部「重置 / 取消 / 创建站点」齐全；防火墙 rules「放行端口」弹窗提交成功（新增 18081/tcp）；文件「新建目录」成功创建 `ui-check-1122`，同名再提交显示「新建目录失败：目标已存在」且输入保留；`/settings/security` 导入信任公钥弹窗底部「重置 / 取消 / 导入公钥」齐全；全部 0 CompileError / 0 console error。
- **补收口（`76412df`）**：站点详情「新增附加域名」与设置「导入信任公钥」从内联表单改为按钮 + 弹窗，字段/校验/动作不变；最终升级后 accept 仍 49 PASS / 0 FAIL / 3 SKIP。
- **插件 schema 实测**：`com.skiff.firewall` 的 `rules` schema 含 2 个 dialog，页脚均为 reset/close/submit，form 均注入 no-op api。
- **未覆盖（限制）**：cron 官方包在本轮 `/www/skiff/official` 中缺失，cron 弹窗只做了源级/schema 断言；ftp/guard/monitor/sources 插件未装；TLS 形态一/二、aarch64、镜像/网络/卷实际创建、容器新建提交未做浏览器实测；测试产生的 `ui-check-1122` 目录与 18081/tcp 放行规则按要求保留。

**2026-11-21 23:10 干净首装 `a3722fb`：budding 1.8.4 同步，accept.sh 46 PASS / 0 FAIL / 4 SKIP，43 路由 42 PASS**

- **前置**：测试机还原为 T1（Docker 29.7.2 在位，无 skiff / 无 `/etc/skiff`，18080/18081 空闲，UID 201000 与 `skiff-plugins` 组未占用）。
- **上游同步与适配**：上游 1.8.3（`1120d3`）→ 1.8.4（`1121d5`）。主包 1,020,608 B（gzip 326,917）、CSS 574,185 B（67,505）、icons 4,645 + 3,992 B、code pack 90,331 B、terminal pack 395,014 B；JS/CSS 较 1.8.3 分别 −23.5% / −13.3%。**唯一破坏点**：`admin` 的 `content.breadcrumbs` 插槽被移除，壳页改 `props.breadcrumbs: false`（旧写法整页 `CompileError`）；`SHELL_VERSION` 31→32，新增源级断言。
- **安装（通过）**：`install.sh --no-docker --tls off --listen 0.0.0.0:18080` exit **0**；前缀 **`kfzehjpk`**；库 `user_version=22`、`integrity_check=ok`；healthz 200；二进制 17,618,704 B `aea78b89…`；资产包内 `checksums.txt` 复核 exit 0。
- **accept.sh（通过）**：`--group all --install-exit 0` = **46 PASS / 0 FAIL / 4 SKIP**（PF3 无参考时钟、PF4 装后端口、B4 未开限流、D0b 未传 `--pkg-dir`）。关键断言：权限位 711/711/700/600/644/711/660/600 全对；restart 自恢复、kill -9 后 2 s 拉起；B 组完整 CSRF→登录→错误口令 401→改密 200→`initial-admin.txt` 删除→旧会话 303→新会话 200；D0 Docker 29.7.2。
- **8 官方插件（通过）**：`--yes` 逐个安装，grants 采用 manifest `resource_hints`（monitor 含 `path:/proc`、`path:/etc/passwd`），全部 installed + enabled。
- **浏览器巡检（43 路由 / 42 PASS / 0 console error）**：概览、站点列表/运行环境/Web 服务、站点详情 6 个子页、文件（含回收站/配额/查看器）、Docker 6 页、插件 3 页、设置 4 页、9 个插件功能页全部正常。唯一 FAIL 是**故意**访问 `/sites/runtimes/none`，返回 400 +「运行环境不存在」（预期）。
- **交互实测（17/17 PASS）**：登录页 960×571 无滚动条、三字段 + 通栏按钮；新建站点 / 新建运行环境 / 新建目录 / 从归档恢复四个弹窗正常；站点详情「打开文件管理」深链列出目录；`.tar.gz` 行出现「解压」而 `.txt` 行没有（行作用域 `visibleOn` 生效）；容器终端 WS 连上并回显 `skiff-ok`；代码查看器 63 个高亮 token；code/terminal pack 均 200。
- **台账复验（三项收敛）**：
  - `skiff admin reset-password --yes`：旧会话 401 → 新口令登录 → `redirect_to …?force=1`（强制改密）→ 改回已知口令并复登落概览；`initial-admin.txt` 改密后删除。
  - SITE-21 从归档恢复：3 条作业（校验/快照/解包）全成功；`<时间戳>-pre-restore.tar` 生成；被删文件回来、`keep.txt` 回到归档版本、`extra.txt` 保留；审计 `sites.restore` 注明 nginx 已生效；`confirm` 不匹配 400、`archive=../x.tar` 400、损坏归档只失败「校验」作业且站点目录未改动。
  - SITE-07 站点目录：详情显示「站点目录 / 文档根」两行与「打开文件管理」链接（`?path=` 编码正确）；越界实测 **403 `auth.forbidden`（路径含非法片段）**，矩阵原写 400 已更正。
- **未覆盖（限制）**：TLS 形态一/二、aarch64、镜像/网络/卷的创建与删除、PHP 运行环境安装与扩展、FTP/fail2ban 实装、真实备份计划（本轮用人工放入的归档）、≤880px 布局与浅/深主题切换后的逐页比对。

**2026-11-21 09:55–10:22 还原后干净安装 `5bec69b` → 修复 `d5680b6` 升级复验：accept.sh 49 PASS / 0 FAIL / 2 SKIP，3 页 `CompileError` 修复后 3/3 PASS**

- **前置**：测试机还原为 T1（Docker 29.7.2 在位，无 skiff / 无 `/etc/skiff`、18080/18081 空闲、`skiff-plugins` 组与 UID 201000 均未占用）；系统时间与开发机偏差 1 s。
- **安装（通过）**：GitHub Release run `202`（head `5bec69b`）amd64 包 **12,913,533 B** `sha256=3c6d40b6…b424`（侧车 + 包内 `checksums.txt` 双重校验）；`install.sh --no-docker --tls off --listen 0.0.0.0:18080` exit **0**（重复执行仍 0）；前缀 **`88qfm2s8`**；healthz 200；doctor 全绿。形态 C（直接明文），`allow_insecure_http` / `allow_reduced_sandbox` 仍人工追加。
- **accept.sh（通过）**：`--group all --install-exit 0` = **49 PASS / 0 FAIL / 2 SKIP**（PF4 装后端口、B4 限流）。关键断言：权限位 711/711/700/600/644/711/660/600 全对；restart 自恢复、kill -9 后 2 s 拉起；B 组完整 CSRF→登录→错误口令 401→改密 200→`initial-admin.txt` 删除→旧会话 303→新会话 200；D0 Docker 29.7.2 + `--check-only` exit 0 且不改动 `daemon.json`。改密后口令写 `/root/iter/admin-pw.txt`。
- **8 官方插件（通过）**：`--yes` 逐个安装，grants 采用 manifest `resource_hints`（monitor 含 `path:/proc`、`path:/etc/passwd`；firewall 11 个 bin；ftp 16 bin + `object:plugin:com.skiff.firewall`），全部 `installed + enabled`。
- **浏览器巡检（31 页，0 console error）**：概览、任务、作业、站点列表、运行环境、Web 服务、文件、配额、Docker 6 页、插件 3 页、设置 4 页、9 个插件功能页（monitor 总览/进程、firewall、ftp、guard、cron、sources、testkit、files）均正常。
- **首轮 FAIL（3 页，根因同一处）**：`/sites/list`、`/sites/runtimes`、`/files` 整页 `CompileError: 动作 … 的 body 应写在顶层 content.body`。根因：budding 1.8.3 把 `dialog` 的 `body/footer` 登记为动作顶层 `content` 插槽，写在 `props` 里直接编译失败；上一提交只修了 `page`/`form`，7 处弹窗（5 前端 + 2 服务端）漏改。
- **修复（`d5680b6`）**：7 处弹窗统一改为 `action: { type:'dialog', props:{title,width}, content:{body,footer} }`；`SHELL_VERSION` 25→26 破缓存；`budding_boot_create_dialogs_keep_deep_links` 增补 7 条源级回归断言。Release run `203` 四 job 全绿。
- **升级复验（通过）**：run `203` amd64 包 **12,913,194 B** `sha256=152e5902…e257`；`install.sh` exit 0，前缀 `88qfm2s8` 不变，8 插件/配置/口令全部保留；二进制 18,053,136 B `6066afda…e902`。
- **复验 3/3 PASS**：站点列表表格 + 「新建站点」弹窗；运行环境表格 + 「新建运行环境」弹窗；文件页「新建目录」弹窗创建 `verify-dir` 落盘并出现在列表（已清理）；0 console error。
- **未覆盖（限制）**：未装 Web 服务（nginx 未安装，`/sites/web` 显示不可用）、未装 pure-ftpd/fail2ban；TLS 形态一/二、aarch64、站点 SSL 上传弹窗未做浏览器实测（仅源级断言）；`accept.sh` 的 PF4/B4 按设计 SKIP。

**2026-11-21 01:00 干净重装 `26f53ec`：登录页 CSS v23 最终方案（body overflow:hidden !important）+ 上游 budding 1.8.3 同步**

- **安装（通过，干净重装）**：清测试机（stop + 删 /www/skiff /etc/skiff / skiff.service / skiff-http-fwd.service）→ Release run `197`（head `26f53ec`，dispatch `ref=main` + `publish=true`）→ 二进制 `bf1ce05fa711732ede991f959b46bd7cce2e31d39cfd3771d79a51e29202162d`（21,125,136 B，curl GitHub download 直连 + sha256 校验）→ `install.sh --no-docker --tls off --listen 127.0.0.1:18080` exit 0。库 **v22**、`integrity_check=ok`；前缀 **`ewtdhtgx`**；转发层 `skiff-http-fwd.service` active。healthz `{"docker":"available","status":"ok","version":"0.1.0"}`。
- **登录页 CSS v23 最终方案**：
  - `body.skiff-login-shell { overflow: hidden !important; overflow-y: hidden !important; padding-top: 0 !important; }` —— **根治滚动条**。
  - `html:has(body.skiff-login-shell) { overflow: hidden !important; }`。
  - `.login-layout { max-height: 100dvh; box-sizing: border-box; }`。
  - `.login-form-col { flex: 0 0 360px; padding: var(--sp-4) var(--sp-4); }`。
  - `.login-form-col .v-field / .v-btn / .v-label / .v-messages` 大量覆盖 Vuetify 组件间距。
  - `SHELL_VERSION=23`（缓存键 `?v=23`）。
- **上游 budding 1.8.3 同步**：`C:\dev\budding\package.json 1.8.3`，commit `1120d3`。主包 1304KB + CSS 647KB + icons + 分包（`.mjs` ESM 改名为 `.min.js`）。
- **浏览器验收（browser_use 4/4 PASS）**：
  1. ✅ CSS href `skiff.css?v=23`，budding `?v=1.8.3` 正确加载；curl HTML 确认 `body class="skiff-login-shell"` + login-layout/login-form-col DOM 完整；CSS 文件确认 body 规则 `overflow: hidden !important` 就位。
  2. ✅ **无垂直滚动条**：`htmlScrollHeight=571 === htmlClientHeight=571`，`bodyScrollHeight=571 === bodyClientHeight=571`（960×571 视口）；当前部署概览页同样 scrollHeight==clientHeight。
  3. ✅ 所有表单字段（用户名 top:95/bottom:127、密码 top:149/bottom:182、动态码 top:204/bottom:236）+ 登录按钮（top:278/bottom:314）完全可见；主题切换按钮（top:515/bottom:551）在视口内。
  4. ✅ admin / SGJRnUdksZfxZc8a 登录成功 → 概览页（容器/网站/插件/工具菜单正常渲染）。
- **已实测数字**：
  - 二进制 sha256 `bf1ce05f…2162d`（21.1 MB）
  - 入口前缀 `ewtdhtgx`
  - healthz uptime 6s 时 docker=available
  - install.sh exit 0
  - schema v22 / integrity_check ok
  - 转发层 curl 返回 `<title>登录 · Skiff</title>` + `body class="skiff-login-shell"`
- **已知限制**：本机 L1 门禁因可用内存 3 MB < 1024 MB 被 cargo 闸门拒绝，未跑 fmt/clippy；编译级以 Release run `197` 四 job 全绿为准；budding 分包格式从 UMD 改 ESM `.mjs`，改名为 `.min.js` 运行时兼容性需进一步验证（本轮只验登录页，登录流程正常说明主包 OK）。

**2026-11-20 23:34 全新安装 `6d98116`：登录页 CSS 修复（SHELL_VERSION 20），浏览器验收全 PASS**

- **安装（通过，T1 空库）**：Release run `194`（head `6d98116`，dispatch `ref=main` + `publish=true`），amd64 包 **18,016,272 B** `sha256=948b9312…7977`（二进制）；`install.sh` exit 0；库 **v22**、`integrity_check=ok`；前缀 **`fp9ji8uu`**；转发层 `skiff-http-fwd.service` active。
- **登录页 CSS 修复**：body `overflow: hidden` → `overflow-x: hidden` + `margin: auto` 弹性居中；padding/gap/basis 全面收紧；`min-height: 100dvh` 替代 `100vh`；`SHELL_VERSION` 19→20。
- **浏览器验收（browser_use）**：
  1. ✅ 布局正确：品牌面板（456px）+ 表单卡片（380px）并排，24px gap。
  2. ✅ **1080px 视口下无垂直滚动条**：`scrollHeight = clientHeight = 1080px`，内容总高 ~637px。
  3. ✅ 所有元素完全可见：品牌特性列表底部 452px < 1080px，登录按钮底部 406px < 1080px。
  4. ✅ admin / R2aJJDUKJyZDGnJP 登录成功 → `/fp9ji8uu` 仪表盘（占位视图）。
- **已知限制**：本机 L1 门禁因可用内存 3 MB < 1024 MB 被 cargo 闸门拒绝（exit 3），编译级以 Release run 194 四 job 全绿为准；未装官方插件。

> 本节只保留验收断言、实测数字与限制，是「测试机上发生了什么」的唯一来源；各轮的代码改动、根因与提交链见 [changes/2026-11-20.md](../../changes/2026-11-20.md)，用例编号与预期结果见 [TEST_MATRIX.md](../TEST_MATRIX.md)，未闭环项见 [TEST_LEDGER.md](../TEST_LEDGER.md)。

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
