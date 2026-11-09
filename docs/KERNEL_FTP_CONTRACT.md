# 内核 FTP 原语契约（FTP Contract）

> 状态：已上线（2026-11-07，随 `v0.1.0` 发布；上线时 DB schema v11，当前基线 v15）。
> 本文件是 `ftp` 能力面的**契约**：主体、授权、账号与权限口径、审计、错误码与测试矩阵。
> 实现细节见 `crates/core/src/kernel/ftp.rs`；插件实现约定见 `plugins/README.md`；
> 与内核能力面的关系见本文件 §1.2 与 §8。

## 0. 一页结论

- **新增能力面 `ftp`**：内核在 Linux 宿主机上管理 pure-ftpd（apt 安装、systemd 托管、`pure-pw` 账号、chroot、监听端口、被动端口、连接上限、TLS、fail2ban）。
- **唯一调用方是插件**：经 Supervisor 的 UDS 调用，内核从 `cap::PluginId` 取主体、从 grants 校验动词；插件自身不装包、不碰 `/etc`、不执行宿主命令。
- **只在 Native 形态提供**：`Form::DockerRestricted` 下返回 503；`pkgfmt` 的 `available_in` 亦不列 Docker 受限形态。
- **账号绑定站点**：一个站点可多账号，共用该站点 PHP 容器内 `www-data` 的 uid/gid；chroot 固定 `<site_root>/index`，看不到 `conf/`、`log/`、`ssl/`，也到不了别的站点。
- **服务配置单一真相**：监听端口、被动端口、连接上限、TLS 模式与证书路径、登录防护开关持久化在 `/etc/skiff/ftp-service.json` 由内核唯一维护；插件不再保留 `service-config.json`。
- **审计口径**：`ftp.<method>`，actor `plugin:<id>`，source `uds`，结果 `ok | fail | deny`；detail 只含配置摘要/登录名/启用状态，密码与私钥不入 argv/日志/审计。

> **2026-11-09 变更（未编译验证）**：fail2ban 已从 FTP 能力面收口到独立防火墙插件
> `com.skiff.firewall`（契约见 [KERNEL_FIREWALL_CONTRACT.md](KERNEL_FIREWALL_CONTRACT.md)）。
> FTP 插件安装/保存配置时显式传 `login_guard=false`，登录防护改为可选项并经 interop 调用
> `fail2ban.*`；内核 `ftp.rs` 内的 fail2ban 分支暂未删除（受「不动内核业务层」约束），
> 待后续独立提交移除。本文以下涉及 fail2ban 的描述保留为历史口径。

## 1. 背景与现状

### 1.1 现状事实（2026-11-07 只读核查，未改服务）

| 事实 | 值 | 影响 |
| --- | --- | --- |
| 测试机系统 | Debian 13，Docker 29.7.2 | `pure-ftpd`/`fail2ban` 可从官方源安装 |
| 测试机 PHP 容器 | `skiff-php-batch1`（PHP 8.5-fpm-alpine） | 容器内 `www-data` = **82:82**；宿主 `www-data` = **33:33** |
| 站点目录 | `/www/sites/<domain>` = `root:skiff-plugins 0755`，`index/` 同为 `root:988 0755` | PHP 现状**写不进** `index/`，FTP 首次建号必须只对齐 `index/` 子树 |
| 端口 | 21 与 30000–30100 空闲；无活动防火墙（nft 在、iptables 无规则） | 默认被动端口段可用 |
| fail2ban | 未安装；自带 `pure-ftpd` filter（journalmatch `pure-ftpd.service`） | 登录尝试限制走专用 jail |

### 1.2 能力缺口（为什么必须是内核原语）

| 现有能力面 | 不足 |
| --- | --- |
| `fs` | 只在授权根内读写文件；不能装宿主软件包、不能写 `/etc`、不能管 systemd |
| `docker` | 白名单不开放 `create` / `exec`；即便开放，FTP 也不走容器 |
| `scheduler` | 只调度任务，不执行宿主命令 |

结论：FTP 不能由插件自行实现，必须新增内核能力面；插件只做 UI 与编排。缺口的如实描述同时写进 `docs/PLUGIN_CATALOG.md` 与 `plugins/README.md`。

### 1.3 目标与非目标

目标：
- 按站点建 FTP 账号（每站多账号），上传的文件可被该站点 PHP-FPM 容器读写；
- 首次使用安装 pure-ftpd，幂等；卸载可回收包与配置；
- 监听端口、被动端口范围、连接上限、TLS 模式与证书路径可配置并持久化，保存后重载；
- 登录尝试限制可开可关，首装默认开启；状态回传 fail2ban 安装/运行/jail/封禁详情；
- 账号可启用/禁用；最后登录、配额与速率按可得性如实回传或标注后续项；
- 全程审计。

非目标：
- 不做 FTP 客户端、不做文件在线预览/编辑（属内核文件管理页）；
- 不管理非 Skiff 站点目录之外的 FTP 账号；
- 不为每个账号单独分配 uid（账号不是隔离边界，站点才是）；
- 不支持 Docker 受限形态（能力面收缩为 `docker/identity`）。

## 2. 主体与授权模型

### 2.1 两类主体

| 主体 | 身份来源 | 权限 |
| --- | --- | --- |
| 插件 | Supervisor 注入的 `cap::PluginId`（UDS 请求扩展） | 只可使用 manifest 声明的 `ftp` 动词 |
| 内核 | 进程自身（原生 systemd 服务） | 执行 apt/systemctl/pure-pw；不对 Web 路由开放 |

管理员不直接调用 `ftp` 能力面；管理员通过插件 UI（iframe → `skiff.rpc` → 内核 `/{prefix}/plugins/{id}/rpc` → 插件 UDS）间接操作，权限归属仍是插件主体。

### 2.2 授权链

1. `manifest.toml`：`requires = ["ftp"]` + `[[permissions]] capability = "ftp" verbs = [...]`；
2. 安装时内核把 verbs 落成 grants：`perm:ftp:<verb>`；`ftp:*` 表示全量；
3. 调用 `POST /api/v1/capability/ftp/call` 时：
   - 取不到 `PluginId` → 403；
   - grants 无 `ftp:*` 且无 `ftp:<method>` → 403，并审计 `ftp.deny.<method>`；
   - 通过后进 `dispatch`，结果审计 `ftp.<method>`。
4. 单点实现：能力面枚举只在 `crates/pkgfmt`；UDS 路径常量只在 `crates/wire`；SDK 只在 `crates/sdk` 暴露 `ftp_call`；实际动作只在 `crates/core/src/kernel/ftp.rs`。

### 2.3 方法清单（api = 1.2）

| 方法 | 参数 | 说明 |
| --- | --- | --- |
| `status` | — | 真实服务状态：安装/运行/监听/监听端口/当前连接数、连接上限、TLS、账号启停计数、fail2ban 细节与持久化配置快照；取不到的字段回传 `null`，不用 0/空串冒充 |
| `install` | `listen_port`、`passive_port_start`、`passive_port_end`、`max_clients`、`max_per_ip`、`tls_mode`、`tls_cert_path`、`tls_key_path`、`login_guard`（均可选） | 幂等安装；未传字段沿用 `/etc/skiff/ftp-service.json`；`login_guard=true` 且系统无 fail2ban 时随 pure-ftpd 一起安装 |
| `service.configure` | 同 `install`，均可选 | 校验并持久化配置；服务已安装时立即写 conf/TLS 并重载，未安装时只落盘 |
| `uninstall` | `remove_accounts`、`remove_package`（均可选） | 停服务、删本插件 jail 并重启 fail2ban；`remove_package` 时 `apt-get remove pure-ftpd`（非 purge）；**不删除 `ftp-service.json`**，便于重装沿用 |
| `sites.list` | — | 可绑定站点（含静态/反代）与运行时信息 |
| `accounts.list` | — | 账号 → 站点反查（按 home 归一化）；含 `enabled`、`last_login`（当前恒为 `null` + `unavailable`）、`quota_supported=false`、`rate_supported=false` |
| `account.create` | `site`、`account`、`password` | 建号 + 对齐 `index/` + 只降不升 `MinUID` |
| `account.password` | `login`、`password` | 改密（重写 passwd 后 `mkdb`）；禁用账号返回 400，需先启用 |
| `account.delete` | `login` | 删号（含禁用行）并 `mkdb`；不删除站点文件 |
| `account.set_enabled` | `login`、`enabled` | 启用/禁用：只改 passwd 行 `#` 前缀并 `mkdb`；禁用后登录失败，账号与目录保留 |
| `service.restart` | — | 只重启 `pure-ftpd` |

## 3. 账号模型

- **站点绑定**：`account.create` 必须带已存在站点的域名；站点 → 账号一对多。
- **登录名**：内核生成 `skiff_<slug16>_<fnv4>_<account>`：
  - 强制 `skiff_` 前缀，避免与系统用户、其它服务账号或人工建号重名；
  - `slug` 取域名小写、非字母数字转 `-`；`fnv4` 是域名的 4 位 FNV 短哈希，防同名 slug 碰撞；
  - 账号名 1–32 位、首字符字母数字、仅 `[A-Za-z0-9_-]`。
- **密码**：8–128 字节、禁控制字符（防 stdin 注入与日志污染）。只经 stdin 传给 `pure-pw useradd`，落 `pureftpd.passwd`（哈希）后 `mkdb`；内核不存明文，面板 DB 不存密码，审计不含密码。`pure-pw` 的 `-y` 表示「最大文件数」，**不得设置**。
- **被动端口**：`PassivePortRange` 由内核配置写入，范围 1024–65535、`start < end`、跨度 ≤ 500，默认 `30000 30100`。
- **监听端口与连接上限**：`Bind`（Debian wrapper 的 `-S [addr,]port`，只写端口，默认 21）、`MaxClientsNumber`（默认 30）、`MaxClientsPerIP`（默认 3）；改监听端口后 UI 提示宿主防火墙放通监听端口与被动端口范围，内核不改防火墙。写 `conf/Port` 会被 wrapper 当作未知项，故不得使用。
- **TLS**：`tls_mode` ∈ `off | optional | required`（默认 `optional`，接受 `explicit` 作为 `optional` 别名）；`optional`/`required` 时优先使用配置的证书/私钥路径（两路径必须同时给出），校验文件存在、非空、PEM 标记匹配，并把 key+cert 合并写入 `/etc/ssl/private/pure-ftpd.pem`；未配置路径时沿用既有 PEM，缺失则生成自签。配置了路径但缺失/非法 → 400 可读错误，**不静默回退自签**。
- **账号启停**：禁用只把 `pureftpd.passwd` 行加 `#` 前缀并 `mkdb`；PureDB 中该账号消失，登录失败，passwd 行与原 home 保留；启用反向操作。删除同时回收启用行与禁用行，但不动站点文件。
- **最后登录/配额/速率**：pure-ftpd 的 passwd/PureDB 不记录最后登录，内核不解析 journal 冒充；`accounts.list` 明确返回 `last_login=null` + `last_login_source=unavailable`。配额与速率未实现，返回 `quota_supported=false`、`rate_supported=false`，列为后续项，不做半成品。
- **chroot**：`ChrootEveryone=yes`，账号 home 固定 `<site_root>/index`；FTP 用户只能看到该目录子树。

## 4. 权限模型（uid/gid 与 PHP 容器对齐）

### 4.1 身份探测

| 站点类型 | uid | gid | 说明 |
| --- | --- | --- | --- |
| `runtime`（PHP） | `docker exec -u www-data <runtime容器> id -u` | `id -g` | 容器内值，**不是宿主值**（实测 82:82 vs 33:33） |
| `static` | 系统用户 `skiff-ftp`（无登录 shell） | 站点 `index/` 的组 | 静态站点无 PHP 进程；拒绝 uid 0 |
| `proxy` | 拒绝 | — | 没有文档根可写 |

### 4.2 MinUID：只降不升

`pure-ftpd` 的 `MinUID` 是全局配置（默认 1000）。实测容器 uid 为 82 时，1000 会拒绝登录。策略：

- `install` **不写死** `MinUID`；
- `account.create` 每次探测到 uid 后调用 `ensure_min_uid(uid)`：`MinUID = min(当前值, 探测值)`；
- 只下调，避免换镜像后把已存在账号（低 uid）全部锁死；换高 uid 镜像时无需改动。

### 4.3 目录与 umask

- `Umask=002:002`：FTP 新建文件/目录对同组可写；`/etc/pure-ftpd/conf/Umask` 必须写
  `002 002`（两个空白分隔的三位八进制数），Debian `pure-ftpd-wrapper` 不接受 `002:002`。
- `pure-pw` 路径按 `/usr/bin` → `/usr/sbin` 顺序探测（Debian/Ubuntu 在 `/usr/bin`）；都不存在时
  返回明确错误，不把 `Failed to find executable` 当成账号冲突。
- 认证链由 `/etc/pure-ftpd/auth/50pure -> ../conf/PureDB` 符号链接生效；安装时移除包自带的
  `65unix` / `70pam`。Debian wrapper 只认 auth 目录链接，conf 里写 `no` 不会停用 PAM。
- 首次建号对齐范围**仅** `<site_root>/index` 子树：目录 `chown uid:gid` + `2775`（setgid 继承组），文件 `0664`/可执行 `0775`；
- 跳过 symlink（不跟随），不碰 `log/`、`ssl/`、`conf/`，不碰其它站点；
- 站点父目录与 `/www` 保持 `root:skiff-plugins 0755`/`0711` 口径，不放宽整体可写。

### 4.4 目录布局

```
/www/sites/<domain>/            root:skiff-plugins 0755   （站点边界）
├── index/                      <容器 uid>:<容器 gid> 2775  （FTP chroot 根）
├── conf/  log/  ssl/           root:skiff-plugins 0755   （FTP 不可见）
```

## 5. 安全边界

1. **目录穿越**：home 由 `validate_index` 计算：canonicalize 后必须同时位于 `/www/sites` 与站点根内，且站点 `index` 本身不得是 symlink；`..` 段一律拒绝。
2. **跨站点**：home 只能是当前站点 `index`；`accounts.list` 按 home 反查站点，不信任登录名。
3. **账号边界**：只管理 `skiff_` 前缀账号；删除/改密前校验该账号在 passwd 文件中存在且 home 合法。
4. **登录尝试限制**：fail2ban 专用 jail（10 分钟内 3 次失败，`bantime` 10 分钟）；连接上限与每 IP 上限改为配置项（默认 `MaxClientsNumber=30`、`MaxClientsPerIP=3`），`DontResolve=yes` 固定。
   `fail2ban` **不是内核内置**，是宿主系统包：首装且 `login_guard=true` 时与 pure-ftpd 一起
   `apt-get install`；安装前已存在则只复用。jail 写到
   `/etc/fail2ban/jail.d/skiff-pureftpd.conf`，内容为 `[pure-ftpd] enabled` / `backend=systemd` /
   `port = <listen_port>` / `maxretry=3` / `findtime=10m` / `bantime=10m`；`port` 随 `service.configure`
   的监听端口改写，端口变更后重写 jail 并重启 fail2ban。只有 fail2ban 由本次安装新装时，才在同一文件写
   `[sshd] enabled=false`，避免影响共享宿主机的 SSH；已有 fail2ban 的机子不改其 sshd jail。
   `login_guard=false` 只移除本插件的 pure-ftpd jail（若同文件还保留安装时写的 `[sshd] enabled=false`，
   则重写为仅含该段）并重启 fail2ban，**不卸载 fail2ban 包**，也不改动其它 jail。
   `ftp.status` 回传 fail2ban `installed` / `running` / `jail_configured` / `jail_enabled` /
   `currently_banned` / `total_banned` / `recent_ban`（口径
   `fail2ban-client get pure-ftpd banip --with-time`）；取不到时为 `null`，不返回 0 冒充。
5. **命令注入**：所有宿主命令用 `Command::new` + 参数数组，不拼 shell；密码只走 stdin；TLS 私钥只读校验后合并写入固定 PEM，不识别的文件内容直接拒绝。
6. **服务控制面**：只允许操作 `pure-ftpd.service`；`uninstall` 不清除其它包；`ftp-service.json` 保留供重装沿用。
7. **配置持久化**：`/etc/skiff/ftp-service.json` 是唯一真相来源（systemd unit 的 `ReadWritePaths` 覆盖 `/etc/skiff`）；插件侧 `service-config.json` 已删除，避免双份真相漂移。
8. **状态未知态**：`status` 的 `running` / `listening` / `current_connections` / `jail_enabled` / 封禁数在探测失败时为 `null`，UI 显示「未知（读取失败）」。

## 6. API 形态

- 插件侧：`Plugin::ftp_call(method, params)` → `POST /api/v1/capability/ftp/call`，body `{"method": "...", "params": {...}}`，响应为内核 JSON。
- 管理面不新增路由：FTP UI 在插件 iframe 内，管理员权限仍由面板会话与插件菜单控制。
- 非 Native 形态：所有方法返回 503（`form != Native`）。
- 非 Linux 目标：返回 503（编译期剔除宿主实现）。

## 7. 审计口径

| 字段 | 值 |
| --- | --- |
| actor | `plugin:<id>` |
| source | `uds` |
| action | `ftp.<method>`；授权缺失时 `ftp.deny.<method>` |
| object | 插件 id（deny 时）；成功调用为空 |
| result | `ok` / `fail` / `deny`（403 为 `deny`，其余错误为 `fail`） |
| detail | 失败原因；成功时只写定位摘要：`install`/`service.configure` 写监听端口、被动范围、TLS 模式、登录防护与是否已应用；账号动词只写登录名与启用状态；**不含密码、私钥内容** |

一次建号会产生 `ftp.account.create` 一条；内部对齐目录、`mkdb`、重启不单独记审计（属于该动作的一部分）。
`service.configure` 与 `account.set_enabled` 各记一条；配置摘要不含证书路径原文之外的敏感信息。

## 8. 部署形态取舍：宿主软件包 vs 容器

| 方案 | 结论 | 理由 |
| --- | --- | --- |
| 宿主 `pure-ftpd` 包（采用） | 采用 | 发行版签名与安全更新；被动端口段直接监听；与 native systemd 生命周期一致；不引入第三方 FTP 镜像 |
| 自制容器 | 放弃 | 供应链风险；需要发布 100 个被动端口；容器内 uid 与宿主文件对齐更复杂；Docker 受限形态也无法复用 |
| 1Panel 商店包 | 不提供 | 能力面收缩为 `docker/identity`，无 apt/systemd |

安装幂等：已装跳过 apt；重复 `install` 只重写配置并重启。`service.configure` 在未安装时只落
`/etc/skiff/ftp-service.json`，安装后沿用。卸载：停服务；按参数删除本插件账号；
若 `remove_package=true`，`apt-get remove -y pure-ftpd` 并删除 pure-ftpd 的 passwd/pdb/conf，
但不执行 `apt purge`，也**不卸载 fail2ban**；本插件 jail 的清理走与 `login_guard=false` 相同的
逻辑（保留安装时写入的 `[sshd] enabled=false`），并重启 fail2ban。`ftp-service.json` 保留。

### 8.1 特权执行通道（加固约束）

`deploy/systemd/skiff.service` 带 `ProtectSystem=full`（`/usr`、`/boot`、`/etc` 只读）与
`NoNewPrivileges=yes`，apt/dpkg 写 `/usr`、`pure-pw`/`useradd` 与配置写 `/etc` 在服务进程内会被
直接拒绝（上机实测报 `Read-only file system`）。因此所有**宿主落盘动作**统一经
`/usr/bin/systemd-run --unit <临时单元> --wait --pipe --collect --quiet -- <命令>` 起独立瞬态单元执行：
瞬态单元不继承主服务沙箱，从而能写 `/usr`、`/etc`。约束不变——

- 命令仍是固定二进制 + 固定参数数组，不经 shell；密码仍只经 stdin（`--pipe` 透传）；
- 只有落盘/装包/改系统状态的动词走该通道；`systemctl is-active/is-enabled/restart`、`getent`、
  `dpkg-query` 等只读或走 D-Bus 的调用仍用普通子进程；
- 该通道不改变授权与审计：调用主体仍是插件 `perm:ftp:<verb>`，审计仍落 `ftp.<method>` /
  `ftp.deny.<method>`，瞬态单元不额外接受其它来源的命令。

## 9. 错误码

| HTTP | 场景 |
| --- | --- |
| 400 | 参数非法（站点/账号/密码/端口范围）、站点不存在的前置校验 |
| 403 | grants 缺失、未知方法、home 越界/软链、非 `skiff_` 前缀账号 |
| 404 | 站点不存在、账号不存在 |
| 409 | 账号重复 |
| 503 | 非 Native 形态、非 Linux、服务未安装 |

## 10. 页面与菜单

- 插件 id `com.skiff.ftp`，挂在「工具」下（order 50，icon `network`）；
- 二级：`accounts`（站点账号）、`runtime`（服务状态/回收）；
- UI 契约遵循 `docs/PLUGIN_UI_CONTRACT.md`（iframe + `skiff.rpc`/`skiff.theme`/`skiff.height`，CSP `default-src 'none'; script-src 'self'; connect-src 'none'`）；
- 菜单契约遵循 `docs/MENU_CONTRACT.md`（manifest 声明，停用即消失）。

## 11. 测试矩阵

见 `docs/TEST_MATRIX.md` 的 `FTP-01`–`FTP-09`；未闭环项记 `docs/TEST_LEDGER.md`。

## 12. 变更记录

| 日期 | 变更 |
| --- | --- |
| 2026-11-07 | 初版：新增 `ftp` 能力面（wire/pkgfmt/SDK/内核/插件）；宿主 pure-ftpd；MinUID 改按探测值只降不升；审计与安全边界定稿 |
| 2026-11-07 | 补充 §8.1：宿主落盘动作改经 `systemd-run` 瞬态单元执行，绕开 `ProtectSystem=full` 的只读限制（上机 install 实测暴露） |
| 2026-11-08 | 能力面扩容：监听端口/连接上限/TLS 证书路径/登录防护开关可配并持久化到 `/etc/skiff/ftp-service.json`；新增 `service.configure` 与 `account.set_enabled`；`status` 回传监听/连接数与 fail2ban 详情；插件删除本地 `service-config.json`，只读标注改为真实交互 |
