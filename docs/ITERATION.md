# 迭代验证回路

> 状态：**已上机**。2026-11-03 已完成首装轮、升级轮、重装轮（PASS 47 / 34 / 47，FAIL 全 0），工具见 §8；**回滚协同仍未实测**（测试机从未触发过回滚）。  
> 本文是「改代码 → 上机验证」的唯一依据。交付门槛见 [AGENTS.md](../AGENTS.md) §3，工作流见 [CI.md](CI.md)，  
> 验收用例原文见 [DEPLOY\_TEST.md](DEPLOY_TEST.md)（本文 §5 是其可执行版）。

## 0. 结论

**编译不进测试机；测试机只回答一个问题：干净系统上装完能不能用。**

| 层      | 在哪                     | 做什么                                                | 每轮                    |
| ------ | ---------------------- | -------------------------------------------------- | --------------------- |
| L1 门禁  | 本机 Windows（增量）         | `fmt` / `clippy` / `test` / **Linux target check** | 90 s                  |
| L2 单测  | GitHub `test` job（已存在） | Linux 全量单测，unix-only 用例真跑                          | 2.5 min（实测，可与 L2′ 并行） |
| L2′ 验收 | **测试机（回滚）**            | 装包 → 起服务 → `accept.sh`                             | 40 s                  |
| L3 发布物 | 里程碑：`release.yml`      | musl 静态包 + 官方插件签名                                  | 4 min（实测），不每轮跑        |

不把编译放测试机，原因三条：开发机 `target` 已 24 GB（Linux 4~8 GB），每轮回滚都要搬；编译会污染"干净系统"这个前提；改到底部 crate 时增量退化成全量，收益不稳。不加第二台机器，因为 `ci.yml` 的 `test` job 本来就是 L2——public 仓库不计费，占地为零。

## 1. 测试机（已确认）

| 属性     | 值                        | 影响                   |
| ------ | ------------------------ | -------------------- |
| 网络     | 内网 VM，**可出外网**；本机**可直达** | Docker 走现成在线链路，无需离线源 |
| 数据     | 可随意删除，允许暴露               | 重置可粗暴，验收可直接明文 HTTP   |
| 回滚     | **全量回滚到初始状态**，人工触发       | 每轮可得干净系统；会清时钟与缓存（§4） |
| Docker | **初始无，需脚本安装，在测试范围内**     | 安装链路本身是待测对象 → 基线分两档  |
| 凭据     | 无需特殊处理，可直接写本文            | 见 §7                 |

## 2. 基线

| 基线     | 内容                                   | 用于                     | 成本             |
| ------ | ------------------------------------ | ---------------------- | -------------- |
| **T0** | 纯净系统 + 公钥 + `/etc/skiff-baseline` 标记 | 首装轮 / Docker 轮：验完整安装链路 | 一次             |
| **T1** | T0 之上已装好 Docker 并自检通过                | 升级轮与其余轮次               | 一次（**需要时说一声**） |

T0 用来测"能不能装上"，T1 用来避免每轮都为 Docker 付 3~5 分钟。

## 3. 轮次

| 类型       | 基线 | 装法                            | 主要验证              | 人工   |
| -------- | -- | ----------------------------- | ----------------- | ---- |
| 首装轮      | T0 | `install.sh`（含 Docker）        | A 组全量 + D0 + 权限位  | 回滚一次 |
| Docker 轮 | T0 | 重点看 `docker-bootstrap.sh` 各分支 | D0/D1/D2          | 回滚一次 |
| 升级轮      | T1 | 换二进制 + restart                | F 组升级路径、B 组认证     | 零    |
| 里程碑轮     | T1 | `release.yml` 正式包             | L3：musl、插件签名、TOFU | 零    |

默认配比 **3 轮升级 : 1 轮首装**。升级轮补上了原先只写在文档里、从未自动验过的 `§2.5 升级` 与 F 组版本窗口。

## 4. 单轮步骤

1. L1 门禁（含 Linux target check，挡 `cfg(unix)` 编译错）
2. 提交到 `iter/<topic>` 并 push
3. **push 即并行触发** CI 与迭代包（`iter.yml` dispatch），不等 CI 绿
4. 轮询两个 run，下载迭代包（musl 裸包，无 `official/`，约 3~5 MB）
5. 需要干净系统 → 请人回滚（§4.1）→ 校时 → `scp` 上机
6. 首装：`install.sh`；升级：换二进制 + restart
7. `accept.sh` 出 PASS/FAIL/SKIP 表；**日志与报告立即回传**（回滚会清掉）
8. 追加 `docs/changes/YYYY-MM-DD.md`；CI 若红标红，验收照跑

### 4.1 回滚协同协议

回滚由人手动操作，脚本只负责"请求 + 等待 + 接续"，人不需要回话。

1. 输出 `【需要回滚】请恢复到 T0/T1`，附轮次、commit、包摘要
2. 每 5 s 轮询 SSH，判据两条同时成立：可连 **且** `/proc/uptime` 首字段 < 180
3. 连续 300 s 未恢复 → 报错退出（**不静默继续**，否则在旧状态上跑出假绿）
4. 校验 `/etc/skiff-baseline` 的基线名与 commit，不符中止
5. **强制校时**：`chronyc makestep` / 重启 `systemd-timesyncd`，断言偏差 < 30 s

**顺序硬约束：先回滚，再传包。** 全量回滚会把 `scp` 上去的文件一起冲掉。第 5 步不可省——快照恢复后时钟停在快照时刻，TOTP 与 session 判定都会错乱，B 组会稳定假红且极难定位。

### 4.2 时间预算（设计目标，实测后回填 §9）

| 段         | 升级轮          | 首装轮（T1）      | 首装轮（T0，含 Docker） |
| --------- | ------------ | ------------ | ---------------- |
| 本地门禁      | 90 s         | 90 s         | 90 s             |
| CI 与迭代包并行 | ~4 min       | ~4 min       | ~4 min           |
| 下载 + scp  | 20 s         | 20 s         | 20 s             |
| 回滚（人工）    | 0            | 30~60 s      | 30~60 s          |
| 安装        | 20 s         | 40 s         | 40 s + 3~5 min   |
| 验收        | 30 s         | 40 s         | 40 s             |
| **合计**    | **≈5.5 min** | **≈6.5 min** | **≈10~12 min**   |

## 5. 验收口径

### 5.1 前置断言（不满足即 FAIL/SKIP，别跑到一半炸）

root（`id -u`=0）· systemd 为 PID 1 · 与真实时间偏差 < 30 s · 端口 18080 空闲 · 磁盘可用 ≥ 2 GB · 内核 ≥ 5.13 且 landlock ABI ≥ 1（不满足则 E 组 SKIP，不算通过）· cgroups v2（仅记录）。

### 5.2 分组

`deploy/accept.sh` 输出 `PASS/FAIL/SKIP` 表 + 退出码，任一 FAIL 即非零。

| 组            | 要点                                                                                                                                                                                                                         | 轮次        |
| ------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------- |
| A 安装与启动      | `install.sh` 退出 0；`/var/lib/skiff` 0711、`backup` 0700、`skiff.db` 0600、`/run/skiff` 0711 且属组 `skiff-plugins`；`systemctl is-active`；`/healthz` 20 s 内 200；`initial-admin.txt` 0600；`GET /<入口>` 303 → `/login`；`GET /` 必须 404 | 首装轮       |
| D0 Docker 安装 | 无 Docker 时装上且 `docker info` 通；已装**不重装**；`--check-only` 退出 2 且不改动；装失败面板仍起（降级）                                                                                                                                               | Docker 轮  |
| D1 Docker 策略 | 三层策略在 create 之前拦截；bind 源须在 `authorized_roots` 内。**无需镜像即可断言**                                                                                                                                                               | Docker 轮  |
| D2 容器操作      | 创建/启动/日志/exec 需镜像与 registry                                                                                                                                                                                                | Docker 轮  |
| B 认证会话       | 首登强制改密（`?force=1`）；改密后 `initial-admin.txt` 消失、旧会话失效；错误口令连击触发限流                                                                                                                                                             | 首装轮       |
| F 备份升级       | 升级后 DB 与插件 `data/` 保留；N-1 版本窗口可回滚                                                                                                                                                                                          | 升级轮       |
| E 插件体系       | 装 → 列表 → 卸载保 `data`；未签名/错公钥必须被拒                                                                                                                                                                                            | 里程碑       |
| C 传输反代       | TLS、HSTS、`X-Forwarded-*` 仅在 `trusted_proxies` 内采信                                                                                                                                                                          | 需证书，单独轮   |
| G 资源共存       | 闲置 RSS、并发登录、与 1Panel 共存                                                                                                                                                                                                    | 长时观测，不进每轮 |

**每条断言必须断言具体文案或数值，不能只看状态码。** 历史教训：`Multipart` 超限被 `map_err` 包成 400，与业务校验的 400 撞在一起，只看状态码分辨不出哪一层拒的。

`deploy/accept.sh` 当前覆盖 **preflight / A / B / D0** 四组（2026-11-03）；D1/D2/E/F/C/G 尚未实现，
调用时会明确报 SKIP 而不是悄悄跳过——**SKIP 不算通过**。

> 一轮只能跑**一次** B 组：改密会消耗 `initial-admin.txt`，重跑 B2 必然失败。要再验一次就重装（`rm -rf /var/lib/skiff` 后跑 `install.sh`，约 1 s）。
> `B4` 登录限流会把 admin 锁 5 分钟，只有首装轮末尾建议开 `--with-rate-limit`。

## 6. 已知问题

### 6.1 已修：按发布包装机，服务起不来（P0，2026-11-03 上机复现并修复）

`install.sh` 把 `config.toml.example` 原样拷成 `/etc/skiff/config.toml`，其中 `allow_insecure_http` 是注释态（默认 `false`），systemd 的 `ExecStart` 不带 `--dev`，`listen` 默认 loopback → `Config::validate` 直接 bail。

**上机复现证据**（首装轮，包为 release run 41311345915 的 amd64 产物 + 当轮 `install.sh`）：

```
skiff[7648]: 错误: 未配置 TLS：loopback 调试请显式加 --dev 或 allow_insecure_http = true（§6.2）…
systemd[1]: skiff.service: Scheduled restart job, restart counter is at 9.
```

`install.sh` 当时仍打印「已启动 skiff.service」并退出 0 —— 这是比 bug 更坏的一层：**装失败被判成装成功**。

**修法（两处，都已在 2026-11-03 应用到首轮）**

1. `install.sh` 新增 `ensure_insecure_http`：拷贝示例配置后，仅当「未配 TLS **且** `listen` 是 loopback」时追加
   `allow_insecure_http = true` 并告警；非 loopback 明文保持拒绝启动（不绕过 §6.2 裸奔防护）。
2. `install.sh` 启动段改为**验证 active**：`systemctl restart` 后轮询 20 s，仍非 active 就打印 journal 尾部并 `exit 1`。
   安装脚本不再能「报告成功但服务在 crash loop」。

**修后实测**：`INSTALL_EXIT=0`、`systemctl is-active=active`、`/healthz` 4 s 内 200、A 组 24 条与 B 组 12 条全绿（见 §9 与 `docs/changes/2026-11-03.md`）。

### 6.2 约束：明文 HTTP 下测不了登录

`dev_http == false` 时 cookie 带 `Secure`，明文 HTTP 下 curl 与浏览器都不回传。故 `accept.sh` 需两档：dev 档走 `--dev`（cookie 降级为 `skiff_dev_session`，可用 `curl -c/-b`），prod 档配自签 TLS + `curl -k` 验真实 `Secure`/`SameSite` 属性。

### 6.3 已消除：Docker 外网依赖

`docker-bootstrap.sh` 依赖 `linuxmirrors.cn` 与 `mirrors.tencent.com`，原为最大未知；现已确认测试机可出外网，走现成链路，`exit 4` 路径仅作降级断言。

### 6.4 已修：systemd 静默改掉数据根属主（只有上机才看得见）

首轮 A 组权限断言 A6a/A6b/A6c 全红：`/var/lib/skiff` 实得 `711 root:skiff-plugins`，而 §1.1 写的是 `root:root`。
根因是 unit 里的 `StateDirectory=skiff`：systemd 按 `User=/Group=` 把它 chown 成 `root:skiff-plugins`，**把 install.sh 的 chown 变成空操作**。

已删掉 `StateDirectory=`，数据根口径回归「单一来源 = install.sh」（`root:root 0711`）；`RuntimeDirectory=` 保留（`/run/skiff` 在 tmpfs 上，必须由 systemd 建，属组正好是 `skiff-plugins`，与 §1.1 一致）。

同时修正一处**文档口径**（不是代码缺陷）：服务进程创建的文件（`skiff.db`、`initial-admin.txt`）属组是 `skiff-plugins`——unit 的 `Group=` 决定进程主 gid。权限闸门是 `0600`（组位为 0），不是属主。`DEPLOY_TEST.md` §1.1 已按实测改正。

## 7. 测试机与凭据

内网测试机，可随意重置，凭据无需特殊处理。填完即用：

| 字段                         | 值                        |
| -------------------------- | ------------------------ |
| 地址 `SKIFF_TEST_HOST`       | 10.10.15.4               |
| 端口 `SKIFF_TEST_PORT`       | 22                       |
| 用户 `SKIFF_TEST_USER`       | root                     |
| 密码 `SKIFF_TEST_PASSWORD`   | 1                        |
| 私钥 `SKIFF_TEST_KEY`        | `~/.ssh/skiff-iter`（已生成） |
| 回滚方式 `SKIFF_TEST_ROLLBACK` | 人工手动操作，需要时告知我            |

公钥（**加入 `authorized_keys` 后再打基线**，顺序反了回滚即失联）：

```
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM6Y+VSKG/QjciBaXQIObQwyg99CEEV+YKhJ5Op68Nrh skiff-iter@DESKTOP-5P3Q9PS
```

首连只做环境探测、不装任何东西：`uname -m/-r`、`/etc/os-release`、`nproc`、`free -h`、`df -h`、`id -u`、`sudo -n true`、`ss -lntp`、`curl -I linuxmirrors.cn`、`apt-get -s update`、landlock ABI、cgroups v2、`systemd --version`。结果回填 §9。

### 7.1 内网 HTTP 代理（GitHub 出网缺口就靠它）

内网有五个 HTTP 代理，都能到 `github.com:443`。2026-11-03 实测（HEAD 一个空响应，**只验通断不跑流量**）：

| 代理              | 本机→github.com | 测试机→github.com | 备注            |
| --------------- | ------------- | ------------- | ------------- |
| `10.66.10.22:1010` | 200 / 0.53 s  | 200 / 1.85 s  | **首选**        |
| `10.66.10.12:1010` | 200 / 0.69 s  | 200 / 0.68 s（偶发 403，重试即可） | 次选 |
| `10.66.10.9:1010`  | 200 / 1.37 s  | 200 / 1.89 s  | 可用          |
| `10.66.10.21:1010` | 200 / 4.83 s  | 200 / 2.34 s  | 偏慢，兜底       |
| `10.66.10.13:1010` | 200 / 1.42 s  | **超时 8 s**    | 本机可用、测试机到不了 |

用法：`git` 只认环境变量，不认 `curl -x`；**必须**加 `-c http.sslBackend=openssl`，否则 Windows 的 schannel 过代理会握手失败（就是 §9.1 记的那个报错）。

```sh
export HTTPS_PROXY=http://10.66.10.22:1010
export HTTP_PROXY=$HTTPS_PROXY
git -c http.sslBackend=openssl push origin <branch>
curl -x $HTTPS_PROXY -sS -I https://github.com          # 单次探活
```

实测旁证：同一条 `git push` 之前用本机代理端口（`127.0.0.1:50121` 之类）一直失败，换成 `10.66.10.22:1010` 后**一次成功**
（`* [new branch] iter/first-install -> iter/first-install`）。`git ls-remote` 也通，返回 `main` 的 commit。

## 8. 工具落点

| 文件                           | 作用                                                                            | 状态 |
| ---------------------------- | ----------------------------------------------------------------------------- | --- |
| `deploy/accept.sh`           | §5 可执行版，PASS/FAIL/SKIP + 退出码                                                  | **已建并实跑**（preflight / A / B / D0；D1/D2/E/F/C/G 未实现，缺则报 SKIP） |
| `tools/iter.sh`              | 单轮编排：门禁 → 取包 → 回滚协同 → 装 → 验收 → 报告                                        | **已建**；gate/fetch/run 三档实跑过，`rollback-wait` 未实测 |
| `tools/linux-check.sh`       | 本地 `cargo check --target x86_64-unknown-linux-gnu`，挡 `cfg(unix)` 编译错          | **已建并实跑**：全量（含 `plugins/` 第二 workspace）**50 s**；`--quick` 只查主 workspace |
| `.github/workflows/iter.yml` | 迭代包：只编 musl x86_64、`--skip-plugins`、`cancel-in-progress: true`、artifact 留 3 天 | **已首次 dispatch 实跑**：main@41beba5 / run 41616343492，出包 **118 s**、产物 9.4 MB |

`release.yml` **不动**（已修四轮，里程碑才跑）。单独建 `iter.yml` 而非加参数：`release.yml` 的 `cancel-in-progress: false` 是刻意防"发出半个包"的，每轮 dispatch 会累积排队并污染发布历史。

**已定默认值**：rustc 1.98.1（与 `ci.yml` 一致）· target `x86_64-unknown-linux-musl` · 分支 `iter/<topic>` 稳定后 squash 进 `main` · 失败即停不重试 · 报告追加 `docs/changes/YYYY-MM-DD.md`。

## 9. 实测回填（未填即视为未验证）

| 项                                              | 值         | 日期/依据                  |
| ---------------------------------------------- | --------- | ---------------------- |
| 本机到测试机 TCP 可达                                  | 通（TCP 22 open，密钥登录 OK） | 2026-11-03 实测 |
| 出网：linuxmirrors.cn / mirrors.tencent.com / apt | `linuxmirrors.cn` 200、`mirrors.tencent.com/docker-ce` 301 | 2026-11-03 实测 |
| 架构 / 发行版 / 内核                                  | x86_64 / Debian GNU/Linux 13 (trixie) / 6.12.113+deb13-amd64 | 2026-11-03 实测 |
| CPU / 内存 / 磁盘                                  | 4 vCPU / 3.9 GB（可用 3.5 GB）/ 120 GB 根分区，可用 113 GB | 2026-11-03 实测 |
| systemd 是否 PID 1                               | 是（systemd 257，257.13-1~deb13u1） | 2026-11-03 实测 |
| landlock ABI / cgroups v2                      | landlock 在 LSM 列表内（内核 6.12，ABI ≥ 1；`/sys/kernel/security/landlock` 目录未出现，以 LSM 列表为准）/ cgroups v2 | 2026-11-03 实测 |
| Docker 安装耗时                                    | apt 装包 5 s（16:22:14→16:22:19，114 MB 下载 6 s）；整段含 `docker.sh` 拉取约 2 min（起止未精确打点） | 2026-11-03 实测 |
| 回滚耗时                                           | 未测（本轮机器本身是干净的，没有触发回滚） |                        |
| 单轮墙钟（升级轮 / 首装轮）                                | 升级型（Docker 已装）：`install.sh` **0.8~0.9 s**、`accept.sh` **5.6 s**；重装轮（`rm -rf` + 装）：`install.sh` 0.9 s、`accept.sh` 6.8 s；首装轮含 Docker 约 2.5 min | 2026-11-03 实测（`/` 前值 ≤1 s / 7 s 同轮量级一致） |
| 迭代包出包（`iter.yml` musl amd64 裸包）                  | **118 s**，产物 9.4 MB（run 41616343492，main@41beba5） | 2026-11-03 实测 |
| 升级轮（换二进制，数据保留）                                | 二进制 sha `ae7153ef…`→`5ed37d21…`（8.48→8.52 MB）；`skiff.db` 表与行数不变、入口前缀不变、`config.toml` 与 `official/` 6 个文件 md5 全同、`plugins/` 仍空；验收 **PASS 34 / FAIL 0 / SKIP 3** | 2026-11-03 实测 |
| 重装轮（`rm -rf /var/lib/skiff` + 装）                  | 新库迁移 `from=0 to=1`、`initial-admin.txt` 0600、新前缀；验收 **PASS 47 / FAIL 0 / SKIP 1**（含 B4 限流 `503`+「请 299 秒后重试」） | 2026-11-03 实测 |
| `install.sh` 装完是否即起                            | **已修，修后绿**：`INSTALL_EXIT=0`、`is-active=active`、`/healthz` 4 s 内 200 | 2026-11-03 实测（§6.1） |

### 9.1 已解决：`git push` 走内网代理（原「github.com 不通」）

**现象**（2026-11-03 上机前）：本机与测试机都只有 `api.github.com` 通，`github.com:443` 两边都失败
（本机 schannel 握手失败 / openssl `unexpected eof`；测试机超时）。后果是 `git push` 推不上去，`iter.yml` 没法 dispatch，
首装轮只能「取已有 `release.yml` run 的 artifact + 在测试机上下载」（13 MB 十几秒，本机到 blob 只有十几 KB/s）。

**解法**：走内网 HTTP 代理，五个地址与实测延迟见 [§7.1](#71-内网-http-代理github-出网缺口就靠它)，首选 `10.66.10.22:1010`。
加上 `-c http.sslBackend=openssl` 后 `git push` / `git ls-remote` 一次通过，分支 `iter/first-install` 已推上去，
`iter.yml` 恢复可用。

**留下的经验**：以后本机任何「GitHub 出网」问题，先按 §7.1 探一遍那五个代理再找别的绕行，不要在 `127.0.0.1:xxxxx` 的
本地代理端口上反复试——那一路本来就不稳（端口还会轮换）。

### 9.2 首连发现的两处基线偏差（不是代码问题，但会挡住自动化）

1. **公钥没进 `authorized_keys`。** 首次连接受 `Permission denied (publickey,password)`，改用密码
   （`SSH_ASKPASS` + `SSH_ASKPASS_REQUIRE=force`）登入后才补上 §7 那把公钥。
   协议上「先加公钥再打基线」的顺序必须先做，否则回滚一次就失联；本轮靠密码登录兜底。
2. **`/etc/skiff-baseline` 不存在。** §4.1 第 4 步的基线名校验目前只能降级为「有则校验、无则告警」，
   等下一次打检查点时补上（内容建议 `name=T0 commit=<sha> ts=<iso>`）。

### 9.3 本机门禁的一处环境性红灯（不是代码缺陷）

2026-11-03 升级轮前跑 L1 门禁，`cargo test --workspace` 在本机 Windows 上
`skiff-pkgfmt` 的 `package::tests::staging_extracts_files_and_cleans_on_drop` 连续 4 次失败
（`未认领的 staging 必须被清理`）。定位：`UnpackedPackage::drop` 里 `remove_dir_all` 返回
`ERROR_DIR_NOT_EMPTY (os error 145)`——解包刚写完的文件被杀毒/索引**短暂占用**，
不是 staging 里有残留（同样的目录用 `Remove-Item -Recurse -Force` 秒删成功）。

临时把清理改成「重试 5 次 × 100 ms」后同一测试立即通过（已还原，工作区无改动），
说明是瞬时占用。同 commit 的 CI `单测（Windows）` job 全绿（run 41608812184），
故判定为**本机环境性**，不作为返工项；`fmt` / `clippy -D warnings` /
Linux target check（含 `plugins/` 第二 workspace）三项本轮全绿。

> 若以后这条在本机反复红，再考虑给 `drop` 加 Windows 重试（属鲁棒性改进，需同时补测试与文档）。


## 10. 变更记录

| 日期         | 变更                                                                                  |
| ---------- | ----------------------------------------------------------------------------------- |
| 2026-11-03 | 初稿：分层、回滚协同、验收口径                                                                     |
| 2026-11-03 | 按「内网 VM + 全量回滚 + Docker 需测」复审：基线分 T0/T1，Docker 升为待测（D0/D1/D2），回滚加强制校时，新建 `iter.yml` |
| 2026-11-03 | 按「可出外网 + 本机可达 + 可打 T0/T1」定稿：Docker 外网依赖降级为已消除；凭据改为直接写入本文；全文精简                       |
| 2026-11-03 | **首次上机（首装轮）**：§6.1 P0 复现并修复（`install.sh` 追加 `allow_insecure_http` + 启动后验 active）；§6.4 删 `StateDirectory=`（systemd 静默改掉数据根属主）；建 `accept.sh` / `iter.sh` / `linux-check.sh` / `iter.yml`；回填 §9 环境数字；记 §9.1 github.com 不通与绕行。结果 **PASS 47 / FAIL 0 / SKIP 1**（SKIP = 装后阶段不查端口占用） |
| 2026-11-03 | **GitHub 出网打通**：内网五个 HTTP 代理实测可用（§7.1），`iter/first-install` 推送成功，`iter.yml` 恢复可 dispatch；§9.1 从「环境限制」改写为「已解决」并留经验 |
| 2026-11-03 | **升级轮 + 重装轮（main@41beba5）**：`iter.yml` 首次 dispatch 实跑（118 s / 9.4 MB），包内脚本与 HEAD md5 全同；升级轮 PASS 34 / FAIL 0 / SKIP 3（数据与入口前缀保留）；重装轮 PASS 47 / FAIL 0 / SKIP 1（含 B4 限流）。回填 §9 升级/重装/出包墙钟；记 §9.3 本机 Windows 单测环境性红灯 |
