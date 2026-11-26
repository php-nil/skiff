# 内核防火墙原语契约（Firewall Contract）

> 状态：**能力面已冻结（2026-11-16 起）——本文件是行为规格，不是实现路径**。
> 15 条方法的宿主命令编排已全部迁到插件侧 `plugins/firewall`，经内核 exec 能力面提交
> （`bin:` 授权 + 作业记录 + 全审计），见 [`HOST_EXEC_DESIGN.md`](../plugin/HOST_EXEC_DESIGN.md)。
> 内核 `CAP_FIREWALL_CALL` 保留为**迁移提示入口**，对所有方法统一返回 400
> （冻结决定见 HOST_EXEC_DESIGN §五；为何不删见 `docs/impl/KERNEL_ARCH_AUDIT.md` §2 N29）。
> 下文 §2–§6、§8 仍是防火墙插件的**行为规格**（主体/授权、作用域边界、审计口径、错误码）；
> 现行实现：`plugins/firewall/src/main.rs`（编排与命令）、`crates/core/src/kernel/firewall.rs`
> （冻结入口的鉴权与审计）、`crates/plugin/src/firewall.rs`（冻结入口的拒绝实现）。
> 插件约定见 [`plugins/README.md`](../../plugins/README.md)。

## 0. 一页结论

- **能力面 `firewall` 已冻结为迁移提示**：原设计是内核在 Linux 宿主机上代执行 nftables/iptables
  与 fail2ban 命令；2026-11-16 起改为防火墙插件经 exec 面自行提交，内核不再代执行。
- **调用方仍是插件，但授权换了面**：旧包经 `POST /api/v1/capability/firewall/call` 调用会拿到
  400「已迁移」；现行插件走 exec，权限由 `bin:` 授权 + `exec:run` 判定，命令与退出码进作业中心。
- **只动自有作用域**：nft `inet skiff` 表的 `input` 链、iptables `SKIFF` 链、
  `/etc/fail2ban/jail.d/skiff-<jail>.conf`；宿主既有规则只读，默认策略保持 accept。
- **fail2ban 归防火墙插件管**：FTP 不再安装/配置 fail2ban，改为可选登录防护经 interop 调用
  `com.skiff.firewall:fail2ban.*`；未安装防火墙插件只影响该可选项。
- **只在 Native 形态提供**：非 Native 或非 Linux 返回 503；`pkgfmt` 的 `available_in` 不列 Docker 受限形态。
- **审计口径**：冻结入口仍记 `firewall.<method>`（actor `plugin:<id>`、source `uds`、
  结果 `ok | fail | deny`，detail 只含 `jail/port/protocol/source/action/ip/id`）；现行插件走 exec，
  审计在作业记录里。

## 1. 背景与能力缺口

| 现有能力面 | 不足 |
| --- | --- |
| `net` | 一律 501，且语义是「插件出网」，不是宿主防火墙配置 |
| `fs` | 只能在授权根内读写文件；改不了内核规则表、不能重启 fail2ban |
| `docker` | 与宿主防火墙无关 |
| `scheduler` | 只调度任务，不执行宿主命令 |

宿主防火墙与 fail2ban 都需要 root；按仓库红线「能力面实现只在 `crates/plugin`」，新增 `firewall`
能力面，插件只做 UI 与编排，特权命令由内核经瞬时 systemd 单元执行。

## 2. 主体与授权模型（只对冻结入口成立）

| 主体 | 身份来源 | 权限 |
| --- | --- | --- |
| 插件（旧包） | Supervisor 注入的 `cap::PluginId`（UDS 请求扩展） | 只可使用 manifest 声明的 `firewall` 动词 |
| 内核 | 进程自身（原生 systemd 服务） | 冻结入口只做鉴权与审计，**不再执行** nft/iptables/fail2ban-client/systemctl |

授权链（**仅对仍在声明 `firewall` 动词的旧包成立**，现行插件已不再声明）：

1. `manifest.toml`：`requires = ["firewall"]` + `[[permissions]] capability = "firewall" verbs = [...]`；
2. 安装时把 verbs 落成 grants：`perm:firewall:<verb>`；`firewall:*` 表示全量；
3. 调用 `POST /api/v1/capability/firewall/call`：
   - 取不到 `PluginId` → 403；
   - grants 无 `firewall:*` 且无 `firewall:<method>` → 403，并审计 `firewall.deny.<method>`；
   - 通过后进 `dispatch`，**统一返回 400「已迁移，请更新调用方」**并审计 `firewall.<method>`；
4. 单点来源：UDS 路径常量只在 `crates/wire`（`CAP_FIREWALL_CALL`）；能力枚举只在
   `crates/pkgfmt`（`Capability::Firewall`）；SDK 只在 `crates/sdk` 暴露 `firewall_call`；
   拒绝实现只在 `crates/plugin/src/firewall.rs`。

现行插件的授权与上一套无关：manifest 写 `requires = ["exec", "interop"]`，权限是 `exec:run`
加 `resource_hints` 里的 `bin:<绝对路径>`，见 [`HOST_EXEC_DESIGN.md`](../plugin/HOST_EXEC_DESIGN.md)。

## 3. 方法清单（api = 1.2，冻结前的动词口径）

> 下列 15 个名字仍是**现行接口**——防火墙插件的 RPC 与 interop 用同名方法（`plugins/firewall/src/main.rs`），
> 插件 UI 与 FTP 的登录防护都按这些名字调用。变化只在授权面：不再走 `firewall:<verb>` grants，
> 改由 exec 的 `bin:` 授权判定。
>
> **下表「说明」列的语义已于 2026-11-26 上机核对并复验，按插件实现校正**（复验包 `64f64cc`，`sha256=3098f223…`）：
> ① `fail2ban.status` 返回 `{installed, running, state, reason, jails}`——三态由 RPC 直接给出，
> `jails` 只列 `skiff-*` 受管 jail；② `fail2ban.install` 先写 `skiff-defaults.conf`
> （`[sshd] enabled = false`）再 `enable` + `restart`，装完 `fail2ban-client status` 不含 `sshd`；
> ③ `fail2ban.start` = `enable` + `restart` + 用 `fail2ban-client status` 复核，返回
> `{running, jails, residual_jails}`（`residual_jails` = 请求停用但仍在跑，通常是宿主
> `jail.d` 里排序更晚的文件覆盖了托管默认值）；④ `fail2ban.uninstall` 用 `apt-get purge`、
> 删 `skiff-defaults.conf`，再用 `rm -df` 清掉因此残留的空 `jail.d` / `/etc/fail2ban`
> （dpkg 只在目录为空时删目录，purge 那一刻托管文件还在），返回 `{state: "missing", removed: true}`；
> ⑤ `fail2ban.jail.ensure` 显式落盘 `filter = <filter>`（缺省回落到 jail 名）与 `logpath`（可选），
> 落盘前校验 `filter.d/<filter>.conf` 与 `logpath` 存在 → 写文件 → **`fail2ban-client -t` 预检**
> → restart → 复核该 jail 真的在运行集合里；预检/重启/复核任一失败都撤回文件，并区分「本次配置
> 的问题」与「宿主既有配置本来就有问题」。2026-11-26 前这一层完全缺失：`filter` 与 `logpath`
> 都被静默丢弃，jail 配置坏掉时整个服务 exit 255，而 `ensure` 仍返回 `ensured: true`。
> 回归项与代码位置见 [`TEST_MATRIX/06-FIREWALL_JOBS.md`](../test/TEST_MATRIX/06-FIREWALL_JOBS.md)。

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
| `fail2ban.stop` | — | `systemctl stop`（不改开机自启）；未安装时由内核面判定回 503 |
| `fail2ban.uninstall` | — | 停止服务（失败不阻断）、`disable`、`apt-get purge -y fail2ban`（**失败即报错**，不报假成功）、删 `skiff-defaults.conf`、`rm -df` 清残留空目录；返回 `{state: "missing", removed: true}` |
| `fail2ban.jail.ensure` | `jail`、`filter`(缺省 = jail)、`port`、`backend`(缺省 `auto`)、`maxretry`、`findtime`、`bantime`、`logpath`(可选) | 校验名字字符集、`filter.d/<filter>.conf`、`logpath` 存在 → 写 `/etc/fail2ban/jail.d/skiff-<jail>.conf`（含 `filter =` 与可选 `logpath =`）→ `fail2ban-client -t` 预检 → 重启 → 复核 jail 在运行集合；任一步失败撤回文件并报错。返回 `{ensured, jail, filter, logpath, path, jails}` |
| `fail2ban.jail.remove` | `jail` | 删除本能力面写入的 jail 文件（删不掉即报错）并重启 fail2ban，复核该 jail 已消失；只认 `skiff-` 前缀 |
| `fail2ban.banned` | `jail` | 读取该 jail 的封禁 IP 列表；`fail2ban-client` 非零退出转成错误（不返回空列表冒充「0 个封禁」） |
| `fail2ban.ban` | `jail`、`ip` | 手动封禁一个 IP |
| `fail2ban.unban` | `jail`、`ip` | 解除封禁 |

参数校验（写入 argv 前完成，不拼 shell 字符串）：

- `jail`：长度 1–32，只含字母、数字、下划线与连字符（`[A-Za-z0-9_-]`）；
- `port`：单端口或 `起始-结束`，范围 1–65535 且起始 ≤ 结束；
- `source`：IPv4/IPv6 或 CIDR，`valid_cidr` 校验；
- `backend`：仅接受 fail2ban 支持的 `auto|systemd|polling|gamin|pyinotify`；
- `id`：nft 为十进制 handle；iptables 为 `-A SKIFF` 之后的参数串（`-p tcp --dport 80 -j ACCEPT`），
  比 `-S` 行号稳定——删除一条后其余行号会顺移，参数串不会；
- `ip`：非空、长度受限、仅十六进制字符与 `.`/`:`，拒绝空白与 shell 元字符。

## 4. 作用域与安全边界

- **nftables**：只操作 `inet skiff` 表的 `input` 链；表/链不存在时按需创建，默认策略 accept。
  新规则带 `comment "skiff-<nanos>"` 标识；删除按 handle，先 `list` 再删，避免误删同名规则。
- **iptables**：只操作 `SKIFF` 链；链不存在时创建并挂到 `INPUT`。删除按 id（`-A SKIFF` 后的
  参数串）拼成 `-D SKIFF <参数>`，只删 `SKIFF` 链内的行；不做 `-F` 整链清空。
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

- **通道已换成 exec**：插件把命令交给内核 `exec:run`，执行体走 `skiff_host::run_stream`
  （`systemd-run --wait --pipe --collect` 瞬态单元，不经 shell、超时与取消走 `systemctl stop`），
  实现在 `crates/host`；语义与不变式见 [`HOST_EXEC_DESIGN.md`](../plugin/HOST_EXEC_DESIGN.md) §四。
  冻结入口不再持有任何执行通道——这也是 `plugin::FirewallHost` 被删的原因（无生产使用方）。
- 原设计里由内核 `FirewallHost::run` 控制的超时，现由 exec 作业的 `timeout_sec` 承担；
  超时由单元停止保证，不只 kill 客户端。
- fail2ban jail 文件仍经 `tee` + stdin 写入，不在 argv 暴露内容；`tee` 已列入插件 manifest 的
  `bin:` 授权。

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

## 9. 测试矩阵

> 下表是冻结前的能力面验收口径，保留作为**行为预期**；现行回归项的上机坐标与步骤见
> [`docs/test/TEST_MATRIX/06-FIREWALL_JOBS.md`](../test/TEST_MATRIX/06-FIREWALL_JOBS.md)。
> 涉及该路由的预期已经变了：FW-01（缺授权）仍是 403、FW-11（非 Native/非 Linux）仍是 503——
> 这两道判定在统一拒绝之前；其余方法类用例（FW-02…FW-10、FW-12…FW-17）经内核路由现在一律得到
> 400「已迁移」，同样的行为要验就得打插件的 RPC / interop。

| 编号 | 触发 | 预期 |
| --- | --- | --- |
| FW-01 | 未授予 `firewall:status` 的插件调用 `status` | 403 + `firewall.deny.status` 审计 |
| FW-02 | Native 形态下 `status` | 返回后端探测结果；无 nft/iptables 时 `backend=null` 且带告警 |
| FW-03 | `rule.add` 合法参数 | 规则出现在 `rules.list`，带 `skiff-<id>` 标识 |
| FW-04 | `rule.add` 非法端口/CIDR | 400，无规则写入 |
| FW-05 | `rule.remove` 用自身 id | 只删自有规则；用其它 id 不误删宿主规则 |
| FW-06 | `port.allow` | 等价于追加 accept 规则，参数校验与 `rule.add` 一致 |
| FW-07 | `fail2ban.install`（未安装） | 安装成功、写 `skiff-defaults.conf`、`enable` + `restart`；装完 `fail2ban-client status` 不含 `sshd`；已安装则复用且不动宿主既有 jail |
| FW-08 | `fail2ban.jail.ensure` | 写 `skiff-<jail>.conf`（含 `filter =` 行）并重启；jail 未出现在 status 或 restart 失败即回滚文件 + 报错；非法名/不存在的 filter 400；`filter` 缺省回落 jail 名 |
| FW-09 | `fail2ban.jail.remove` | 只删 `skiff-` 前缀文件；不存在时幂等；删不掉或重启后 jail 仍在则报错 |
| FW-10 | `fail2ban.banned` / `ban` / `unban` | 与 `fail2ban-client` 结果一致；`fail2ban-client` 非零退出一律转错误（不返回空列表冒充「0 个封禁」） |
| FW-11 | 非 Native / 非 Linux | 503 |
| FW-12 | FTP 未装防火墙插件时开启防护 | `available=false` + 原因；FTP 其它功能正常 |
| FW-13 | FTP 已装防火墙插件时开启防护 | jail `pure-ftpd` 生成，端口取 `listen_port` |
| FW-14 | FTP 关闭防护 | 只移除 `pure-ftpd` jail，不卸载 fail2ban |
| FW-15 | `fail2ban.status` 三态 | 未安装 `state=missing`；未运行 `state=stopped` 且 `reason` 可读；运行中 `state=running` |
| FW-16 | `fail2ban.start` / `stop` | `start` = `enable` + `restart` 并复核 jail（返回 `residual_jails`），`stop` = `systemctl stop`，均返回真实状态；未安装时 `start` 返回可读错误 |
| FW-17 | `fail2ban.uninstall` | 停服务、删 `skiff-defaults.conf`、`apt-get purge`；返回 `missing`，审计 `firewall.fail2ban.uninstall` |

单元测试随实现一起迁到了插件：`plugins/firewall/src/main.rs` 的 `#[cfg(test)]` 现有 8 条
（nft 规则解析跳过结构行、iptables 只留 append 行、`port` 参数接受数字与 `a-b` 形式、
`Jail list` 两种排版、托管默认值只关不启、`residual_jails` 语义、`jail_conf` 必含 `filter =`、
jail/filter 名字字符集）。
jail 名 / CIDR 校验、fail2ban 三态、未安装时启停 503 等原内核侧用例已随 `plugin::firewall`
的实现删掉，现有覆盖只能靠上机回归（TEST_MATRIX 06）。Web 侧 RPC 状态映射见
`crates/web/src/handlers/plugins.rs` 的 `rpc_error_passes_kernel_status_through`。

## 10. 未决与决策项

1. **内核 `ftp` 能力面仍保留 fail2ban/login_guard 业务**：本轮受「不动内核业务层」约束未删除，
   FTP 插件显式传 `login_guard=false` 阻止其代管；建议后续独立提交移除该内核分支。
2. **是否支持「结束进程」等写操作**：本能力面不涉及；进程管理目前只读，写操作需先定权限模型。
3. **默认策略**：当前始终 accept；是否提供「一键收紧默认拒绝」需单独评审（误锁风险高）。
4. **iptables 序号删除的竞态**：`-S` 与 `-D` 之间若被其它进程改链，序号可能漂移；
   当前以自有链为前提降低风险，后续可改为 `-C` 校验 + 重试。
