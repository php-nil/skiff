# 内核 FTP 原语契约（FTP Contract）

> 本文件是 `ftp` 能力面的**契约**：主体、授权、账号与权限口径、审计、错误码与测试矩阵。
> 实现细节见 `crates/core/src/kernel/ftp.rs`；插件实现约定见 `plugins/README.md`；
> 与内核能力面的关系见本文件 §1.2 与 §8。

## 0. 一页结论

- **新增能力面 `ftp`**：内核在 Linux 宿主机上管理 pure-ftpd（apt 安装、systemd 托管、`pure-pw` 账号、chroot、被动端口、TLS、fail2ban）。
- **唯一调用方是插件**：经 Supervisor 的 UDS 调用，内核从 `cap::PluginId` 取主体、从 grants 校验动词；插件自身不装包、不碰 `/etc`、不执行宿主命令。
- **只在 Native 形态提供**：`Form::DockerRestricted` 下返回 503；`pkgfmt` 的 `available_in` 亦不列 Docker 受限形态。
- **账号绑定站点**：一个站点可多账号，共用该站点 PHP 容器内 `www-data` 的 uid/gid；chroot 固定 `<site_root>/index`，看不到 `conf/`、`log/`、`ssl/`，也到不了别的站点。
- **审计口径**：`ftp.<method>`，actor `plugin:<id>`，source `uds`，结果 `ok | fail | deny`，密码不入 argv/日志/审计。

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
- 被动端口、TLS、登录尝试限制可配置；
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
| `status` | — | 服务/包/端口/TLS/账号数概览 |
| `install` | `passive_port_start`、`passive_port_end`、`tls_mode`、`login_guard` | 幂等安装并配置 |
| `uninstall` | `purge`（可选） | 停服务、删配置与账号；`purge` 时卸包 |
| `sites.list` | — | 可绑定站点（含静态/反代）与运行时信息 |
| `accounts.list` | — | 账号 → 站点反查（按 home 归一化） |
| `account.create` | `site`、`account`、`password` | 建号 + 对齐 `index/` + 只降不升 `MinUID` |
| `account.password` | `login`、`password` | 改密（重写 passwd 后 `mkdb`） |
| `account.delete` | `login` | 删号并 `mkdb` |
| `service.restart` | — | 只重启 `pure-ftpd` |

## 3. 账号模型

- **站点绑定**：`account.create` 必须带已存在站点的域名；站点 → 账号一对多。
- **登录名**：内核生成 `skiff_<slug16>_<fnv4>_<account>`：
  - 强制 `skiff_` 前缀，避免与系统用户、其它服务账号或人工建号重名；
  - `slug` 取域名小写、非字母数字转 `_`；`fnv4` 是域名的 4 位 FNV 短哈希，防同名 slug 碰撞；
  - 账号名 1–32 位、首字符字母数字、仅 `[A-Za-z0-9_-]`。
- **密码**：8–128 字节、禁控制字符（防 stdin 注入与日志污染）。只经 stdin 传给 `pure-pw useradd -y 2`，落 `pureftpd.passwd`（哈希）后 `mkdb`；内核不存明文，面板 DB 不存密码，审计不含密码。
- **被动端口**：`PassivePortRange` 由插件配置写入，范围 1024–65535、`start < end`、跨度 ≤ 500，默认 `30000 30100`。
- **TLS**：`tls_mode` ∈ `off | optional | required`（默认 `optional`）；`optional`/`required` 时生成自签 PEM（`/etc/ssl/private/pure-ftpd.pem`）。
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

- `Umask=002:002`：FTP 新建文件/目录对同组可写；
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
4. **登录尝试限制**：fail2ban 专用 jail（10 分钟内 3 次失败，`bantime` 1 小时）；新装 fail2ban 时关闭默认 sshd jail，避免影响共享主机；`MaxClientsPerIP=3`、`MaxClientsNumber=30`、`DontResolve=yes`。
5. **命令注入**：所有宿主命令用 `Command::new` + 参数数组，不拼 shell；密码只走 stdin。
6. **服务控制面**：只允许操作 `pure-ftpd.service`；`uninstall` 不清除其它包。

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
| detail | 失败原因；**不含密码** |

一次建号会产生 `ftp.account.create` 一条；内部对齐目录不单独记审计（属于该动作的一部分）。

## 8. 部署形态取舍：宿主软件包 vs 容器

| 方案 | 结论 | 理由 |
| --- | --- | --- |
| 宿主 `pure-ftpd` 包（采用） | 采用 | 发行版签名与安全更新；被动端口段直接监听；与 native systemd 生命周期一致；不引入第三方 FTP 镜像 |
| 自制容器 | 放弃 | 供应链风险；需要发布 100 个被动端口；容器内 uid 与宿主文件对齐更复杂；Docker 受限形态也无法复用 |
| 1Panel 商店包 | 不提供 | 能力面收缩为 `docker/identity`，无 apt/systemd |

安装幂等：已装跳过 apt；重复 `install` 只重写配置并重启。卸载：停服务、删账号与配置；`purge=true` 时 `apt purge` 回收包。

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

见 `docs/TEST_MATRIX.md` 的 `FTP-01`–`FTP-05`；未闭环项记 `docs/TEST_LEDGER.md`。

## 12. 变更记录

| 日期 | 变更 |
| --- | --- |
| 2026-11-07 | 初版：新增 `ftp` 能力面（wire/pkgfmt/SDK/内核/插件）；宿主 pure-ftpd；MinUID 改按探测值只降不升；审计与安全边界定稿 |
