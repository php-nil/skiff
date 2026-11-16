# 形态 A：取包、安装、首启与 TLS 三态（§2.1–2.4）

> 上级索引：[DEPLOY_TEST.md](../DEPLOY_TEST.md) ｜ 拆自 `docs/test/DEPLOY_TEST.md` 原 §2.1–2.4（2026-11-16 整理，正文未改）。

---

## 2. 形态 A：宿主机 systemd 直装（主形态）

### 2.1 取得发布物并校验

Release 资产：每个架构 `skiff-<ver>-<target>.tar.gz` + 同名 `.sha256`，另有跨架构汇总
`checksums.txt`。**不做 detached 签名**（§11 决策 12，2026-11-01）：只用 SHA-256 摘要防传输损坏；
`.sig` / `verify-file` 链路已移除。

**安装顺序固定：先验摘要，失败立即停手。** 包内 `install.sh` 不校验包含自己的压缩包（自校验悖论）。

```sh
sha256sum -c skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256   # 本机架构的单包摘要（推荐）
# checksums.txt 含两个架构；只下载一份时不能直接整文件 -c
```

### 2.2 解包安装

**在解压目录内直接执行即可**，不依赖仓库的 `deploy/` 相对路径：

```sh
tar xzf skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz
cd skiff-0.1.0-x86_64-unknown-linux-musl
sha256sum -c checksums.txt        # 包内摘要再复核一次（可选，已含在 Releases 校验内）
sh install.sh                     # 可加 --datadir / --listen / --no-docker 等
```

> 要留安装日志时用**重定向**，不要把脚本接进管道：
> `sh install.sh --no-docker >/tmp/install.log 2>&1; rc=$?`。
> `sh install.sh | tee ...` 的退出码来自 `tee`；用 `${PIPESTATUS[0]}` 取真实码是 bash 语法，
> 在 `sh`/dash 下会报 `Bad substitution` 并把失败误判成成功（见 `docs/test/TEST_ENV.md`）。
> 需要「保留输出 + 原样退出码」时，按 `tools/iter.sh` 的写法先重定向再读 `$?`。

`install.sh` 做的事（幂等，可重复执行）：

| 步骤 | 结果 |
|---|---|
| 建运行组 | `skiff-plugins`（system 组，已存在则跳过） |
| 建站点统一身份 | `skiff-web` 201000:201000（已存在同名同号则复用；UID/GID 被其它账号占用则 `die`，不换号） |
| 建数据目录 | `/www/skiff` 0711 root:root，含 `plugins/ logs/ backup/` |
| 放示例配置 | `/etc/skiff/config.toml`（**仅当不存在时**，已有配置只读不改） |
| 装二进制 | `/usr/local/bin/skiff`（先写临时文件再原子替换） |
| 装插件工具 | `/usr/local/bin/skiff-plugin`（包内有就装，用于打包/验签） |
| 放官方插件 | 包内有 `official/` 时 `install -m 0644` 到 `/www/skiff/official/`（7 个 `.wbpkg` = 6 个官方插件 + testkit，另加 `public-key.txt`）；**不自动安装**——权限与对象授权必须逐条确认；公钥由首启自动信任 |
| 装 unit | `/etc/systemd/system/skiff.service` + `daemon-reload` + `enable` |
| 启动 | `systemctl restart skiff` 并验 active（20 s 不起即失败退出；`--no-start` 可跳过） |

`--no-systemd` 只装二进制（容器/CI 用）；`--prefix /opt/skiff` 改安装前缀时，**unit 里的 `ExecStart` 要同步改**，否则 systemd 找不到可执行文件。

装完立刻按 §1.1 对一遍权限，再进 §2.3。

**旧站点一次性迁移**（安装/升级后执行；无 schema 变更，幂等）：

```sh
skiff sites chown-index --dry-run   # 先打印将调整的路径与数量，不落盘
skiff sites chown-index             # 逐站 chown -R 到 skiff-web:skiff-web
```

范围仅 `<web_root>/sites/<domain>/index`，跳过符号链接；每站一条 `sites.chown-index` 审计，
单站失败不影响其它站（该站本次已改项会尽量回滚）。`log/`、`ssl/`、`conf/` 不动。

> **容器身份相关约束**：不启用容器级 `--user`、不启用 `userns-remap`、不挂载宿主
> `/etc/passwd` 片段。PHP 容器保持镜像默认入口（root 启动），由只读挂载的
> `php-account-init.sh` 在容器内幂等建立 `skiff-web` 后 `exec php-fpm`，再按池配置降权。
> Node / Rust 运行时站点的代码目录仍归 root；FTP v1 不服务这两类站点。

### 2.3 首启与首次登录

```sh
systemctl status skiff --no-pager
journalctl -u skiff -n 40 --no-pager
```

启动日志会打印安全入口前缀与登录地址：

```text
Skiff 已启动
  监听      : 127.0.0.1:18080（明文开发模式）
  数据根    : /www/skiff
  安全入口  : /a1b2c3d4
  配置      : /etc/skiff/config.toml
  登录地址  : http://127.0.0.1:18080/a1b2c3d4/login
  初始口令  : /www/skiff/initial-admin.txt（用户 admin，可直接登录；建议尽快修改口令）
```

`grep` 关键字是「安全入口」：`journalctl -u skiff --no-pager | grep 安全入口`。
监听写成 `0.0.0.0` 时日志里的地址不能直接点，换成真实 IP/域名访问。

初始口令在 `/www/skiff/initial-admin.txt`（0600），**默认不强制首登改密**：登录后直接进入概览，可直接使用面板；仅显示非阻塞的「建议修改口令」提示。改密成功后该文件自动删除。

**监听端口与安全入口前缀**（`listen` / `entry_prefix`）可在「设置 → 后台端口 / 安全入口 / TLS」页修改，
保存即写回 `config.toml`，**需重启生效**；页面会明确提示「需重启」，不会静默失败。规则：

| 项 | 规则 | 失败时 |
|---|---|---|
| 端口 | 1–65535；排除 0；保存前用 `TcpListener::bind` 探针检测占用（与当前监听同值视为自己占用，允许） | 页面就地报错，不落盘 |
| 入口前缀 | 6–32 位 ASCII 字母/数字/`-`/`_`，首字符须字母或数字；不得为 `/`；保留段 `api`/`healthz`/`static`/`assets`/`favicon.ico`/`robots.txt` 拒绝 | 页面就地报错，不落盘 |
| 来源 | `entry_prefix` 显式值（config / 环境变量）优先；未配时首启随机 8 位并持久化到库 `settings.security.entry_prefix` | — |

**防锁死**：前缀变更后，旧前缀在 24 小时内对 GET 请求 302 到新前缀（`settings.security.entry_prefix_previous`
+ `..._previous_until` 两个键控制窗口），窗口外旧前缀直接失效；新前缀始终可用。建议改完先在新标签页
确认新入口能打开，再关旧标签。非当前/非旧前缀的路径一律 404（不是 403）。

**升级行为不变**：已有部署的 `config.toml` 里没有 `entry_prefix` 时继续用库里的旧前缀，入口不变。

需要恢复「首登强制改密」时显式开启（**只对新建管理员生效；已有账号一律不改口令、不改标志**）：

```toml
# /etc/skiff/config.toml
require_initial_password_change = true
```

等价方式：环境变量 `SKIFF_REQUIRE_INITIAL_PASSWORD_CHANGE=1`，或启动参数 `--require-initial-password-change`。重装/升级时 `bootstrap` 只在空库创建管理员，已有 `admin` 的口令哈希与 `must_change_password` 保持原值。

### 2.4 TLS 三态与监听形态（对外前必读）

默认监听 `127.0.0.1:18080`，**默认不启用 TLS**（示例配置里没有 `tls_enabled`，
走旧行为自动推断：loopback + `allow_insecure_http` 明文可登录）。

`tls_enabled` 是三态开关，和 `entry_prefix` / `listen` 落在同一处 `Config`：

| 值 | 含义 | 是否必须有证书 |
|---|---|---|
| `true` | 启用 TLS，本进程终止 | 是；缺失拒绝启动（报错可读，提示补路径或改 `false`） |
| `false` | 显式直接明文 HTTP | 否；不读证书、不生成自签、日志打一行明文提示 |
| 不写 | 旧行为自动推断（loopback / 反代 / 拒绝裸奔） | 有则用，无则按规则矩阵判定 |

对外有三条正路，按场景选一条：

**A. 本进程终止 TLS（公网/跨网推荐）**

```toml
listen      = "0.0.0.0:18080"
tls_enabled = true
tls_cert    = "/etc/skiff/tls/fullchain.pem"
tls_key     = "/etc/skiff/tls/privkey.pem"
```

证书文件须 root 可读、非全局可写。启动时校验文件存在性，缺失直接退出（报错会指出
缺哪个字段、可改 `tls_enabled = false` 走明文）。

自签只用于内网复核（2026-11-03 已实测）。**SAN 必须含实际访问的 IP/域名**，否则浏览器会报名称不匹配：

```sh
mkdir -p /etc/skiff/tls && chmod 0755 /etc/skiff/tls
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout /etc/skiff/tls/privkey.pem \
  -out    /etc/skiff/tls/fullchain.pem \
  -subj "/CN=10.10.15.4" \
  -addext "subjectAltName=IP:10.10.15.4,IP:127.0.0.1,DNS:skiff.local"
chmod 0600 /etc/skiff/tls/privkey.pem
chmod 0644 /etc/skiff/tls/fullchain.pem
```

私钥只放 `/etc/skiff/tls/`（0600 root），不要留在 `/root` 或临时目录。生产请换正式证书
（Let's Encrypt 等）或让反代终止 TLS；自签仅限内网复核，浏览器需手动忽略警告。

**B. 可信反代终止 TLS**

```toml
listen              = "127.0.0.1:18080"
allow_insecure_http = true          # 显式确认「明文只到本机/可信反代」
trusted_proxies     = ["127.0.0.1/32", "::1/128"]
external_url        = "https://panel.example.com"
# tls_enabled 不写：由 loopback + allow_insecure_http + trusted_proxies 推断为「反代后明文」
```

> **不要**在这个形态里写 `tls_enabled = false`：那会把 Cookie 降级成无 `Secure`，
> 浏览器虽仍能登录，但等于放弃了反代形态下本可保留的 `__Host-` + `Secure` 保护。

**C. 直接明文监听（仅内网，浏览器可登录）**

```toml
listen      = "0.0.0.0:18080"
tls_enabled = false                 # 显式关闭 TLS：直接明文，不依赖反代/转发进程
```

`tls_enabled` 是三态：`true` 启用；`false` 显式直接明文；不写 = 旧行为自动推断。
显式关闭时**不走** `allow_insecure_http`/`trusted_proxies` 的旧判据，启动日志会打一行
「明文 HTTP 监听：Cookie 已降级…」；设置页同时给出告警。

规则（§6.2，代码里就是这么判的）：

| tls_enabled | 监听 | 其他条件 | 结果 | 会话 Cookie |
|---|---|---|---|---|
| `true` | 任意 | cert/key 都存在 | HTTPS | `__Host-skiff_session` + Secure |
| `true` | 任意 | cert/key 缺失 | **拒绝启动**，提示补路径或改 `false` | — |
| 未写 | loopback | 无 TLS，`allow_insecure_http=true` | 启动，明文 | `skiff_dev_session`，无 Secure |
| 未写 | 非 loopback | 无 TLS，`allow_insecure_http=true` + `trusted_proxies` **非空** | 启动，视为反代终止 TLS，日志告警 | `__Host-skiff_session` + Secure |
| 未写 | 非 loopback | 无 TLS，`allow_insecure_http=true` + `trusted_proxies` 空 | **拒绝启动**（裸奔） | — |
| 未写 | 非 loopback | 无 TLS，`allow_insecure_http=false` | 拒绝启动 | — |
| `false` | 任意（含 loopback/内网） | 忽略 `allow_insecure_http`/`trusted_proxies` | 启动，明文直连，日志/页面告警 | `skiff_dev_session`，无 Secure；HttpOnly/SameSite 保留 |

三种形态的 Cookie 语义：

- **TLS 开启**：浏览器与面板之间是 HTTPS，用 `__Host-skiff_session` + `Secure`。
- **反代终止 TLS**：浏览器与反代是 HTTPS，反代到面板是明文；仍用 `__Host-` + `Secure`，
  因此浏览器正常回传，但前提是只有 `trusted_proxies` 里的反代能直连面板端口。
- **TLS 关闭（直接明文）**：浏览器直接走 HTTP，`Secure`/`__Host-` 会被丢弃，所以降级为
  `skiff_dev_session`（HttpOnly/SameSite 保持不变）。仅限内网或隔离测试网。

只有直连 peer 命中 `trusted_proxies` 才采信 `X-Forwarded-For/Proto/Host`，否则一律清除伪造头。

**安装脚本与配置的对应关系**（`install.sh`；重装跑也幂等）：

| 参数 / 环境变量 | 写入 `config.toml` |
|---|---|
| `--tls on` / `SKIFF_TLS=on` | `tls_enabled = true`，并要求 `--tls-cert`/`--tls-key`（或从 LE 目录自动发现）；证书复制到 `/etc/skiff/tls/`（0600） |
| `--tls off` / `--no-tls` / `SKIFF_TLS=off` | `tls_enabled = false`，打印明文告警 |
| 不给 | 已有配置有 `tls_enabled` 则沿用；有证书路径视为开、无则交互询问（非交互默认 on 并报错要求补证书） |
| `--listen` / `SKIFF_LISTEN` | `listen` |

配置与环境变量**同一结构**：`SKIFF_TLS_ENABLED`（`true/false/on/off/1/0`）、`SKIFF_ENTRY_PREFIX`、
`SKIFF_LISTEN`、`SKIFF_TLS_CERT`、`SKIFF_TLS_KEY`；优先级为环境变量 > `config.toml` > 默认。

**生效方式**：`listen` / `tls_enabled` / 证书路径 / `entry_prefix` 都是**重启生效**（`systemctl restart skiff`），
无热加载。设置页保存后会明确提示「需重启」，不做静默失败。

#### 2.4.1 内网直连（明文、无 TLS，浏览器可登录）

```toml
listen      = "0.0.0.0:18080"
tls_enabled = false
```

- `tls_enabled = false` 是**显式直接明文**：`serve` 走明文分支，并在启动日志打一行
  「明文 HTTP 监听：Cookie 已降级，仅限内网或反代之后」，设置页同步显示告警。
- **Cookie 口径**：会话 Cookie 降级为 `skiff_dev_session`（无 `Secure`、无 `__Host-` 前缀，
  `HttpOnly` / `SameSite=Strict` 保留），登录页 CSRF Cookie 同理。浏览器从 `http://` 正常
  回传，**登录、改密、页面跳转都能走通**——这是「不依赖反代、不依赖转发进程」的形态。
- `allow_insecure_http` / `trusted_proxies` 在显式 `tls_enabled = false` 时**不参与判定**
  （见 §2.4 的规则矩阵）。`trusted_proxies` 仍只决定**采信哪些 peer 的 `X-Forwarded-*`**：
  填 `127.0.0.1/32` 时，内网客户端伪造的 `X-Forwarded-For` 一律被清除，审计记真实 peer IP；
  **不要**为了让“更多来源能用”把客户端网段填进去，那等于允许它们伪造源地址。
- 安全边界：明文仅限内网 / 隔离测试网。跨公网必须走 TLS 或「反代终止 TLS + `trusted_proxies`」。
- 重装/升级：`sh install.sh --tls off --listen 0.0.0.0:18080`（或 `SKIFF_TLS=off`
  + `SKIFF_LISTEN=0.0.0.0:18080`）。不给参数时脚本按交互/`tls_enabled` 现状决定，且**不改**已有
  `listen`；只有 `/etc/skiff/config.toml` 被删掉才会回到示例默认 `127.0.0.1:18080`。
  `skiff.service` 不含任何监听覆盖项。
- 防火墙（宿主机实测是 iptables，无 firewalld/ufw）：放行回环、`10.10.15.0/24` 与指定开发机 `/32` 的
  面板 18080、站点 80/443，其余 `DROP`；由 `skiff-firewall.service`（oneshot + enabled）在开机时重放，
  iptables 自身不持久。

#### 2.4.2 同端口 TLS 人工复核（自签，2026-11-03 已实测）

配置（`listen` 保持同端口，不再走明文分支；有 TLS 时 `allow_insecure_http` 无意义，已移除）：

```toml
listen   = "0.0.0.0:18080"
tls_cert = "/etc/skiff/tls/fullchain.pem"
tls_key  = "/etc/skiff/tls/privkey.pem"
```

实测环境：证书路径、权限、SAN 与有效期统一见 [TEST_ENV/01-ACCESS.md](../TEST_ENV/01-ACCESS.md) §2（当轮 SHA-256 指纹
`D7:49:D0:06:BC:64:70:3C:E9:D2:AE:D5:D6:62:07:25:B9:97:60:CE:54:0C:57:D8:27:5B:FB:5B:D3:92:67:78`）。

| 项 | 实测结果 |
|---|---|
| `curl -k https://10.10.15.4:18080/6gp3m9vq/login` | **200** |
| `curl -k https://10.10.15.4:18080/healthz` | **200** |
| 错前缀 `/xxxx/login` | **404** |
| `curl http://10.10.15.4:18080/6gp3m9vq/login` | curl `000`（`Received HTTP/0.9 when not allowed`：收到 TLS 告警字节，**不返回页面**） |
| `ss -ltnp` / 日志 | `0.0.0.0:18080`；`INFO HTTPS 监听中 addr=0.0.0.0:18080` |
| cookie-jar 全流程 | GET 登录页 → `skiff_login=<csrf>; Secure`；POST → **303** `/{p}/settings?force=1` + `__Host-skiff_session=…; HttpOnly; SameSite=Strict; Secure`（CSRF 不再 403）；改密 POST → **303** `/6gp3m9vq` + 新 Secure 会话；首页 **200**（`概览 · Skiff`） |
| 重启测试机 | 服务 active、`0.0.0.0:18080`、证书两文件、`initial-admin.txt`、HTTPS 200、iptables 规则全部保持 |
| 重装 `install.sh` | 只在 `config.toml` 不存在时拷贝示例，且见 `tls_cert` 时 `ensure_insecure_http` 直接返回；实测重装前后 `config.toml` sha256 **相同**，HTTPS 200、明文仍被拒 |

复核后的状态恢复：用停机快照 `/var/backups/skiff/datadir-initial-state.tgz` 回滚数据根（入口前缀存库
`settings.entry_prefix`，清库重装会换前缀，不能替代快照）；恢复后 `initial-admin.txt`（0600 root:skiff-plugins）
回来，登录页 200。初始口令只从该文件取，不在聊天/工单里贴明文。

**HSTS 未实现**：`crates/` 内没有下发 `Strict-Transport-Security` 的代码（实测响应头也没有该字段）。
所以本节的结论是「TLS 链路已实测」，**不等于 HSTS 已启用**。

**18081 明文端口不涉及本轮**：内核配置只有单一 `listen`，`Config` 无第二监听字段；要给 API 联调提供
loopback 明文入口，需要单独加一个监听（未实现，也没有临时 hack）。本轮 18081 未改动。

#### 2.4.3 前置反向代理 / 转发层的超时要求（长静默请求）

转发层（nginx 反代、TCP 转发脚本、负载均衡）**必须允许长时间不产出字节的请求**，否则下面这类
操作会在面板上报失败，而实际动作已经完成。

- 受影响操作：`compose down`、`compose up`（含拉镜像）、运行环境安装、大文件/镜像相关请求。
  实测 `compose down` 全程约 **10.4 s** 未向客户端写出任何字节（容器 kill → 网络回收完成后才回包）。
- 现象：浏览器/curl 报连接被提前关闭（`curl: (52) Empty reply from server`）或网关报 502/504；
  但服务端动作已生效——容器与网络实际已删除，审计里有对应记录，`journalctl -u skiff` 无 panic/错误。
- 判定方法（按顺序，一条即可定性）：
  1. 同一请求分别打**直连端口**与**经转发端口**：直连 200 且耗时与断开时刻接近，经转发失败 → 转发层超时。
  2. `nginx` 的 `error.log` 出现 `upstream timed out (110: Connection timed out)` → 读取超时阈值小于操作耗时。
  3. 检查实际结果（`docker ps -a`、`docker network ls`、运行环境列表）已生效 → 动作完成，失败只是响应没回来。
- nginx 配置要求（按最长操作的耗时留余量，示例 300 s）：

```nginx
location / {
    proxy_pass http://127.0.0.1:18080;
    proxy_read_timeout 300s;   # 上游迟迟不写响应（默认 60s，长操作会踩线）
    proxy_send_timeout 300s;   # 上游迟迟不收请求体
    proxy_http_version 1.1;
}
```

- 自写 TCP 转发脚本：建连超时可以用（如 10 s），但**建连成功后必须清掉**（Python 例：
  `upstream.settimeout(None)`）。`socket.create_connection(timeout=10)` 的超时会一直留在已连上的
  套接字上，静默超过 10 s 即被判超时并断开——测试机 18081 曾因此复现该现象，修复与复验见
  [TEST_ENV/04-ACCEPTANCE.md](../TEST_ENV/04-ACCEPTANCE.md) §8（2026-11-13 20:15 条目）。
