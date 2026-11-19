# 结论、测试机、基线、轮次、单轮步骤与验收口径（§0–5）

> 上级索引：[ITERATION.md](../ITERATION.md) ｜ 拆自 `docs/test/ITERATION.md` 原 §0–5（2026-11-16 整理，正文未改）。

---

> 状态：**已上机**。首装/升级/重装/覆盖轮均已跑通（逐轮记录见 [TEST_ENV/03-DEPLOY_LOG.md](../TEST_ENV/03-DEPLOY_LOG.md) 与 `docs/changes/`）；**回滚协同仍未实测**（需具备快照回滚的环境）。未闭环项见 [TEST_LEDGER.md](../TEST_LEDGER.md)。  
> 本文是「改代码 → 上机验证」的唯一依据。交付门槛见 [AGENTS.md](../../../AGENTS.md) §4.2 / [CI.md](../../ops/CI.md) §2，工作流见 [CI.md](../../ops/CI.md)，  
> 验收用例原文见 [DEPLOY\_TEST.md](../DEPLOY_TEST.md)（本文 §5 是其可执行版）。

## 0. 结论

**编译不进测试机；测试机只回答一个问题：干净系统上装完能不能用。** 验证分层（L1 门禁 / L2 单测 / L2′ 验收 / L3 发布物）与每轮耗时见 [test.md](../../test.md)「验证分层」。

**⚠️ 测试机禁止 Rust 编译**：编译链路统一走 GitHub Release musl 静态资产（`release.yml`，`publish=true`）。测试机只装二进制 + systemd + 验收。任何向测试机装 `rustup`/`cargo`/musl 工具链的操作都是违规——已在 AGENTS.md §4.3 列为硬禁令。历史上的 "测试机自编译" 调试路径已废弃。

不把编译放测试机，原因三条：开发机 `target` 已 24 GB（Linux 4~8 GB），每轮回滚都要搬；编译会污染"干净系统"这个前提；改到底部 crate 时增量退化成全量，收益不稳。不加第二台机器，因为 `ci.yml` 的 `test` job 本来就是 L2——public 仓库不计费，占地为零。

## 1. 测试机（已确认）

| 属性     | 值                        | 影响                   |
| ------ | ------------------------ | -------------------- |
| 网络     | 内网 VM，**可出外网**；本机**可直达** | Docker 走现成在线链路，无需离线源 |
| 数据     | 可随意删除，允许暴露               | 重置可粗暴，验收可直接明文 HTTP   |
| 回滚     | **全量回滚到初始状态**，人工触发       | 每轮可得干净系统；会清时钟与缓存（§4） |
| Docker | **初始无，需脚本安装，在测试范围内**     | 安装链路本身是待测对象 → 基线分两档  |
| 凭据     | 存取方式统一见 [TEST_ENV.md](../TEST_ENV.md)；本文不落明文 | 见 §7 |

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

### 4.2 时间预算（设计目标，实测后回填 TEST_ENV）

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
[TEST_LEDGER.md](../TEST_LEDGER.md)：**只写未闭环项，PASS 即删**。

### 5.2.1 测试期口令约定（2026-11-05 起）

- **默认不强制首登改密**：`initial-admin.txt` 里的初始口令可直接登录并进入概览，面板只显示非阻塞的「建议修改口令」提示。
- 需要复验强制流程时，在 `/etc/skiff/config.toml` 写 `require_initial_password_change = true`（或 `SKIFF_REQUIRE_INITIAL_PASSWORD_CHANGE=1` / `--require-initial-password-change`），重启服务后**仅对新建管理员生效**。
- 测试机账号、口令文件（`/root/iter/admin-pw.txt`，0600）与读取方式见 [TEST_ENV/01-ACCESS.md](../TEST_ENV/01-ACCESS.md) §3。重置/轮换口令后必须同步更新该文件；验收结论以「用该文件口令登录后落到概览」为准。
- **重装/升级不得覆盖已有 admin 口令与 `must_change_password`**：只有空库才生成新管理员；已有账号一律不改哈希、不改标志。因此升级后可直接用 `admin-pw.txt` 登录，不再需要再改一次密码。
- 测试期需要临时放开强制改密时，只改当前账号标志（`UPDATE users SET must_change_password=0 WHERE username='admin'` 或反之），不得清库、不得重装、不得删用户数据。

> 旧口径（强制改密）下，一轮只能跑**一次** B 组：改密会消耗 `initial-admin.txt`，重跑 B2 必然失败。要再验一次就重装（`rm -rf /www/skiff` 后跑 `install.sh`，约 1 s）。默认口径下 B2 可重复登录；只有主动改密（B3）后才会消耗该文件。
> `B4` 登录限流会把 admin 锁 5 分钟，只有首装轮末尾建议开 `--with-rate-limit`。
