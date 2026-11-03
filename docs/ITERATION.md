# 迭代验证回路

> 状态：**方案定稿，待上机**（2026-11-03，同日三次修订后收敛）。  
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

## 6. 已知问题

### 6.1 P0：按发布包装机，服务起不来

`install.sh` 把 `config.toml.example` 原样拷成 `/etc/skiff/config.toml`，其中 `allow_insecure_http` 是注释态（默认 `false`），systemd 的 `ExecStart` 不带 `--dev`，`listen` 默认 loopback → `Config::validate` 直接 bail：

```
错误: 未配置 TLS：loopback 调试请显式加 --dev 或 allow_insecure_http = true（§6.2）。
```

2026-11-03 用等价配置实跑 `skiff serve --config` 复现，进程启动即退出。之所以至今未暴露，是因为装机链路**从未在真机执行过**（`DEPLOY_TEST.md` §9.5 自述"仍未验证"）。

候选修法：① `install.sh` 在"无 TLS + loopback"时写入 `allow_insecure_http = true` 并告警；② 示例配置默认打开该行；③ 装完生成自签证书。**上机复现后再定，不基于推断改码。**

### 6.2 约束：明文 HTTP 下测不了登录

`dev_http == false` 时 cookie 带 `Secure`，明文 HTTP 下 curl 与浏览器都不回传。故 `accept.sh` 需两档：dev 档走 `--dev`（cookie 降级为 `skiff_dev_session`，可用 `curl -c/-b`），prod 档配自签 TLS + `curl -k` 验真实 `Secure`/`SameSite` 属性。

### 6.3 已消除：Docker 外网依赖

`docker-bootstrap.sh` 依赖 `linuxmirrors.cn` 与 `mirrors.tencent.com`，原为最大未知；现已确认测试机可出外网，走现成链路，`exit 4` 路径仅作降级断言。

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

## 8. 工具落点（待建）

| 文件                           | 作用                                                                            |
| ---------------------------- | ----------------------------------------------------------------------------- |
| `deploy/accept.sh`           | §5 可执行版，PASS/FAIL/SKIP + 退出码                                                  |
| `tools/iter.sh`              | 单轮编排：门禁 → 同步 → 取包 → 回滚协同 → 装 → 验收 → 报告                                        |
| `tools/linux-check.sh`       | 本地 `cargo check --target x86_64-unknown-linux-gnu`，30 s 挡 `cfg(unix)` 错       |
| `.github/workflows/iter.yml` | 迭代包：只编 musl x86_64、`--skip-plugins`、`cancel-in-progress: true`、artifact 留 3 天 |

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
| Docker 安装耗时                                    | 待测        |                        |
| 回滚耗时                                           | 待测        |                        |
| 单轮墙钟（升级轮 / 首装轮）                                | 待测        |                        |
| `install.sh` 装完是否即起                            | 待测（本轮上机执行中，§6.1 预期红） | 2026-11-03 代码审读 + 本地实跑 |

### 9.1 首连发现的两处基线偏差（不是代码问题，但会挡住自动化）

1. **公钥没进 `authorized_keys`。** 首次连接受 `Permission denied (publickey,password)`，改用密码
   （`SSH_ASKPASS` + `SSH_ASKPASS_REQUIRE=force`）登入后才补上 §7 那把公钥。
   协议上「先加公钥再打基线」的顺序必须先做，否则回滚一次就失联；本轮靠密码登录兜底。
2. **`/etc/skiff-baseline` 不存在。** §4.1 第 4 步的基线名校验目前只能降级为「有则校验、无则告警」，
   等下一次打检查点时补上（内容建议 `name=T0 commit=<sha> ts=<iso>`）。


## 10. 变更记录

| 日期         | 变更                                                                                  |
| ---------- | ----------------------------------------------------------------------------------- |
| 2026-11-03 | 初稿：分层、回滚协同、验收口径                                                                     |
| 2026-11-03 | 按「内网 VM + 全量回滚 + Docker 需测」复审：基线分 T0/T1，Docker 升为待测（D0/D1/D2），回滚加强制校时，新建 `iter.yml` |
| 2026-11-03 | 按「可出外网 + 本机可达 + 可打 T0/T1」定稿：Docker 外网依赖降级为已消除；凭据改为直接写入本文；全文精简                       |
