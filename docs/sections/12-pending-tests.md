# 12. 待实测清单（未实测前不得当事实引用）

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§12 ｜ 上一章：[§11 已拍板的决策](11-decisions.md) ｜ 下一章：无（全文结束）

- 1Panel 渲染 compose 对空变量 / `${VAR:-default}` 的处理（仅可选 Docker 形态）。
- 商店对镜像源 / digest 的校验；本地商店上传 zip 的内部层级要求（仅可选 Docker 形态）。
- 全部内存/CPU/体积目标值（含镜像 ≈23MB 估算）。
- 插件 API N-1 兼容承诺、跨实现签名字节与两遍安全解包链路实测；恶意 tar/重复路径/链接/解压炸弹用例（M4）。
- Docker 对象 grant、Skiff 自保护、HostConfig 未列字段拒绝，以及 named volume `o=bind`/`type=none` 变形拦截（M2/M3）。
- 可选 Docker 形态所有宿主 bind 与 bind 语义 volume 均拒绝，named volume 正常路径可用（M3/M5）。
- RLIMIT_AS 256MB 下 Go/JVM/.NET 插件的实际行为（M4，决定运行时支持矩阵）。
- 插件降权后的权限矩阵：kernel.sock 可达且 peer UID/token 绑定，docker.sock / skiff.db / 其他插件进程与数据不可达（M4）。
- landlock + cgroups v2（内存/PID/CPU）沙箱在主流发行版的实际行为；内核 <5.13 默认拒绝及 `allow_reduced_sandbox` 显式降级路径（M4）。
- project quota 在目标文件系统上的可用性；不支持时磁盘仅软限额的 UI 与验收口径（M4）。
- 插件迁移超时/失败后的进程树清理、数据快照恢复、旧版 schema 不兼容时保持停用（M4）。
- postMessage 的 source/nonce 重放与跨插件伪造用例；WS Origin、短期票据、反代头伪造与远程 HTTP 拒绝用例（M1/M4）。
- nftables owner-match 默认拒绝出网与 iptables-nft/ufw/firewalld 共存环境的行为（M4）。
- net 能力（端口规则）在主流发行版的适配实测；失败降级只读路径（M4）。
- 与 1Panel / 宝塔同机共存实测（M3/M5）。
- 备份在新环境真实恢复并核对核心数据库与插件数据，而非只检查备份文件存在（M5）。
- 发布 tar.gz / Docker 包的 detached `SHA-256` 摘要与**解包前摘要不符即退出**的安装链路（M5）。发布物**不做签名**（§11 决策 12，2026-11-01），原「独立签名 / 验签失败即退出」项随链路移除一并作废。
- Docker 引导脚本（`docker-bootstrap.sh`）在真实 Linux 上仅做过 Windows 分支级验证（mock daemon），以下均未实测：全新安装走 apt/dnf + LinuxMirrors `docker.sh`、Alpine 走 `apk`、无 tty 环境下不卡交互；以及「已有 docker 不重装」「已有 docker 且 daemon 挂时拒绝安装（exit 3）」两条行为（M5，交叉 §5.1）。

## 12.1 架构层缺口（本轮审查补充，未实测前同样不得当事实引用）

这 5 项不是既有方案的实测点，而是**方案里尚未闭合的架构条件**，不定下来会影响 M1/M4 的编码与验收口径。

- **最小内核 + 发行版支持矩阵未定义**：landlock 需内核 ≥5.13、`openat2` 需 ≥5.6、cgroups v2 需可 delegation、project quota 需 ext4/xfs。四项组合才是真实门槛，目前分散在 §4.2 与 §5.5，需给出一张「不满足时产品可用到什么程度」的降级表（M1）。
- **运行时授权的语义未定**：landlock 规则在进程启动时设定、运行期不可追加，因此 admin 新增授权目录**必然要求重启该插件进程**。需确认 UI 与文档把它写成设计行为（授权→提示重启→生效），否则会被当成 bug（M1）。
- **`RLIMIT_AS` 与 cgroup `memory.max` 双轨冲突**：一个限虚拟地址空间、一个限物理内存，同时设置会互相误伤（托管运行时尤其明显）。建议 cgroups 管实际用量、`RLIMIT_AS` 只对声明 C/Rust 的插件设，待 M4 支持矩阵确认（M4，交叉 §10 风险 14）。
- **cgroup 进程树清理方式**：`cgroup.kill`（需 ≥5.14）比遍历 `/proc` 杀子孙更可靠，但同属内核门槛，须并入上面的支持矩阵一并判定（M4，交叉 §4.2）。
- **插件 UI 在 opaque origin 下的资源行为**：sandbox 去掉 `allow-same-origin` 后，`/plugins/{id}/ui/*` 仍能加载，但 iframe 内以相对路径 `fetch`/读 JSON 会被 opaque origin 拒绝。插件 UI 若自带数据文件会静默失败，需把「UI 只能经 postMessage 取数」写成 API 契约的一部分（M4）。**2026-11-01 状态**：详情页 iframe 已实际带 `sandbox="allow-scripts allow-modals"`，`plugins/README.md` 也写死了「同源随包分发、不 fetch 数据」的约定；四个官方插件（guard/monitor/files/cron）均只用 `parent.postMessage`，但真机浏览器回归仍待做。
