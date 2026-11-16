# 验收测试用例（§6）

> 上级索引：[DEPLOY_TEST.md](../DEPLOY_TEST.md) ｜ 拆自 `docs/test/DEPLOY_TEST.md` 原 §6（2026-11-16 整理，正文未改）。

---

## 6. 验收测试用例

记录方式：每条填「实测」「结论」两列；失败即阻断该里程碑，不要“先放过去”。角色列 `v/o/a` 依次为 viewer / operator / admin。

> **2026-11-03 首装轮已把 A 组（除依赖特定前置的几条）、B 组 12 条、D0 四条跑成脚本**：`deploy/accept.sh`
> （`--group preflight,a,b,d0`）。脚本里的编号（A3/A5/A6a…/B2a/B3e/D0c…）与下表逐条对应，
> 结果 **PASS 47 / FAIL 0 / SKIP 1**（SKIP 是「装后阶段不查端口占用」，按设计跳过）。
> 首轮还抓出两个真缺陷，见 [ITERATION/02-ISSUES_ENV.md](../ITERATION/02-ISSUES_ENV.md) §6.1（install.sh 装完服务起不来）与 §6.4（systemd 改掉数据根属主）。

### 6.1 A 组：安装与启动

| 编号 | 前置 | 步骤 | 预期 | 实测 | 结论 |
|---|---|---|---|---|---|
| A1 | 干净机器 | 改一个字节后跑 `sha256sum -c` | 校验失败并退出 | | |
| A2 | 发布物 | 只给 tar.gz、不给 `.sha256` | 不得安装（缺摘要 = 没有发布物，§0.3） | | |
| A3 | root | `sh install.sh` | unit 已 enable 且 active | | |
| A4 | 已装 | 再跑一次 `install.sh` | 幂等，无报错，数据不丢 | | |
| A5 | 已装 | `systemctl stop` 后 `start` | 自动恢复，前缀不变 | | |
| A6 | 无 Docker 机器 | `sh install.sh --no-docker` 后启动面板 | 正常启动；容器页显示“Docker 不可用”，其余可用 | | |
| A7 | 已装 | `skiff health` | 退出码 0 | | |
| A8 | 已装 | `skiff doctor` | 各检查项输出，无不期望的 ERROR | | |
| A9 | 已装 | `kill -9` 主进程 | systemd `Restart=always` 拉起（`RestartSec=3`） | | |
| A10 | 已装 | 按 §2.10 逐条查权限 | 全部符合 §1.1 表 | | |
| A11 | 已装 | `./target/debug/skiff-plugin --help`、`skiff --help` | 只打帮助，不在当前目录落文件 | | |
| A12 | 无 Docker、有外网 | `sh install.sh` | 自动装 Docker CE（默认源 mirrors.tencent.com）；`docker info` 成功；容器页**不是**空态 | | |
| A13 | 已有 docker（含发行版 docker.io）且跑着容器 | `sh install.sh` | 不重装、不停服；输出「Docker 已就绪」；容器不中断 | | |
| A14 | 无 Docker、断网 | `sh install.sh` | Docker 引导 warn 后继续；面板装好并启动；`skiff doctor` 报 socket 缺失，其余检查正常 | | |
| A15 | 已有 docker + 其它 daemon.json 字段（如 data-root） | `sh install.sh --registry-mirror <h>` | **不**改写 daemon.json，打印手工合并方式；加 `--force-registry` 才重写 | | |
| A16 | 无外网 | `sh install.sh --no-docker` | 全程不下载任何东西，不调用 docker-bootstrap | | |
| A17 | 面板已在跑、无 Docker | `sh docker-bootstrap.sh` | 装完提示「需 restart skiff」；`systemctl restart skiff` 后容器页恢复 | | |
| A18 | Alpine | `sh install.sh` | 走 `apk add docker`，不调用 docker.sh；`rc-update add docker default` 已执行 | | |
| A19 | 无 tty（`ssh -T`／CI） | `sh install.sh` | 全程不出现交互等待；确认 docker.sh 以 `--pure-mode --ignore-backup-tips` 执行 | | |
| A20 | 任意 | `sh docker-bootstrap.sh --check-only` | 只报告不落盘：就绪 exit 0，未就绪 exit 2 | | |
| A21 | 无 Docker、公司内网需代理 | `sh install.sh --docker-proxy <url>` | 写 `docker.service.d/10-skiff-http-proxy.conf` 并重启 daemon；`docker pull` 能出网 | | |
| A22 | 已有 docker + `--docker-proxy` | `sh install.sh --docker-proxy <url>` | **不写** drop-in、不重启 daemon，只打印配置片段 | | |

### 6.2 B 组：认证与会话

| 编号 | 前置 | 步骤 | 预期 | 实测 | 结论 |
|---|---|---|---|---|---|
| B1 | 首启 | 打开非入口前缀的任意路径 | 404（不是 403） | | |
| B2 | 首启 | 用 `initial-admin.txt` 口令登录 | 成功，**默认直接进入概览**（不跳 `settings?force=1`）；显式开启后才强制改密 | | |
| B3 | B2 | 改密后 | `initial-admin.txt` 已删除；旧会话全部失效 | | |
| B4 | 任意 | 连续 6 次错误口令 | 第 6 次被拒并提示剩余封禁秒数（5 次/5min，封 5min） | | |
| B5 | 已登录 | 表单缺 `csrf` 或改值后提交 | 拒绝（CSRF 校验） | | |
| B6 | v 账号 | 调 `POST /containers/{id}/start` | 403 + 审计有记录 | | |
| B7 | o 账号 | 删除容器 | 403（销毁类仅 admin） | | |
| B8 | a 账号 | 开启 TOTP，重新登录 | 要求 TOTP；备份码可救急一次 | | |
| B9 | 已登录 | `skiff token create ci` 后用 Bearer 调 API | 200；token 只在创建时打印一次 | | |
| B10 | a 账号 | 改密后旧 API Token | 会话失效，API Token 需重新签发（scope 不变） | | |
| B11 | 已登录 | 空闲 30min 后再操作 | 会话过期需重登（idle 30min / abs 12h） | | |
| B12 | 已登录 | 连续 3 次登录失败后成功登录 | 失败计数清零 | | |

### 6.3 C 组：传输与反代

| 编号 | 前置 | 步骤 | 预期 | 实测 | 结论 |
|---|---|---|---|---|---|
| C1 | `listen=0.0.0.0` 无 TLS | 启动 | 拒绝启动并给出可操作提示 | | |
| C2 | C1 + `allow_insecure_http=true`，`trusted_proxies` 为空 | 启动 | 仍拒绝（裸奔防护） | | |
| C3 | C2 + `trusted_proxies=172.16.0.0/12` | 启动 | 成功，日志告警“非 loopback 明文” | | |
| C4 | 反代白名单外 IP | 伪造 `X-Forwarded-For: 1.2.3.4` 登录失败 | 审计里记录的是真实 peer IP，不是 1.2.3.4 | | |
| C5 | 已登录 | 从异源页面发起 WS 连接 | 拒绝（Origin 校验） | | |
| C6 | 已登录 | 打开容器终端后强制断开网络 | 后端 exec 流被回收，不残留 | | |
| C7 | 配了 `tls_cert/tls_key` | `curl -I https://<host>:18080/healthz` | 200；HTTP 访问同端口被拒 | `https` 登录页/`/healthz` **200**、错前缀 **404**；`http` 同端口 curl `000`（`Received HTTP/0.9`，无页面）；`ss` 仍 `0.0.0.0:18080`（2026-11-03 实测） | 通过 |

### 6.4 D 组：Docker 管理（三层策略）

| 编号 | 前置 | 步骤 | 预期 | 实测 | 结论 |
|---|---|---|---|---|---|
| D1 | 有 Docker | 容器列表/详情/日志 | 正常；日志首屏 N 行，SSE 跟随不卡死 | | |
| D2 | v 账号 | 启动/停止/重启容器 | 403 + 审计 | | |
| D3 | o 账号 | 启动/停止/重启 | 成功 | | |
| D4 | a 账号 | 删除 Skiff 自身容器（`name=skiff` 或带 `io.skiff.protected=true`） | 拒绝 | | |
| D5 | a 账号 | 对 1Panel 托管容器（带 `createdBy: Apps`）执行删除 | 默认不出现/拒绝，除非显式选择 | | |
| D6 | native 形态 | 创建容器时挂 `/etc` 为 bind | 拒绝（授权根外） | | |
| D7 | native 形态 | 授权根 `/srv` 内路径挂载 | 允许 | | |
| D8 | native 形态 | 授权根内路径，校验后把中间目录换成 symlink 再提交 | **当前会通过**：校验用 `canonicalize`，检查与挂载之间仍有窗口，`open_dir_nofollow` 未接入（§8）。本用例只用于记录窗口存在，**不作为阻断项** | | |
| D9 | 任意 | 提交带 `privileged` / `cap_add` 的创建请求 | 拒绝（字段不在白名单，`deny_unknown_fields`） | | |
| D10 | 任意 | 提交含 `build:` 的 Compose | 明确报错“不支持”，不是静默忽略 | | |
| D11 | 受限形态 | 任意 bind 挂载请求 | 403 | | |
| D12 | 受限形态 | 引用 `driver_opts` 含 `o=bind` 的既有卷 | 按 bind 语义处理并拒绝 | | |
| D13 | 有容器 | 打开 exec 终端，输入 `ls` | 有输出；关闭页面后 Engine 侧 exec 结束 | | |

### 6.5 E 组：插件体系

| 编号 | 前置 | 步骤 | 预期 | 实测 | 结论 |
|---|---|---|---|---|---|
| E1 | §2.9 已造包并导入公钥 | 安装已签名 `.wbpkg` | 权限逐条确认页 → 安装成功 | | |
| E2 | 同上 | 安装未签名包（`allow_unsigned=false`） | 拒装，提示原因 | | |
| E3 | 同上 | 篡改包内某文件后安装 | 拒装（哈希清单不匹配） | | |
| E4 | 同上 | 包内含 `../` 或绝对路径条目 | 拒装 | | |
| E5 | 同上 | 包内重复路径 | 拒装 | | |
| E6 | 同上 | 构造高压缩比解压炸弹 | 拒装（解压总量/条目数上限） | | |
| E7 | 已装插件 | 运行中 `kill -9` 插件进程 | 面板 100% 可用；按 0.5s/2s/8s 退避重启，3 次失败熔断 | | |
| E8 | 已装插件 | 插件尝试读 `/www/skiff/skiff.db` | 权限拒绝（独立 UID，库 0600 root） | | |
| E9 | 已装插件 | 插件绕过 Broker 直连 `docker.sock` | 拒绝（不在插件可访问路径/无权限） | | |
| E10 | 已装插件 | 插件访问另一个插件的 `data/` | 拒绝（独立 UID + 0700） | | |
| E11 | 已装插件 | 卸载（不勾删数据） | 代码删除、`data/` 保留；重装后数据还在 | | |
| E12 | 受限形态 | 安装声明需要 fs/net 的插件 | 装前拒装并说明原因 | | |
| E13 | 插件 UI | 从异源 iframe 伪造 `postMessage` | 拒绝（校验 source + nonce） | | |
| E14 | 已装插件 | `skiff plugin config <id> api_base https://example.com`，再让插件读配置 | 插件侧 `Plugin::config()` 拿到该项；`skiff audit` 出现 `plugin.config.set`（detail 只有键名） | | |
| E15 | 同上 | `skiff plugin config <id> 不存在的键 x` | 拒绝并提示「未声明配置项」，不写库 | | |
| E16 | 同上 | 写入 `number` 类型的项但值是 `abc` | 拒绝（取值类型校验） | | |
| E17 | 同上 | 插件读配置（内核 `capability/config`） | 审计出现 `capability.config`，detail 只有条数、**不含取值** | | |
| E18 | 已装插件运行中 | 按 §2.10 看 `Uid/Gid/Groups` | Uid=Gid=200xxx；Groups 含 skiff-plugins；主进程仍是 root | | |
| E19 | 已装插件 | 插件用 `SKIFF_PLUGIN_DATA_DIR` 自己读写文件；再用 `fs.write` 能力写一个 | 两种方式都能读写；内核写的文件属主是 200xxx（不是 root） | | |
| E20 | 打包目录含 `target/` 与私钥 | `skiff-plugin pack` 后 `tar tzf` 看清单 | 不含 `target/`、不含 `skiff-signing.key`；体积与 `bin/` 相当 | | |
| E21 | 已装两个插件 | 后装的 id 哈希撞上前一个的 UID | 装前被拒并说明冲突（1000 个槽位） | | |

### 6.6 F 组：备份、升级、回滚

| 编号 | 前置 | 步骤 | 预期 | 实测 | 结论 |
|---|---|---|---|---|---|
| F1 | 有数据 | `skiff backup` | 生成 `skiff-<ts>.db` + 插件数据，保留 3 份，记录版本与校验和。**不停止插件**：库走 `VACUUM INTO`（在线一致），插件 `data/` 是直接复制，写入中的文件可能不一致 | | |
| F2 | F1 | 检查备份目录权限 | 0700 | | |
| F3 | F1 | 对活动库直接 `tar` 后恢复 | **应当失败或数据不一致**——验证“禁止 tar 活动库”这条约束的必要性 | | |
| F4 | F1 | 新环境恢复备份并登录 | 用户/插件/配置齐全（M5 项，未做） | | |
| F5 | 已装 | 插件迁移失败时升级 | 保持停用可回退，或按声明禁止自升级（M4 项） | | |
| F6 | 已装 | 降级二进制（库 schema 更新） | 拒绝启动并提示 `SchemaTooNew` | | |

### 6.7 G 组：资源与共存

| 编号 | 前置 | 步骤 | 预期 | 实测 | 结论 |
|---|---|---|---|---|---|
| G1 | 空载 5min | 看 RSS（`ps -o rss= -p <pid>` 除以 1024 得 MB） | 设计目标 ≤25MB（待实测回填） | | |
| G2 | 空载 | 看二进制体积（`ls -l /usr/local/bin/skiff`） | 设计目标 ≤14MB（Windows PE 实测 6.65MB，Linux 待测） | | |
| G3 | 与 1Panel 同机 | 面板建容器、1Panel 看应用列表 | 互不污染；自建容器不打 `createdBy: Apps` | | |
| G4 | 与 1Panel 同机 | 两边同时启停各自容器 | 无冲突，不互相接管 | | |
| G5 | 有 Docker | 100 个容器下列表页 | 分页/懒渲染不卡死，首屏可交互 | | |
