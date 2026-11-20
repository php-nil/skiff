# 验收结论：当前轮与最近两轮（§8）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §8 最近三轮 + 历史轮次台账（其余轮次已精简）（2026-11-16 整理）。

---

## 8. 最近一次验收结论（截至 2026-11-20 18:26 `9d8d7a4` 全新安装：L1 门禁全绿 + accept.sh 44 PASS + 浏览器 8 页面 0 JS 错误）

> 本节只保留验收断言、实测数字与限制，是「测试机上发生了什么」的唯一来源；各轮的代码改动、根因与提交链见 [changes/2026-11-20.md](../../changes/2026-11-20.md)，用例编号与预期结果见 [TEST_MATRIX.md](../TEST_MATRIX.md)，未闭环项见 [TEST_LEDGER.md](../TEST_LEDGER.md)。

**2026-11-20 18:12–18:50 全新安装 `9d8d7a4`：L1 门禁 4 通道全绿 → Release run 44717281516 → accept.sh 44 PASS / 0 FAIL / 5 SKIP → 浏览器 8 页 PASS，0 JS error**

- **前置：L1 门禁 4 通道全绿**（根 workspace fmt + clippy --all-features；plugins workspace fmt + clippy 均 exit 0）。这是本轮最关键的改动——从 pre-existing 单测 FAIL 收敛到 0：`69efce0` Clippy/xtask/assets 三项引入性失败 → `bdf0944` ExecRequest::parse dead_code → `7882e88` 修复 10 个 pre-existing 单测 FAIL → `edcd9cb` Clippy unused variable + rustfmt → `9d8d7a4` 剩余 3 项 default_view + CSRF + p2_row site_page.rs。
- **安装（通过，T1 口径全新空库）**：Release run `44717281516`（head `9d8d7a4`，publish 成功，v0.1.0 资产覆盖），包 **12,895,953 B** id=771372800，`sha256=4211de8c…8acd`（侧车 / checksums.txt / 测试机 sha256sum 三方一致）；二进制 `/usr/local/bin/skiff` static-pie 19 MB；`install.sh --no-docker --tls off --listen 127.0.0.1:18080` exit 0；daemon schema **v0→v21** 完整迁移；前缀 **`dm9sdqjv`**；`initial-admin.txt` 口令自动生成（B 组后重设多次）；`skiff.service` active。**临时修补**：install.sh 未自动写 `allow_reduced_sandbox=true` + `allow_insecure_http=true`（需手动追加，契约缺口待修复）。
- **转发层（Python /root/iter/skiff-fwd.py）**：0.0.0.0:18081 → 127.0.0.1:18080，剥掉 `Sec-Fetch-Site/Mode/Dest/Origin/Referer/Access-Control-*` 头让 CSRF dev_http 放行。PID 1716，curl 18081/healthz 返回 200。
- **8 官方插件（全部验签+安装+启用）**：`skiff plugin install <pkg> --resource path:/www --yes`；签名 key_id 一致 `ed73de3e8…cd47`；沙箱降级提示正常。
- **accept.sh 全组（通过，preflight + A + B + D0 = 44 PASS / 0 FAIL / 5 SKIP）**：
  - preflight 8 项：PF1 root ✅ PF2 systemd ✅ PF5 磁盘 ✅ PF6 kernel 6.12 ✅ PF7 landlock ✅ PF8 cgroups v2 ✅（PF3/PF4 SKIP 合理）
  - A 组 22 项：A3a enabled ✅ A3b active ✅ A4a/A4b healthz ✅ A5a–A5d 前缀+跳转+404 ✅ A6a–A6h 权限位全部正确（711 root:root / 711 plugins / 700 backup / 600 skiff.db / 644 config.toml / 711 run / 660 kernel.sock / 600 initial-admin.txt）✅ A7/A8a/A8b health+doctor ✅ A9a/A9b restart 自恢复 ✅ A10a kill-9 后 2 s 拉起 ✅ A11 --help/version 不落文件 ✅
  - B 组 14 项：B0 ✅ B1 登录页 200 ✅ B1a csrf_token len=119 ✅ B2b 错误口令 401 ✅ B2b1 文案 ✅ B2 初始口令登录 200 ✅ B2a 不强制改密 ✅ B5 缺 CSRF 403 ✅ B3a 设置数据 200 ✅ B3c 改密成功 ✅ B3d initial-admin.txt 自动删除 ✅ B3e 旧会话 303 踢回登录页 ✅ B3f 新会话 200 ✅（B4 SKIP 未开限流）
  - D0 2 项：D0a Docker 29.7.2 可用 ✅（D0b SKIP 路径）
- **浏览器验收（8/8 PASS，0 JS 错误）**：登录页渲染（标题"登录 · Skiff" + 用户名/口令/TOTP + 登录按钮）→ admin 登录成功跳 `/dm9sdqjv` → 概览仪表盘（容器 0/插件 8/运行环境正常/异常 0）→ 容器页（空态表格 + 全部/仅异常筛选）→ 插件已装（8 官方列表正确）→ 文件管理（nginx/php/runtime 目录 + 9 导航入口）→ 设置页（账号安全 + 两步验证 + 改密表单）。**每页面 console 均空数组**。
- **本轮发现的问题（已修复）**：① accept.sh 首轮 B 组 4 FAIL 根因是密码文件 `admin-pw.txt` 含标签 `口令: `，修复后二轮 14 PASS；② `/ui/v1/auth/login` 无入口前缀（设计意图：JSON API 命名空间独立于前缀壳），accept.sh `BASE=http://127.0.0.1:$PORT` 正确构造。
- **未覆盖（限制）**：B4 登录限流（需 `--with-rate-limit`）；FTP、防火墙、站点 SSL、TLS 形态一/二、aarch64 未复跑；站点本轮未建（`sites=0`）；accept.sh E 组未实现；install.sh 自动追加 `allow_reduced_sandbox/allow_insecure_http` 契约缺口待修。
- **收尾状态**：`skiff.service` + `skiff-fwd.py` (PID 1716) 均 active；8 插件已装；入口 `dm9sdqjv`；`/root/iter/` 下保留 `pkg/rel-9d8d7a4/` + `accept.sh` + `admin-pw.txt` + `skiff-fwd.py` + `reset_admin.py`。

**2026-11-20 10:42–10:58 空库首装 `a02f1d0`：accept.sh 48 PASS / 0 FAIL / 3 SKIP，浏览器 9 页面 0 console error，write_atomic A6e 根治**

- **安装（通过）**：Release run `44574500026`（head `a02f1d0`，publish 成功，v0.1.0 资产覆盖），包 **12,821,377 B** id=768570200，`sha256=2fd680b5…b5c`（API digest / 本地 / 测试机三方一致）；二进制 `/usr/local/bin/skiff` `b7d4556f…111`；`install.sh --no-docker --tls off --listen 127.0.0.1:18080` exit 0；daemon schema **v0→v21** 完整迁移（7 条日志无跳过）；前缀 **`t2mhspmy`**；`initial-admin.txt` 口令 `5PvxpDNdQ8QyR9Ny`；`skiff.service` active。**临时修补**：install.sh 未自动写 `allow_reduced_sandbox=true` + `allow_insecure_http=true`（需手动追加，本轮发现契约缺口）。
- **8 官方插件（全部验签+安装+拉起，reduced sandbox）**：`com.skiff.{cron,files,firewall,ftp,guard,monitor,sources,testkit}` 签名 key_id 一致 `ed73de3e8…cd47`；`com.skiff.guard` / `com.skiff.monitor` 巡检由内核调度器驱动（supervisor 按需拉起，`edf2a75` plugin rpc 通道改为 HTTP 后插件不再自带定时器）。
- **accept.sh（通过，两轮复验 48 PASS / 0 FAIL / 3 SKIP）**：preflight A D0 = **35 PASS / 0 FAIL / 1 SKIP**（PF4 装后端口占正常跳过）；B 组补 `allow_insecure_http=true` 后二轮 **14 PASS / 0 FAIL / 1 SKIP**（B4 登录限流需 --with-rate-limit）。关键断言：**A6e config.toml 644 root:root ✅（`a02f1d0` write_atomic 在 rename 覆盖后显式 chown + chmod 恢复目标属性，根治）**；A6d skiff.db 600 root:skiff-plugins；A9 restart 自恢复（health 200）；A10 kill -9 后 systemd 2 s 拉起（旧 3831 → 新 3856）；A11 --help/version 不落文件；B0 首轮 SKIP 是 dev_mode 判定挡，二轮全过；B1a csrf_token len=119；B2 初始口令登录 200；B2b 错误口令 401「用户名或口令不正确」；B3c 改密成功 200；B3d initial-admin.txt 自动删除；B3e 旧会话失效 303 踢回登录页；B5 缺 CSRF 403。
- **浏览器全量抽查（通过，9 页面 0 console error）**：修复转发层剥掉 `Sec-Fetch-Site / Origin / Referer` 后，跨端口 `10.10.15.4:18081` 访问被 Skiff dev_http 放行。登录页 → 概览（系统状态）→ 容器（空态表格+新建按钮）→ 镜像（nginx:alpine / php:8.5-fpm-alpine）→ 网络（bridge/none/host）→ 卷（空列表+创建表单）→ Compose（YAML 编辑器）→ Docker 设置（镜像加速配置）→ 插件已装（8 个）→ 文件管理（nginx/php/runtime 目录）。**每页面均无 blank、无 error/warning**。
- **install.sh / accept.sh 契约缺口（本轮发现，待决策）**：① install.sh `--tls off` 只写 `tls_enabled=false` + `listen=127.0.0.1:18080`，但官方插件需要 `allow_reduced_sandbox=true`（沙箱降级）、accept.sh B 组需要 `allow_insecure_http=true`（dev_mode 判定）——建议 install.sh 在检测到 loopback + TLS off 时自动追加。② accept.sh `dev_mode()` 只认非注释 `allow_insecure_http = true`，不认 `tls_enabled = false`（同样"允许明文"语义）——应加 `tls_enabled = false` 分支。
- **转发层位置（待整改）**：当前 Python 脚本 `.workbuddy/skiff-http-fwd.py`（gitignore），建议做成 deploy/ 正式资产或 systemd unit，每轮部署自动拉起。
- **未覆盖（限制）**：FTP、防火墙、站点 SSL、TLS 形态一/二、aarch64 未复跑；站点本轮未建（`sites=0`）；accept.sh 未实现 E 组；本地 clippy 未完整跑（CI run 184 已全绿）。
- **收尾状态**：`skiff.service` + `skiff-http-fwd.py`（PID 2490）均 active；8 插件已装；入口 `t2mhspmy`；`/root/iter/` 下保留 `skiff.tar.gz` + `accept.sh` + 两轮 accept 输出 + 转发层脚本。
**2026-11-19 08:31–09:40 还原后全新安装 `f5267ec` + 覆盖 `8f78221`：accept.sh 27 PASS / 0 FAIL，三项需求浏览器实测通过**

- **安装（通过，全新 + 升级）**：全新 `44428042366`（head `f5267ec`）→ 覆盖 `44435964314`（head `8f78221`），两次 `install.sh --no-docker --tls off --listen 127.0.0.1:18080` 均 exit 0；末次包 **13,272,804 B** `f04eafe8…dba7`（API `digest` / 本地 / 测试机三方一致）；二进制 19,179,408 B `e4284515…bfc4`；库 **v21**、`integrity_check=ok`；前缀 **`zgvvx5xz`**；配置加 `allow_reduced_sandbox=true` 后官方 8 插件全装。
- **accept.sh（通过）**：A + D0 = **27 PASS / 0 FAIL / 2 SKIP**；B 组因安装配置为 TLS/prod 档（`tls_enabled=false` 显式关闭）整体 SKIP，`initial-admin.txt` 保留、未改密；preflight 全过（landlock、Docker 29.7.2、18080/18081 空闲）。
- **新建 PHP 环境可选 apk 源（通过，本轮新增）**：`GET /ui/v1/runtimes/catalog` 返回 5 个候选 + 当前态；插件页固定 TUNA 后目录接口 `apk_mirror_current={fixed:true,base:…tuna…,label:"清华 TUNA"}`；新建弹窗浏览器实测出现「软件包源（仅 PHP 生效）」且**预选清华 TUNA**（类型 PHP），0 console error；`POST /ui/v1/runtimes` 带 `apk_mirror` 创建 `php-8.5`，任务 **10 s** 完成（此前默认源 30 分钟超时），容器内 `/etc/apk/repositories` 为 tuna `v3.24/{main,community}`，`php -v` = **8.5.12**；非法源 `ftp://…` 返回 **400**。
- **软件源插件复查（通过，本轮修复 3 处）**：`status` 的 `json!` 赋值修复后插件 workspace 恢复可编译；`applyApk` 改读 `r.base`；补 `ui/schema/main.json` 后入口不再退回 `?legacy=1`（原先 legacy 桥无 nonce，`rpc("status")` 永不返回、页面停在「读取中…」）。浏览器实测：apk 表 5 行、当前态「清华 TUNA（已固定…）」、`测速全部` 出真实速率（官方 2.5 MB/s、TUNA 10.8 MB/s、USTC 10.0 MB/s、阿里云 286 KB/s、腾讯云 28.8 MB/s），0 console error。apk 「恢复自动探测」写空缓存文件（`-s` 判空 = 未固定）。
- **架构收口回归修复（通过，由 CI 用例暴露）**：`MENU_ROUTE` 常量被 axum 当两段参数导致所有 SSR 页 500（改 `menu_route(prefix)`）；`verify_detached` 二次拼域前缀导致发布自检验签全失败（改直接验签）；`parse_public_key` 补 `is_weak()`。同批 CI 从 **52 失败** 收敛到 **全绿**（`44434687589`）。
- **未覆盖（限制）**：B 组（改密/限流）本轮按配置跳过；FTP、防火墙、站点 SSL、TLS 形态一/二、aarch64 未复跑；站点本轮未建（`sites=0`），PHP 只验了运行环境与容器内源，未复跑建站链路；E 组未实现。
- **收尾状态**：`php-8.5` running、apk 源固定为 TUNA；测试机现场 `/root/iter2/pkg/`、`/root/iter/accept.sh`、`/root/iter/skiff-http-fwd.py`；`skiff` 与 `skiff-http-fwd` 均 active。

**2026-11-18 21:15–21:40 就地覆盖 `d0c8cc4`（三项修复）：accept.sh A+D0 26 PASS / 0 FAIL，25 页 0 console error**

- **安装（通过）**：Release run `44310262459`（head `d0c8cc4`）success；包 **11,951,277 B** `fc2269cb…69b2`（API `digest` / 本地 / 测试机三方一致）；`install.sh --no-docker --tls off --listen 127.0.0.1:18080` exit 0；二进制 **19,146,512 B** `d7a49eaa…1f53`；库 **v21**、`integrity_check=ok`；前缀 `2u3uiv85` 未变；7 插件 / 2 站点（`uiaccept.local`、`matrix.local`）/ `php-8.5` running 全保留。
- **accept.sh（通过）**：A + D0 = **26 PASS / 0 FAIL / 2 SKIP**（A6h `initial-admin.txt` 已不在、B4 未在本轮开启——B4 已于 20:58 单独实测通过）。A 组含重启自恢复（health 200）、前缀不变、`kill -9` 后 systemd 2 s 拉起、`doctor` 无 ERROR。
- **登录页回车（通过，本轮修复）**：登录页脚本链为 `theme-init.js` + `budding-{ver}.min.js` + **`form-enter-submit.js?v=6`** + `budding-login.js`；浏览器只按回车（不点按钮）→ `POST /ui/v1/auth/login` **200** → 落到 `/{p}`，0 console error。壳页脚本链为 `budding-1.8.1-r2.min.js` + `form-enter-submit.js?v=6` + `budding-boot.js?v=6`，全部 200。
- **容器生命周期与错误分类（通过，本轮修复）**：`nginx:alpine` 建容器 200 → 列表/详情 → 启动（`docker inspect Running=true`）→ 日志 200 → 停止（`Running=false`）→ 删除（无残留）= 12 项全过；重名 → **409 `domain.conflict`**、镜像不存在 → **400 `shape.invalid`**（修复前两者都是 503 `dependency.unavailable`）。
- **PHP 站点回归（通过）**：SITE-02 / 09 / 11 / 14 / 15 共 **17 项断言**全过（删站带 `confirm` 后重建、`PHP Version 8.5.12`、socket `0:65534 660`、`pdo_mysql` 生效、`conf.d` 与 `conf.d-skiff` 各一次、无 `already loaded`）。
- **B4 登录限流（通过，首次上机）**：同 IP 连续 5 次错误口令（均 401）后，第 6 次用**正确口令**仍 **503 `dependency.unavailable`**「登录失败次数过多，请 299 秒后重试」；封禁 300 s，重启进程即清。
- **SITE-12 站点 SSL（通过，本轮首次上机）**：`action=generate` 生成自签（`CN=matrix.local`，`expires Feb 20 2029`）→ 详情 `https.enabled=true` / `ssl.cert_exists=true` → `curl -k https://…` **200**；`action=force enabled=true` → HTTP **301** 跳转、HTTPS 仍 200；`force=false` → HTTP 200；`action=disable` → HTTPS 不再响应（`000`）、HTTP 仍 200，证书文件保留（`cert_exists` 仍 true）。共 **13 项断言全过**。
- **未覆盖（限制）**：viewer 账号不存在，`plugin/*` 403 分支仍只有单测；TLS 形态一/二、FTP、防火墙、aarch64 本轮未复跑；SITE-15 Debian 家族未跑。
- **收尾状态**：`matrix.local` 的 HTTPS 已按验收脚本末尾关闭（HTTP 200 可用），证书留在 `/www/sites/matrix.local/ssl/`。

**2026-11-18 20:27–20:50 PHP 运行环境与站点验收（沿用 `bcdfd22` 安装）：SITE-02 / 09 / 11 / 14 / 15 共 17 项断言全过**

- **运行环境安装（通过，含缺陷修复前后对比）**：`php-8.5`（`php:8.5-fpm-alpine`，扩展 `pdo_mysql`）首装第 1 次在「安装扩展」**30 分钟被判超时**（`job-1795003035-274fc34d8`，`error_class=timeout`），重建后第 2 次 **559 s 成功**（`job-1795004860-27d8f8389`，`php -m` 见 `pdo_mysql`）。取证见 [changes/2026-11-18.md](../../changes/2026-11-18.md)「PHP 运行环境首次安装超时」。
- **SITE-02 运行环境建站（通过）**：`POST /ui/v1/sites`（`site_type=runtime`、`runtime_id=php-8.5`）→ 200 `created:true`；`/www/sites/matrix.local` 下 `index` / `log` / `ssl` / `conf` 四目录齐全，`index.php` 探针生成；`Host: matrix.local` → **200** 且页面含 **`PHP Version 8.5.12`**；重复建站 409 `domain.conflict`、删除需 `confirm` 等于域名（删除保护生效）。
- **SITE-11 socket / 端口 / 网络（通过）**：`/www/runtime/php/php-8.5/php-fpm.sock` = `socket 0:65534 660`；`skiff-web-nginx` `NetworkMode=host` 且 `ss -ltnp` 见 `:80`；PHP 容器 `/www:/www:true` 读写挂载，`www.conf` / `php.ini` / `conf.d` / `skiff-php-init.sh` 均只读挂载。
- **SITE-14 nobody 组（通过，Alpine）**：容器内 `getent group nobody` = `65534`；`docker logs` 无 `cannot get gid for group 'nobody'`。注：Debian 家族 65534 的组名是 `nogroup`、Alpine 是 `nobody`，断言按 **gid** 而非组名。
- **SITE-15 / SITE-09 扩展（通过）**：`php --ini` 同时扫到 `conf.d` 与 `conf.d-skiff`，无 `Module already loaded`；`pdo_mysql` 生效。`redis`（PECL，需编译）安装任务 succeeded → `php -m` 出现、`extension_loaded('redis')=yes`、站点 200 → 重建（`action/rebuild`）后仍加载 → 卸载任务 succeeded → `php -m` 消失、`site_runtimes.extensions` 回到 `pdo_mysql,openssl,opcache,mbstring,curl`、站点仍 200。
- **确认保护（通过）**：`action/rebuild` 不带 `confirm` → 409「该运行环境已被 1 个站点使用（matrix.local），rebuild会影响全部站点；确认后重试」；带 `{"confirm":true}` → 200 提交后台任务。
- **未覆盖（限制）**：SITE-15 的 Debian 家族（`php:8.5-fpm`）未跑，本轮只有 Alpine 实例；站点 SSL（SITE-12）与 PHP 会话/上传类用例未跑。

**2026-11-18 19:27–19:55 机器还原后全新安装 `12d8ea2` + 覆盖 `bcdfd22`：accept.sh 49 PASS / 0 FAIL，25 页 0 console error**

- **安装（通过，全新 + 升级）**：全新安装 `12d8ea2`（包 11,951,970 B `2445983b…6e21`）→ 覆盖 `bcdfd22`（包 11,953,417 B `523a352e…e970`），两次 `install.sh --no-docker --tls off --listen 127.0.0.1:18080` 均 exit 0；二进制 19,142,416 B `d9284680…6f23`；库 v21 / `integrity_check=ok`；前缀 `2u3uiv85`；配置 sha `5bfb00d9…16af8` 升级前后未变。
- **accept.sh（通过）**：首装轮 preflight+A+B+D0 = **49 PASS / 0 FAIL / 2 SKIP**（B4 限流未开、E 组未实现）；覆盖升级轮 A+D0 = **34 PASS / 0 FAIL / 2 SKIP**。
- **旧 UI 清理（通过）**：22 条规范路径全 200 壳页；`/m/overview` 仍 SSR；`/m/core/{docker/containers,sites/list,files,settings/security,store}` 全 404；`GET /ui/v1/menu` 34 条目无 `m/core`；壳页 DOM 内无 `/m/core` 链接；站内 24 条链接全部 200。
- **budding 1.8.1-r2（通过）**：只加载 `budding-1.8.1-r2.min.{js,css}` / `budding-{code,terminal}-1.8.1-r2.min.js` / `budding-icons-1.8.1.*`，`budding-boot.js?v=5`；`.bud-admin` 计算样式 `border:0 / radius:0`（上游去外框生效）；会话失效走 `redirect` 组件（「登录已失效 / 2 秒后自动跳转 / 立即登录」→ `login?next=…`）。
- **交互（通过）**：计划任务回车筛选（`/tasks?q=audit` 6 → 1）、审计筛选、站点详情六页签切换、新建站点弹窗非法域名内联报错、文件管理进入 `/www/sites`、插件列表 7 个、菜单隐藏/重置、任务启停与立即运行、插件停用/启用、站点停站 404 / 启站 200、文件 mkdir/write/read/rename/remove 闭环、Web 服务安装（`skiff-web-nginx` running）、别名添加与 Host 200。
- **未覆盖（限制）**：无普通用户账号，`plugin/*` 的 403 分支未上机（单测覆盖）；B4 登录限流未跑；Docker 容器创建（无镜像）、FTP、防火墙、TLS、aarch64 未复跑（本轮未触及）；`/m/plugin/{id}` SSR 工作台仍 200，`/m/plugin/{id}/main` 404（工作台只认 `?view=`，非缺陷）。

**2026-11-17 23:17–2026-11-18 06:20 机器还原后全新安装 + 五轮覆盖安装，八项任务回归 0 FAIL**

- **安装（通过，全新 + 升级路径）**：全新安装 `a9a0b59` → 覆盖 `cb81325` → `8004dc1` → `286a5b7` → `36840f2`（末次资产 12,121,107 B `7ac7f9ce…321e`，API `digest` / 本地 / 测试机三方一致）。末次：`sh install.sh --no-docker --tls off --listen 127.0.0.1:18080` **exit 0**；二进制 19,646,224 B `2aede7a7…877a`；库 `user_version=21`、`integrity_check=ok`；配置 sha `fad99a3d…` 未变；权限 `711/711`；前缀 `kyuyjr44`；`/healthz` 18080/18081 均 200。
- **站点（通过）**：`php.local` / `node.local` / `rust.local` 均 200，字节数 83,105 / 51 / 50；运行环境 PHP 8.5.14、Node 22（9200）、Rust 1.98（9100）全部 running；Web 服务 OpenResty 已安装。
- **budding 1.8.1 + redirect（通过）**：壳页资产为 `budding-1.8.1.min.{js,css}`；强制 401 实测显示「登录状态已失效」+「2 秒后自动跳转」+「立即登录」，随后跳 `login?next=…`；壳脚本 9 处 401 分支统一接入。
- **旧版页面切换（通过，菜单口径）**：浏览器逐一点击侧栏 27 个条目，全部落在新壳路由（`/sites`、`/runtimes`、`/containers`、`/tasks`、`/jobs`、`/files`、`/audit`、`/settings*`、`/plugins/<id>/<view>`），无 `id="skiff-app"` 旧页。`/{p}/m/core/...` 仍按契约作为旧 SSR 兼容深链保留，不是菜单入口。
- **界面与设置（通过）**：界面设置 5 套预设（星蓝/翡翠/石墨/暖岩/玫瑰）、15 个色卡；设置/菜单设置/审计/站点详情（4 指标条，明细不重复）/Web 服务（指标化，无卡片套卡片）/文件插件（目录与文件拆表、编辑区等宽）逐页截图无重叠、无 console error。
- **防火墙（通过，含本轮修复）**：后端 nftables、fail2ban 已安装运行中；`port.allow {port:"59991",protocol:"both"}` → 列表出现 `tcp/udp` 两条（handle 10/11）→ 按 id 逐条删除 → 链清空。此前 UI 表单端口为字符串会报 `端口需在 1..65535`，已由 `36840f2` 修复（`u64_param`）。
- **FTP（通过）**：pure-ftpd active；账号 `skiff_php-local_866107650_ftp-php`（home `/www/sites/php.local/index`，uid/gid 1000）登录、上传/下载通过；`accounts` 返回 1 条、`sites` 返回 3 条。
- **文件能力（通过）**：`files.list` 读取 `/www/sites/php.local/index` 成功，`read_text` 可读（72 B）；内核文件页 5 个条目正常渲染。
- **未覆盖（限制）**：FTP 账号仅支持 PHP 站点（node/rust 站点由内核标记 `supported=false`：「该运行环境不是 PHP，v1 未验证其文件身份」）；aarch64 资产未实机验证；`accept.sh` 全组未复跑，沿用 2026-11-17 00:06 轮全组结论（29+14+4 PASS）。
- **现场**：`/root/iter/rel-36840f2/`、`admin-pw.txt`、`skiff-http-fwd.py`；防火墙验收规则已清理，`inet skiff` 链为空。

**2026-11-17 19:51–19:56 `0816073` 资产就地覆盖安装 + testkit 新壳验收：四个视图壳内原生渲染、RPC 结果图正确，0 FAIL**

- **安装（通过，升级路径）**：资产 12,153,912 B `5f6bcbc7…1ee38`（本地 sha256 / 侧车 / `checksums.txt` 三方一致，测试机 `sha256sum -c` OK）→ `sh install.sh --no-docker --tls off --listen 127.0.0.1:18080` **exit 0**。新二进制 19,855,120 B `07564549…`；库 `user_version=21`、`integrity_check=ok` 未变；配置 sha `5bfb00d9…16af8` 未变；权限 `711/711` 保持；前缀 `cb5mgdk7` 未变；`/healthz` 18080 与 18081 均 200。
- **插件自动升级（通过）**：覆盖安装后启动日志为 `插件安装完成 plugin=com.skiff.testkit version=0.1.0` + `官方插件内容已随发布包更新（同版本重装）`；库内该插件 `api=1.3`（原 1.2），`/www/skiff/official/com.skiff.testkit-0.1.0.wbpkg` 329,175 B；7 个官方插件全在。
- **testkit 四视图（通过，1440×900，除预期 400/403 外无 console error）**：
  - `/plugins/com.skiff.testkit/main`：三级菜单 `总览与 RPC / 能力面自检 / 菜单与槽位`，当前项「总览与 RPC」，**无 iframe**；进程与授权卡片显示 `API：1.3`、`PID`、`data_dir` 与六项 grants。
  - `/check`、`/group`：当前项各自正确，无 iframe；表格（检查项 / 预期 / 验证什么）与按钮组正常渲染。
  - `/single`：**不出现三级菜单**，侧栏高亮落在「测试台 · 单视图」，`API：1.3`。
- **RPC 结果图（通过，本轮新增能力）**：`ping` → `HTTP 200 · 成功分支` + 插件原样 JSON（`pong=true`）；`fail` → `HTTP 400 · 失败分支` + `shape.invalid` / `-32602` 原样 message；`fs 越界` → `HTTP 403` + `auth.forbidden`。请求行回显 `POST /cb5mgdk7/plugins/com.skiff.testkit/rpc` 与 `method`/`params`，失败不再被 toast 吞掉。
- **壳层资产版本（通过）**：壳页对 `budding-boot.js` 与 `skiff-shell.css` 均带 `?v=3`（`assets::SHELL_VERSION`），实测新壳脚本已生效（`inspect` 行为可见）。
- **未覆盖（限制）**：accept.sh 全组未复跑——本轮服务端接口无变更（改动集中在 testkit 包、壳页 JS、`spa.rs` 壳页引用、`pkgfmt` 白名单），沿用 2026-11-17 00:06 轮全组结论。`overview.actions` / `overview.side` 槽位在新壳仍未接（只原生渲染 `overview.health`），概览页看不到 testkit 的挂载卡片。
- **现场**：站点 `a.com`（php-8.5 运行中）、`skiff-web-nginx` running；`/root/iter/` 新增 `rel-0816073/`。

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
