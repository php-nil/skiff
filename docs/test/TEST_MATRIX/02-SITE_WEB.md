# D 网站与运行环境 / E Web 服务

> 上级索引：[TEST_MATRIX.md](../TEST_MATRIX.md) ｜ 拆自 `docs/test/TEST_MATRIX.md` 原 D–E（2026-11-16 整理，正文未改）。

---

## D. 网站与运行环境

> 重跑触发：改了建站表单、vhost 生成、域名/别名、站点目录、运行环境安装/启停/解绑、扩展、socket/端口、SSL、站点备份与恢复、删除保护、PHP 容器身份初始化脚本或池模板、PHP 扩展安装脚本的包管理器分支。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| SITE-01 | 站点列表与状态列 | `[上机]` 至少 1 个站点 | 打开 `/$P/sites/list`；检查列表、默认标记、状态列 | 列表由壳页渲染、数据来自 `GET /ui/v1/sites`（SSR 列表已删除，无第二套表格）；默认站带「默认」徽标；状态列同时显示运行状态与 HTTPS 状态（已启用 / 证书缺失 / 已上传 / 未配置） | □通过 / □失败 | `crates/web/src/handlers/ui/sites.rs`、`crates/web/static/budding/budding-boot.js` |
| SITE-02 | 运行环境建站（PHP） | `[上机]` nginx 与 php-8.5 已安装 | 新建 `matrix.local`，类型「运行环境 / PHP」；访问 HTTP；打开详情概览 | 建站 303；HTTP 200 且含 `PHP Version 8.5`；四子目录 `index/log/ssl/conf` 生成；HTTPS 需完成 SITE-12 后才 200 | □通过 / □失败 | `crates/app/src/sites.rs`、`runtime_catalog.rs`、`templates/site_new_dialog.html` |
| SITE-03 | 静态站建站 | `[上机]` nginx 就绪 | 新建 `matrix-static.local`，类型「静态」；在 `index/` 放自定义 `index.html` | 建站 303；访问返回自定义内容；无 PHP 解析 | □通过 / □失败 | `crates/app/src/sites.rs`、`templates/site_new_dialog.html` |
| SITE-04 | 反向代理建站 | `[上机]` 上游 HTTP 服务已起（含 18081） | 新建 `matrix-proxy.local` 指向 `127.0.0.1:18081`；访问；查上游日志 | 站点 200；上游自定义响应头（如测试服务的 `X-Upstream`）原样透传；上游收到 `Host: matrix-proxy.local` | □通过 / □失败 | `crates/app/src/sites.rs`、`templates/site_new_dialog.html` |
| SITE-05 | Rust 运行环境建站 | `[上机]` rust 运行环境已安装、镜像可拉取 | 新建 `matrix-rust.local`，类型「运行环境 / Rust」；启动运行环境；访问站点 | vhost 生成 303；容器 running；站点返回 Rust 服务响应 | □通过 / □失败 | `crates/app/src/sites.rs`、`runtime_catalog.rs` |
| SITE-06 | 域名与别名校验 | `[上机]` 已登录 | 依次提交重复域名、非法域名、IP、通配符；再为已有站点加别名 | 重复/非法/IP/通配符被拒并给内联错误（`POST /ui/v1/sites/{id}/domains` 返回 `site.*` 错误码）；合法别名保存后同一 vhost 生效 | □通过 / □失败 | `crates/app/src/sites.rs`、`crates/web/src/handlers/ui/sites.rs`、`crates/web/static/budding/budding-boot.js` |
| SITE-07 | 站点目录展示与越界保护 | `[上机]` 已建站 | 打开站点详情概览，确认「站点目录」「文档根」两行及其「打开文件管理」链接；点链接进文件管理，再手工请求 `GET /ui/v1/files?path=<站点根>/…` 试越界 | 详情页显示宿主绝对路径，链接跳到 `/{p}/files?path=<编码后的路径>` 并列出该目录（产品**不提供**站点目录浏览页签）；`../log`、`../ssl`、绝对路径越界一律 **403 `auth.forbidden`（「路径含非法片段」）**（契约 [KERNEL_FS_CONTRACT/02](../../contract/KERNEL_FS_CONTRACT/02-AUDIT_TRASH_QUOTA.md) §121：越界 403）；`/www/sites` 为 0755，数据根 `/www/skiff` 与 `plugins` 为 0711 | □通过 / □失败 | `crates/web/src/handlers/ui/site_page.rs`（`overview_section`）、`crates/app/src/admin_fs.rs`、`crates/web/src/handlers/ui/files.rs`、`deploy/install.sh` |
| SITE-08 | 运行环境安装/启停/重建 | `[上机]` 已登录，镜像源可用 | 安装 php-8.5；停、启、重建；每步看状态与 `docker ps` | 安装各步骤全部成功；停/启 `POST /ui/v1/runtimes/{id}/action/{action}` 返回 200 且容器状态一致；重建后站点仍 200 | □通过 / □失败 | `crates/app/src/sites.rs`、`crates/web/src/handlers/ui/runtimes.rs`、`crates/web/static/budding/budding-boot.js` |
| SITE-09 | PHP 扩展安装/卸载 | `[上机]` php-8.5 已安装 | 在运行环境详情安装一个扩展；重建；卸载；再重建 | 安装/卸载返回 200；`php -m` 或 `phpinfo` 出现/消失对应扩展；站点不 500 | □通过 / □失败 | `crates/app/src/runtime_catalog.rs`、`crates/app/src/sites.rs`、`crates/web/src/handlers/ui/runtimes.rs` |
| SITE-10 | 解绑与删除保护 | `[上机]` 有绑定站点的运行环境 | 运行环境详情尝试删除；先解绑一个站点再删；删除绑定过的站点 | 有绑定站点时删除被拒并列出域名；解绑后列表更新；删站不影响其他站与运行环境 | □通过 / □失败 | `crates/app/src/sites.rs`（`unbind_runtime_site`、`delete_site_runtime`） |
| SITE-11 | socket、端口与网络模式 | `[上机]` PHP 与 nginx 运行中 | 检查 `/www/runtime/php/<name>/php-fpm.sock` 权限与属主；`docker inspect skiff-web-nginx` 与 PHP 容器；`ss -ltnp` 看 80 | socket 0660 `0:65534`（root:gid 65534）；PHP 容器挂载 `web_root`（读写）与 pool 配置（只读）；nginx `NetworkMode=host` 且监听 80；面板不占用 80 | □通过 / □失败 | `crates/app/src/sites.rs`（`SOCKET_MODE`/`SOCKET_GID`、`nginx_spec`、`php_spec_for`）、`deploy/install.sh`、`docs/test/DEPLOY_TEST.md` §1.1 |
| SITE-12 | 证书与强制 HTTPS | `[上机]` 站点已建 | 生成自签证书；上传证书；开启强制跳转；访问 HTTP 与 HTTPS | 证书落 `/www/sites/<domain>/ssl/`；HTTPS 200；HTTP 301/308 到 HTTPS；`docker exec skiff-web-nginx nginx -t` 通过 | □通过 / □失败 | `crates/app/src/sites.rs`、`crates/web/src/handlers/ui/sites.rs`、`crates/web/static/budding/budding-boot.js` |
| SITE-13 | 高风险动作只在「管理」页签 | `[上机]` 已登录 admin 且至少 1 个站点 | 打开站点详情概览，检查顶部操作区；再切到「管理」页签启停一次并删除 | 概览顶部只有访问/日志/默认与返回，无「停止/启动」「删除站点」；管理页签内两种动作均可执行（`POST /ui/v1/sites/{id}/start|stop`、`/delete`）；非管理员看不到「管理」页签；启停**只 reload**：动作前后 `docker inspect -f '{{.State.StartedAt}}' skiff-web-nginx` 不变，且动作后其它站点**首个**请求就 200（重启会让别站在窗口内 `Failed to connect`） | □通过 / □失败 | `crates/web/static/budding/budding-boot.js`（站点详情页签）、`crates/web/src/handlers/ui/sites.rs`（`site_power` / `site_delete` 的 `SITE_ACTION` 门禁） |
| SITE-14 | PHP 池 `nobody` 组别名（Debian/Alpine 两家族） | `[本机]` 单测；`[上机]` 两家族镜像各一个运行环境 | 分别用 `php:8.5-fpm`（Debian）与 `php:8.5-fpm-alpine`（Alpine）建实例并启动；`docker logs` 查 FPM 启动；`docker exec … getent group nobody`、`ls -l /www/runtime/php-*/php-fpm.sock` | 两家族都 running；`getent group nobody` = 65534；socket `root:nobody 0660`；Debian 系无 `cannot get gid for group 'nobody'`；自建镜像同样成立（脚本不按家族分支） | □通过 / □失败 | `crates/app/src/sites.rs`（`PHP_ACCOUNT_INIT_SCRIPT`，单测 `php_init_script_resolves_nobody_from_gid_not_family_name`）、`crates/app/src/sites_vhost.rs` |
| SITE-15 | PHP 扩展安装在 Debian/Alpine 两家族可用 | `[本机]` 单测；`[上机]` 两家族 PHP 运行环境各一个，均 running 且**已按新环境变量重建** | 在 `php:8.5-fpm`（Debian）与 `php:8.5-fpm-alpine`（Alpine）环境分别安装一个需编译的扩展（如 `imagick` / `redis` / PECL `msgpack`）；看安装任务日志；`docker exec … php --ini`、`php -m`；访问绑定站点 | 两家族都 `succeeded`、主路径 exit 0；日志无 `cannot create /etc/apk/config`；`php --ini` 中 `conf.d` 与 `conf.d-skiff` **各出现一次**，无 `Module already loaded` 告警；`/etc/apk/config`、`apk add/del` 只出现在 Alpine 分支；Debian 走 `apt-get install --no-install-recommends` 装 `-dev` 包；`php -m` 出现扩展且站点仍 200；容器内无 `apk` 也无 `apt-get` 时明确报错退出码非 0 | □通过 / □失败 | `crates/app/src/runtime_catalog.rs`（`installer_prelude`、`legacy_deps_branch`、`debian_build_package`）、`crates/app/src/sites.rs`（`PHP_INI_SCAN_DIR_VALUE`，单测 `php_ini_scan_dir_lists_each_directory_once`、`extension_scripts_branch_on_container_package_manager`、`installer_prelude_enables_apk_package_cache`） |
| SITE-16 | 默认站点双向切换与失败恢复 | `[本机]` 单测；`[上机]` 至少 2 个运行中站点（先建的 rowid 更小） | 先把默认站设为后建的 b，再切回先建的 a；再制造 reload 失败（停掉 nginx 容器或断开 Docker）后切一次 | 两个方向都 303，库、`00-skiff-default.conf`、nginx 行为三者一致；reload 失败时返回错误，库仍指向切换前站点，conf 按旧默认重写且不含失败方向 | □通过 / □失败 | `crates/store/src/db/sites.rs`（`default_site_switches_in_both_rowid_directions`）、`crates/app/src/sites/crud.rs::set_default_site`、`crates/app/src/sites/tests.rs`（`set_default_site_rolls_back_db_and_conf_when_reload_fails`） |
| SITE-17 | 保存 vhost 走 reload 优先、证书成对写入不产生 mismatch | `[本机]` 单测；`[上机]` nginx 运行中、站点已启用 HTTPS | 4 会话并发保存同一站点 vhost 若干轮，确认无「配置已回滚：nginx 未能保持运行」且 `docker ps` 显示容器未重启（`StartedAt` / 容器 ID 不变）；反复上传/生成证书并同时保存 vhost，`nginx -t` 与站点访问无 `key values mismatch`；再提交一份坏配置确认仍被回滚 | 并发保存不触发容器重启、无窗口回滚；日志出现「nginx 已平滑 reload（worker PID 换代）」，`StartedAt` 不变即证明走的是 reload 而非 restart；worker 探测读 `/proc/1/task/1/children`（`-r` + 读后判空，不用恒为 0 size 的 `-s`），退化扫描同时认 `nginx` 与 OpenResty 的 `openresty`；证书与私钥始终配对，私钥 rename 失败时回滚成替换前的成对文件（不得删掉新证书留下悬空引用），首次上传失败时库 `https=0` 且 vhost 不再含 `ssl_certificate`，`nginx -t` 通过、站点 200；坏配置仍返回 `Conflict`（校验失败）或 `Unavailable`（起不来并回滚）；**建站、增删别名、站点启停、默认站切换、删站同属「配置已写好只需采纳」，一律 `activate_nginx`（reload 优先），回滚路径才用 `restart_nginx`；生效为异步收敛（实测 0.2–0.5 s 内旧 worker 仍按旧配置应答），用例按「2 秒内收敛」判，不断言首个请求** | □通过 / □失败 | `crates/app/src/sites/crud.rs`（`activate_with`、`reload_nginx` 回退链、`nginx_worker_generation` / `parse_worker_probe` / `is_nginx_comm`、`replace_cert_pair`、`repair_no_cert_state`、`nginx_write_lock`）、`crates/app/src/sites/tests.rs`（`nginx_activation::openresty_comm_reaches_reload_branch`、`cert_pair_*`、`failed_first_upload_repairs_dangling_ssl_state` 等）、`crates/core/src/server.rs`（巡检 reopen 同锁） |
| SITE-18 | 站点详情主键深链 | `[上机]` 至少 1 个站点 | 从网站列表进入详情，复制地址；再改站点附加域名后刷新详情；直接访问旧域名路径 | 地址为 `/{p}/sites/<id>`；改域名后刷新仍 200；`/{p}/sites/<旧域名>` 不再作为详情命中（按未知路由 404/壳页列表）；写接口路径同样使用 id | □通过 / □失败 | `crates/web/static/budding/budding-boot.js`、`crates/web/src/handlers/ui/sites.rs`（`site_domain`）、`crates/store/src/db/sites.rs`（`site_by_id`） |
| SITE-19 | PHP 伪静态预设 | `[上机]` PHP 站点 + 真实 PHP 框架或 `index.php` 路由 | 在配置页签依次选 `wordpress` / `laravel` / `thinkphp` 保存；访问一个不存在的路径；再改回 `none` | 保存 200 且 vhost 出现对应 `try_files`/`rewrite`；不存在的路径交给 `index.php` 而不是 404；改回 `none` 后恢复 404；非 PHP 站点不显示伪静态选择；未知预设被接口 400 拒绝 | □通过 / □失败 | `crates/app/src/sites/vhost.rs`（`VhostPolicy`、`validate_rewrite`）、`crates/app/src/sites/crud.rs`（`set_site_policy`）、`crates/web/static/budding/budding-boot.js` |
| SITE-20 | 站点出口限速 | `[上机]` 有可下载的静态文件（如 5MB） | 配置页签填 `limit_rate_kbps=512` 保存；用 `curl -o /dev/null` 下载并看速度；改回 0 | vhost 出现 `limit_rate 512k;`；下载速度约 512 KB/s；改回 0 后 `limit_rate` 消失、速度恢复；非数字/超大值被拒绝；保存失败时库、vhost 与站点访问保持一致 | □通过 / □失败 | `crates/app/src/sites/vhost.rs`、`crates/app/src/sites/crud.rs`、`crates/store/src/db/sites.rs`、`crates/web/static/budding/budding-boot.js` |
| SITE-21 | 站点从归档恢复 | `[上机]` 站点已开启备份计划并至少有一份归档（或把一份 `YYYYMMDD-HHMMSS.tar` 放进 `<data>/backups/sites/<域名>/`） | 详情 → 备份页签查看归档列表；删除站点里一个文件；点某份归档的「恢复」，手输域名确认；看任务中心的校验 / 快照 / 解包三条作业；再看站点访问与目录；另试 `confirm` 不匹配、`archive=../x.tar`、损坏的归档 | 列表显示名称 / 大小 / 时间且按时间倒序；恢复受理后返回 `started=true`（不阻塞请求），三步作业依次成功；同目录多出 `<时间戳>-pre-restore.tar`，被删文件回来、归档里没有的文件保留；审计有 `sites.restore` 且 detail 注明 nginx 是否重载成功；`confirm` 不匹配 400；带 `../` 的文件名被拒；损坏归档报错且站点目录未被改动 | □通过 / □失败 | `crates/app/src/tasks.rs`（`check_restore_target` / `restore_site`）、`crates/app/src/sites/backup.rs`、`crates/app/src/commands/site.rs`（`restore`）、`crates/web/src/handlers/ui/sites.rs`（`site_restore`）、`crates/web/src/handlers/ui/site_page.rs`（`archive_card`）、`crates/web/static/budding/budding-boot.js` |
| SITE-22 | 浮动 tag 的运行环境新装取最新补丁 | `[上机]` 宿主已存在旧补丁的 `php:8.5-fpm-alpine`（如 8.5.12），Docker Hub 已发布新补丁（如 8.5.13） | 删除已有 php-8.5 运行环境后重装；看任务日志的「拉取镜像」步骤、运行环境列表显示的真实版本、`docker image inspect php:8.5-fpm-alpine --format '{{index .Config.Env}}'` | 日志出现「本地已存在（旧 digest），仍按 tag 拉取以取最新补丁」；拉取后版本为新补丁（列表主显 `8.5.13`，与镜像 `PHP_VERSION` 一致）；已登记 digest 的实例重建仍按原 digest、版本不变 | □通过 / □失败 | `crates/app/src/sites/spec.rs`（`plan_image_pull` / `ensure_runtime_image`）、`crates/app/src/sites/runtime.rs` |
| SITE-23 | 运行环境版本清单（三类）与自定义版本 | `[上机]` 能打开「运行环境」页；`[catalog].mirrors` 首项指向镜像服务（默认 `http://10.66.52.7:8080`），且该服务已发布三类清单 | 打开 `/sites/runtimes` → 新建运行环境，依次把类型切到 PHP / Node.js / Rust 看版本下拉；点右上「刷新版本清单」；把 `[catalog].mirrors` 首项改成不可达地址（如 `http://127.0.0.1:9`）并重启面板，再刷新并新建；恢复地址后在「自定义版本」填 `8.5.11` 提交；再填 `9.0.1` 提交 | 三个类型各有自己的下拉（`version_php` / `version_node` / `version_rust`），切换类型时只显示当前那份、互不串味；PHP 每个大版本列最近三个补丁号（如 `8.6.0`；`8.5.13 / 8.5.12 / 8.5.11`，每个大版本至多 4 条），Node.js 列具体版本（如 `24.22.3 / 24.22.2 / 24.22.1`，同一大版本跨 minor 合计至多 4 条），Rust 列 `1.100.0`（当前分支只有一个补丁）；均按版本降序、首个未 EOL 的标「最新」，预选清单 `default`（未标则取首个未 EOL），已 EOL 标「已 EOL」排最后；提示行给出清单固定路径 `…/channels/stable/runtime-index`（php.json / node.json / rust.json）与各类型来源；刷新后提示「版本清单已刷新：php N 个版本，…」并重载；某类型未发布（404）时提示「镜像服务未发布…」，该类型回退目录大版本仍可选；地址不可达时 PHP 提示「PHP 内置副本：清单获取失败：…」、Node/Rust 提示「… 目录大版本：清单获取失败：…」，都仍能选到版本；PHP 清单比内置副本旧时提示「PHP 内置副本：镜像服务清单（…）比内置副本（…）更旧，已忽略」；`8.5.11` 提交成功且镜像为 `php:8.5.11-fpm-alpine`；`9.0.1` 返回 400「版本无效」 | □通过 / □失败 | `crates/app/src/runtime_index.rs`（`parse_index` / `sort_and_trim` / `index_url` / `index_file` / `load`）、`crates/app/assets/runtime-index/php.json`（唯一的服务端产物快照）、`tools/skiff-server/src/build.rs`（`runtime_index_bytes`）、`tools/skiff-server/src/serve.rs`（`RUNTIME_INDEX_FILES`）、`crates/app/src/runtime_catalog.rs`（`version_supported` / `image_for`）、`crates/web/src/handlers/ui/runtimes.rs`（`runtime_versions` / `catalog_json`）、`crates/web/src/handlers/ui/runtimes_page.rs`（`create_dialog` / `kind_versions`）、`crates/web/static/budding/budding-boot.js`（`runtimeCreate` / `runtimeVersionsRefresh`） |

最小回归命令：

```bash
curl -ks -o /dev/null -w 'sites=%{http_code}\n' -b "$J" "$B/$P/sites/list"
curl -ks -o /dev/null -w 'site_php=%{http_code}\n' -H 'Host: matrix.local' 'http://10.10.15.4/'
curl -ks -o /dev/null -w 'site_static=%{http_code}\n' -H 'Host: matrix-static.local' 'http://10.10.15.4/'
curl -ks -o /dev/null -w 'site_proxy=%{http_code}\n' -H 'Host: matrix-proxy.local' 'http://10.10.15.4/'
curl -ks -o /dev/null -w 'traversal=%{http_code}\n' -b "$J" "$B/$P/sites/matrix.local?tab=dir&path=../log"
curl -ks -o /dev/null -w 'site_manage=%{http_code}\n' -b "$J" "$B/$P/sites/matrix.local?tab=manage"
```

本机脚本断言（扩展安装两家族分支、`nobody` 组别名、ini 扫描目录不重复）：

```bash
cargo test -p skiff-app runtime_catalog
cargo test -p skiff-app php_init_script
cargo test -p skiff-app php_ini_scan_dir
```

## E. Web 服务

> 重跑触发：改了 nginx 主配置/性能参数、`nginx -t` 校验与回滚、日志读取、模块页或 Web 服务容器操作。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| WEB-01 | Web 服务状态与最近 reload | `[上机]` nginx 已安装 | 打开 `/$P/sites/web` | 200；显示容器状态、版本、配置路径、最近一次 reload 结果 | □通过 / □失败 | `crates/web/src/handlers/nginx.rs`、`templates/nginx.html` |
| WEB-02 | 性能参数保存并生效 | `[上机]` nginx 运行中 | 保存 worker、gzip、keepalive、client_max_body_size；看生成配置；`nginx -t` | 303 且页面提示成功；生成配置含新值；`nginx -t` 成功 | □通过 / □失败 | `crates/app/src/nginx.rs`、`handlers/nginx.rs` |
| WEB-03 | 非法参数拒绝且不落库 | `[上机]` 已登录 | 提交越界值（例如 worker=0、body=0、keepalive 超范围） | 303 回页面并显示内联错误；旧配置与旧值保持不变 | □通过 / □失败 | `crates/app/src/nginx.rs`（校验）、`handlers/nginx.rs` |
| WEB-04 | `nginx -t` 失败时配置回滚 | `[上机]` 有站点 vhost | 在站点配置页写入非法指令并保存；访问站点；检查 vhost 文件 | 303 回配置页并显示 `nginx -t` 错误；旧 vhost 保留；站点仍 200 | □通过 / □失败 | `crates/app/src/sites.rs`（配置保存）、`crates/app/src/nginx.rs` |
| WEB-05 | 日志按需加载与过滤 | `[上机]` 有访问日志 | 打开 `/$P/sites/web?tab=logs` 不带 `load`；再带 `load=1`；按 `q`、`status`、`lines` 过滤 | 不加载时只显示入口；加载后返回尾部日志；过滤只保留匹配行 | □通过 / □失败 | `crates/web/src/handlers/nginx.rs`、`templates/nginx.html` |
| WEB-06 | 模块列表 | `[上机]` nginx 运行中 | 打开 `/$P/sites/web?tab=modules` | 200；列出已编译模块；与 `nginx -V` 输出一致 | □通过 / □失败 | `crates/app/src/nginx.rs`、`handlers/nginx.rs` |
| WEB-07 | 站点日志增长与面板可读 | `[上机]` 已建站 | 连续访问站点 10 次；看 `/www/sites/<domain>/log/` 与 `/$P/sites/<domain>/logs` | access log 增长；面板站点日志页可读同一文件；无越权读取其他站日志 | □通过 / □失败 | `crates/app/src/sites.rs`、`handlers/sites.rs`、`/www/sites` |
| WEB-08 | 合法配置保存后 reload 生效 | `[上机]` nginx 运行中 | 在主配置页改一处合法指令（如 `server_tokens off;`）保存；刷新状态页；访问站点 | 303；状态页「最近 reload」时间更新；站点仍 200；`nginx -t` 输出变化 | □通过 / □失败 | `crates/app/src/nginx.rs`（`save_nginx_main`、`NGINX_LAST_RELOAD_KEY`）、`templates/nginx.html` |

最小回归命令：

```bash
curl -ks -o /dev/null -w 'web=%{http_code}\n' -b "$J" "$B/$P/sites/web"
curl -ks -o /dev/null -w 'logs_lazy=%{http_code}\n' -b "$J" "$B/$P/sites/web?tab=logs"
curl -ks -o /dev/null -w 'logs_load=%{http_code}\n' -b "$J" "$B/$P/sites/web?tab=logs&load=1&lines=50"
curl -ks -o /dev/null -w 'modules=%{http_code}\n' -b "$J" "$B/$P/sites/web?tab=modules"
```
