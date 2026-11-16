# 上机前的三道闸与环境前置（§0–1）

> 上级索引：[DEPLOY_TEST.md](../DEPLOY_TEST.md) ｜ 拆自 `docs/test/DEPLOY_TEST.md` 原 §0–1（2026-11-16 整理，正文未改）。

---

> 主文档：[DESIGN.md](../../DESIGN.md) ｜ 本文件：部署与验收测试 ｜ 设计依据：§5 部署 / §6 安全 / §9 里程碑 / §12 待实测

本文只讲**怎么装、怎么验**，不重复设计论证。每条结论分三档，看表格结论列即可：

| 档位 | 含义 |
|---|---|
| 已实测 | 有人在本机/目标机器跑过，记录了命令与输出 |
| 设计目标 | 方案里写下的数字或行为，**尚未实测**，不得当既有事实引用 |
| 未实现 | 代码里没有，或只留了接口并显式返回“不支持” |

§12 的每一项在本文要么有一条对应验收用例，要么落在 §8 的未实现清单里——不存在“既没测也没记”的项。

## 0. 上机前的三道闸

**先讲清一件事**：开发机是 Windows，`#[cfg(unix)]` 里的进程隔离、Unix socket、setuid 全都不参与编译。
「在开发机上 `cargo check` 通过」**不等于**「Linux 上编得过」。

2026-10-31 第一次为 Linux 目标做类型检查，一次就抓出 3 个只在 unix 上存在的编译错误（原 §9.4 清单已随 §9 精简删除，逐条记录见 `docs/changes/2026-10-31.md`）。
所以上机第一件事是**编译**，不是安装。

| 闸 | 命令（在目标机或同架构 Linux 上） | 通过标准 | 不过怎么办 |
|---|---|---|---|
| 1 编译 | `cargo check --workspace` | 0 error、0 warning | 停下来修，不要带着编译错误上机 |
| 2 单测 | `cargo test --workspace` | 以 CI 的 `test` / `test-win` job 与 [TEST_MATRIX.md](../TEST_MATRIX.md) 为准；Windows 既存的 `remove_dir_all` 文件锁用例属历史说明（§9 已精简） | 同上；失败项按输出定位，别整体重跑蒙过去 |
| 3 结构 | `cargo run -p xtask -- check` | 结尾「全部通过 ✅」 | 商店包变量/Compose 字段对齐被改坏了 |

三道都过再往下走。只拿预编译二进制、机器上没有 Rust 的话，跳过这三闸，但**必须**做 §2.1 的摘要校验和 §2.10 的权限链自检。

### 0.1 编译环境依赖

| 依赖 | 版本 | 说明 |
|---|---|---|
| rustc | ≥ 1.89 | `rust-version` 在 workspace 里写死。**1.85 已不够**：`rusqlite 0.41` 要求 1.89、`libsqlite3-sys 0.39`/`sha2 0.11`/`icu_*` 要求 1.88 —— 用 1.85 会在依赖解析阶段直接报 `requires rustc 1.89.0`（已实测） |
| C 工具链 | cc / gcc / clang 任一 | `rusqlite(bundled)`、`zstd-sys`、`ring` 都要编 C；缺了会报 `x86_64-linux-gnu-gcc not found` |
| pkg-config | 任意 | 上述 crate 的构建脚本会探测 |
| 磁盘 | ≥ 3GB 空闲 | `target/` 在 release + LTO 下比较吃空间 |

```sh
apt-get update && apt-get install -y build-essential pkg-config curl ca-certificates
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
rustc --version    # 需要 ≥ 1.89
```

### 0.2 取得发布物：标准路径是「从 Release 下载安装包」

**交付链路（唯一标准路径）：GitHub Release 下载 → 校验 → 解压 → `install.sh`。**
不再把开发机工作树拷到测试机当常态部署；源码拷贝/本机编译只算开发调试（见本节末尾）。

```sh
# 目标机（无需 Rust；把 <ver> 换成实际版本、<target> 换成 triples）
BASE=https://github.com/<owner>/<repo>/releases/download/v<ver>
curl -LO $BASE/skiff-<ver>-x86_64-unknown-linux-musl.tar.gz
curl -LO $BASE/skiff-<ver>-x86_64-unknown-linux-musl.tar.gz.sha256
sha256sum -c skiff-<ver>-x86_64-unknown-linux-musl.tar.gz.sha256   # 只校本机架构这一份
# 注意：Release 级 checksums.txt 同时列了 amd64 / aarch64 两个安装包与两个
# skiff-server-<target> 裸二进制。只下载其中一部分时不要直接 `sha256sum -c
# checksums.txt`——未下载的条目会报 "FAILED open or read" 并以非零退出（文件本身没问题）。
```

内网/无 GitHub 出网时，用临时 HTTP 静态服务把同一份安装包下发（**这是模拟真实用户路径**，
不要用 scp 传单个二进制替代）：

```sh
# 构建机（装包所在目录）
python3 -m http.server 8000
# 目标机
curl -O http://<构建机内网IP>:8000/skiff-<ver>-x86_64-unknown-linux-musl.tar.gz
```

包内自带 `checksums.txt`（覆盖包内所有文件），解压后可再复核一次：

```sh
tar xzf skiff-<ver>-x86_64-unknown-linux-musl.tar.gz
cd skiff-<ver>-x86_64-unknown-linux-musl && sha256sum -c checksums.txt
```

**构建/调试用路径（非交付路径，不得作为测试机常态部署）**

```sh
# 开发机就地出包（等价于 CI 的同一份 pack-release.sh；Windows 产物不能装 Linux）
sh deploy/pack-release.sh --version 0.1.0 --target x86_64-unknown-linux-musl --skip-plugins

# 或 Linux 构建机上编译后手工出包
rustup target add x86_64-unknown-linux-musl
apt-get install -y musl-tools
sh deploy/pack-release.sh --version 0.1.0 --target x86_64-unknown-linux-musl
```

`pack-release.sh` 产出 `dist/skiff-<ver>-<target>.tar.gz` + 同名 `.sha256`，并在包内生成
`checksums.txt`。包内含 `skiff`、`skiff-plugin`、`install.sh`、`uninstall.sh`、`skiff.service`、
`config.toml.example`、`docker-bootstrap.sh`、`official/*.wbpkg`（4 个）、
`official/public-key.txt`、`README.txt`。
在非 Linux 主机上跑也能出包，但产物是 PE/ELF 混合的假包，脚本会明确警告——**那种包不能装到 Linux 上**。

CI 出的包与手工出包**是同一份脚本、同一套布局**（CI 只是把 `CARGO` 换成 `cross` 并传入版本与 target），
所以 §2 的装机步骤不用改。见 [CI.md](../../ops/CI.md) §4。

### 0.3 传到目标机的文件

| 文件 | 必需 | 用途 |
|---|---|---|
| `skiff-<ver>-<target>.tar.gz` | 是 | 主程序、安装脚本、官方插件与包内摘要 |
| 同名 `.tar.gz.sha256` | 是 | 单包摘要校验 |
| `checksums.txt`（Release 资产） | 否 | 多架构汇总摘要；只装单架构时以同名 `.sha256` 为准（汇总文件直接 `-c` 会因另一架构缺失而非零退出） |
| `skiff-server-<target>` | 否 | 分发侧静态服务裸二进制（`serve`，musl 静态、不在安装包内）；仅自建商店分发时需要，见 [SKIFF_SERVER.md](../../plugin/SKIFF_SERVER.md) §16 |
| 源码目录 | 否 | 仅开发调试（本机编译、给插件测试物编 demo），不属交付路径 |

## 1. 环境与前置条件

> 测试机访问信息（地址、账号、口令文件读取方式、面板 URL/入口前缀、TLS 当前值、当前部署版本与资产摘要）统一见 [TEST_ENV.md](../TEST_ENV.md)；本节只列通用前置条件，不重复测试机凭据。

| 项 | 要求 | 说明 |
|---|---|---|
| 操作系统 | Linux x86_64 / aarch64 | 主形态依赖 systemd；macOS/Windows 只能用于开发与单测 |
| init | systemd | `install.sh` 装 unit 并 enable |
| 内核 | 无硬性下限（建议 ≥ 5.4） | MVP 的路径校验是 `canonicalize` 后比对授权根，**不依赖 `openat2`**；逐段 `O_NOFOLLOW` 打开的实现（`open_dir_nofollow`）已写好但**未接入**判定链，检查与挂载之间仍有窗口（§8）。landlock（≥ 5.13）与 cgroups v2 未实现 |
| Docker Engine | ≥ 24 | 通过 bollard 0.21 走 Unix socket；无 Docker 时面板可启动，容器功能显示“不可用” |
| 内存 | ≥ 512MB 可用 | 闲置 RSS 目标 ≤25MB 属设计目标（§7），待 M5 回填 |
| 磁盘 | 二进制 + 数据目录 | release 实测 **6.65 MB**（Windows PE；Linux 产物体积待测，目标 ≤14MB）。数据目录随容器镜像/插件增长，无硬下限 |
| 端口 | 默认 18080 | 避开 1Panel 10086 / 宝塔 8888 |
| 运行身份 | root | 主形态固定 root（§5.1）：需建插件 UID/GID、读 docker.sock、后续 nftables |
| 时钟 | 与真实时间偏差 < 30s | TOTP 依赖；容器形态注意 `TZ` 与宿主一致 |
| 运行组 | `skiff-plugins` | 由 `install.sh` 创建；插件进程靠它拿到 `/run/skiff` 与 `kernel.sock` 的组权限 |
| 站点统一身份 | `skiff-web` = **201000:201000** | 由 `install.sh` 固定创建（宿主与 PHP 容器同名同号）；安装前 preflight，UID/GID 被其它账号占用直接拒绝，不静默换号 |

安装前自检（在目标机器上逐条跑，全绿再装）：

```sh
uname -m && uname -r
systemctl --version | head -1
docker version --format '{{.Server.Version}}' 2>/dev/null || echo "Docker 不可用（面板仍可装，容器页会显示不可用）"
ss -lntp | grep -E ':(18080|10086|8888)' || echo "端口空闲"
id -u                                   # 必须是 0
getent group skiff-plugins || echo "运行组尚未创建（install.sh 会建）"
getent passwd 201000 && echo "201000 已被占用，install.sh 会拒绝安装（站点统一身份要求固定值）"
getent group 201000 && echo "201000 已被占用，install.sh 会拒绝安装"
mount | grep -q 'cgroup2' && echo "cgroups v2 已挂载（限额未实现，仅记录）"
```

### 1.1 权限口径（装完必须长这样）

| 路径 | 模式 | 属主:属组 | 为什么是这个值 |
|---|---|---|---|
| `/www/skiff` | 0711 | root:root | 可穿越、**不可列目录**。设成 0700 会让降权后的插件走不到自己的 `data/`（ancestor 缺 x 位 = 路径不可达） |
| `/www/skiff/plugins` | 0711 | root:root | 同上 |
| `/www/skiff/plugins/{id}` | 0755 | root:root | 目录名可见无妨，插件代码在下一层 |
| `/www/skiff/plugins/{id}/versions` | 0755 | root:root | 代码目录**只读**：插件不得改自己的二进制 |
| `/www/skiff/plugins/{id}/versions/{ver}` | 0755 | root:root | 降权进程要能**穿越**这层；曾因 staging 带 `0700` 而全线 `Permission denied`（2026-11-03 修复） |
| `/www/skiff/plugins/{id}/versions/{ver}/bin/**` | 0755 | root:root | 二进制与库必须可执行；`.wbpkg` 解包不落 tar 的 mode 位，安装时必须显式置位 |
| `/www/skiff/plugins/{id}/data` | 0700 | 插件 UID | 业务数据，只有该插件能进 |
| `/www/skiff/plugins/{id}/cache` | 0700 | 插件 UID | 缓存 |
| `/www/skiff/plugins/{id}/run` | 0700 | 插件 UID | pid/临时文件；**每次重启重建后要重新 chown** |
| `/www/skiff/backup` | 0700 | root:root | 库快照 |
| `/www/skiff/official` | 0755 | root:root | 随发布包携带的官方插件 `.wbpkg` 与 `public-key.txt`；**只有安装器写**，插件不可写（否则等于给插件一个改信任源的位置） |
| `/www/skiff/skiff.db` | 0600 | root:skiff-plugins | 插件读不到库（E8）。**属组是 `skiff-plugins` 不是 `root`**：unit 里 `Group=skiff-plugins` 决定进程主 gid，服务创建的文件随它落组；真正的闸门是 0600（组位为 0）。2026-11-03 上机实测改正，见下注 |
| `/etc/skiff/config.toml` | 0644 | root:root | 不含密钥；TLS 私钥另放 `/etc/skiff/tls/` |
| `/run/skiff` | 0711 | root:skiff-plugins | 可穿越；socket 本身仍按组判权 |
| `/run/skiff/kernel.sock` | 0660 | root:skiff-plugins | 插件靠**附加组**进来，不是靠 umask |
| `/run/skiff/plugins/{id}/plugin.sock` | 0600 | 插件 UID | 只允许内核连这个插件的 RPC |
| `/www/sites/<domain>/` | 0755 | root:skiff-plugins | 站点边界；`index/` 之外的 `log/ssl/conf` 仍归 root |
| `/www/sites/<domain>/index/` | 2775（目录）/ 0664（文件） | `skiff-web:skiff-web` | 站点内容统一身份：建站、FTP、面板文件管理、插件写入四处一致；`ls -ln` 可直接判断权限 |
| `/www/skiff/php-account-init.sh` | 0644 | root:root | PHP 容器只读挂载的初始化脚本；容器内幂等建号后 `exec php-fpm` |

> 插件 UID 是 `sha256(id)` 取模分配的 200000–200999，**不是**系统 passwd 里的用户；`getent passwd` 查不到是正常的。

> **两处 2026-11-03 上机实测驱动的修正**（不是缺陷，是口径与实现对齐）：
> 1. **服务创建的文件属组是 `skiff-plugins`**（`skiff.db`、`initial-admin.txt`）——unit 的 `Group=` 决定进程主 gid，
>    文件随它落组。权限闸门是 `0600`（组与其他人都是 0），属主仍是 root。原表写 `root:root` 已按实测改。
> 2. **数据根的属主只由 `install.sh` 决定。** unit 里原本还有 `StateDirectory=skiff`，systemd 会按 `User=/Group=`
>    把它 chown 成 `root:skiff-plugins`，**静默覆盖 install.sh 设的 `root:root 0711`**（实测 A6a/A6b/A6c 三条全红）。
>    已删掉该行：数据根由安装器建、口径单一来源；`/run/skiff` 仍在 unit 里（tmpfs，重启即失，必须由 systemd 建）。
