# 内核防火墙原语契约（Firewall Contract）

> 状态：**代码已落地，未经编译验证、未上机验收（2026-11-09）**。
> 本文件是 `firewall` 能力面的契约：主体、授权、方法口径、作用域与安全边界、审计、错误码与测试矩阵。
> 判定与命令编排实现见 `crates/plugin/src/firewall.rs`；内核鉴权/审计/特权通道见
> `crates/core/src/kernel/firewall.rs`；插件约定见 [`plugins/README.md`](../plugins/README.md)。

## 0. 一页结论

- **新增能力面 `firewall`**：内核在 Linux 宿主机上管理 nftables/iptables 规则与 fail2ban jail。
- **唯一调用方是插件**：经 Supervisor 的 UDS 调用，内核从 `cap::PluginId` 取主体、从 grants 校验动词；
  插件自身不碰 `/etc`、不执行宿主命令。
- **只动自有作用域**：nft `inet skiff` 表的 `input` 链、iptables `SKIFF` 链、
  `/etc/fail2ban/jail.d/skiff-<jail>.conf`；宿主既有规则只读，默认策略保持 accept。
- **fail2ban 归防火墙插件管**：FTP 不再安装/配置 fail2ban，改为可选登录防护经 interop 调用
  `com.skiff.firewall:fail2ban.*`；未安装防火墙插件只影响该可选项。
- **只在 Native 形态提供**：非 Native 或非 Linux 返回 503；`pkgfmt` 的 `available_in` 不列 Docker 受限形态。
- **审计口径**：`firewall.<method>`，actor `plugin:<id>`，source `uds`，结果 `ok | fail | deny`；
  detail 只含 `jail/port/protocol/source/action/ip/id`，不含完整命令行。

## 1. 背景与能力缺口

| 现有能力面 | 不足 |
| --- | --- |
| `net` | 一律 501，且语义是「插件出网」，不是宿主防火墙配置 |
| `fs` | 只能在授权根内读写文件；改不了内核规则表、不能重启 fail2ban |
| `docker` | 与宿主防火墙无关 |
| `scheduler` | 只调度任务，不执行宿主命令 |

宿主防火墙与 fail2ban 都需要 root；按仓库红线「能力面实现只在 `crates/plugin`」，新增 `firewall`
能力面，插件只做 UI 与编排，特权命令由内核经瞬时 systemd 单元执行。

## 2. 主体与授权模型

| 主体 | 身份来源 | 权限 |
| --- | --- | --- |
| 插件 | Supervisor 注入的 `cap::PluginId`（UDS 请求扩展） | 只可使用 manifest 声明的 `firewall` 动词 |
| 内核 | 进程自身（原生 systemd 服务） | 执行 nft/iptables/fail2ban-client/systemctl/apt-get；不对 Web 路由直接开放 |

授权链：

1. `manifest.toml`：`requires = ["firewall"]` + `[[permissions]] capability = "firewall" verbs = [...]`；
2. 安装时把 verbs 落成 grants：`perm:firewall:<verb>`；`firewall:*` 表示全量；
3. 调用 `POST /api/v1/capability/firewall/call`：
   - 取不到 `PluginId` → 403；
   - grants 无 `firewall:*` 且无 `firewall:<method>` → 403，并审计 `firewall.deny.<method>`；
   - 通过后进 `dispatch`，结果审计 `firewall.<method>`；
4. 单点来源：UDS 路径常量只在 `crates/wire`（`CAP_FIREWALL_CALL`）；能力枚举只在
   `crates/pkgfmt`（`Capability::Firewall`）；SDK 只在 `crates/sdk` 暴露 `firewall_call`；
   判定与命令编排只在 `crates/plugin/src/firewall.rs`。

## 3. 方法清单（api = 1.2）

| 方法 | 参数 | 说明 |
| --- | --- | --- |
| `status` | — | 后端探测（nftables/iptables 是否可用）、受管规则数、fail2ban 状态摘要、告警；取不到回 `null`，不用 0 冒充 |
| `rules.list` | — | 列出受管规则（nft handle / iptables 序号、动作、协议、端口、来源、comment） |
| `rule.add` | `action`(accept/drop)、`protocol`(tcp/udp)、`port`(1–65535 或 `a-b`)、`source`(可选 CIDR)、`comment`(可选) | 在自有链追加一条规则 |
| `rule.remove` | `id`（nft handle 或 iptables 序号） | 按 id 删除受管规则 |
| `port.allow` | `port`、`protocol`、`source`(可选) | `rule.add` 的便捷入口：放行某端口/协议 |
| `fail2ban.status` | — | 安装/运行状态（`state` = `missing` / `stopped` / `running`）、未运行时的可读 `reason`、jail 列表与启用状态、封禁计数；取不到回 `null` |
| `fail2ban.install` | — | 安装或复用宿主 fail2ban；新装时写 `skiff-defaults.conf` 关闭 `sshd` jail，随后 `enable` + `restart` 并复核实际启用的 jail 集合（见 §4） |
| `fail2ban.start` | — | `systemctl enable` + `restart`（不是对已运行服务无效的 `enable --now`）并按 §4 复核 jail；未安装回 503 |
| `fail2ban.stop` | — | 显式停止并取消开机自启（`systemctl disable --now`）；未安装回 503 |
| `fail2ban.uninstall` | — | 停止服务、删除本能力面写入的 `skiff-defaults.conf`、`apt-get purge -y fail2ban`；未安装回 503；停止失败不阻断卸载，原因放返回的 `warning` |
| `fail2ban.jail.ensure` | `jail`、`filter`、`port`、`backend`、`maxretry`、`findtime`、`bantime`、`logpath`(可选) | 写入 `/etc/fail2ban/jail.d/skiff-<jail>.conf` 并重启 fail2ban |
| `fail2ban.jail.remove` | `jail` | 删除本能力面写入的 jail 文件并重启 fail2ban；只认 `skiff-` 前缀 |
| `fail2ban.banned` | `jail` | 读取该 jail 的封禁 IP 列表 |
| `fail2ban.ban` | `jail`、`ip` | 手动封禁一个 IP |
| `fail2ban.unban` | `jail`、`ip` | 解除封禁 |

参数校验（写入 argv 前完成，不拼 shell 字符串）：

- `jail`：长度 1–32，只含字母、数字、下划线与连字符（`[A-Za-z0-9_-]`）；
- `port`：单端口或 `起始-结束`，范围 1–65535 且起始 ≤ 结束；
- `source`：IPv4/IPv6 或 CIDR，`valid_cidr` 校验；
- `backend`：仅接受 fail2ban 支持的 `auto|systemd|polling|gamin|pyinotify`；
- `id`：nft 为十进制 handle，iptables 为十进制序号；
- `ip`：非空、长度受限、仅十六进制字符与 `.`/`:`，拒绝空白与 shell 元字符。

## 4. 作用域与安全边界

- **nftables**：只操作 `inet skiff` 表的 `input` 链；表/链不存在时按需创建，默认策略 accept。
  新规则带 `comment "skiff-<nanos>"` 标识；删除按 handle，先 `list` 再删，避免误删同名规则。
- **iptables**：只操作 `SKIFF` 链；链不存在时创建并挂到 `INPUT`。删除按 `-S` 列出的序号，
  只删 `SKIFF` 链内的行。
- **fail2ban**：只写 `/etc/fail2ban/jail.d/skiff-*.conf`；不改系统级 `jail.conf`、不改其它 jail。
  `fail2ban.install` 新装时写 `skiff-defaults.conf` 关闭 `sshd`，避免新装 fail2ban 把宿主 SSH 登录封掉；
  已有 fail2ban 的机器不改其既有 sshd jail。显式管理面提供 `start` / `stop` / `uninstall`：
  卸载会 `apt-get purge` 软件包，但只删除本能力面写入的 `skiff-defaults.conf`，
  其它 `skiff-<jail>.conf` 保留（重装后可复用）。
- **配置生效口径**：`install` 与 `start` 都是「`systemctl enable` + 显式 `systemctl restart`」两步。
  包安装的 postinst 会先拉起服务，`enable --now` 对运行中的服务是 no-op、不会重读 `jail.d`，
  所以不能把它当作「新配置生效」。重启后按 `skiff-*.conf` 中 `enabled = true` 的段复核实际
  `fail2ban-client status` 的 jail 集合：多出来的 jail（典型是 Debian 自带的 `sshd`）先
  `fail2ban-client stop <jail>` 立即生效；`skiff-defaults.conf` 尚无该段时追加 `enabled = false`
  再重启一次，把「关闭」落到配置层。只有 `skiff-defaults.conf` 存在时才做这步复核——
  宿主原有 fail2ban 且没有本能力面 defaults 时，`start` 只重启、不改其既有 jail。
- **返回体一致性**：`disabled_jails` / `residual_jails` 都以**最终复核后的** `fail2ban-client status`
  实际 jail 集合为准，而不是「已请求停用」集合。请求停用且复核后确实消失的 jail 记入
  `disabled_jails`；请求停用但复核后仍在（典型是宿主 `jail.d/zz-*.conf` 排序晚于 `skiff-*.conf`
  重新覆盖）记入 `residual_jails`。两个数组互斥，且与同一响应的 `jails` 不交叉：
  `jails` 里的 jail 一定不出现在 `disabled_jails` 里。
- **不做**：清空全部规则、修改默认策略为 drop、flush 其它表/链、执行任意命令；
  不删除用户创建的其它 jail 文件。

## 5. 审计口径

| 字段 | 值 |
| --- | --- |
| `action` | `firewall.<method>`；拒绝为 `firewall.deny.<method>` |
| `actor` | `plugin:<id>` |
| `source` | `uds` |
| `result` | `ok` / `fail` / `deny` |
| `detail` | 只含 `jail` / `port` / `protocol` / `source` / `action` / `ip` / `id`，**不含**完整命令行、stdin 内容或文件全文 |

## 6. 错误码

| 场景 | HTTP |
| --- | --- |
| 未知方法、参数非法（端口/CIDR/jail/backend/id/ip） | 400 |
| 非 Native 形态或非 Linux | 503 |
| 宿主命令执行失败（安装/启停/卸载/jail 写入） | 500，message 带原始 stderr 摘要 |
| `fail2ban.start` / `stop` / `uninstall` 时未安装 | 503，message 为「fail2ban 未安装，无法…」 |

面板代理（`crates/web/src/handlers/plugins.rs::rpc_error`）按同一语义透传
401/403/404/409/500/503/504/507，不把 5xx 压成 400。

## 7. 部署形态与特权通道

- 特权命令经 `/usr/bin/systemd-run --unit skiff-fw-<nanos> --wait --pipe --collect --quiet`
  的瞬态单元执行，不使用 shell；`stdin` 仅用于 `tee` 写 jail 文件的场景。
- 所有命令带超时（由 `FirewallHost::run` 的 `timeout_secs` 控制），超时终止子进程。
- 文件写入走 `tee /etc/fail2ban/jail.d/skiff-<jail>.conf`，内容经 stdin 传入，不在 argv 暴露。

## 8. 与插件的关系（interop）

- 防火墙插件 manifest 用 `[[interop.provides]]` 暴露
  `fail2ban.status` / `fail2ban.install` / `fail2ban.jail.ensure` / `fail2ban.jail.remove` /
  `fail2ban.banned` / `fail2ban.ban` / `fail2ban.unban`。
- `fail2ban.start` / `stop` / `uninstall` **只提供插件 UI 显式管理**，不通过 interop 暴露：
  调用方（FTP 等）只应确保 jail，生命周期由管理员在防火墙页决定。
- 调用方写 `com.skiff.firewall:<method>`，并声明 `requires = [..., "interop"]`、
  `[[permissions]] capability = "interop" verbs = ["call"]`、
  `resource_hints = ["plugin:com.skiff.firewall"]`、`[interop].consumes`。
- 四道闸、15s 超时、busy 防环与 `_interop.caller` 注入沿用既有 interop 契约。
- 目标插件未安装/未启用时调用失败：FTP 的 `protection` 返回 `available=false` 与原因，
  **不阻断** FTP 的安装、账号与配置。

## 9. 测试矩阵（本轮未编译执行）

| 编号 | 触发 | 预期 |
| --- | --- | --- |
| FW-01 | 未授予 `firewall:status` 的插件调用 `status` | 403 + `firewall.deny.status` 审计 |
| FW-02 | Native 形态下 `status` | 返回后端探测结果；无 nft/iptables 时 `backend=null` 且带告警 |
| FW-03 | `rule.add` 合法参数 | 规则出现在 `rules.list`，带 `skiff-<id>` 标识 |
| FW-04 | `rule.add` 非法端口/CIDR | 400，无规则写入 |
| FW-05 | `rule.remove` 用自身 id | 只删自有规则；用其它 id 不误删宿主规则 |
| FW-06 | `port.allow` | 等价于追加 accept 规则，参数校验与 `rule.add` 一致 |
| FW-07 | `fail2ban.install`（未安装） | 安装成功、写 `skiff-defaults.conf`、`enable` + `restart`；装完 `fail2ban-client status` 不含 `sshd`；已安装则复用且不动宿主既有 jail |
| FW-08 | `fail2ban.jail.ensure` | 写 `skiff-<jail>.conf` 并重启 fail2ban；非法 jail 名 400 |
| FW-09 | `fail2ban.jail.remove` | 只删 `skiff-` 前缀文件；不存在时幂等 |
| FW-10 | `fail2ban.banned` / `ban` / `unban` | 与 `fail2ban-client` 结果一致 |
| FW-11 | 非 Native / 非 Linux | 503 |
| FW-12 | FTP 未装防火墙插件时开启防护 | `available=false` + 原因；FTP 其它功能正常 |
| FW-13 | FTP 已装防火墙插件时开启防护 | jail `pure-ftpd` 生成，端口取 `listen_port` |
| FW-14 | FTP 关闭防护 | 只移除 `pure-ftpd` jail，不卸载 fail2ban |
| FW-15 | `fail2ban.status` 三态 | 未安装 `state=missing`；未运行 `state=stopped` 且 `reason` 可读；运行中 `state=running` |
| FW-16 | `fail2ban.start` / `stop` | `start` = `enable` + `restart` 并复核 jail（非预期 jail 被 stop/disable），`stop` = `disable --now`，均返回真实状态；未安装 503 |
| FW-17 | `fail2ban.uninstall` | 停服务、删 `skiff-defaults.conf`、`apt-get purge`；返回 `missing`，审计 `firewall.fail2ban.uninstall` |

单元测试见 `crates/plugin/src/firewall.rs` 的 `#[cfg(test)]`（jail/端口/CIDR 校验、
nft/iptables 解析、fail2ban 输出解析、`RuleSpec` 解析、`port.allow`、
fail2ban 三态与可读原因、未安装时启停返回 503、卸载 purge 流程、
装后 `enable`+`restart` 并停用非预期 jail、启动时补写 `enabled = false`、
无托管 defaults 时不触碰宿主既有 jail）。Web 侧 RPC 状态映射见
`crates/web/src/handlers/plugins.rs` 的 `rpc_error_passes_kernel_status_through`。

## 10. 未决与决策项

1. **内核 `ftp` 能力面仍保留 fail2ban/login_guard 业务**：本轮受「不动内核业务层」约束未删除，
   FTP 插件显式传 `login_guard=false` 阻止其代管；建议后续独立提交移除该内核分支。
2. **是否支持「结束进程」等写操作**：本能力面不涉及；进程管理目前只读，写操作需先定权限模型。
3. **默认策略**：当前始终 accept；是否提供「一键收紧默认拒绝」需单独评审（误锁风险高）。
4. **iptables 序号删除的竞态**：`-S` 与 `-D` 之间若被其它进程改链，序号可能漂移；
   当前以自有链为前提降低风险，后续可改为 `-C` 校验 + 重试。
