# 官方插件、插件页、vhost、站点 HTTPS 与镜像源（§2.11–2.14）

> 上级索引：[DEPLOY_TEST.md](../DEPLOY_TEST.md) ｜ 拆自 `docs/test/DEPLOY_TEST.md` 原 §2.11–2.14（2026-11-16 整理，正文未改）。

---

### 2.11 官方插件：导入公钥 → 安装 → 复核（2026-11-03 已实测）

> **历史记录**：以下内容记录 2026-11-03 的实测过程。其中 `com.skiff.nginx` 已于 2026-11-04 内核化为「网站」一级功能（见 [`docs/changes/2026-11-04.md`](../../changes/2026-11-04.md)），其安装命令、`/etc/nginx/conf.d` 授权与 RPC 复核步骤不再适用；其余 4 个插件（guard/monitor/files/cron）的步骤仍有效。

**为什么会出现「一个插件都看不到」**（本机逐项实测）：

- 列表页 `GET /{prefix}/plugins` 读的是 SQLite 的 `plugins` 表（`state.db().plugins()`），**不扫描目录**；
  顶部导航的插件入口也由这些行生成。表为空时页面显示「还没有安装插件」，导航里没有插件项。
- `install.sh` 只把发布包里的 `official/` 复制到 `/www/skiff/official/`，**不自动安装未装插件**
  （脚本随后只打印提示）。`official/` 下的 `.wbpkg` 是分发物，不等于已安装。
  已安装的官方插件由内核在启动早期做版本/内容比对后自动补齐（见 §2.5），
  `data/`、既有授权与停用状态都不变。
- 信任库为空时安装会被 TOFU 拒绝。**2026-11-04 起**，`official/public-key.txt` 会在首启时自动写入信任库；仅当公钥文件缺失或指纹不符时，才需要执行 `skiff trust import <公钥文件> --label official`（或面板「设置 → 信任公钥」）。CLI 报错会区分「包自带公钥但未信任」与「包里没带公钥」，并打印带真实路径的一步命令。
- `allow_reduced_sandbox = false` 时，官方 8 个包的 manifest 没有显式关闭 `requires_full_sandbox`
  （默认 true），安装会全部被拒；本机配置 `allow_reduced_sandbox = true` 才可装。
- 停机快照 `/var/backups/skiff/datadir-initial-state.tgz` 内的库是 **0 插件 / 0 公钥**，且不含 `official/`；
  回到该快照就是回到「未装插件」状态。

结论：当前实例是「库空 + 公钥空 + official 未落盘」，**插件列表入口与路由本身没有缺失**，也不需要新增页面。

**实际落地（本机命令，仓库外无改动）**

```sh
# 0) 发布物落到数据根（install.sh 的布局；只复制，不安装）
install -d /www/skiff/official
cp -a /root/iter/official/. /www/skiff/official/

# 1) 官方公钥：首启已自动信任（无需手动）；若缺失/不符，才需
#    skiff trust import /www/skiff/official/public-key.txt --label official
#    或 UI 设置页「信任公钥」/ Bearer Token POST /{p}/settings/keys

# 2) 安装（非交互逐条授权；安装默认 enabled=true）
cd /www/skiff/official
skiff plugin install com.skiff.monitor-0.1.0.wbpkg --yes \
  --grant fs:read --grant fs:list --resource path:/proc
skiff plugin install com.skiff.files-0.1.3.wbpkg --yes \
  --grant fs:read --grant fs:write --grant fs:list --grant fs:mkdir \
  --grant fs:remove --grant fs:rename \
  --resource path:/www/sites/example.com/index
skiff plugin install com.skiff.guard-0.1.0.wbpkg --yes \
  --grant docker:containers.list --grant docker:containers.inspect \
  --grant docker:containers.restart --grant fs:read --grant fs:write --grant fs:list \
  --resource container:nginx
skiff plugin install com.skiff.cron-0.1.0.wbpkg --yes \
  --grant identity:self --grant interop:call --resource plugin:*
# 历史记录（2026-11-03）：com.skiff.nginx 已于 2026-11-04 内核化为「网站」，不再安装
#skiff plugin install com.skiff.nginx-0.1.0.wbpkg --yes \
#  --grant fs:read --grant fs:write --grant fs:list --grant fs:remove --grant fs:rename \
#  --grant docker:containers.list --grant docker:containers.restart --grant interop:call \
#  --resource path:/etc/nginx/conf.d --resource container:nginx --resource plugin:com.skiff.cron

skiff plugin list   # 当时 5 行 signed/enabled/running（nginx 内核化后为 4 行）
```

**systemd 沙箱缺口（历史，部署侧，未改 unit 模板）**：`deploy/systemd/skiff.service` 用 `ProtectSystem=full`，
整个进程对 `/etc` 只读，当时 nginx 插件写 `/etc/nginx/conf.d` 会报 `Read-only file system`。测试机用 drop-in 放开：

```sh
mkdir -p /etc/systemd/system/skiff.service.d
printf '[Service]\nReadWritePaths=/etc/nginx/conf.d\n' \
  > /etc/systemd/system/skiff.service.d/nginx-sites.conf
systemctl daemon-reload && systemctl restart skiff
```

**UI 复核**（Bearer 绕过登录仅用于验证；正常用户走浏览器会话）：

| 项 | 结果 |
|---|---|
| 插件列表 `GET /{p}/plugins` | 200，当时 5 行在列、导航出现 5 个插件项（nginx 内核化后为 4 行） |
| 详情页 `GET /{p}/plugins/com.skiff.nginx`（历史） | 200 |
| 托管 UI `GET /plugins/com.skiff.nginx/ui/index.html`（历史） | 200，带 CSP `default-src 'none'; script-src 'self'` |
| nginx RPC `sites` / `create` / `read` / `backup` / `scheduleBackup`（历史） | 200；`/etc/nginx/conf.d/example.com.conf` 落盘；备份与 cron 任务创建成功 |
| nginx `reload`（历史） | 400「找不到容器 nginx」（本机无 Docker 容器；不再是授权 403） |

**顺带修复的内核缺陷**：`kernel/capability.rs` 把 `target: null` 当成 id/name 全空的容器，
导致 `containers.list` 去匹配永远不存在的 `object:container:` 授权（nginx `reload`、guard 拉取容器列表被 403）。
已改为「缺省或 null = 无目标」并补两个单测；**测试机二进制按源码现编（glibc 动态）**，仍不是 musl 发布包。

### 2.11.1 插件信息页与多视图路由（2026-11-05 实测）

manifest 升到 `api = "1.2"` 后，路由契约变为：

| 路由 | 用途 | 2026-11-05 实测（`/swn4nbhe` 前缀） |
| --- | --- | --- |
| `GET /{p}/plugins/{id}` | 内核渲染的信息 + 管理页 | 5 个插件均 200；版本/API、签名与指纹、沙箱、授权、启停、卸载、关联菜单、日志入口齐全 |
| `GET /{p}/plugins/{id}/main` | 默认功能页（`ui/main.html`） | files 200；无 L3 声明，无标签栏 |
| `GET /{p}/plugins/{id}/<view>` | 声明的 L3 视图 | cron `list`/`new`、guard `containers`/`events`、monitor `overview`/`history` 200，标签栏 2 项；testkit `main`/`check`/`group` 200，标签栏 3 项 |
| `GET /{p}/plugins/{id}/<未知>` | 未知视图 | 404，文案「插件 com.skiff.files 不存在视图 nope，或该视图对当前角色不可见」 |
| `GET /{p}/plugins/{id}/<view>?legacy=1` | iframe 工作台（无 schema 的旧视图） | cron `jobs`/`list`/`new`、files、testkit `main`/`check`/`single` 全 200 且标签栏项数正确 |

testkit 实测：`ping` / `state` / `probe` RPC 200；`interop_cron` 200（`jobs:[], ok:true`）；
`fail` HTTP 400 且文案含 `-32602`；`fs_read /etc/passwd` 400（「内核返回 HTTP 403：路径不在
已授权的 path: 根内」）；`fs_read /proc/uptime` 200。概览出现 `data-slot="overview.actions"`；
停用 testkit 后槽位与导航链接消失、`testkit/main` 404，重新启用即恢复。

> 同版本可直接重装（原位覆盖版本目录，保留 data/ 与既有授权，不再弹确认页）；
> 官方包签名密钥一致时同样适用。如需重新选择授权集，先 `skiff plugin remove <id>`（保留 data）再安装。

### 2.12 网站 vhost：默认站与兜底站语义（2026-11-04 实测）

生成物分两类：**内核公共配置**落在 `conf_dir`（默认 `/www/nginx/conf.d/`），**各站 vhost** 落在站点自己的 `conf/`。

| 文件 | 何时存在 | 关键配置 |
| --- | --- | --- |
| `/www/nginx/conf.d/00-skiff-fallback.conf` | **始终存在** | `listen 80; server_name skiff-fallback; return 404;` |
| `/www/nginx/conf.d/00-skiff-default.conf` | **仅当设置了默认站点** | `listen 80 default_server; server_name _;` + 站点 root |
| `/www/nginx/conf.d/10-skiff-sites.conf` | **始终存在** | `include /www/sites/*/conf/*.conf;`（`*.conf` 不匹配 `.conf.disabled`） |
| `/www/sites/<域名>/conf/<域名>.conf` | 每站一个 | `root /www/sites/<域名>/index;`、`access_log` 与 `error_log` 指向 `/www/sites/<域名>/log/...`、`ssl_certificate` 指向 `/www/sites/<域名>/ssl/cert.pem` |

站点树是「一站一目录”：`index/`（文档根）、`log/`（`access.log`/`error.log`）、`ssl/`（`cert.pem`/`key.pem`）、`conf/`（vhost）。备份单个站点目录即备份整站。**不提供旧布局迁移**：测试期无正式库，启动不搬移/清理任何旧路径（`public/`、`/www/logs/nginx/`、`/www/certs/<域名>/`、公共 `conf.d` 站点 conf 均忽略）；新建、日志、证书、vhost 全部直接落在站点目录内。

未知 Host / 裸 IP 访问分两种状态，**不要混为一谈**：

1. **无默认站点**：80 端口没有 `default_server`，nginx 取第一个 server（按文件名是兜底站），裸 IP 与未知 Host 都返回 **404**。
2. **有默认站点**：`default_server` 由默认站接管，裸 IP 与未知 Host 都落到默认站并返回 **200**（1Panel 行为）；兜底站此时不参与匹配。

兜底站不使用 `server_name _`（默认站已占用 `_`），因此 `nginx -t` 与 reload 不应再出现
`conflicting server name "_" on 0.0.0.0:80` 警告。

### 2.12.1 网站运行环境与三类建站（2026-11-05 全新库实测）

schema v7 起 `sites` 整表重建为 `site_type(runtime/static/proxy)` + `runtime_id` + 反代字段，
`site_runtimes` 种入唯一 `php-8.5`；**测试期不做旧数据迁移**，重置库后由面板重新建站。

- 全新库 `PRAGMA user_version=7`，`site_runtimes` 仅 `php-8.5`（默认），`sites=0`；
  `POST /ui/v1/runtimes` 完成环境安装后 `docker exec skiff-web-nginx nginx -t` 成功
  （当时走 `POST /{p}/sites/env/install`，该路由已于 2026-11-19 删除）。
- 三类站（入口前缀 `/swn4nbhe`）：
  - PHP（`php.local` + `runtime_id=php-8.5`）：HTTP 与 HTTPS 均 200，正文含 `PHP Version 8.5.12`；
  - 静态（`static.local`）：自定义 `index/index.html` 返回 200；
  - 反代（`proxy.local` → `http://127.0.0.1:18081`）：200，`Server: openresty/1.31.8.1`、
    `X-Upstream: 18081`，上游收到 `Host: proxy.local`。
- 删除三个站均 303：站目录与 `conf/<域名>.conf` 消失，`nginx -t` 成功，`Host` 回落 404，
  库内 `sites=0`；`/www/sites` 为空。
- 拒绝路径（当时为服务器表单页内联告警；2026-11-19 起同一校验经 `POST /ui/v1/sites` 返回 4xx JSON 信封）：重复域名「域名已存在」；`ftp://` 上游
  非 http(s)；`site_type=node|go` 只支持三类；未知 `runtime_id` 运行环境不存在。
- 运行环境页：`/{p}/runtimes`、`/{p}/runtimes/php-8.5` 均 200
  （`/{p}/sites/env*` 别名已于 2026-11-19 删除，直达 404，见 [`URL_CONTRACT.md`](../../contract/URL_CONTRACT.md) §7）；PHP 配置保存 303，
  `/www/php/www.conf` 写入 `memory_limit=512M`、`upload_max_filesize=128M`、`post_max_size=256M`、
  `max_execution_time=120`、`pm=ondemand`，PHPInfo 页面显示 `upload_max_filesize=128M`；
  关联站点数 1→0 正确；绑定 `php.local` 时删除运行环境被拒并列出域名。

### 2.13 站点 HTTPS（2026-11-04 已实测）

一句话：证书按站点落 `/www/sites/<域名>/ssl/{cert.pem,key.pem}`，vhost（`/www/sites/<域名>/conf/<域名>.conf`）追加 `listen 443 ssl`，
强制跳转把 80 块换成 `return 301 https://$host$request_uri;`；所有改动先写盘、再
`nginx -t`，通过才 reload，失败按文件快照与库开关回滚。

| 操作 | 面板入口 | 落盘效果 |
|---|---|---|
| 生成自签 | 站点详情 → SSL → 生成自签证书 | `openssl req -x509 -newkey rsa:2048 -sha256 -days 825 -nodes`，`CN=<域名>`、`subjectAltName=DNS:<域名>[,DNS:<别名>]`；`cert.pem` 0644、`key.pem` **0600** |
| 上传 PEM | 同页 `<details>` 表单 | 先校验 CERTIFICATE / PRIVATE KEY 标记与公钥配对，再原子替换；不匹配直接拒绝，不动现网文件 |
| 强制 HTTPS | 同页「强制切换」 | `force_https=1` 且必须已启用 HTTPS；80 返回 **301** 到 `https://$host$request_uri` |
| 关闭 / 删除 | 同页「关闭 HTTPS」「删除证书」 | 关闭保留文件；删除先关 TLS 再删目录 |

真机实测（Debian 13，`demo.local` 为默认站，域名解析到 `10.10.15.4`）：

```sh
# 生成自签 + 启用
curl -sk https://10.10.15.4/ -o /dev/null -w '%{http_code}\n'      # 200
grep -o 'PHP Version 8.5.12' <响应体>                               # 命中
openssl x509 -in /www/sites/demo.local/ssl/cert.pem -noout -subject -ext subjectAltName
stat -c '%a' /www/sites/demo.local/ssl/key.pem                      # 600
docker exec skiff-web-nginx nginx -t                                # syntax ok，无 warning

# 强制跳转
curl -s -o /dev/null -D - http://10.10.15.4/ | head -2               # 301 + Location: https://10.10.15.4/
curl -sk https://10.10.15.4/ -o /dev/null -w '%{http_code}\n'       # 200
```

回滚实测：手工塞入一个坏 conf（`zz-bad.conf`）后点「关闭 HTTPS」，页面以 flash 展示
「nginx -t 失败…」（不再用 `?error=` 传参），站点 conf 的 md5 与库内 `https/force_https` **均未变化**；
坏 conf 移除后 `nginx -t` 通过、HTTPS 仍是 200。非法 PEM 上传同样被拒且现有证书不受影响。

> 注意：校验失败时**不得重启 nginx**——若坏 conf 来自外部，重启会让现网 nginx 起不来。
> 实现里「`nginx -t` 失败」分支只恢复文件与库开关；只有「重启动作本身失败」才重试重启。

### 2.14 镜像源自动落盘（2026-11-04 已实测）

「网站 → 设置」里的 `registry_mirror` 保存后不再只是写库，而是合并进 `/etc/docker/daemon.json`：

1. 解析：逗号（含中文逗号）/空白分隔，必须带 `http(s)://`，去掉尾部 `/`；
2. 合并：保留 `daemon.json` 其它顶层键，只替换 `registry-mirrors`；空值删除该键但保留文件合法；
3. 备份：内容有变化时先写 `daemon.json.bak.<时间戳>`，再临时文件 + `rename` 原子替换；
4. 重启：**有运行中容器**时只落盘不重启，页面明确提示；无运行中容器才 `systemctl restart docker`，
   等 socket 就绪并按最多 8 次、每次 1 s 重试列表接口（避开面板旧连接池的首个失败请求）；
5. 回滚：重启或可用性检查失败则恢复备份并再次重启，接口以 flash 报错（不再用 `?error=` 传参），库内设置不更新。

真机实测（原有 `daemon.json` 含 `log-driver` 等键）：

```sh
# 有运行中容器：只落盘
docker ps --format '{{.Names}}'          # skiff-web-nginx / skiff-web-php 在跑
# 页面保存后：notice 提示「有运行中容器，未重启」；备份文件出现；JSON 合并成功

# 停容器后再保存：写入并重启
docker stop skiff-web-nginx skiff-web-php
# 页面保存后：notice 提示「Docker 已重启」；docker active；docker pull hello-world:latest 成功
docker start skiff-web-nginx skiff-web-php
```

重启 docker 后，面板容器列表页仍为 **200**（连接池自动重建），网站 HTTPS 恢复 200。
