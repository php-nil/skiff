# 迭代验证回路

> 状态：**已上机**。2026-11-03 已完成首装轮、升级轮、重装轮（PASS 47 / 34 / 47，FAIL 全 0），并完成一轮「逐项覆盖」：装机/认证/fs 能力面/插件/迭代包自检，唯一代码缺陷（插件装完降权进程 Permission denied）已修；**回滚协同仍未实测**（测试机从未触发过回滚）。同日测试机改为内网明文直连（`0.0.0.0:18080` + iptables 白名单），并修掉「非 loopback 明文永远起不来」的判据缺陷。未闭环项见 [TEST_LEDGER.md](TEST_LEDGER.md)。  
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
| 凭据     | 存取方式统一见 [TEST_ENV.md](TEST_ENV.md)；本文不落明文 | 见 §7 |

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
4. 轮询 CI；**迭代取包口径 = GitHub Release 资产**：dispatch `release.yml`
   （`version=0.1.0`、`publish=true`，资产覆盖到同一既有 Release，不新建 tag），
   `gh release download` 取 `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` 并用
   `checksums.txt` 校验。`iter.yml` 的 workflow artifact 只是「本地快包」，不算交付验证。
5. 需要干净系统 → 请人回滚（§4.1）→ 校时 → **内网 HTTP 下发安装包**（构建机
   `python3 -m http.server`，测试机 `curl` 到 `/root`），不再 `scp` 单文件
6. 首装：解压后在解压目录内 `install.sh`；升级：取新 Release 包覆盖重跑 `install.sh`
   （配置与数据不动）
7. `accept.sh` 出 PASS/FAIL/SKIP 表；**日志与报告立即回传**（回滚会清掉）
8. 追加 `docs/changes/YYYY-MM-DD.md`；CI 若红标红，验收照跑

### 4.1 回滚协同协议

回滚由人手动操作，脚本只负责"请求 + 等待 + 接续"，人不需要回话。

1. 输出 `【需要回滚】请恢复到 T0/T1`，附轮次、commit、包摘要
2. 每 5 s 轮询 SSH，判据两条同时成立：可连 **且** `/proc/uptime` 首字段 < 180
3. 连续 300 s 未恢复 → 报错退出（**不静默继续**，否则在旧状态上跑出假绿）
4. 校验 `/etc/skiff-baseline` 的基线名与 commit，不符中止
5. **强制校时**：`chronyc makestep` / 重启 `systemd-timesyncd`，断言偏差 < 30 s

**顺序硬约束：先回滚，再传包。** 全量回滚会把 HTTP 下发到 `/root` 的安装包一起冲掉。第 5 步不可省——快照恢复后时钟停在快照时刻，TOTP 与 session 判定都会错乱，B 组会稳定假红且极难定位。

### 4.2 时间预算（设计目标，实测后回填 §9）

| 段         | 升级轮          | 首装轮（T1）      | 首装轮（T0，含 Docker） |
| --------- | ------------ | ------------ | ---------------- |
| 本地门禁      | 90 s         | 90 s         | 90 s             |
| CI 与迭代包并行 | ~4 min       | ~4 min       | ~4 min           |
| 下载包 + HTTP 下发 | 20 s         | 20 s         | 20 s             |
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
| A 安装与启动      | `install.sh` 退出 0；`/www/skiff` 0711、`backup` 0700、`skiff.db` 0600、`/run/skiff` 0711 且属组 `skiff-plugins`；`systemctl is-active`；`/healthz` 20 s 内 200；`initial-admin.txt` 0600；`GET /<入口>` 303 → `/login`；`GET /` 必须 404 | 首装轮       |
| D0 Docker 安装 | 无 Docker 时装上且 `docker info` 通；已装**不重装**；`--check-only` 退出 2 且不改动；装失败面板仍起（降级）                                                                                                                                               | Docker 轮  |
| D1 Docker 策略 | 三层策略在 create 之前拦截；bind 源须在 `authorized_roots` 内。**无需镜像即可断言**                                                                                                                                                               | Docker 轮  |
| D2 容器操作      | 创建/启动/日志/exec 需镜像与 registry                                                                                                                                                                                                | Docker 轮  |
| B 认证会话       | **默认不强制首登改密**：初始口令可直接登录并落到概览；显式开启（`require_initial_password_change`/环境变量/CLI）后才 `?force=1`；改密后 `initial-admin.txt` 消失、旧会话失效；错误口令连击触发限流 | 首装轮       |
| F 备份升级       | 升级后 DB 与插件 `data/` 保留；N-1 版本窗口可回滚                                                                                                                                                                                          | 升级轮       |
| E 插件体系       | 装 → 列表 → 卸载保 `data`；未签名/错公钥必须被拒                                                                                                                                                                                            | 里程碑       |
| C 传输反代       | TLS（同端口、自签已实测）、`X-Forwarded-*` 仅在 `trusted_proxies` 内采信；HSTS **未实现**（代码无下发）                                                                                                                                                                          | TLS 已测（2026-11-03）；反代正向采信待单独轮   |
| G 资源共存       | 闲置 RSS、并发登录、与 1Panel 共存                                                                                                                                                                                                    | 长时观测，不进每轮 |

**每条断言必须断言具体文案或数值，不能只看状态码。** 历史教训：`Multipart` 超限被 `map_err` 包成 400，与业务校验的 400 撞在一起，只看状态码分辨不出哪一层拒的。

`deploy/accept.sh` 当前覆盖 **preflight / A / B / D0** 四组（2026-11-03）；D1/D2/E/F/C/G 尚未实现，
调用时会明确报 SKIP 而不是悄悄跳过——**SKIP 不算通过**。

accept.sh 之外的项（fs 能力面、插件安装/卸载/降权、会话失效、限流、迭代包自检）只能手工跑，结论记入
[TEST_LEDGER.md](TEST_LEDGER.md)：**只写未闭环项，PASS 即删**。

### 5.2.1 测试期口令约定（2026-11-05 起）

- **默认不强制首登改密**：`initial-admin.txt` 里的初始口令可直接登录并进入概览，面板只显示非阻塞的「建议修改口令」提示。
- 需要复验强制流程时，在 `/etc/skiff/config.toml` 写 `require_initial_password_change = true`（或 `SKIFF_REQUIRE_INITIAL_PASSWORD_CHANGE=1` / `--require-initial-password-change`），重启服务后**仅对新建管理员生效**。
- 测试机账号、口令文件（`/root/iter/admin-pw.txt`，0600）与读取方式见 [TEST_ENV.md](TEST_ENV.md) §3。重置/轮换口令后必须同步更新该文件；验收结论以「用该文件口令登录后落到概览」为准。
- **重装/升级不得覆盖已有 admin 口令与 `must_change_password`**：只有空库才生成新管理员；已有账号一律不改哈希、不改标志。因此升级后可直接用 `admin-pw.txt` 登录，不再需要再改一次密码。
- 测试期需要临时放开强制改密时，只改当前账号标志（`UPDATE users SET must_change_password=0 WHERE username='admin'` 或反之），不得清库、不得重装、不得删用户数据。

> 旧口径（强制改密）下，一轮只能跑**一次** B 组：改密会消耗 `initial-admin.txt`，重跑 B2 必然失败。要再验一次就重装（`rm -rf /www/skiff` 后跑 `install.sh`，约 1 s）。默认口径下 B2 可重复登录；只有主动改密（B3）后才会消耗该文件。
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
2026-11-03 已按 prod 档在测试机跑通（同端口 TLS、cookie jar 登录与改密），结果见 §9 与
[`DEPLOY_TEST.md`](DEPLOY_TEST.md) §2.4.2。

### 6.3 已消除：Docker 外网依赖

`docker-bootstrap.sh` 依赖 `linuxmirrors.cn` 与 `mirrors.tencent.com`，原为最大未知；现已确认测试机可出外网，走现成链路，`exit 4` 路径仅作降级断言。

### 6.4 已修：systemd 静默改掉数据根属主（只有上机才看得见）

首轮 A 组权限断言 A6a/A6b/A6c 全红：`/www/skiff` 实得 `711 root:skiff-plugins`，而 §1.1 写的是 `root:root`。
根因是 unit 里的 `StateDirectory=skiff`：systemd 按 `User=/Group=` 把它 chown 成 `root:skiff-plugins`，**把 install.sh 的 chown 变成空操作**。

已删掉 `StateDirectory=`，数据根口径回归「单一来源 = install.sh」（`root:root 0711`）；`RuntimeDirectory=` 保留（`/run/skiff` 在 tmpfs 上，必须由 systemd 建，属组正好是 `skiff-plugins`，与 §1.1 一致）。

同时修正一处**文档口径**（不是代码缺陷）：服务进程创建的文件（`skiff.db`、`initial-admin.txt`）属组是 `skiff-plugins`——unit 的 `Group=` 决定进程主 gid。权限闸门是 `0600`（组位为 0），不是属主。`DEPLOY_TEST.md` §1.1 已按实测改正。

### 6.5 已修：官方插件装完降权进程起不来（Permission denied，2026-11-03 上机发现）

**现象**：5 个官方插件安装接口全部返回成功（`rc=0`、`enabled=true`），但实际拉起的进程立刻退出，
journal 里只有 `Permission denied (os error 13)`；此时插件目录长这样：

```
/www/skiff/plugins/com.skiff.files/versions/0.1.0       0700 root root
/www/skiff/plugins/com.skiff.files/versions/0.1.0/bin/amd64/plugin   0644
```

**根因两处**：解包目录来自 staging（0700），且 `.wbpkg` 解包不落 tar 的 mode 位。降权到 200xxx 之后，
既穿不过 `versions/0.1.0`（缺 x 位），也不能执行 `bin/…/plugin`（缺 x 位）。`data/` 已按插件 UID 落权，
唯独代码目录漏了。

**修法**：`crates/plugin/src/install.rs` 的 `set_plugin_ownership` 末尾把 `versions/` 置 0755、
其下每个版本目录置 0755、`bin` 全树置 0755（`mark_tree_executable`）。同时把目录删除（
`UnpackedPackage::drop`、`prune_versions`、`uninstall_package`）改成有界重试，避免 Windows 上
杀毒/索引的瞬时占用把「删除失败」伪装成真失败（§9.3）。

**机上验证**：手工 `chmod` 同样的两处后，进程立刻以 **uid 200161 / 200941** 正常常驻，证实诊断；
代码修复只过了本机门禁，**尚未重新出包上机复验**（见 [TEST_LEDGER.md](TEST_LEDGER.md)）。

### 6.6 已修：非 loopback 明文监听永远起不来（2026-11-03 内网直连时发现）

**现象**：配置 `listen = "0.0.0.0:18080"` + `allow_insecure_http = true` + 非空 `trusted_proxies` 后，
`Config::validate()` 通过并把 `plaintext_behind_proxy=true`，但 `serve()` 只认 `dev_http`，
直接 `bail("内部错误：未通过 TLS 校验却进入明文模式")`——`test/deploy` 文档里写的
「非 loopback + 明文 + 可信反代」这一格实际上从未能启动。

**修法**：`crates/core/src/server.rs` 抽出 `plaintext_allowed()`，`dev_http || plaintext_behind_proxy`
才放行，并分别打不同告警；补单测 `plaintext_allowed_covers_dev_and_trusted_proxy` 覆盖三条分支。

**验证**：测试机用本机源码现编（Debian glibc 动态，非发布包）后 `ss` 显示 `0.0.0.0:18080`，
`/login` 200、`/healthz` 200；**发布包（musl 静态）尚未重新出包**，见 [TEST_LEDGER.md](TEST_LEDGER.md)。

## 7. 测试机与凭据

地址、SSH 用户/密钥/公钥、面板 URL/入口前缀/TLS、管理员口令文件与读取方式、当前部署版本与资产摘要，**统一见 [TEST_ENV.md](TEST_ENV.md)**；本文不重复，也不保存明文口令。

- 公钥必须先加入 `authorized_keys`，再打基线快照；顺序反了回滚后即失联。
- 首连只做环境探测、不装任何东西：`uname -m/-r`、`/etc/os-release`、`nproc`、`free -h`、`df -h`、`id -u`、`sudo -n true`、`ss -lntp`、`curl -I linuxmirrors.cn`、`apt-get -s update`、landlock ABI、cgroups v2、`systemd --version`。结果回填 §9。
- 回滚方式：人工手动操作，需要时告知机器所有者。

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

**本机已固化**（2026-11-07）：不再依赖手打 `-c`，全局配置把代理与 sslBackend 都限定在 `github.com` 作用域，其它流量不受影响：

```sh
git config --global http.https://github.com.proxy http://10.66.10.22:1010
git config --global http.https://github.com.sslBackend openssl
```

复测：`git ls-remote origin refs/heads/main` 未加任何 `-c` 直接返回 SHA。五个代理当日通断（`curl -x ... -I https://github.com`）：
`.22` 200 / 3.1 s、`.12` 200 / 2.26 s、`.13` 200 / 1.84 s、`.9` 200 / 2.47 s、`.21` 200 但 10 s 内只收 81 KB（传输超时，仅作兜底）。

## 8. 工具落点

| 文件                           | 作用                                                                            | 状态 |
| ---------------------------- | ----------------------------------------------------------------------------- | --- |
| `deploy/accept.sh`           | §5 可执行版，PASS/FAIL/SKIP + 退出码                                                  | **已建并实跑**（preflight / A / B / D0；D1/D2/E/F/C/G 未实现，缺则报 SKIP） |
| `tools/iter.sh`              | 单轮编排：门禁 → 取包 → 回滚协同 → 装 → 验收 → 报告                                        | **已建**；gate/fetch/run 三档实跑过，`rollback-wait` 未实测；`fetch` 默认取 Release 资产，workflow artifact 模式保留给本地快包 |
| `tools/linux-check.sh`       | 本地 `cargo check --target x86_64-unknown-linux-gnu`，挡 `cfg(unix)` 编译错          | **已建并实跑**：全量（含 `plugins/` 第二 workspace）**50 s**；`--quick` 只查主 workspace |
| `.github/workflows/iter.yml` | 迭代本地快包：只编 musl x86_64、`--skip-plugins`、`cancel-in-progress: true`、artifact 留 3 天 | **已首次 dispatch 实跑**：main@41beba5 / run 41616343492，出包 **118 s**、产物 9.4 MB；**不再作为交付验证来源** |

`release.yml` 已改为「测试期手动 dispatch、资产覆盖同一既有 Release、不新建 tag」（见
[CI.md](CI.md) §4）；`iter.yml` 只负责本地快包，`cancel-in-progress: false` 仍留在
`release.yml` 上防止发出半个包。

**已定默认值**：rustc 1.98.1（与 `ci.yml` 一致）· target `x86_64-unknown-linux-musl` · 分支 `iter/<topic>` 稳定后 squash 进 `main` · 失败即停不重试 · 报告追加 `docs/changes/YYYY-MM-DD.md`。

## 9. 实测回填（未填即视为未验证）

> 本节是 2026-11-03 的历史实测；当时官方包口径为 5 个（4 个插件 + testkit），当前为 6 个（5 个官方插件 + testkit），以 [TEST_ENV.md](TEST_ENV.md) §6 为准。

| 项                                              | 值         | 日期/依据                  |
| ---------------------------------------------- | --------- | ---------------------- |
| 本机到测试机 TCP 可达                                  | 通（TCP 22 open，密钥登录 OK） | 2026-11-03 实测 |
| 出网：linuxmirrors.cn / mirrors.tencent.com / apt | `linuxmirrors.cn` 200、`mirrors.tencent.com/docker-ce` 301 | 2026-11-03 实测 |
| 架构 / 发行版 / 内核                                  | x86_64 / Debian GNU/Linux 13 (trixie) / 6.12.113+deb13-amd64 | 2026-11-03 实测 |
| CPU / 内存 / 磁盘                                  | 4 vCPU / 3.9 GB（可用 3.5 GB）/ 120 GB 根分区，可用 113 GB；当前值见 [TEST_ENV.md](TEST_ENV.md) §1 | 2026-11-03 实测 |
| systemd 是否 PID 1                               | 是（systemd 257，257.13-1~deb13u1） | 2026-11-03 实测 |
| landlock ABI / cgroups v2                      | landlock 在 LSM 列表内（内核 6.12，ABI ≥ 1；`/sys/kernel/security/landlock` 目录未出现，以 LSM 列表为准）/ cgroups v2 | 2026-11-03 实测 |
| Docker 安装耗时                                    | apt 装包 5 s（16:22:14→16:22:19，114 MB 下载 6 s）；整段含 `docker.sh` 拉取约 2 min（起止未精确打点） | 2026-11-03 实测 |
| 回滚耗时                                           | 未测（本轮机器本身是干净的，没有触发回滚） |                        |
| 单轮墙钟（升级轮 / 首装轮）                                | 升级型（Docker 已装）：`install.sh` **0.8~0.9 s**、`accept.sh` **5.6 s**；重装轮（`rm -rf` + 装）：`install.sh` 0.9 s、`accept.sh` 6.8 s；首装轮含 Docker 约 2.5 min | 2026-11-03 实测（`/` 前值 ≤1 s / 7 s 同轮量级一致） |
| 迭代包出包（`iter.yml` musl amd64 裸包）                  | **118 s**，产物 9.4 MB（run 41616343492，main@41beba5） | 2026-11-03 实测 |
| 升级轮（换二进制，数据保留）                                | 二进制 sha `ae7153ef…`→`5ed37d21…`（8.48→8.52 MB）；`skiff.db` 表与行数不变、入口前缀不变、`config.toml` 与 `official/` 6 个文件 md5 全同、`plugins/` 仍空；验收 **PASS 34 / FAIL 0 / SKIP 3** | 2026-11-03 实测 |
| 重装轮（`rm -rf /www/skiff` + 装）                  | 新库迁移 `from=0 to=1`、`initial-admin.txt` 0600、新前缀；验收 **PASS 47 / FAIL 0 / SKIP 1**（含 B4 限流 `503`+「请 299 秒后重试」） | 2026-11-03 实测 |
| `install.sh` 装完是否即起                            | **已修，修后绿**：`INSTALL_EXIT=0`、`is-active=active`、`/healthz` 4 s 内 200 | 2026-11-03 实测（§6.1） |
| 覆盖轮：装机与运行                                 | 安装 0.9 s、幂等重装 rc=0、卸载 rc=0（unit/bin 移除、数据保留）、重启后 `/healthz` 200；`/www/skiff` 0711 root:root、`backup` 0700、`skiff.db`/`initial-admin.txt` 0600；配置追加 `bogus_key` 后 `serve` 退出 1（`deny_unknown_fields` 生效） | 2026-11-03 实测 |
| 覆盖轮：认证与会话                                 | 首登未改密时全部 UI 路由 303 → `/{p}/settings?force=1`；改密 303 后旧 Bearer 令牌 401；5 次错误口令后第 6 次（**即使口令正确**）503「请 299 秒后重试」；伪造 `X-Forwarded-For: 1.2.3.4` 时审计源地址仍为 `127.0.0.1` | 2026-11-03 实测（脚本 `/root/iter/skiff_cover.py`） |
| 覆盖轮：fs 能力面（files 插件 + `path:` 授权根）      | 越界读 400、授权根内读 200、分页 `page_max=262144` 且 `eof=false`、覆盖写 `size=12`、300 KiB 写入 400 | 2026-11-03 实测 |
| 覆盖轮：插件体系                                  | 未导入公钥时官方签名包拒装（TOFU 未知密钥）；`requires_full_sandbox` 且未开 `allow_reduced_sandbox` 时 5 个官方包全部拒装；开后 5 个 rc=0 且 `enabled=true`；UI 200 且 CSP 收紧；RPC 200；卸载 cron 后 `versions/cache/run` 删除、`data/jobs.json` 保留、库行消失 | 2026-11-03 实测 |
| 覆盖轮：人工复核入口（历史明文口径）                   | 2026-11-03 时 10.10.15.4:18080 **只绑 127.0.0.1**；SSH 隧道后 `http://127.0.0.1:18080/<前缀>/login` 200、`/<错前缀>/login` 404、根路径 307。当前访问方式与 TLS 见 [TEST_ENV.md](TEST_ENV.md) §2 | 2026-11-03 实测 |
| 内网明文直连（历史口径）                            | 2026-11-03 时 `listen=0.0.0.0:18080` + `allow_insecure_http=true` + `trusted_proxies=["127.0.0.1/32"]`；`ss` 为 `0.0.0.0:18080`；开发机直连 `/login` 200、错前缀 404、根 307、`/healthz` 200；防火墙 iptables 白名单 + systemd oneshot，重启后规则与监听仍在；重装 `--listen` 不丢、不给参数不改写。当前已改为同端口 TLS，访问信息见 [TEST_ENV.md](TEST_ENV.md) §2 | 2026-11-03 实测（发现并修复 §6.6 启动判据缺陷） |
| 同端口 TLS（自签，人工复核入口）                      | 证书路径/权限/SAN/有效期见 [TEST_ENV.md](TEST_ENV.md) §2（当轮 SHA-256 `D7:49…:67:78`）；`config.toml` 加 `tls_cert`/`tls_key`、去 `allow_insecure_http`；`ss` 仍 `0.0.0.0:18080`，日志 `HTTPS 监听中`；`https` `/login` 200、`/healthz` 200、错前缀 404；`http` 同端口 curl `000`（无页面）；cookie jar：登录 POST 303 → `/settings?force=1`，`skiff_login`/`__Host-skiff_session` 均 `Secure`，改密 303 → 首页 200；重启后证书/监听/登录页保持；重装前后 `config.toml` sha256 相同 | 2026-11-03 实测（HSTS 未实现，见 DEPLOY_TEST §2.4.2） |

### 9.1 已解决：`git push` 走内网代理（原「github.com 不通」）

**现象**（2026-11-03 上机前）：本机与测试机都只有 `api.github.com` 通，`github.com:443` 两边都失败
（本机 schannel 握手失败 / openssl `unexpected eof`；测试机超时）。后果是 `git push` 推不上去，`iter.yml` 没法 dispatch，
首装轮只能「取已有 `release.yml` run 的 artifact + 在测试机上下载」（13 MB 十几秒，本机到 blob 只有十几 KB/s）。

**解法**：走内网 HTTP 代理，五个地址与实测延迟见 [§7.1](#71-内网-http-代理github-出网缺口就靠它)，首选 `10.66.10.22:1010`。
加上 `-c http.sslBackend=openssl` 后 `git push` / `git ls-remote` 一次通过，分支 `iter/first-install` 已推上去，
`iter.yml` 恢复可用。

**留下的经验**：本机任何「GitHub 出网」问题，先按 §7.1 探一遍那五个内网代理，并先确认 `git config --global` 里
`github.com` 作用域的代理是否已生效；不要在 `127.0.0.1:xxxxx` 的本地代理端口上反复试——那一路本来就不稳（端口还会轮换），
也不要在系统代理开关上打转：**内网代理不在本机监听，loopback 端口和系统代理设置都查不到它**。

2026-11-07 的复查就踩了这个坑：直连 `github.com` 被间歇重置后，只在 loopback 端口与系统代理开关上排查，退避重试五轮才推上去，
全程没翻 §7.1。结论是「github.com 单 IP/单路径抖动」而不是「整体出网不可用」，与仓库改动无关；正确的第一步应该是先查内网代理。

### 9.2 首连发现的两处基线偏差（不是代码问题，但会挡住自动化）

1. **公钥没进 `authorized_keys`。** 首次连接受 `Permission denied (publickey,password)`，改用密码
   （`SSH_ASKPASS` + `SSH_ASKPASS_REQUIRE=force`）登入后才补上 §7 那把公钥。
   协议上「先加公钥再打基线」的顺序必须先做，否则回滚一次就失联；本轮靠密码登录兜底。
2. **`/etc/skiff-baseline` 不存在。** §4.1 第 4 步的基线名校验目前只能降级为「有则校验、无则告警」，
   等下一次打检查点时补上（内容建议 `name=T0 commit=<sha> ts=<iso>`）。

### 9.3 已修：Windows 上目录删除的瞬时占用（原「环境性红灯」）

2026-11-03 升级轮前跑 L1 门禁，`cargo test --workspace` 在本机 Windows 上
`skiff-pkgfmt` 的 `package::tests::staging_extracts_files_and_cleans_on_drop` 连续 4 次失败
（`未认领的 staging 必须被清理`）。定位：`UnpackedPackage::drop` 里 `remove_dir_all` 返回
`ERROR_DIR_NOT_EMPTY (os error 145)`——解包刚写完的文件被杀毒/索引**短暂占用**，
不是 staging 里有残留（同样的目录用 `Remove-Item -Recurse -Force` 秒删成功）。

当轮只做了临时重试并还原，判定为本机环境性。**覆盖轮把它转成正式修复**：

- `crates/pkgfmt/src/package.rs`：`UnpackedPackage::drop` 改为「5 次 × 100 ms」有界重试；
- `crates/plugin/src/install.rs`：新增 `remove_tree`（50 ms 起、指数退避、5 次），用于
  `prune_versions` / 版本目录替换 / `uninstall_package`——同一故障点会让两个迁移测试偶发红。

修后 `skiff-pkgfmt` 与 `skiff-plugin` 相关用例连跑 **3 次**均绿，Linux 上首次即成功、无额外开销。
这属鲁棒性改进（不是环境豁免）：删除失败不再被当成「真失败」，也不会在 CI 上偶发。


## 10. 变更记录

| 日期         | 变更                                                                                  |
| ---------- | ----------------------------------------------------------------------------------- |
| 2026-11-03 | 初稿：分层、回滚协同、验收口径                                                                     |
| 2026-11-03 | 按「内网 VM + 全量回滚 + Docker 需测」复审：基线分 T0/T1，Docker 升为待测（D0/D1/D2），回滚加强制校时，新建 `iter.yml` |
| 2026-11-03 | 按「可出外网 + 本机可达 + 可打 T0/T1」定稿：Docker 外网依赖降级为已消除；凭据改为直接写入本文；全文精简                       |
| 2026-11-03 | **首次上机（首装轮）**：§6.1 P0 复现并修复（`install.sh` 追加 `allow_insecure_http` + 启动后验 active）；§6.4 删 `StateDirectory=`（systemd 静默改掉数据根属主）；建 `accept.sh` / `iter.sh` / `linux-check.sh` / `iter.yml`；回填 §9 环境数字；记 §9.1 github.com 不通与绕行。结果 **PASS 47 / FAIL 0 / SKIP 1**（SKIP = 装后阶段不查端口占用） |
| 2026-11-03 | **GitHub 出网打通**：内网五个 HTTP 代理实测可用（§7.1），`iter/first-install` 推送成功，`iter.yml` 恢复可 dispatch；§9.1 从「环境限制」改写为「已解决」并留经验 |
| 2026-11-03 | **升级轮 + 重装轮（main@41beba5）**：`iter.yml` 首次 dispatch 实跑（118 s / 9.4 MB），包内脚本与 HEAD md5 全同；升级轮 PASS 34 / FAIL 0 / SKIP 3（数据与入口前缀保留）；重装轮 PASS 47 / FAIL 0 / SKIP 1（含 B4 限流）。回填 §9 升级/重装/出包墙钟；记 §9.3 本机 Windows 单测环境性红灯 |
| 2026-11-03 | **覆盖轮**：按「装机/认证/Docker/fs/插件/包自检」逐项跑；§6.5 记录并修复插件装完降权进程 Permission denied；§9.3 从环境性豁免转为正式修复（`drop` 与 `remove_tree` 有界重试）；新增 [TEST_LEDGER.md](TEST_LEDGER.md) 承接未闭环项；回填 §9 五行覆盖结论 |
| 2026-11-03 | **同端口 TLS**：自签证书 + `tls_cert`/`tls_key` 启用，18080 仅 HTTPS；cookie jar 登录/改密跑通（`Secure` 正常回传、CSRF 不 403）；重启与重装后证书/监听/配置保持；§6.2 prod 档实测、§5.2 C 组口径与 §9 回填。HSTS 代码未实现，明确不计入已测 |
| 2026-11-03 | **内网直连改造**：`install.sh` 新增 `--listen` / `SKIFF_LISTEN`（默认保持 loopback、不指定则不改已有配置）；§6.6 记录并修复非 loopback 明文启动判据缺陷；测试机落地 iptables 白名单 + `skiff-firewall.service` 持久化，重启与重装后均实测通过；§9 回填直连结论 |
