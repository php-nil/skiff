# 验收结论：当前轮与最近两轮（§8）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §8 最近三轮 + 历史轮次台账（其余轮次已精简）（2026-11-16 整理）。

---

## 8. 最近一次全链路验收结论（截至 2026-11-15 `v0.1.0` Release 资产全新安装 + 全量回归）

> 2026-11-16 07:53 的原地重装（budding `1.8.0-r7`）是叠加复验，未另起轮次；其结论记在 [02-DEPLOY_STATE.md](02-DEPLOY_STATE.md) §5 的「验收」项。

> 本节只保留验收断言、实测数字与限制，是「测试机上发生了什么」的唯一来源；各轮的代码改动、根因与提交链见 [changes/2026-11-10.md](../../changes/2026-11-10.md)、[changes/2026-11-09.md](../../changes/2026-11-09.md)、[changes/2026-11-15.md](../../changes/2026-11-15.md)，用例编号与预期结果见 [TEST_MATRIX.md](../TEST_MATRIX.md)，未闭环项见 [TEST_LEDGER.md](../TEST_LEDGER.md)。

**2026-11-16 23:22–23:55 `280345e` 资产全新安装 + 定向回归：安装 / 认证 / 授权 / 审计全通过，0 FAIL**

- **安装（通过）**：还原后空盘探测（无 `/usr/local/bin/skiff`、无 `/etc/skiff`、无 `/www/skiff`、18080/18081 无监听、无 `skiff*` unit）→ 资产 12,130,214 B `7dc33e92…f051`（`.sha256` / `checksums.txt` / 测试机 `sha256sum -c` 三方一致）→ `sh install.sh --no-docker --tls off --listen 127.0.0.1:18080` exit 0；空库 v21、`integrity_check=ok`；权限 711（`/www/skiff`、`plugins`、`/run/skiff`）、`skiff.db` 600、`kernel.sock` 660。二进制 19,822,352 B `0d82bc95…581b`。
- **accept.sh preflight + A（通过）**：**32 PASS / 0 FAIL / 3 SKIP**。含 A4a `/healthz` 20 s 内 200、A5b/c 未登录 303 跳登录、A5d 裸 `/` 404、A6a–h 八项权限位、A9a/b 重启自恢复且前缀不变、A10a `kill -9` 后 systemd 2 s 拉起、A11 `--help`/`version` 不落文件。SKIP 三项：PF4（装后阶段不适用）、D0b（可 `--pkg-dir` 补跑）、B0（已由重跑解决）。
- **认证（通过，脚本修正后）**：`sh accept.sh --group b` → **13 PASS / 0 FAIL / 1 SKIP**（B4 未开 `--with-rate-limit`）。首跑 `B1a` 判 FAIL（脚本仍按 SSR 表单抓 csrf），判定为**测试资产脱节**而非产品缺陷，已按 budding 原生链路重写 B 组并复跑全绿；根因与改法见 [changes/2026-11-16.md](../../changes/2026-11-16.md)。
- **本轮改动定向回归（通过）**：
  - **授权**：admin 访问 `/{p}/sites/new`、`/{p}/sites/web`、`/{p}/m/core/sites/web`、`/ui/v1/runtimes`、`/ui/v1/web` 均 **200**；把库内角色临时改 `viewer` 后同样 5 处全部 **403 + `auth.forbidden`「角色 viewer 无权执行 admin」**，`/ui/v1/sites`、`/ui/v1/session` 仍 200；改回 admin 后恢复 200（`require_admin` → `authz::authorize_for` 的等价性证明）。
  - **审计**：`auth.login` 记 `actor=admin, ip=127.0.0.1`；插件侧 `actor=plugin:com.skiff.*, ip=uds`；调度 `actor=system, ip=sched`（`audit_actor` 单一实现生效）。
  - **冒烟**：`/ui/v1/{sites,runtimes,web,containers,plugins,tasks,jobs,audit,menu,files}` 与壳页 `/{p}/{sites,settings,plugins}`（`Accept: text/html`）均 200；未知视图 404；`/ui/v1/networks` 重名创建为幂等返回（未产生重名网络，`docker network ls` 仅 4 项）。
- **未覆盖（限制）**：`map_docker_cmd` 的 Conflict 分支（409）未在真机触发——网络创建是幂等接口、compose 校验在 DTO 层即 400；该映射由三份逐字相同的 `match` 合并为一处，逻辑零变化，由 CI 单测覆盖。B4 限流、TOTP、建站/删站、FTP/防火墙、文件配额与回收站沿用 2026-11-15 的结论，本轮未复跑。
- **清理**：库内角色已改回 `admin`；`audit-probe` 站点未创建成功（无残留）；`/root/iter/` 保留安装包、`accept.sh` 与 `skiff-http-fwd.py`。

**2026-11-15 23:04–23:57 `v0.1.0` Release 资产全新安装 + 全量回归：可在本机执行的批次全部通过**

- **安装与基线（通过）**：安装前逐项探测确认已还原（无 `/usr/local/bin/skiff`、无 `/etc/skiff`、18080/18081 无监听、无 `skiff*` unit、Docker 29.7.2 可用且无 `skiff-*` 容器）。资产 12,070,181 B `114a8320…5f67`（`.sha256` 一致），安装后二进制 19,682,960 B `f0947ed4…487e`；全新空库 `user_version=21`、`integrity_check=ok`；前缀 `cuyf5neg`、口令按 AUTH-07 改为 `SkiffPass2026x`；7 个官方插件全部装成；`skiff-web-nginx` + 种子 `php-8.5` 运行中；`skiff-http-fwd`（`0.0.0.0:18081 → 127.0.0.1:18080`）active；`/healthz` 外部 18081 与本地 18080 均 200；数据根与 `plugins/` 权限 0711。
- **AUTH / 菜单（通过）**：登录错误口令 401、正确口令 200 并写入会话；连续失败 5 次后 503（300s 窗口）；未登录访问受保护路由 302/401；AUTH-07 口令修改经 `POST /ui/v1/settings/password`（`x-csrf-token`）成功，旧口令失效、新口令可登录；AUTH-08 TOTP `setup` → `enable` → 带 `otp` 登录 200，测后用库清理回单因子。菜单偏好 `POST /ui/v1/settings/menu` 的 hide/show/pin/unpin/up/down/reset 均生效，概览与会话安全页按契约锁定（显隐静默 no-op）；浏览器实测「显隐」提示 `已保存菜单偏好` 且刷新后保持。
- **插件（通过，1 项口径修正）**：7 个官方插件版本/签名/权限授予列表可读；插件页标签、授权站点目录选择器、启停按钮与侧栏入口联动正常；`站点文件` 插件页 `document.title=Skiff · 站点文件`（内核标题守卫覆盖插件自写标题）；PLUGIN-12 内置任务删除实测 **400**「内置任务不可删除（只能停用）」，`/ui/v1/tasks/builtin:db-vacuum/delete` 同为 400 同文案，矩阵预期已按当前命令层/插件 RPC 统一口径修正（不再要求 403）。
- **站点网络八项复跑（全部通过）**：① 首选 `172.18.0.0/16` 被 `bridge`/`docker0` 占用时回退 `172.19.0.0/16`，`sites.network_fallback` 与运行环境详情、内核 WARN 三处同文披露；② 运行环境重建前后容器 ID 变化、`container_ip` 与容器内地址稳定在 `172.19.16.1:9200`；③ vhost 仅 `proxy_pass http://172.19.16.1:9200;`，`127.0.0.1:9200` 出现 0 次，502/503/504 兜底 `@skiff_maintenance`；④ Node 24 站 `Host: node.local` 200 返回 `Skiff Node.js runtime ready`，PHP 8.5 站走 `php-fpm.sock` 返回 `SKIFF-PHP-PROBE:42`；⑤ 抢占 9200 的陌生宿主进程时站点进 503 维护页而非把请求交给陌生进程；⑥ 3 秒巡检发现容器退出后按静态地址自动恢复，恢复后域名 200；⑦ 对外发布开关 `publish_port` 置空/置端口均即时反映在宿主监听与 `docker port`；⑧ 八项后短回归：概览、站点列表、详情、日志、任务页无回退。
- **FS / Web / 任务（通过）**：`/ui/v1/files/write` 原子写入 + 并发写冲突 409；`read` 返回文本；越界路径与 symlink 逃逸被拒（插件侧 `check_ancestors`）；配额设置/取消 + 回收站删除/恢复/清空；站点配置回滚；数据根直接下载 403；管理员「文件管理」按设计可读宿主全盘（含 `/etc/passwd`），与插件侧受限面区分。重复提交 Web 安装时观察到同目标 job 冲突（409 `domain.conflict`），按设计拒绝二次并发安装。
- **Docker（通过）**：容器创建/启动/停止/重启/删除、日志、详情页操作与确认弹窗；`POST /ui/v1/containers/{id}/limits` 同时提交 `memory_limit_mb` + `memory_swap_mb` 成功（仅提交 memory 时 Docker 自身 409，非产品缺陷）；容器终端 WebSocket 真连可交互；Compose 项目解析/部署/删除主链路可用；`docker ps -a` 与面板状态一致。
- **站点 CRUD（浏览器实操，通过）**：新建站点非法域名即时校验、合法域名创建成功并生成 `index/`、vhost、PHP 探针；详情页信息与库一致；删除弹窗输入错误域名被拒、正确域名删除成功并级联清理目录/vhost/别名/备份任务。
- **FTP / 防火墙（通过，1 项瞬时竞态）**：FTP 插件装成，虚拟用户统一 UID/GID 201000，`pure-ftpd` 实登可上传/下载，跨身份 PHP 读取授权站点文件通过；防火墙规则 `port.allow` / `rules.list` / `rule.remove` 生效且可回滚；fail2ban `install` → `status(installed=true, running=true)` → 未装时 `start` 返回 503 → `jail.ensure` → `ban/unban` → `jail.remove` → `uninstall` 全链路通过。首次 `jail.ensure` 后立即 `banned` 出现一次 500（fail2ban socket 尚未就绪），重试即成功；属首次重启后的短竞态，未达产品阻断，记入 changes 观察项。
- **面板 / TLS / TOTP（通过）**：入口前缀修改 → 旧前缀 24h 内 302 到新前缀 → 重启后新前缀生效 → 改回；TLS 三态回显与「非 loopback 无证书无 trusted_proxies 拒绝保存」校验；TOTP 绑定/登录/清理如上。
- **cron / 作业 / 升级覆盖（通过）**：cron 自定义任务 `create`（target `state`/`probe`/`identity`）→ `run` 返回 `ok` → `detail` 记录 `last.result=ok` → `toggle` → `remove`；内置任务拒绝修改/删除；作业列表/详情进度与日志行可读，活动冲突 409；就地覆盖安装后二进制替换、库/配置/插件/站点保留、权限仍 0711。
- **本机未执行项（限制）**：AUTH-06 会话过期属 `[本机]` 单测，受「不在本地 Windows 编译」红线约束未在本轮执行（交由 CI）；PANEL-06 的三态回显与拒绝保存已上机，`auto/on` 组合的单测未跑；SITE-15 Debian/Alpine 双家族扩展因未装 Debian 运行环境未跑；STORE-01～03 需商店目录启用，本机 `GET /ui/v1/store` 返回 404「商店目录未启用」（按设计关闭态）；API-01～04 因 `skiff-server` 未部署未覆盖；PERF 冒烟未跑。
- **口径偏差（已判定，非阻断）**：① PLUGIN-12 由 403 修正为 400（命令层与插件 RPC 一致，文案不变）；② 商店关闭时 `/ui/v1/store` 404 为设计关闭态；③ 未知 SPA 视图/未安装插件深链对 `Accept: text/html` 返回 200 外壳、由前端渲染 404（契约行为）；④ Docker 内存限制需同时提交 swap（Docker 语义）；⑤ 管理员文件管理可读宿主全盘（设计边界，插件侧仍受限）。
- **清理（完成）**：删除 `matrix.local`/`static.local`/`proxy.local`/`node.local` 四站、`node-node-a1` 运行环境、2 个 FTP 账号、`/www/sites/{fslab,uitest}`、`/www/uitest`、`/www/skiff/vhost-check`、回收站与配额记录、`uilab-ct` 容器；fail2ban 与 PHP `bcmath` 扩展测后卸载；最终仅保留 `skiff`、`skiff-http-fwd`、`skiff-web-nginx`、种子 `skiff-php-php-8.5` 与 7 个官方插件；站点列表为空、运行环境仅 `php-8.5`（浏览器复核）。

**2026-11-15 22:33–23:05 `704b890` 站点网络模型验收：八项全部通过**

- **环境**：上一轮 `704b890` 全新安装（坐标见 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md)）；`skiff-web-nginx`（openresty/1.31.8.1，host 网络，80）running；`node:24-alpine`、`php:8.5-fpm-alpine` 本地已有；外部入口 `http://10.10.15.4:18081/34h8y9da/`。
- **第 1 项 站点网络（通过）**：宿主 `docker network inspect bridge` → `172.18.0.0/16`，`ip -4 -o addr` → `docker0 172.18.0.1/16`、`eth0 172.17.16.10/20`、`eth1 10.10.15.4/24`；新建 Node 24 运行环境 `node-a1` 后自动创建 `skiff-sites`，实测 `Subnet 172.19.0.0/16`、`IPRange 172.19.0.0/24`、`Gateway 172.19.0.1`。`sites.network_cidr=172.19.0.0/16`，`sites.network_fallback=首选 172.18.0.0/16 被占用：Docker 网络 bridge（172.18.0.0/16）、宿主网卡 docker0（172.18.0.1/16）；当前使用 172.19.0.0/16`；`/ui/v1/runtimes/node-a1` 的 `network` 块给出 `cidr/preferred/fallback_reason/dynamic_pool/static_pool_start/static_pool_end`，静态池 `172.19.16.1–172.19.31.254`；运行环境详情页显示「站点网络 / 当前网段 / 首选网段 / 动态池 / 静态池」与同一 warn 文案；内核日志 WARN「站点首选网段被占用，已回退到候选网段」。站点容器地址 `172.19.16.1:9200` 落在实际网段静态池内。
- **第 2 项 地址稳定（通过）**：`node-a1` 登记 `container_ip=172.19.16.1`、`port=9200`；重建前容器 `54d3989…`、重建后容器 `ac3474f…`（容器 ID 变化），容器与库内地址均保持 `172.19.16.1`；宿主 `ss -lntp` 无 9200 监听、`docker port` 无发布端口；宿主直接访问 `http://172.19.16.1:9200/` 返回应用正文。
- **第 3 项 vhost 上游（通过）**：站点 `node704.local` 的 vhost 仅 `proxy_pass http://172.19.16.1:9200;`，全文 `127.0.0.1:9200` 出现 **0** 次、`172.19.16.1:9200` 出现 1 次；维护兜底为 `error_page 502 503 504 = @skiff_maintenance`。
- **第 4 项 域名访问（通过）**：Node 站 `curl -H "Host: node704.local" http://127.0.0.1/` → **200** 正文 `Skiff Node.js runtime ready / version: 24 / port: 9200`；PHP 8.5 站 `php704.local` vhost 为 `fastcgi_pass unix:/www/runtime/php/php-8.5/php-fpm.sock`，探针返回 **200** `SKIFF-PHP-PROBE:42 php=8.5.12`、`X-Powered-By: PHP/8.5.12`。
- **第 5 项 抢占场景（通过）**：停 `node-a1`（带确认）后 **1 s** 内 vhost 被巡检改写为纯维护页（无 `proxy_pass`/`fastcgi_pass`）`return 503 "Node.js 容器已退出（exit code 1），站点不会转发到任何其它进程"`；宿主 `python3 -m http.server 9200 --bind 0.0.0.0` 占住 9200 后请求返回 **503** + `Retry-After: 5`，python 访问日志 **无任何请求**（流量没有交给陌生进程）；启动环境后 **1 s** 内 vhost 写回 `proxy_pass http://172.19.16.1:9200`，请求恢复 **200** 应用正文。
- **第 6 项 巡检（通过）**：3 秒级巡检在停止后 1 s、恢复后 1 s 完成维护页切换/写回；站点列表 `status.label=维护中` 且 `maintenance_reason` 为容器退出原因；站点详情 `runtime_down=true`、`runtime_alert` 给出「站点已进入维护状态（访问返回 503）…恢复后自动解除维护」；恢复后列表回到 `运行中`、`maintenance_reason` 清空。
- **第 7 项 对外发布（通过）**：默认 `publish_port=NULL`、`docker port` 为空；填宿主 `18082` 后重建 → `9200/tcp -> 0.0.0.0:18082` 与 `[::]:18082`，`http://10.10.15.4:18082/` 直接返回应用正文，容器地址仍 `172.19.16.1`、站点 Host 访问不受影响；清空保存后重建 → `docker port` 为空、18082 无监听、外部访问返回 `000`。
- **第 8 项 短回归（通过）**：r6 版本化资产 `budding-1.8.0-r6.min.{js,css}` **200 + `public, max-age=31536000, immutable`**，稳定别名 `budding.min.js`/`budding.css` **200 + `no-cache`**，`favicon.svg` 200；概览 `containers 3/3`、`plugins 7`、`sites 2`、`runtimes abnormal 0`；容器列表 3 个均「运行中」；文件管理 `/ui/v1/files?path=/www` 返回 5 项；审计 `/ui/v1/audit` 正常分页；作业列表 6 条、最新 `已提交→已完成 100%` 且详情含 `logs`/`steps`；插件列表 7 个启用（guard/monitor 运行中）；确认契约生效：不带 confirm 的 `stop` 返回 **409**「该运行环境已被 1 个站点使用…确认后重试」，`/{p}/jobs` 页 200。
- **清理（通过）**：删除 `node704.local`、`php704.local` 与 `node-a1`（含容器）；`/www/sites` 为空、`sites=0`、`aliases=0`、`site_runtimes` 仅种子 `php-8.5`（running）；`docker ps -a` 仅 `skiff-web-nginx` 与 `skiff-php-php-8.5`；9200/18082 无监听；`skiff-sites`（172.19.0.0/16）作为产品网络保留；库 v21、`integrity_check=ok`；`/healthz` 18080/18081 均 200；`skiff`/`skiff-http-fwd`/`docker` active。
- **回滚面（通过）**：未改宿主 Docker 配置，`/etc/docker/daemon.json` md5 `7df09da892241ee3bc36ed2a96d8f1bb` 与安装前一致；旧安装完整保留在 `/root/iter/archive-e5df734/`；未改代码。

**2026-11-15 21:12–21:30 `e5df734` 站点网络模型验收：第 1 项即停手（专用网络与宿主 docker0 网段冲突，第 1–6 项无法进入；第 7 项被动复现通过）**

- **环境**：上一轮 `e5df734` 全新安装（坐标见 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md)）；`skiff-web-nginx`（openresty，host 网络，80）running；`node:24-alpine`、`php:8.5-fpm-alpine` 本地已有；外部入口 `http://10.10.15.4:18081/34h8y9da/`。
- **第 1 项（不通过，停手）**：新建 Node 24 运行环境 `node-e5`（`id=node-node-e5`，端口 9200）时，安装任务在 15% 失败，`error_detail` 原文：
  「准备站点网络失败：宿主已存在占用 172.18.0.0/16 的 Docker 网络：bridge（172.18.0.0/16）；无法创建 skiff-sites。请先删除或调整冲突网络（内核不会自动更换网段，也不会接管该网络）」。
  重试同一作业（`job-1794748501-6ff6e12`）逐字复现同一失败。运行环境详情 `container_ip=null`、`container_state=missing`、`state=状态漂移`、`endpoint_full=容器地址未分配（需重建环境）`。
- **根因证据**：`docker network inspect bridge` → `{"Subnet":"172.18.0.0/16","Gateway":"172.18.0.1"}`，`ip -4 addr` → `docker0 inet 172.18.0.1/16`、`eth0 inet 172.17.16.10/20`。即宿主 LAN 已占用 `172.17.0.0/16`，Docker 默认 bridge 自动退到 `172.18.0.0/16`，与 `SITE_NETWORK_SUBNET` 完全重叠；`docker network ls` 仅 `bridge/host/none`，`skiff-sites` 不存在（`network skiff-sites not found`）。
- **第 7 项（被动复现通过）**：上述错误即「宿主已有网络与 172.18.0.0/16 重叠」场景，内核如实报错、列出冲突网络名与网段、未静默换段、未创建/接管任何网络，与 [SITE_RUNTIME_CONTRACT.md](../../contract/SITE_RUNTIME_CONTRACT.md) §网段与冲突、T5b 一致。
- **第 2–6 项**：被第 1 项阻断——没有 `skiff-sites` 就没有 `container_ip`，无法生成 `proxy_pass http://<容器地址>:<端口>` 的 vhost、无对外发布端口可验、无运行环境可供巡检进入维护态。按「任一项失败立即停手上报」停在此处，未改代码。
- **未触碰宿主 Docker 配置**：`/etc/docker/daemon.json` md5 `7df09da892241ee3bc36ed2a96d8f1bb` 与安装前一致（仅 `registry-mirrors` + `log-driver/log-opts`，无 `bip` / `default-address-pools`）；未改 `docker.service`、未重启 Docker。
- **处理结论（2026-11-15 21:40 定）**：不改宿主 Docker 配置、不换机；内核改为「首选 `172.18.0.0/16`，被占用时按固定候选序列回退到不与任何既有网络、宿主网卡重叠的 /16」，并在页面、接口与日志里写明「首选被占、当前使用哪段、占用者是谁」。上一轮第 1–6 项待新 head 重新出包装机后复跑；第 7 项改为核对「回退是否发生、选定网段与占用原因是否可见、站点地址是否落在实际选中的网段内」。
- **清理**：删除失败运行环境 `node-e5`（接口 `deleted=true`）；`/www/runtime` 下无残留实例目录、站点 0、运行环境仅种子 `php-8.5`、`docker ps -a` 仅 `skiff-web-nginx`；旧安装完整保留在 `/root/iter/archive-3b5c433/`。

## 历史轮次台账（2026-11-16 精简）

> 下表只留「日期 + 版本 + 轮次标题」，每轮完整的逐项断言与实测数字已随本轮整理删除；需要复盘时看当轮 `docs/changes/YYYY-MM-DD.md`、对应提交与 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标。
> 现行口径：只保留最近三轮完整结论（上文），旧轮次不再作为判定依据。

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
