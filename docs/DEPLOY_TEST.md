# 部署与测试（运维视角）

> 主文档：[DESIGN.md](DESIGN.md) ｜ 本文件：部署与验收测试 ｜ 设计依据：§5 部署 / §6 安全 / §9 里程碑 / §12 待实测

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

2026-10-31 第一次为 Linux 目标做类型检查，一次就抓出 3 个只在 unix 上存在的编译错误（清单见 §9.4）。
所以上机第一件事是**编译**，不是安装。

| 闸 | 命令（在目标机或同架构 Linux 上） | 通过标准 | 不过怎么办 |
|---|---|---|---|
| 1 编译 | `cargo check --workspace` | 0 error、0 warning | 停下来修，不要带着编译错误上机 |
| 2 单测 | `cargo test --workspace` | Linux 目标机复跑应为 215 项全过（待实测）；Windows 上 212 ok、3 项 `remove_dir_all` 文件锁红（既存限制，见 §9.4） | 同上；失败项按输出定位，别整体重跑蒙过去 |
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
# 注意：Release 级 checksums.txt 同时列了 amd64 与 aarch64 两个包。
# 只下载其中一个架构时不要直接 `sha256sum -c checksums.txt`——另一个架构的
# 条目会报 "FAILED open or read" 并以非零退出（文件本身没问题）。
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
所以 §2 的装机步骤不用改。见 [CI.md](CI.md) §4。

### 0.3 传到目标机的文件

| 文件 | 必需 | 用途 |
|---|---|---|
| `skiff-<ver>-<target>.tar.gz` | 是 | 主程序、安装脚本、官方插件与包内摘要 |
| 同名 `.tar.gz.sha256` | 是 | 单包摘要校验 |
| `checksums.txt`（Release 资产） | 否 | 多架构汇总摘要；只装单架构时以同名 `.sha256` 为准（汇总文件直接 `-c` 会因另一架构缺失而非零退出） |
| 源码目录 | 否 | 仅开发调试（本机编译、给插件测试物编 demo），不属交付路径 |

## 1. 环境与前置条件

> 测试机访问信息（地址、账号、口令文件读取方式、面板 URL/入口前缀、TLS 当前值、当前部署版本与资产摘要）统一见 [TEST_ENV.md](TEST_ENV.md)；本节只列通用前置条件，不重复测试机凭据。

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

安装前自检（在目标机器上逐条跑，全绿再装）：

```sh
uname -m && uname -r
systemctl --version | head -1
docker version --format '{{.Server.Version}}' 2>/dev/null || echo "Docker 不可用（面板仍可装，容器页会显示不可用）"
ss -lntp | grep -E ':(18080|10086|8888)' || echo "端口空闲"
id -u                                   # 必须是 0
getent group skiff-plugins || echo "运行组尚未创建（install.sh 会建）"
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

> 插件 UID 是 `sha256(id)` 取模分配的 200000–200999，**不是**系统 passwd 里的用户；`getent passwd` 查不到是正常的。

> **两处 2026-11-03 上机实测驱动的修正**（不是缺陷，是口径与实现对齐）：
> 1. **服务创建的文件属组是 `skiff-plugins`**（`skiff.db`、`initial-admin.txt`）——unit 的 `Group=` 决定进程主 gid，
>    文件随它落组。权限闸门是 `0600`（组与其他人都是 0），属主仍是 root。原表写 `root:root` 已按实测改。
> 2. **数据根的属主只由 `install.sh` 决定。** unit 里原本还有 `StateDirectory=skiff`，systemd 会按 `User=/Group=`
>    把它 chown 成 `root:skiff-plugins`，**静默覆盖 install.sh 设的 `root:root 0711`**（实测 A6a/A6b/A6c 三条全红）。
>    已删掉该行：数据根由安装器建、口径单一来源；`/run/skiff` 仍在 unit 里（tmpfs，重启即失，必须由 systemd 建）。

## 2. 形态 A：宿主机 systemd 直装（主形态）

### 2.1 取得发布物并校验

Release 资产：每个架构 `skiff-<ver>-<target>.tar.gz` + 同名 `.sha256`，另有跨架构汇总
`checksums.txt`。**不做 detached 签名**（§11 决策 12，2026-11-01）：只用 SHA-256 摘要防传输损坏；
`.sig` / `verify-file` 链路已移除。

**安装顺序固定：先验摘要，失败立即停手。** 包内 `install.sh` 不校验包含自己的压缩包（自校验悖论）。

```sh
sha256sum -c skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256   # 本机架构的单包摘要（推荐）
# checksums.txt 含两个架构；只下载一份时不能直接整文件 -c
```

### 2.2 解包安装

**在解压目录内直接执行即可**，不依赖仓库的 `deploy/` 相对路径：

```sh
tar xzf skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz
cd skiff-0.1.0-x86_64-unknown-linux-musl
sha256sum -c checksums.txt        # 包内摘要再复核一次（可选，已含在 Releases 校验内）
sh install.sh                     # 可加 --datadir / --listen / --no-docker 等
```

`install.sh` 做的事（幂等，可重复执行）：

| 步骤 | 结果 |
|---|---|
| 建运行组 | `skiff-plugins`（system 组，已存在则跳过） |
| 建数据目录 | `/www/skiff` 0711 root:root，含 `plugins/ logs/ backup/` |
| 放示例配置 | `/etc/skiff/config.toml`（**仅当不存在时**，已有配置只读不改） |
| 装二进制 | `/usr/local/bin/skiff`（先写临时文件再原子替换） |
| 装插件工具 | `/usr/local/bin/skiff-plugin`（包内有就装，用于打包/验签） |
| 放官方插件 | 包内有 `official/` 时 `install -m 0644` 到 `/www/skiff/official/`（7 个 `.wbpkg` = 6 个官方插件 + testkit，另加 `public-key.txt`）；**不自动安装**——权限与对象授权必须逐条确认；公钥由首启自动信任 |
| 装 unit | `/etc/systemd/system/skiff.service` + `daemon-reload` + `enable` |
| 启动 | `systemctl restart skiff` 并验 active（20 s 不起即失败退出；`--no-start` 可跳过） |

`--no-systemd` 只装二进制（容器/CI 用）；`--prefix /opt/skiff` 改安装前缀时，**unit 里的 `ExecStart` 要同步改**，否则 systemd 找不到可执行文件。

装完立刻按 §1.1 对一遍权限，再进 §2.3。

### 2.3 首启与首次登录

```sh
systemctl status skiff --no-pager
journalctl -u skiff -n 40 --no-pager
```

启动日志会打印安全入口前缀与登录地址：

```text
Skiff 已启动
  监听      : 127.0.0.1:18080（明文开发模式）
  数据根    : /www/skiff
  安全入口  : /a1b2c3d4
  配置      : /etc/skiff/config.toml
  登录地址  : http://127.0.0.1:18080/a1b2c3d4/login
  初始口令  : /www/skiff/initial-admin.txt（用户 admin，可直接登录；建议尽快修改口令）
```

`grep` 关键字是「安全入口」：`journalctl -u skiff --no-pager | grep 安全入口`。
监听写成 `0.0.0.0` 时日志里的地址不能直接点，换成真实 IP/域名访问。

初始口令在 `/www/skiff/initial-admin.txt`（0600），**默认不强制首登改密**：登录后直接进入概览，可直接使用面板；仅显示非阻塞的「建议修改口令」提示。改密成功后该文件自动删除。

**监听端口与安全入口前缀**（`listen` / `entry_prefix`）可在「设置 → 后台端口 / 安全入口 / TLS」页修改，
保存即写回 `config.toml`，**需重启生效**；页面会明确提示「需重启」，不会静默失败。规则：

| 项 | 规则 | 失败时 |
|---|---|---|
| 端口 | 1–65535；排除 0；保存前用 `TcpListener::bind` 探针检测占用（与当前监听同值视为自己占用，允许） | 页面就地报错，不落盘 |
| 入口前缀 | 6–32 位 ASCII 字母/数字/`-`/`_`，首字符须字母或数字；不得为 `/`；保留段 `api`/`healthz`/`static`/`assets`/`favicon.ico`/`robots.txt` 拒绝 | 页面就地报错，不落盘 |
| 来源 | `entry_prefix` 显式值（config / 环境变量）优先；未配时首启随机 8 位并持久化到库 `settings.security.entry_prefix` | — |

**防锁死**：前缀变更后，旧前缀在 24 小时内对 GET 请求 302 到新前缀（`settings.security.entry_prefix_previous`
+ `..._previous_until` 两个键控制窗口），窗口外旧前缀直接失效；新前缀始终可用。建议改完先在新标签页
确认新入口能打开，再关旧标签。非当前/非旧前缀的路径一律 404（不是 403）。

**升级行为不变**：已有部署的 `config.toml` 里没有 `entry_prefix` 时继续用库里的旧前缀，入口不变。

需要恢复「首登强制改密」时显式开启（**只对新建管理员生效；已有账号一律不改口令、不改标志**）：

```toml
# /etc/skiff/config.toml
require_initial_password_change = true
```

等价方式：环境变量 `SKIFF_REQUIRE_INITIAL_PASSWORD_CHANGE=1`，或启动参数 `--require-initial-password-change`。重装/升级时 `bootstrap` 只在空库创建管理员，已有 `admin` 的口令哈希与 `must_change_password` 保持原值。

### 2.4 TLS 三态与监听形态（对外前必读）

默认监听 `127.0.0.1:18080`，**默认不启用 TLS**（示例配置里没有 `tls_enabled`，
走旧行为自动推断：loopback + `allow_insecure_http` 明文可登录）。

`tls_enabled` 是三态开关，和 `entry_prefix` / `listen` 落在同一处 `Config`：

| 值 | 含义 | 是否必须有证书 |
|---|---|---|
| `true` | 启用 TLS，本进程终止 | 是；缺失拒绝启动（报错可读，提示补路径或改 `false`） |
| `false` | 显式直接明文 HTTP | 否；不读证书、不生成自签、日志打一行明文提示 |
| 不写 | 旧行为自动推断（loopback / 反代 / 拒绝裸奔） | 有则用，无则按规则矩阵判定 |

对外有三条正路，按场景选一条：

**A. 本进程终止 TLS（公网/跨网推荐）**

```toml
listen      = "0.0.0.0:18080"
tls_enabled = true
tls_cert    = "/etc/skiff/tls/fullchain.pem"
tls_key     = "/etc/skiff/tls/privkey.pem"
```

证书文件须 root 可读、非全局可写。启动时校验文件存在性，缺失直接退出（报错会指出
缺哪个字段、可改 `tls_enabled = false` 走明文）。

自签只用于内网复核（2026-11-03 已实测）。**SAN 必须含实际访问的 IP/域名**，否则浏览器会报名称不匹配：

```sh
mkdir -p /etc/skiff/tls && chmod 0755 /etc/skiff/tls
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout /etc/skiff/tls/privkey.pem \
  -out    /etc/skiff/tls/fullchain.pem \
  -subj "/CN=10.10.15.4" \
  -addext "subjectAltName=IP:10.10.15.4,IP:127.0.0.1,DNS:skiff.local"
chmod 0600 /etc/skiff/tls/privkey.pem
chmod 0644 /etc/skiff/tls/fullchain.pem
```

私钥只放 `/etc/skiff/tls/`（0600 root），不要留在 `/root` 或临时目录。生产请换正式证书
（Let's Encrypt 等）或让反代终止 TLS；自签仅限内网复核，浏览器需手动忽略警告。

**B. 可信反代终止 TLS**

```toml
listen              = "127.0.0.1:18080"
allow_insecure_http = true          # 显式确认「明文只到本机/可信反代」
trusted_proxies     = ["127.0.0.1/32", "::1/128"]
external_url        = "https://panel.example.com"
# tls_enabled 不写：由 loopback + allow_insecure_http + trusted_proxies 推断为「反代后明文」
```

> **不要**在这个形态里写 `tls_enabled = false`：那会把 Cookie 降级成无 `Secure`，
> 浏览器虽仍能登录，但等于放弃了反代形态下本可保留的 `__Host-` + `Secure` 保护。

**C. 直接明文监听（仅内网，浏览器可登录）**

```toml
listen      = "0.0.0.0:18080"
tls_enabled = false                 # 显式关闭 TLS：直接明文，不依赖反代/转发进程
```

`tls_enabled` 是三态：`true` 启用；`false` 显式直接明文；不写 = 旧行为自动推断。
显式关闭时**不走** `allow_insecure_http`/`trusted_proxies` 的旧判据，启动日志会打一行
「明文 HTTP 监听：Cookie 已降级…」；设置页同时给出告警。

规则（§6.2，代码里就是这么判的）：

| tls_enabled | 监听 | 其他条件 | 结果 | 会话 Cookie |
|---|---|---|---|---|
| `true` | 任意 | cert/key 都存在 | HTTPS | `__Host-skiff_session` + Secure |
| `true` | 任意 | cert/key 缺失 | **拒绝启动**，提示补路径或改 `false` | — |
| 未写 | loopback | 无 TLS，`allow_insecure_http=true` | 启动，明文 | `skiff_dev_session`，无 Secure |
| 未写 | 非 loopback | 无 TLS，`allow_insecure_http=true` + `trusted_proxies` **非空** | 启动，视为反代终止 TLS，日志告警 | `__Host-skiff_session` + Secure |
| 未写 | 非 loopback | 无 TLS，`allow_insecure_http=true` + `trusted_proxies` 空 | **拒绝启动**（裸奔） | — |
| 未写 | 非 loopback | 无 TLS，`allow_insecure_http=false` | 拒绝启动 | — |
| `false` | 任意（含 loopback/内网） | 忽略 `allow_insecure_http`/`trusted_proxies` | 启动，明文直连，日志/页面告警 | `skiff_dev_session`，无 Secure；HttpOnly/SameSite 保留 |

三种形态的 Cookie 语义：

- **TLS 开启**：浏览器与面板之间是 HTTPS，用 `__Host-skiff_session` + `Secure`。
- **反代终止 TLS**：浏览器与反代是 HTTPS，反代到面板是明文；仍用 `__Host-` + `Secure`，
  因此浏览器正常回传，但前提是只有 `trusted_proxies` 里的反代能直连面板端口。
- **TLS 关闭（直接明文）**：浏览器直接走 HTTP，`Secure`/`__Host-` 会被丢弃，所以降级为
  `skiff_dev_session`（HttpOnly/SameSite 保持不变）。仅限内网或隔离测试网。

只有直连 peer 命中 `trusted_proxies` 才采信 `X-Forwarded-For/Proto/Host`，否则一律清除伪造头。

**安装脚本与配置的对应关系**（`install.sh`；重装跑也幂等）：

| 参数 / 环境变量 | 写入 `config.toml` |
|---|---|
| `--tls on` / `SKIFF_TLS=on` | `tls_enabled = true`，并要求 `--tls-cert`/`--tls-key`（或从 LE 目录自动发现）；证书复制到 `/etc/skiff/tls/`（0600） |
| `--tls off` / `--no-tls` / `SKIFF_TLS=off` | `tls_enabled = false`，打印明文告警 |
| 不给 | 已有配置有 `tls_enabled` 则沿用；有证书路径视为开、无则交互询问（非交互默认 on 并报错要求补证书） |
| `--listen` / `SKIFF_LISTEN` | `listen` |

配置与环境变量**同一结构**：`SKIFF_TLS_ENABLED`（`true/false/on/off/1/0`）、`SKIFF_ENTRY_PREFIX`、
`SKIFF_LISTEN`、`SKIFF_TLS_CERT`、`SKIFF_TLS_KEY`；优先级为环境变量 > `config.toml` > 默认。

**生效方式**：`listen` / `tls_enabled` / 证书路径 / `entry_prefix` 都是**重启生效**（`systemctl restart skiff`），
无热加载。设置页保存后会明确提示「需重启」，不做静默失败。

#### 2.4.1 内网直连（明文、无 TLS，浏览器可登录）

```toml
listen      = "0.0.0.0:18080"
tls_enabled = false
```

- `tls_enabled = false` 是**显式直接明文**：`serve` 走明文分支，并在启动日志打一行
  「明文 HTTP 监听：Cookie 已降级，仅限内网或反代之后」，设置页同步显示告警。
- **Cookie 口径**：会话 Cookie 降级为 `skiff_dev_session`（无 `Secure`、无 `__Host-` 前缀，
  `HttpOnly` / `SameSite=Strict` 保留），登录页 CSRF Cookie 同理。浏览器从 `http://` 正常
  回传，**登录、改密、页面跳转都能走通**——这是「不依赖反代、不依赖转发进程」的形态。
- `allow_insecure_http` / `trusted_proxies` 在显式 `tls_enabled = false` 时**不参与判定**
  （见 §2.4 的规则矩阵）。`trusted_proxies` 仍只决定**采信哪些 peer 的 `X-Forwarded-*`**：
  填 `127.0.0.1/32` 时，内网客户端伪造的 `X-Forwarded-For` 一律被清除，审计记真实 peer IP；
  **不要**为了让“更多来源能用”把客户端网段填进去，那等于允许它们伪造源地址。
- 安全边界：明文仅限内网 / 隔离测试网。跨公网必须走 TLS 或「反代终止 TLS + `trusted_proxies`」。
- 重装/升级：`sh install.sh --tls off --listen 0.0.0.0:18080`（或 `SKIFF_TLS=off`
  + `SKIFF_LISTEN=0.0.0.0:18080`）。不给参数时脚本按交互/`tls_enabled` 现状决定，且**不改**已有
  `listen`；只有 `/etc/skiff/config.toml` 被删掉才会回到示例默认 `127.0.0.1:18080`。
  `skiff.service` 不含任何监听覆盖项。
- 防火墙（宿主机实测是 iptables，无 firewalld/ufw）：放行回环、`10.10.15.0/24` 与指定开发机 `/32` 的
  面板 18080、站点 80/443，其余 `DROP`；由 `skiff-firewall.service`（oneshot + enabled）在开机时重放，
  iptables 自身不持久。

#### 2.4.2 同端口 TLS 人工复核（自签，2026-11-03 已实测）

配置（`listen` 保持同端口，不再走明文分支；有 TLS 时 `allow_insecure_http` 无意义，已移除）：

```toml
listen   = "0.0.0.0:18080"
tls_cert = "/etc/skiff/tls/fullchain.pem"
tls_key  = "/etc/skiff/tls/privkey.pem"
```

实测环境：证书路径、权限、SAN 与有效期统一见 [TEST_ENV.md](TEST_ENV.md) §2（当轮 SHA-256 指纹
`D7:49:D0:06:BC:64:70:3C:E9:D2:AE:D5:D6:62:07:25:B9:97:60:CE:54:0C:57:D8:27:5B:FB:5B:D3:92:67:78`）。

| 项 | 实测结果 |
|---|---|
| `curl -k https://10.10.15.4:18080/6gp3m9vq/login` | **200** |
| `curl -k https://10.10.15.4:18080/healthz` | **200** |
| 错前缀 `/xxxx/login` | **404** |
| `curl http://10.10.15.4:18080/6gp3m9vq/login` | curl `000`（`Received HTTP/0.9 when not allowed`：收到 TLS 告警字节，**不返回页面**） |
| `ss -ltnp` / 日志 | `0.0.0.0:18080`；`INFO HTTPS 监听中 addr=0.0.0.0:18080` |
| cookie-jar 全流程 | GET 登录页 → `skiff_login=<csrf>; Secure`；POST → **303** `/{p}/settings?force=1` + `__Host-skiff_session=…; HttpOnly; SameSite=Strict; Secure`（CSRF 不再 403）；改密 POST → **303** `/6gp3m9vq` + 新 Secure 会话；首页 **200**（`概览 · Skiff`） |
| 重启测试机 | 服务 active、`0.0.0.0:18080`、证书两文件、`initial-admin.txt`、HTTPS 200、iptables 规则全部保持 |
| 重装 `install.sh` | 只在 `config.toml` 不存在时拷贝示例，且见 `tls_cert` 时 `ensure_insecure_http` 直接返回；实测重装前后 `config.toml` sha256 **相同**，HTTPS 200、明文仍被拒 |

复核后的状态恢复：用停机快照 `/var/backups/skiff/datadir-initial-state.tgz` 回滚数据根（入口前缀存库
`settings.entry_prefix`，清库重装会换前缀，不能替代快照）；恢复后 `initial-admin.txt`（0600 root:skiff-plugins）
回来，登录页 200。初始口令只从该文件取，不在聊天/工单里贴明文。

**HSTS 未实现**：`crates/` 内没有下发 `Strict-Transport-Security` 的代码（实测响应头也没有该字段）。
所以本节的结论是「TLS 链路已实测」，**不等于 HSTS 已启用**。

**18081 明文端口不涉及本轮**：内核配置只有单一 `listen`，`Config` 无第二监听字段；要给 API 联调提供
loopback 明文入口，需要单独加一个监听（未实现，也没有临时 hack）。本轮 18081 未改动。

### 2.5 升级

```sh
systemctl stop skiff     # 停机备份最稳；不停也能备，但插件 data/ 可能被写一半（§6.6 F1）
skiff backup
sh install.sh --no-systemd      # 只换二进制
systemctl start skiff
systemctl status skiff --no-pager
```

数据目录不动；SQLite 迁移幂等、向前，启动自动执行并写 `PRAGMA user_version`。若库的 schema 版本高于二进制支持版本，启动会拒绝并报 `SchemaTooNew`——这是防降级写坏数据，不是 bug。

**官方插件随升级同步**：`install.sh` 更新 `/www/skiff/official/` 后，`skiff serve` 在启动早期
遍历该目录，只对**库内已安装**的插件做比对：包版本更高，或同版本但版本目录内容（文件集合 +
SHA-256）与包不一致时，按普通重装路径安装。`data/` 不覆盖，管理员既有授权原样沿用，
按新 manifest 补齐新增声明的权限与对象 hint（只增不减；敏感动词与任何新增对象授权写
`plugin.grant_upgrade` 审计，记录执行者、来源与版本），已停用插件保持停用；
未安装的插件仍不自动安装（首次安装必须逐条确认权限）。
单个包失败只写 `[warn]` 并继续启动，日志会打印可人工重试的 `skiff plugin install <包>` 命令。

### 2.6 回滚

```sh
systemctl stop skiff
# 1) 换回旧二进制
install -m 0755 skiff-<旧版本>/skiff /usr/local/bin/skiff
# 2) 数据库若已被新版本迁移过，需要回滚备份（备份里带版本与校验和）
ls /www/skiff/backup/
# 3) 用备份替换（停机状态下操作，禁止对活动库直接覆盖）
cp /www/skiff/backup/skiff-<ts>.db /www/skiff/skiff.db
rm -f /www/skiff/skiff.db-wal /www/skiff/skiff.db-shm
systemctl start skiff
```

**注意**：SQLite 迁移只向前，不提供降级脚本。跨 schema 版本回滚必须走备份；备份恢复演练属 M5，**尚未实测**（§8）。

### 2.7 卸载

```sh
sh uninstall.sh            # 保留 /www/skiff 与 /etc/skiff
sh uninstall.sh --purge    # 删程序 + 数据 + 配置（需手输 YES 确认）
```

插件 UID 不是系统用户，`--purge` 时按 200000–200999 扫描 passwd 清理（一般扫不到，属正常）。
**复测建议**：`sh uninstall.sh --purge` 后再从 §2.2 装一次，能验证「干净机器首启」这条路。

### 2.8 常用运维命令

| 命令 | 用途 | 退出码 |
|---|---|---|
| `skiff health` | 探本机 `/healthz` | 0 健康 / 非 0 异常 |
| `skiff doctor` | 只读检查数据目录/配置/TLS/socket | 0 |
| `skiff version` | 版本与架构 | 0 |
| `skiff audit 50` | 最近 50 条审计 | 0 |
| `skiff backup [--out <path>]` | `VACUUM INTO` + 复制插件 data，保留 3 份 | 0 |
| `skiff token create ci` | 建 API Token（仅创建时打印一次） | 0 |
| `skiff plugin list` | 列出已装插件 | 0 |
| `skiff plugin install <pkg> --yes [--grant <perm>] [--resource <obj>]` | 非交互安装；默认采用 manifest 声明的权限与 `resource_hints`（官方包「装完即可用」），显式 `--grant`/`--resource` 覆盖为最小授权 | 0 |
| `skiff plugin remove <id> [--delete-data]` | 卸载，默认保留数据 | 0 |
| `skiff plugin config <id>` | 列出该插件 `config_schema` 声明项与当前值（secret 打码） | 0 |
| `skiff plugin config <id> <key> <value>` | 写入一项配置；是否 secret 由 manifest 决定，键必须在 `config_schema` 内 | 0 |
| `skiff-plugin keygen/new/build/pack/verify` | 插件打包与验签 | 0 |

### 2.9 插件测试物准备（E 组前置）

E 组要装真包，得先造一把测试密钥和一个 demo 插件。**都在目标机跑**（或在构建机跑完把 `.wbpkg` 拷过来）：

```sh
# 1) 生成插件骨架。`new` 会自己生成 <dir>/skiff-signing.key，manifest.key_id 就是它的指纹
skiff-plugin new demo --id com.example.demo --name Demo --sdk-path /root/skiff/crates/sdk

# 2) 编译（产物落到 demo/bin/<arch>/plugin）
skiff-plugin build --dir demo

# 3) 打包。--key 必须与 manifest.key_id 配对，用别的私钥会直接报「指纹不一致」
skiff-plugin pack --dir demo --key demo/skiff-signing.key --out /root/demo-0.1.0.wbpkg

# 4) 装前自查（与内核安装时同一套校验）
skiff-plugin verify /root/demo-0.1.0.wbpkg --pubkey <公钥hex>
```

几个必须知道的点：

| 点 | 说明 |
|---|---|
| SDK 未发布 | `new` 默认找当前目录下的 `crates/sdk`；不在仓库里跑就必须传 `--sdk-path`，否则 Cargo.toml 里的 `skiff-sdk = "0.1"` 解析不到 |
| 公钥从哪来 | `new` 和 `pack` 都会打印`公钥(hex)`与指纹；装包前要在面板「设置 → 信任公钥」导入，未导入=拒装 |
| 包内有什么 | 只有 `bin/ ui/ migrations/` 与 `manifest.toml`；`target/`、`Cargo.toml`、私钥**不进包**（实测 28.9MB → 461KB） |
| 架构 | `build` 按宿主架构产出 `bin/<arch>/plugin`；目标机是 arm64 就在 arm64 机器上 build |
| 目标机没 Rust | 在构建机跑完 `pack`，把 `.wbpkg` 拷到目标机，只做第 4 步验签 |

导入公钥后装包：

```sh
skiff plugin install /root/demo-0.1.0.wbpkg --yes \
  --grant docker:containers.list \
  --grant docker:containers.inspect \
  --grant docker:containers.logs
skiff plugin list
```

**官方插件（随包携带，E 组推荐先用它们冒烟）**——发布包 `official/` 里是 7 个已签名的包 = 6 个官方插件（guard/monitor/files/cron/ftp/firewall）+ `com.skiff.testkit` 架构测试台（原 nginx 插件已于 2026-11-04 内核化为「网站」一级功能，不再随包），装机时已复制到 `/www/skiff/official/`：

```sh
ls /www/skiff/official            # com.skiff.{guard,monitor,cron,firewall,ftp,testkit}-0.1.0.wbpkg + com.skiff.files-0.1.2.wbpkg + public-key.txt
# public-key.txt 不需要手动导入：首次启动会自动写入信任库（见 §2.11 注）
# 仅在公钥缺失/指纹不符时，才需显式执行：
#   skiff trust import /www/skiff/official/public-key.txt --label official

# 主机监控：只读 /proc
skiff plugin install /www/skiff/official/com.skiff.monitor-0.1.0.wbpkg --yes

# 容器守护：manifest 已声明权限；容器对象按需显式追加
skiff plugin install /www/skiff/official/com.skiff.guard-0.1.0.wbpkg --yes \
  --resource container:nginx

# 文件管理：默认授权 path:/www（resource_hints 预勾选），装完即可浏览；
# 想缩小范围就显式覆盖，例如 --resource path:/var/www
skiff plugin install /www/skiff/official/com.skiff.files-0.1.2.wbpkg --yes

# 计划任务：manifest 已声明 scheduler:read/manage/run/admin 与 interop:call；
# 要调度别的插件再追加 --resource plugin:<id>
skiff plugin install /www/skiff/official/com.skiff.cron-0.1.0.wbpkg --yes \
  --resource plugin:*

# 架构测试台（多视图 / 槽位 / RPC 成功失败 / 越界 / 互操作）
skiff plugin install /www/skiff/official/com.skiff.testkit-0.1.0.wbpkg --yes
```

> `--yes` 不再「权限默认全拒」，而是采用 manifest 声明 + `resource_hints`；显式 `--grant`/`--resource` 会把该维度收窄到只给列出的项（少一项即运行时 403，插件会把「该去勾哪个授权」写进错误文案）。确认页同样默认勾选这些声明值，取消勾选即按最小授权安装。**重装/升级按同一插件 id 沿用既有授权**（不重置为 manifest 默认值、manifest 新增声明不自动授予）；**只有显式卸载才清空授权**，重装后无需重新授权。

### 2.10 权限链自检（装完必查）

装完不查权限，插件测试会以「EACCES」「连不上」的形式随机失败，且报错发生在插件侧，很难反推。逐条跑：

```sh
# 目录与 socket
ls -ld /www/skiff /www/skiff/plugins /run/skiff
stat -c '%a %U:%G %n' /run/skiff/kernel.sock
# 期望：drwx--x--x root root …；660 root skiff-plugins

# 插件装好后，逐段看路径可达性
namei -l /www/skiff/plugins/com.example.demo/data
# 代码目录与二进制必须可穿越/可执行（缺这一条 = 降权进程 Permission denied）
ls -ld /www/skiff/plugins/com.example.demo/versions/*/
ls -l /www/skiff/plugins/com.example.demo/versions/*/bin/*/plugin
# 期望：目录 755 root:root；二进制至少 owner 有 x 位（修后为 755）

# 插件跑起来后看它的真实身份（不是 passwd 用户，是哈希 UID）
pid=$(pgrep -f 'plugins/com.example.demo/versions' | head -1)
grep -E '^(Uid|Gid|Groups)' /proc/$pid/status
# 期望：Uid/Gid 都是 200xxx；Groups 里含 skiff-plugins 的 gid
getent group skiff-plugins | cut -d: -f3

# 主进程仍是 root（它要能 setuid）
ps -o user,group,args -p $(systemctl show -p MainPID --value skiff)
```

### 2.11 官方插件：导入公钥 → 安装 → 复核（2026-11-03 已实测）

> **历史记录**：以下内容记录 2026-11-03 的实测过程。其中 `com.skiff.nginx` 已于 2026-11-04 内核化为「网站」一级功能（见 [`docs/changes/2026-11-04.md`](changes/2026-11-04.md)），其安装命令、`/etc/nginx/conf.d` 授权与 RPC 复核步骤不再适用；其余 4 个插件（guard/monitor/files/cron）的步骤仍有效。

**为什么会出现「一个插件都看不到」**（本机逐项实测）：

- 列表页 `GET /{prefix}/plugins` 读的是 SQLite 的 `plugins` 表（`state.db().plugins()`），**不扫描目录**；
  顶部导航的插件入口也由这些行生成。表为空时页面显示「还没有安装插件」，导航里没有插件项。
- `install.sh` 只把发布包里的 `official/` 复制到 `/www/skiff/official/`，**不自动安装未装插件**
  （脚本随后只打印提示）。`official/` 下的 `.wbpkg` 是分发物，不等于已安装。
  已安装的官方插件由内核在启动早期做版本/内容比对后自动补齐（见 §2.5），
  `data/`、既有授权与停用状态都不变。
- 信任库为空时安装会被 TOFU 拒绝。**2026-11-04 起**，`official/public-key.txt` 会在首启时自动写入信任库；仅当公钥文件缺失或指纹不符时，才需要执行 `skiff trust import <公钥文件> --label official`（或面板「设置 → 信任公钥」）。CLI 报错会区分「包自带公钥但未信任」与「包里没带公钥」，并打印带真实路径的一步命令。
- `allow_reduced_sandbox = false` 时，官方 7 个包的 manifest 没有显式关闭 `requires_full_sandbox`
  （默认 true），安装会全部被拒；本机配置 `allow_reduced_sandbox = true` 才可装。
- 停机快照 `/var/backups/skiff/datadir-initial-state.tgz` 内的库是 **0 插件 / 0 公钥**，且不含 `official/`；
  回到该快照就是回到「未装插件」状态。

结论：当前实例是「库空 + 公钥空 + official 未落盘」，**插件列表入口与路由本身没有缺失**，也不需要新增页面。

**实际落地（本机命令，仓库外无改动）**

```sh
# 0) 发布物落到数据根（install.sh 的布局；只复制，不安装）
install -d /www/skiff/official
cp -a /root/iter/official/. /www/skiff/official/

# 1) 官方公钥：首启已自动信任（无需手动）；若缺失/不符，才需
#    skiff trust import /www/skiff/official/public-key.txt --label official
#    或 UI 设置页「信任公钥」/ Bearer Token POST /{p}/settings/keys

# 2) 安装（非交互逐条授权；安装默认 enabled=true）
cd /www/skiff/official
skiff plugin install com.skiff.monitor-0.1.0.wbpkg --yes \
  --grant fs:read --grant fs:list --resource path:/proc
skiff plugin install com.skiff.files-0.1.0.wbpkg --yes \
  --grant fs:read --grant fs:write --grant fs:list --grant fs:mkdir \
  --grant fs:remove --grant fs:rename \
  --resource path:/var/www --resource path:/etc/nginx/conf.d
skiff plugin install com.skiff.guard-0.1.0.wbpkg --yes \
  --grant docker:containers.list --grant docker:containers.inspect \
  --grant docker:containers.restart --grant fs:read --grant fs:write --grant fs:list \
  --resource container:nginx
skiff plugin install com.skiff.cron-0.1.0.wbpkg --yes \
  --grant identity:self --grant interop:call --resource plugin:*
# 历史记录（2026-11-03）：com.skiff.nginx 已于 2026-11-04 内核化为「网站」，不再安装
#skiff plugin install com.skiff.nginx-0.1.0.wbpkg --yes \
#  --grant fs:read --grant fs:write --grant fs:list --grant fs:remove --grant fs:rename \
#  --grant docker:containers.list --grant docker:containers.restart --grant interop:call \
#  --resource path:/etc/nginx/conf.d --resource container:nginx --resource plugin:com.skiff.cron

skiff plugin list   # 当时 5 行 signed/enabled/running（nginx 内核化后为 4 行）
```

**systemd 沙箱缺口（历史，部署侧，未改 unit 模板）**：`deploy/systemd/skiff.service` 用 `ProtectSystem=full`，
整个进程对 `/etc` 只读，当时 nginx 插件写 `/etc/nginx/conf.d` 会报 `Read-only file system`。测试机用 drop-in 放开：

```sh
mkdir -p /etc/systemd/system/skiff.service.d
printf '[Service]\nReadWritePaths=/etc/nginx/conf.d\n' \
  > /etc/systemd/system/skiff.service.d/nginx-sites.conf
systemctl daemon-reload && systemctl restart skiff
```

**UI 复核**（Bearer 绕过登录仅用于验证；正常用户走浏览器会话）：

| 项 | 结果 |
|---|---|
| 插件列表 `GET /{p}/plugins` | 200，当时 5 行在列、导航出现 5 个插件项（nginx 内核化后为 4 行） |
| 详情页 `GET /{p}/plugins/com.skiff.nginx`（历史） | 200 |
| 托管 UI `GET /plugins/com.skiff.nginx/ui/index.html`（历史） | 200，带 CSP `default-src 'none'; script-src 'self'` |
| nginx RPC `sites` / `create` / `read` / `backup` / `scheduleBackup`（历史） | 200；`/etc/nginx/conf.d/example.com.conf` 落盘；备份与 cron 任务创建成功 |
| nginx `reload`（历史） | 400「找不到容器 nginx」（本机无 Docker 容器；不再是授权 403） |

**顺带修复的内核缺陷**：`kernel/capability.rs` 把 `target: null` 当成 id/name 全空的容器，
导致 `containers.list` 去匹配永远不存在的 `object:container:` 授权（nginx `reload`、guard 拉取容器列表被 403）。
已改为「缺省或 null = 无目标」并补两个单测；**测试机二进制按源码现编（glibc 动态）**，仍不是 musl 发布包。

### 2.11.1 插件信息页与多视图路由（2026-11-05 实测）

manifest 升到 `api = "1.2"` 后，路由契约变为：

| 路由 | 用途 | 2026-11-05 实测（`/swn4nbhe` 前缀） |
| --- | --- | --- |
| `GET /{p}/plugins/{id}` | 内核渲染的信息 + 管理页 | 5 个插件均 200；版本/API、签名与指纹、沙箱、授权、启停、卸载、关联菜单、日志入口齐全 |
| `GET /{p}/plugins/{id}/main` | 默认功能页（`ui/main.html`） | files 200；无 L3 声明，无标签栏 |
| `GET /{p}/plugins/{id}/<view>` | 声明的 L3 视图 | cron `list`/`new`、guard `containers`/`events`、monitor `overview`/`history` 200，标签栏 2 项；testkit `main`/`check`/`group` 200，标签栏 3 项 |
| `GET /{p}/plugins/{id}/<未知>` | 未知视图 | 404，文案「插件 com.skiff.files 不存在视图 nope，或该视图对当前角色不可见」 |
| `GET /{p}/m/plugin/{id}/<l2>[/<l3>]` | 菜单深链 | cron `jobs`/`jobs/list`/`jobs/new`、files、testkit `testkit[/check]`、`testkit-single` 全 200 |

testkit 实测：`ping` / `state` / `probe` RPC 200；`interop_cron` 200（`jobs:[], ok:true`）；
`fail` HTTP 400 且文案含 `-32602`；`fs_read /etc/passwd` 400（「内核返回 HTTP 403：路径不在
已授权的 path: 根内」）；`fs_read /proc/uptime` 200。概览出现 `data-slot="overview.actions"`；
停用 testkit 后槽位与导航链接消失、`testkit/main` 404，重新启用即恢复。

> 同版本可直接重装（原位覆盖版本目录，保留 data/ 与既有授权，不再弹确认页）；
> 官方包签名密钥一致时同样适用。如需重新选择授权集，先 `skiff plugin remove <id>`（保留 data）再安装。

### 2.12 网站 vhost：默认站与兜底站语义（2026-11-04 实测）

生成物分两类：**内核公共配置**落在 `conf_dir`（默认 `/www/nginx/conf.d/`），**各站 vhost** 落在站点自己的 `conf/`。

| 文件 | 何时存在 | 关键配置 |
| --- | --- | --- |
| `/www/nginx/conf.d/00-skiff-fallback.conf` | **始终存在** | `listen 80; server_name skiff-fallback; return 404;` |
| `/www/nginx/conf.d/00-skiff-default.conf` | **仅当设置了默认站点** | `listen 80 default_server; server_name _;` + 站点 root |
| `/www/nginx/conf.d/10-skiff-sites.conf` | **始终存在** | `include /www/sites/*/conf/*.conf;`（`*.conf` 不匹配 `.conf.disabled`） |
| `/www/sites/<域名>/conf/<域名>.conf` | 每站一个 | `root /www/sites/<域名>/index;`、`access_log` 与 `error_log` 指向 `/www/sites/<域名>/log/...`、`ssl_certificate` 指向 `/www/sites/<域名>/ssl/cert.pem` |

站点树是「一站一目录”：`index/`（文档根）、`log/`（`access.log`/`error.log`）、`ssl/`（`cert.pem`/`key.pem`）、`conf/`（vhost）。备份单个站点目录即备份整站。**不提供旧布局迁移**：测试期无正式库，启动不搬移/清理任何旧路径（`public/`、`/www/logs/nginx/`、`/www/certs/<域名>/`、公共 `conf.d` 站点 conf 均忽略）；新建、日志、证书、vhost 全部直接落在站点目录内。

未知 Host / 裸 IP 访问分两种状态，**不要混为一谈**：

1. **无默认站点**：80 端口没有 `default_server`，nginx 取第一个 server（按文件名是兜底站），裸 IP 与未知 Host 都返回 **404**。
2. **有默认站点**：`default_server` 由默认站接管，裸 IP 与未知 Host 都落到默认站并返回 **200**（1Panel 行为）；兜底站此时不参与匹配。

兜底站不使用 `server_name _`（默认站已占用 `_`），因此 `nginx -t` 与 reload 不应再出现
`conflicting server name "_" on 0.0.0.0:80` 警告。

### 2.12.1 网站运行环境与三类建站（2026-11-05 全新库实测）

schema v7 起 `sites` 整表重建为 `site_type(runtime/static/proxy)` + `runtime_id` + 反代字段，
`site_runtimes` 种入唯一 `php-8.5`；**测试期不做旧数据迁移**，重置库后由面板重新建站。

- 全新库 `PRAGMA user_version=7`，`site_runtimes` 仅 `php-8.5`（默认），`sites=0`；
  `POST /{p}/sites/env/install` 完成环境安装后 `docker exec skiff-web-nginx nginx -t` 成功。
- 三类站（入口前缀 `/swn4nbhe`）：
  - PHP（`php.local` + `runtime_id=php-8.5`）：HTTP 与 HTTPS 均 200，正文含 `PHP Version 8.5.12`；
  - 静态（`static.local`）：自定义 `index/index.html` 返回 200；
  - 反代（`proxy.local` → `http://127.0.0.1:18081`）：200，`Server: openresty/1.31.8.1`、
    `X-Upstream: 18081`，上游收到 `Host: proxy.local`。
- 删除三个站均 303：站目录与 `conf/<域名>.conf` 消失，`nginx -t` 成功，`Host` 回落 404，
  库内 `sites=0`；`/www/sites` 为空。
- 拒绝路径（均 HTTP 200，表单页内联告警）：重复域名「域名已存在」；`ftp://` 上游
  非 http(s)；`site_type=node|go` 只支持三类；未知 `runtime_id` 运行环境不存在。
- 运行环境页：`/{p}/runtimes`、`/{p}/sites/env`、`/{p}/sites/env/php-8.5` 均 200；PHP 配置保存 303，
  `/www/php/www.conf` 写入 `memory_limit=512M`、`upload_max_filesize=128M`、`post_max_size=256M`、
  `max_execution_time=120`、`pm=ondemand`，PHPInfo 页面显示 `upload_max_filesize=128M`；
  关联站点数 1→0 正确；绑定 `php.local` 时删除运行环境被拒并列出域名。

### 2.13 站点 HTTPS（2026-11-04 已实测）

一句话：证书按站点落 `/www/sites/<域名>/ssl/{cert.pem,key.pem}`，vhost（`/www/sites/<域名>/conf/<域名>.conf`）追加 `listen 443 ssl`，
强制跳转把 80 块换成 `return 301 https://$host$request_uri;`；所有改动先写盘、再
`nginx -t`，通过才 reload，失败按文件快照与库开关回滚。

| 操作 | 面板入口 | 落盘效果 |
|---|---|---|
| 生成自签 | 站点详情 → SSL → 生成自签证书 | `openssl req -x509 -newkey rsa:2048 -sha256 -days 825 -nodes`，`CN=<域名>`、`subjectAltName=DNS:<域名>[,DNS:<别名>]`；`cert.pem` 0644、`key.pem` **0600** |
| 上传 PEM | 同页 `<details>` 表单 | 先校验 CERTIFICATE / PRIVATE KEY 标记与公钥配对，再原子替换；不匹配直接拒绝，不动现网文件 |
| 强制 HTTPS | 同页「强制切换」 | `force_https=1` 且必须已启用 HTTPS；80 返回 **301** 到 `https://$host$request_uri` |
| 关闭 / 删除 | 同页「关闭 HTTPS」「删除证书」 | 关闭保留文件；删除先关 TLS 再删目录 |

真机实测（Debian 13，`demo.local` 为默认站，域名解析到 `10.10.15.4`）：

```sh
# 生成自签 + 启用
curl -sk https://10.10.15.4/ -o /dev/null -w '%{http_code}\n'      # 200
grep -o 'PHP Version 8.5.12' <响应体>                               # 命中
openssl x509 -in /www/sites/demo.local/ssl/cert.pem -noout -subject -ext subjectAltName
stat -c '%a' /www/sites/demo.local/ssl/key.pem                      # 600
docker exec skiff-web-nginx nginx -t                                # syntax ok，无 warning

# 强制跳转
curl -s -o /dev/null -D - http://10.10.15.4/ | head -2               # 301 + Location: https://10.10.15.4/
curl -sk https://10.10.15.4/ -o /dev/null -w '%{http_code}\n'       # 200
```

回滚实测：手工塞入一个坏 conf（`zz-bad.conf`）后点「关闭 HTTPS」，页面以 flash 展示
「nginx -t 失败…」（不再用 `?error=` 传参），站点 conf 的 md5 与库内 `https/force_https` **均未变化**；
坏 conf 移除后 `nginx -t` 通过、HTTPS 仍是 200。非法 PEM 上传同样被拒且现有证书不受影响。

> 注意：校验失败时**不得重启 nginx**——若坏 conf 来自外部，重启会让现网 nginx 起不来。
> 实现里「`nginx -t` 失败」分支只恢复文件与库开关；只有「重启动作本身失败」才重试重启。

### 2.14 镜像源自动落盘（2026-11-04 已实测）

「网站 → 设置」里的 `registry_mirror` 保存后不再只是写库，而是合并进 `/etc/docker/daemon.json`：

1. 解析：逗号（含中文逗号）/空白分隔，必须带 `http(s)://`，去掉尾部 `/`；
2. 合并：保留 `daemon.json` 其它顶层键，只替换 `registry-mirrors`；空值删除该键但保留文件合法；
3. 备份：内容有变化时先写 `daemon.json.bak.<时间戳>`，再临时文件 + `rename` 原子替换；
4. 重启：**有运行中容器**时只落盘不重启，页面明确提示；无运行中容器才 `systemctl restart docker`，
   等 socket 就绪并按最多 8 次、每次 1 s 重试列表接口（避开面板旧连接池的首个失败请求）；
5. 回滚：重启或可用性检查失败则恢复备份并再次重启，接口以 flash 报错（不再用 `?error=` 传参），库内设置不更新。

真机实测（原有 `daemon.json` 含 `log-driver` 等键）：

```sh
# 有运行中容器：只落盘
docker ps --format '{{.Names}}'          # skiff-web-nginx / skiff-web-php 在跑
# 页面保存后：notice 提示「有运行中容器，未重启」；备份文件出现；JSON 合并成功

# 停容器后再保存：写入并重启
docker stop skiff-web-nginx skiff-web-php
# 页面保存后：notice 提示「Docker 已重启」；docker active；docker pull hello-world:latest 成功
docker start skiff-web-nginx skiff-web-php
```

重启 docker 后，面板容器列表页仍为 **200**（连接池自动重建），网站 HTTPS 恢复 200。

## 3. 形态 B：Docker 受限形态（1Panel 商店包，可选）

**先读能力面收缩表，不能接受就别用这个形态。**

| 能力 | 容器形态 | 后果 |
|---|---|---|
| docker | 可用 | 仍走动词/对象/参数三层策略 |
| identity | 可用 | 登录/会话/TOTP/RBAC/审计 |
| fs | 不可用 | 需要 fs 的插件装前拒装；内核「工具 → 文件管理 / 回收站 / 配额管理」入口隐藏，`/{p}/files*` 与 `/{p}/m/core/files*` 直达一律 403「当前部署形态不提供文件管理能力」 |
| net | 不可用 | 需要 net 的插件装前拒装；出网收口无法兑付 |
| 宿主绑定挂载 | 全部 403 | 只能用 named volume |
| 沙箱 | reduced（独立 UID 有，cgroups/landlock 无） | UI 标 reduced sandbox |

安装要点：

1. 表单里 `SKIFF_ALLOW_INSECURE_HTTP` 与 `SKIFF_TRUSTED_PROXIES` 必须成对配置（默认 `1` + `172.16.0.0/12`）。只开明文不给白名单 → 内核拒绝启动，容器反复重启，日志里能看到明确的拒绝原因。
2. 访问入口请用 1Panel 的网站反向代理并开启 HTTPS；直接 `http://IP:18080` 在公网等于把管理面裸奔。
3. 初始口令：`docker exec <容器名> cat /data/initial-admin.txt`。
4. 升级：1Panel 升级流程会先跑 `scripts/upgrade.sh`，在停机状态下把 `skiff.db` 快照到 `data/backup/`（保留 3 份）。这是**升级回滚用**的快照，不替代备份恢复演练。

与主形态的差异（容器形态做不到，别照着主形态文档找）：

| 主形态有 | 容器形态 |
|---|---|
| 宿主 bind 挂载（授权根内） | 全拒 |
| 插件 fs 能力 | 拒装 |
| 内核文件管理（`/{p}/files*`） | 入口隐藏 + 直达 403 |
| 插件 net 能力（端口规则） | 拒装 |
| 完整沙箱（landlock/cgroups） | reduced |

## 4. 本地开发与冒烟

```sh
# loopback 明文（Cookie 降级为 skiff_dev_session，页面持续标红）
cargo run -p skiff --bin skiff -- --dev serve

# 或指定独立数据目录，避免污染开发机
SKIFF_DATA_DIR=./tmp-data SKIFF_RUN_DIR=./tmp-run \
  cargo run -p skiff --bin skiff -- --dev --config ./tmp-config.toml serve
```

开发机限制（Windows/macOS）：

| 项 | 状态 |
|---|---|
| 编译与单测 | 可用 |
| Docker 管理 | 不可用（无 Linux socket），面板显示“Docker 不可用”，其余功能正常 |
| 插件进程隔离（setuid/landlock/cgroups） | 不可用，走 `Unsupported` 分支 |
| kernel.sock（Unix socket） | 不可用 |
| 平台相关代码的类型检查 | **部分**：`cargo check -p skiff-model -p skiff-policy -p skiff-wire --target x86_64-unknown-linux-gnu` 可跑（纯 Rust）；其余 crate 有 C 依赖，Windows 上无法为 Linux 目标编译 |

## 5. 配置速查

优先级：**环境变量 > `/etc/skiff/config.toml` > 内置默认**。未知字段会导致启动失败（`deny_unknown_fields`），这是刻意的——拼错不该静默忽略。

| 环境变量 | 配置文件键 | 默认 | 作用 |
|---|---|---|---|
| `SKIFF_LISTEN` | `listen` | `127.0.0.1:18080` | 监听地址 |
| `SKIFF_DATA_DIR` | `data_dir` | `/www/skiff` | 唯一数据根 |
| `SKIFF_RUN_DIR` | `run_dir` | `/run/skiff` | 运行目录（含 kernel.sock） |
| `SKIFF_DOCKER_SOCKET` | `docker_socket` | `/var/run/docker.sock` | Engine socket |
| `SKIFF_FORM` | `form` | `native` | `native` / `docker`（受限） |
| `SKIFF_DOCKER_TIER` | `docker_tier` | `lifecycle` | `basic`(M2) / `lifecycle`(M3) |
| `SKIFF_TLS_CERT` / `SKIFF_TLS_KEY` | `tls_cert` / `tls_key` | 无 | 内置 TLS |
| `SKIFF_ALLOW_INSECURE_HTTP` | `allow_insecure_http` | `false` | 明文开关（见 §2.4 矩阵） |
| `SKIFF_TRUSTED_PROXIES` | `trusted_proxies` | 空 | 可信反代 CIDR，逗号分隔 |
| `SKIFF_AUTHORIZED_ROOTS` | `authorized_roots` | 空 | 宿主 bind 授权根（空 = 不许 bind） |
| `SKIFF_ALLOW_UNSIGNED` | `allow_unsigned` | `false` | 允许未签名插件（仅开发） |
| `SKIFF_ALLOW_REDUCED_SANDBOX` | `allow_reduced_sandbox` | `false` | 允许沙箱降级装插件 |
| `SKIFF_EXT_EXEC_TIMEOUT_SECS` | `ext_exec_timeout_secs` | `900` | 单扩展一次 `docker exec` 硬超时（秒；0 拒绝，上限 86400；超时作业 `failed` + `error_class=timeout`） |
| `SKIFF_EXT_BATCH_TIMEOUT_SECS` | `ext_batch_timeout_secs` | `1800` | 批量扩展脚本硬超时（秒；口径同上） |
| `SKIFF_EXTERNAL_URL` | `external_url` | 无 | 对外地址（绝对链接兜底） |
| `SKIFF_LOG_LEVEL` | — | `info` | tracing 过滤；排障用 `RUST_LOG` 风格，如 `debug,skiff_plugin=trace` |
| `SKIFF_CONFIG` | — | `/etc/skiff/config.toml` | 配置文件路径 |

> `login_concurrency` 只能写在配置文件里，**没有**对应环境变量。

## 6. 验收测试用例

记录方式：每条填「实测」「结论」两列；失败即阻断该里程碑，不要“先放过去”。角色列 `v/o/a` 依次为 viewer / operator / admin。

> **2026-11-03 首装轮已把 A 组（除依赖特定前置的几条）、B 组 12 条、D0 四条跑成脚本**：`deploy/accept.sh`
> （`--group preflight,a,b,d0`）。脚本里的编号（A3/A5/A6a…/B2a/B3e/D0c…）与下表逐条对应，
> 结果 **PASS 47 / FAIL 0 / SKIP 1**（SKIP 是「装后阶段不查端口占用」，按设计跳过）。
> 首轮还抓出两个真缺陷，见 [ITERATION.md](ITERATION.md) §6.1（install.sh 装完服务起不来）与 §6.4（systemd 改掉数据根属主）。

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

## 7. 故障排查

| 现象 | 可能原因 | 处置 |
|---|---|---|
| 启动即退出，日志提示“非 loopback 且未配置 TLS” | 监听 `0.0.0.0` 但没配 TLS | 配 `tls_cert/tls_key`，或改 loopback + 反代 + `trusted_proxies` |
| 启动即退出，提示 `SchemaTooNew` | 换了旧版本二进制但库已迁移 | 用备份回滚数据库（§2.6） |
| 页面 404，但服务是活着的 | 没带安全入口前缀 | 用日志里的 `/xxxxxxxx/` 前缀 |
| 登录一直 401 | 初始口令已改、或 `initial-admin.txt` 被手动删 | 用 `skiff token create` + API，或重装并保留数据后由 admin 重置 |
| 容器页显示“Docker 不可用” | socket 不存在 / 权限不足 / 非 Linux | 查 `skiff doctor`；确认 unit 里 root 运行 |
| **插件装上了，一启动就停用，插件侧报 EACCES** | 数据根/运行目录缺 other-x（被改成 0700），或 `kernel.sock` 属组不是 `skiff-plugins` | 按 §2.10 查；数据根应为 0711，socket 应为 `660 root:skiff-plugins` |
| **插件日志报“UID 未映射到任何插件”** | 插件进程 uid 与 `sha256(id)%1000+200000` 不一致（手工 chown 过目录、或换了用户跑） | 别手工改插件目录属主；重装插件恢复 |
| **重启后插件突然写不进自己的 `run/`** | `clean_run_dirs` 重建目录后属主本应归还，若仍失败说明不是 root 起的服务 | 确认 `systemctl` 里是 `User=root`；或 `ls -ld .../run` 看属主 |
| **装包报“未导入公钥”** | 信任公钥没导入 | 面板「设置 → 信任公钥」导入 key_id + 公钥 hex |
| **装包报“已安装版本高于包内版本”** | 包版本低于已装版本 | 改 manifest 的 `version`；同版本可重装，降级需先卸载（保留 data）再装 |
| **`pack` 报“指纹不一致”** | `--key` 用的私钥与 manifest.key_id 不是一对 | 用 `new` 生成的那把 `<dir>/skiff-signing.key`，或改 manifest.key_id |
| **`skiff-plugin new` 后 `build` 找不到 skiff-sdk** | SDK 未发布到 crates.io，脚手架写成了版本依赖 | 加 `--sdk-path <仓库>/crates/sdk` 重新生成，或手改 Cargo.toml 为 path 依赖 |
| **编译报 `x86_64-linux-gnu-gcc not found`** | 缺 C 工具链 | `apt install build-essential pkg-config` |
| **systemd 起不来，手跑却正常** | `ExecStart` 路径与实际安装前缀不一致（用了 `--prefix`），或 `ProtectSystem=full` 让某路径只读 | `systemctl cat skiff` 核对路径；需要写 `/etc` 的功能改到 `/var/lib` 或调 unit |
| 反代后 Cookie 不生效 | scheme 判定为非 HTTPS，`__Host-` 前缀被浏览器拒收 | 确认 `trusted_proxies` 命中且反代传 `X-Forwarded-Proto: https` |
| 表单提交 403 | CSRF 令牌与当前会话不匹配（多标签/会话续期） | 刷新页面重取令牌 |

## 8. 未实现 / 未验证清单（不得当事实引用）

| 项 | 现状 | 影响 | 对应 §12 |
|---|---|---|---|
| **插件安装后的代码目录可执行位** | **已修（2026-11-03），未上机复验** | 修前官方插件装完即 `Permission denied`（`versions/{ver}` 0700、`bin/plugin` 0644）；修后仅过本机门禁，需重新出包复验 | §4.2 |
| **Docker 容器生命周期 / 资源限制 / Compose 运行时** | **未实测**：测试机无镜像（registry 443 超时），且 Web 无 create/remove 路由 | 只验到「非法动词 400 / 不存在容器 404 / 列表与 `docker ps` 一致」；不得说「容器管理已可用」 | §5.3、§12 |
| **HSTS 响应头** | **未实现**：`crates/` 内无 `Strict-Transport-Security` 下发，实测响应头也没有 | 浏览器首次仍可被诱导到同端口明文地址（该地址不返回页面）；不得声称已启用 HSTS | §12 |
| landlock 隔离 | **未实现** | 插件隔离主力是“独立 UID + 目录权限 + 附加组” | §12、§9.1 #16 |
| cgroups v2 限额（内存/PID/CPU） | **未实现** | 只有 `setrlimit`（AS 256MB / NOFILE 256 / NPROC / FSIZE / CORE） | §12 |
| nftables 出网收口（net 能力） | **未实现**，kernel RPC 返回 501 | 插件出网不受控；`net` 能力插件不可用 | §9.1 #17 |
| project quota | **未实现** | 磁盘只有软限额 | §12 |
| **bind 挂载的 TOCTOU 窗口** | `canonicalize` 校验；`open_dir_nofollow` 已实现但**未接入** | 校验与 Docker 实际挂载之间有窗口（D8 会通过）。**不得声称已消除** | §5.5 |
| **Linux 目标的完整编译** | **部分**：`model/policy/wire` 已用 `--target x86_64-unknown-linux-gnu` 检查通过；`plugin/sdk/core/web` 因 C 依赖无法在 Windows 上为该目标编译 | 这三个 crate 的 unix 分支目前只经过人工审阅（已修 3 个编译错误，见 §9.4），**必须**在目标机 `cargo check --workspace` 再确认一次 | §12 |
| **备份的一致性** | `skiff backup` 不停插件：库 `VACUUM INTO` 一致，插件 `data/` 直接复制可能抓到写一半的文件 | 需要强一致时先停服再备（§2.5） | §12 |
| **插件 UID 只有 1000 个槽位** | 哈希取模；撞车**装前拒绝**，但没有换槽/动态分配 | 装到几十个插件后可能出现「装不上」 | §4.2 |
| 运行时支持矩阵（Go/JVM/.NET 在 AS 256MB 下） | **未实测** | 决定哪些运行时能跑插件 | §12 |
| 全部资源数字（RSS/体积/CPU） | **设计目标** | M5 回填 | §7、§12 |
| 备份恢复演练 | **未做** | “备份成功”不等于可恢复 | §12 |
| 与 1Panel/宝塔共存实测 | **未做** | 仅设计上不冲突 | §12 |
| **Linux 发行包的二进制体积** | **未实测** | 本机只产出 Windows PE（6.65 MB）；Linux x86_64 的实测体积与 `≤14MB` 目标的对照要在目标机上补 | §7、§12 |
| 发布包（Linux）实跑 | 打包链路**已验证**，Linux 产物**已产出未装机** | `pack-release.sh` 的构建→打包→摘要链路在开发机与 CI（run 41270406760，双 musl）均跑通；产物为 musl 静态、过 `readelf` 自检，**但尚未在目标机实跑安装** | §5.1 |
| 多架构发布包（arm64） | CI 已出包，**装机未实测** | `CARGO=cross sh deploy/pack-release.sh --target aarch64-...` 已在 run 41270406760 跑通；目标机实跑待补 | §12 |
| 1Panel 商店上架 | 包结构就绪，**未上架** | `xtask check-1panel` 只校验变量对齐 | §5.4 |
| 插件配置的管理界面 | **未实现**（`config_schema` 渲染表单没有） | 只能用 `skiff plugin config <id> <key> <value>` 写；插件侧 `Plugin::config()` 与内核 `capability/config` 路由已通 | §5.2 |

**写文档/对外宣讲时的口径**：上表任意一项都不能说成“已支持”。要么写“已实现”，要么写“设计目标/未实现”。

## 9. 实测记录

### 9.1 开发机（Windows 11 + rustc 1.98.1，MSYS/Git Bash）

只覆盖编译、单测与 HTTP 冒烟；Linux 专属路径（setuid 降权、Unix socket、docker.sock、systemd 安装）在真机上才可测。

| 项 | 命令 | 结果 |
|---|---|---|
| 工作区编译 | `cargo check --workspace` | 通过：0 error、0 warning |
| 全量单测 | `cargo test --workspace` | **215 项；Linux 目标机待复跑**。Windows 复跑为 212 passed / 3 failed（3 项为既存 `remove_dir_all` 文件锁限制，见 §9.4） |
| Linux 目标类型检查（纯 Rust 部分） | `cargo check -p skiff-model -p skiff-policy -p skiff-wire --target x86_64-unknown-linux-gnu` | 通过（0 warning）；含 C 依赖的 crate 无法在 Windows 上为该目标编译 |
| 上架与包校验 | `cargo run -p xtask -- check` | 商店包变量 ↔ formFields 对齐；Compose 样例字段全在 MVP 子集内 |
| 二进制构建（release） | `cargo build --release -p skiff` | 通过；**6.65 MB**（Windows PE） |
| 插件脚手架全链路 | `skiff-plugin new/build/pack/verify` | 通过：demo 插件编出 1.45MB 二进制，包 461KB，验签通过 |

逐包用例数（合计 215；Windows 本机 3 项文件锁红，Linux 目标机待复跑）：

| 包 | 用例数 | 覆盖内容 |
|---|---|---|
| `skiff`（core） | 18 | CLI 解析（含 `plugin config`）、配置校验矩阵（非 loopback 明文拒绝、可信反代白名单、非法 CIDR 拒启）、启动装配 |
| `skiff-model` | 28 | 角色序、grant kind、Form 拼写、Permission 映射、CIDR 解析与拒绝、路径代数 |
| `skiff-policy` | 18 | 三层策略、受保护对象（含改状态动词）、bind 授权根、项目名规范化、路径校验 |
| `skiff-store` | 10 | 迁移幂等、用户/会话续期、API Token 联表、TOFU 首用、插件配置 secret、备份快照 |
| `skiff-identity` | 11 | argon2id、TOTP RFC 6238 向量、RBAC、Cookie 名切换、备份码 |
| `skiff-pkgfmt` | 24 | 签名输入编码、篡改载荷拦截、未签名/缺架构拒装、staging 清理、**打包只收载荷目录** |
| `skiff-plugin` | 24 | 版本比较、UID 分配与范围、IPC 帧上限、TOFU 矩阵、安装/沙箱降级、fsgrant |
| `skiff-wire` | 9 | 帧编解码、请求头解析、RPC 封装 |
| `skiff-web` | 30 | gzip 协商与变体、CatchPanic→500+审计、表单/CSRF/CIDR/转义/URI 编码 + `tests/main_flow.rs` 主干集成 |
| `skiff-docker` | 10 | 创建参数只映射白名单字段、Docker 不可用降级、保护对象标签、env 排序可复现 |
| `skiff-sdk` | 5 | SDK 侧 grant 查询与 JSON-RPC 错误码；插件 id 取末段、crate 名合法性 |
| `skiff-compose` | 6 | MVP 子集解析、未支持字段报错、依赖成环、项目名规范化 |
| `skiff-app` | 14 | 登录用例、限流表有界、exec 票据（单次/过期/跨会话/跨容器） |
| `skiff-cron` | 8 | 5 段表达式解析/匹配/下次触发（含闰年与周字段） |

### 9.2 HTTP 冒烟（真实二进制，非 mock）

```sh
SKIFF_DATA_DIR=./tmp-data SKIFF_RUN_DIR=./tmp-run \
  ./target/release/skiff --dev --config ./tmp-data/none.toml serve
```

| 步骤 | 观测结果 | 结论 |
|---|---|---|
| 启动 | 日志打印监听地址、数据根、安全入口、登录地址；并 WARN「Docker 不可用 / 明文开发模式 / allow_reduced_sandbox=false」 | 通过 |
| `GET /healthz` | `200`，body `{"docker":"unavailable","status":"ok","uptime_secs":9,"version":"0.1.0"}` | 通过 |
| 首启产物 | `skiff.db`（含 WAL）、`plugins/`、`logs/`、`backup/`、`initial-admin.txt`（0600） | 通过 |
| `GET /`（不带前缀） | `404` | 通过（入口守卫） |
| `GET /{prefix}/`（带尾斜杠） | `307 → /{prefix}` | 通过 |
| `GET /{prefix}/login` | `200`；下发 `skiff_login` Cookie（HttpOnly/SameSite=Strict）+ CSP 等安全头 | 通过 |
| `POST /{prefix}/login`（正确口令，默认口径） | `303 → /{prefix}`（概览），下发 `skiff_dev_session` | 通过（默认不强制改密；显式开启时才 303 → `settings?force=1`） |
| 未登录访问 `/{prefix}/containers?a=1&b=2` | `303 → /{prefix}/login?next=%2F{prefix}%2Fcontainers%3Fa%3D1%26b%3D2` | 通过（percent-encode，不再被 HTML 转义污染） |
| 改密后各页面 | `/{prefix}`、`/containers`、`/plugins`、`/audit` 全 `200`；容器页显示「Docker 不可用」空态 | 通过（优雅降级） |
| `GET /api/v1/docker/containers`（无/错 token） | `401` | 通过 |

### 9.3 发布打包链路（开发机，产物不可发布）

```sh
sh deploy/pack-release.sh
```

| 步骤 | 观测 | 结论 |
|---|---|---|
| 打包 | `dist/skiff-0.1.0-linux-amd64.tar.gz` 内含 `skiff` + `skiff-plugin` + `install.sh` / `uninstall.sh` / `docker-bootstrap.sh` / `skiff.service` / `config.toml.example` | 通过（2026-10-31 起含插件工具） |
| 摘要 | `sha256sum -c` → `OK` | 通过 |
| 确定性 | 同一输入重复打包，`sha256` 前后一致 | 通过（`--sort=name --mtime` 生效） |

> 发布物签名（`.sig` / `verify-file`）已于 2026-11-01 移除（§11 决策 12）。移除前该链路
> 曾实测通过（正/错公钥、篡改包结论均正确，2026-10-30），历史记录见 `docs/changes/`；
> 验签不再是交付要求。

**这批产物不能发布**：`uname -s` 是 MSYS，包内是 `skiff.exe`。它只证明脚本链路可用；正式包必须在 Linux 上重跑。

### 9.4 上机就绪复查（2026-10-31）：发现并修掉的 9 个问题

复查手段：一是读 `#[cfg(unix)]` 分支的代码，二是给 `x86_64-unknown-linux-gnu` 装了 std 后对纯 Rust crate 做类型检查。
前三个是**编译级**错误，后六个是**运行级**缺陷——它们都不会在 Windows 上暴露。

| # | 问题 | 后果 | 处理 |
|---|---|---|---|
| 1 | `plugin/src/supervisor.rs` 调用了不存在的 `self.socket_path()` | Linux 上**编不过**（`spawn` 整段是 `#[cfg(unix)]`，Windows 下被排除） | 补 `Supervisor::socket_path()`：`/run/skiff/plugins/{id}/plugin.sock`，与 SDK 收到的环境变量同源 |
| 2 | `skiff-policy` 用 `libc::open/openat` 但 Cargo.toml 没声明 `libc` | Linux 上**编不过** | 加 `libc` 依赖（仅 `target_os = "linux"` 分支用到） |
| 3 | `skiff-sdk` 的 unix 分支用了 `Read`/`Write` trait 却未 import | Linux 上**编不过** | 加 `#[cfg(unix)] use std::io::{Read, Write};` |
| 4 | 数据根与 `plugins/` 是 0700 root | 降权到 200xxx 的插件**进不去自己的 data/**（ancestor 缺 x 位），所有插件一启动就 EACCES | 改 0711（可穿越、不可列）：`bootstrap.rs`、`install.sh`、`skiff.service` 三处同步 |
| 5 | `kernel.sock` 是 0660 **root:root** | 插件主 gid 是它自己的 uid，没有组权限 → 所有能力调用 EACCES，且错误发生在插件侧 | 启动时 `chown` 到 `root:skiff-plugins`；插件降权时 `setgroups([skiff-plugins])` 带附加组；组不存在时退化 0666 并告警（鉴权仍靠 peer uid + token） |
| 6 | 每次启动 `clean_run_dirs` 重建 `run/` 后属主变 root | 重启一次插件就写不进自己的 run/（「昨天还好，今天启动就报权限错」） | 重建后 `chmod 0700` + `chown` 回插件 UID |
| 7 | 内核 `fs.write` 以 root 落盘 | 文件属主 root，插件随后直接改同一个文件被自己的权限挡住 | 写后归还属主给插件 UID（0600） |
| 8 | 插件 UID 由 `sha256(id)%1000` 分配，撞车无检测 | 两个插件共用同一文件权限身份，`data/` 互相可读，隔离假成立 | 安装前检测冲突并拒绝，给出明确原因 |
| 9 | `pack_dir` 整目录打包 | **签名私钥 `skiff-signing.key` 被打进分发包**，同时把 `target/`（上百 MB）也塞进去：实测 28.9MB | 只收 `bin/ ui/ migrations/` + manifest；实测 28.9MB → **461KB**，并加回归用例 |

顺带修掉的两处：

- `pack-release.sh` 不带 `skiff-plugin`，目标机上无法打包/验签（E 组整组跑不了）→ 一并构建并安装。
- `skiff-plugin new` 把 SDK 写成 `skiff-sdk = "0.1"`（从未发布到 crates.io）→ 新增 `--sdk-path` / `SKIFF_SDK_PATH`，并自动识别仓库内 `crates/sdk`。

文档层面还纠正了一处**不实描述**：原文写「路径校验是 `canonicalize` + `openat(O_NOFOLLOW)` 逐段打开」，
实际 `open_dir_nofollow` 写好但**从未被调用**，TOCTOU 窗口仍在。已在 §1、§8、D8 三处改为如实表述。

### 9.5 仍未验证（明确留下）

- **已测掉一部分**（2026-11-03 首装轮，Debian 13 / 内核 6.12）：systemd 安装与 enable、`kernel.sock` 组权限、`/run/skiff` 0711、
  首启产物与权限位、安全入口 303 与裸 `/` 404、`healthz`、`doctor`、`kill -9` 拉起、首登强制改密与改密后旧会话失效、
  登录限流 503 文案、Docker CE 安装链路与 `--check-only` 不重装。逐条结果见
  [`docs/ITERATION.md`](ITERATION.md) §9 与 [`docs/changes/2026-11-03.md`](changes/2026-11-03.md)。
- **仍未测**：docker.sock 实际管理动作（测试机无镜像，容器起不来）、备份恢复演练、N-1 升级窗口、
  C 组反代路径（可信代理正向采信 `X-Forwarded-Proto/For`、WS Origin）、HSTS（代码未实现）、
  资源数字（RSS / CPU / 二进制体积）、与 1Panel 共存。
- 含 C 依赖 crate 的 Linux **运行**仍只能靠 CI 的 `test` job；本地只能做到 `cargo check --target`（不链接）。

### 9.6 覆盖轮（2026-11-03，逐项）

| 面 | 结论 | 关键证据 |
|---|---|---|
| 装机与运行 | 安装 / 幂等重装 / 卸载 / 重启自愈 / 权限口径 / `deny_unknown_fields` 全部实测通过 | `install.sh` 0.9 s、卸载保留数据、重启后 `/healthz` 200；`bogus_key` 被拒（`serve` 退出 1） |
| 认证会话 | 首登强制改密、改密后旧会话失效、限流、CSRF、伪造转发头全部通过 | 改密后旧 Bearer 401；第 6 次登录 503「请 299 秒后重试」；`X-Forwarded-For` 伪造不影响审计源地址 |
| Docker 管理 | **部分**：非法动词 400、不存在容器 404、列表一致；**生命周期/资源限制/Compose 运行时未测** | 机上无镜像；Web 无 create/remove 路由 |
| 内网明文直连 | 监听、防火墙、直连、重启持久化、重装不丢 全部实测通过 | `ss` = `0.0.0.0:18080`；开发机 `/login` 200、错前缀 404、根 307、`/healthz` 200；重启后规则与监听仍在 |
| 同端口 TLS（自签） | 证书 SAN/权限、18080 仅 HTTPS、cookie-jar 登录与改密、重启与重装保留 全部实测通过；**HSTS 未实现** | `https` `/login` 200、`/healthz` 200、错前缀 404；`http` 同端口 `000`；`skiff_login` 与 `__Host-skiff_session` 均 `Secure`；改密 303 → 首页 200；重装前后 `config.toml` sha256 相同 |
| 官方插件导入与 UI（2026-11-03，历史） | 快照内 0 插件/0 公钥 → 导入官方公钥后 5 包安装成功（signed/enabled/running；nginx 已于 2026-11-04 内核化，现为 4 包）；列表页 200、导航 5 项、详情页 200、托管 UI 200 + CSP；nginx RPC 站点增删读、备份、定时备份成功 | `skiff plugin list` 5 行（现为 4 行）；`official/` 已落盘；nginx 写 `/etc/nginx/conf.d` 需 `ReadWritePaths` drop-in；`reload` 因无容器 400（已修复 `target:null` 授权误判） |
| fs 能力面 | 路径授权代数、越界拒绝、256 KiB 分页、覆盖写通过 | 越界 400、`page_max=262144`、300 KiB 写 400 |
| 迭代包自检 | 旧包（main@41beba5）静态链接、sha256、8 文件布局通过；本轮改动未出包 | `ldd skiff` 非动态可执行；sha `5ed37d21…`，8.52 MB |
| 网站寄宿迁 `/www`（2026-11-04） | 数据根搬 `/www/skiff` 与 `/www/sites`、旧目录零写入；nginx/php 重建为 host 网络 + unix socket；OpenResty 固定 digest 拉取；默认站/兜底/站点 vhost 全部上盘；日志落宿主且面板可读 全部实测通过 | 旧 `skiff.db` mtime 停在迁移前 10:17:35；`docker inspect` = `NetworkMode=host`、`LogConfig local max-size=10m`；`ss` = `*:80`；裸访问 200 + `PHP Version 8.5`；`Host: demo.local` 200；`/www/logs/nginx/demo.local.access.log` 持续增长；socket `660 root:nogroup`，其他 uid 连接 `Permission denied`；80 被 `python3 -m http.server` 占用时安装步骤 0 失败并报 `pid … python3`，释放后重装恢复 200 |

### 9.7 闭环轮（2026-11-04，逐项）

| 面 | 结论 | 关键证据 |
|---|---|---|
| 站点 HTTPS | 自签生成 / 上传 PEM / 强制跳转 / 关闭删除 / `nginx -t` 回滚 全部实测通过 | `curl -k https://10.10.15.4/` **200** 且含 `PHP Version 8.5.12`；`key.pem` **0600**；`http://` **301** → `https://`；`nginx -t` syntax ok 无 warning；坏 conf 注入后接口报错、conf md5 与库开关不变 |
| 镜像源落盘 | 合并 / 备份 / JSON 校验 / 条件重启 / 失败回滚 全部实测通过 | 有容器时只写盘 + 提示；停容器后 `systemctl restart docker`，`docker pull hello-world` 成功，`daemon.json` 保留其它键、备份文件存在；重启 docker 后面板容器页仍 200 |
| musl 静态包 | 主二进制与 4 个插件包均产出并试跑通过 | `file` = `static-pie linked`、`ldd` = not a dynamic executable；`skiff` 9,404,208 B、sha `6595a1f8…`；临时数据目录 `/<入口>/login` **200**；现网 glibc 二进制 sha 未变 |
| 重启自恢复 | `skiff` / `docker` / 防火墙 / 网站容器 / HTTPS 站点全部自动恢复 | 重启命令 → HTTPS 200 用时 **11.2 s**；开机 → 首个外部 HTTPS 200 约 **6 s**（access log 时间戳）；`systemctl is-active` 三项 active；`iptables -S INPUT` 规则仍在；`docker info` 镜像源仍在 |

### 9.8 round4/round5 部署（2026-11-04，逐项）

| 面 | 结论 | 关键证据 |
|---|---|---|
| 静态资源两种路径 | 根路径与前缀别名均匿名可取 | `/static/skiff.js` 与 `/{p}/static/skiff.js` 均 **200**；`main_flow::prefixed_static_assets_are_served` 对拍 |
| 站点列表列宽合并 | 单表、无独立域名列 | `/swn4nbhe/sites` **200**；`sites-grid` 表计数 **1**；无 `<th>域名</th>` / `col-domain` |
| 插件 UI 主题覆盖 | 5 个 HTML 均带浅色兜底，同目录 JS 均发主题请求 | `index.html`×4 + `health.html` 含 `data-theme="light"`；`app.js`×4 + `health.js` 含 `skiff.theme.request`；5 个托管页 **200** |
| 页面回归 | 概览/插件/菜单设置/深链全部 200 | 4 个插件详情与 `/m/overview`、`/m/core/sites/list`、`/m/core/plugins/installed`、`/m/core/docker/containers`、`/m/core/settings/menu`、`/m/plugin/<id>` 均 **200** |
| 部署态 | 二进制 round4 → round5，服务稳定 | round4 sha `8f74fabe…` 9,272,296 B；round5 sha `da544b5f…` 9,271,912 B；备份 `20261104-202700-round4` / `20261104-203455-round5`；`/var/lib/skiff` 不存在 |
| 深色主题下 iframe 内部 | 未验证（curl 无法执行脚本） | 留待真实浏览器复测 |

### 9.9 round6 部署（2026-11-04，方案 A + 前缀收紧，逐项）

| 面 | 结论 | 关键证据 |
|---|---|---|
| 前缀资源 | 带前缀全部可取，根路径全部 404 | `/{p}/static/{skiff.css,skiff.js,theme-init.js,htmx.min.js,xterm.min.js}` 均 **200**；`/static/skiff.js`、`/static/skiff.css`、`/plugins/<id>/ui` 均 **404** |
| 页面引用 | 无坏链 | 登录页/概览/站点列表/插件详情/菜单设置抽 **33** 条同源 `src|href`，逐条 **200**；iframe src 带前缀且 **200** |
| A 方案结构 | 落地 | 概览含内联 SVG 精灵（31 个 `<svg>`）、5 个 `data-nav-toggle`；无字符图标；CSS 含 `--nav-row-h`/`--nav-icon`，`.subnav.open` 48px |
| 手风琴行为 | 浏览器实测通过 | caret 展开 `aria-expanded=true`；换组互斥；Escape 收起并回焦 |
| 证书列 | 与实际证书目录一致 | `/www/sites/demo.local/ssl/` 有 `cert.pem`+`key.pem`；列表「已启用 HTTPS」1 处、「未配置」0 处 |
| 回归 | 全绿 | 站点 `https://10.10.15.4/` **200** + `PHP Version 8.5.12`；4 插件 enabled；概览/插件/菜单设置与 6 条 `/m/...` 深链均 **200** |
| 部署态 | round5 → round6 | 二进制 sha `9b2158c9…` 9,291,368 B；备份 `20261104-214238-round6`；`/var/lib/skiff` 不存在 |

未闭环项统一登记在 [`docs/TEST_LEDGER.md`](TEST_LEDGER.md)。

### 9.10 2026-11-05 Release 覆盖安装：一站一目录（无迁移）+ 文件管理 0.1.2

Release `v0.1.0` 资产被同一 tag 覆盖（run `41945350367`，未新建 tag）；测试机按真实下载路径安装，不做旧布局迁移。

| 面 | 结论 | 关键证据 |
|---|---|---|
| Release 资产 | 覆盖成功 | x86_64 包 6,592,720 B `sha256:a294bc41…`；aarch64 包 6,301,191 B `sha256:e783c8ca…`；`checksums.txt` 223 B `sha256:6567ed24…`；包内 4 插件 + `public-key.txt`，`checksums.txt` 13 文件全 OK |
| 安装 | 真实路径 | 测试机直连 `github.com` 本轮 SSL EOF，改走 GitHub API 下载后 scp；同名 `.sha256`、包内 `checksums.txt`、`sh install.sh` 全部通过；`config.toml` 哈希不变，`must_change_password=0` |
| 站点新布局 | 新建即生成 | 面板新建 `demo.local` → `/www/sites/demo.local/{index,log,ssl,conf}` 与 `index/index.php`；日志 `log/access.log`；自签证书 `ssl/cert.pem|key.pem` |
| nginx | 通过 | `docker exec skiff-web-nginx nginx -t` 成功；`nginx -s reload` 只有 notice |
| 站点访问 | 200 | `https://10.10.15.4/` 200 含 `PHP Version 8.5.12`；`Host: demo.local` 200 |
| 默认站 | 符合预期 | 设默认 → 裸 IP 200；取消 → 404；恢复 → 200 |
| 详情四标签 | 200 且无旧路径 | 配置/日志/目录/SSL 均 200，旧路径文案计数 0；`../log`、`../ssl` 越界 **400** |
| 删除站点 | 整目录消失 | 删除后 `/www/sites` 空、`conf.d` 仅 fallback+include、库 `sites=0`/`site_aliases=0`、裸 IP/Host 均 404；`/www/logs/nginx`、`/www/certs` 不存在 |
| 文件管理 | 0.1.2 可用 | 旧 0.1.1 与 Release 官方密钥 key_id 不同，按 TOFU 守卫先 `plugin remove`（保留 data）再安装；`plugin list` = 0.1.2/running；RPC list/mkdir/write/read/rename/remove 全 OK，307,200 B 读 `eof=false`、`page_max=262144`，超限写与越界读 `400` |
| 插件 UI 浏览器 | 通过（Chromium） | 主题跟随 light↔dark、Ctrl 多选、右键菜单、行内重命名、编辑器保存、超限只读均实测；「新建文件夹」修复 `sandbox` 缺 `allow-forms` 后通过 |

插件 UI 地址：`<TEST_ENV §2 的基址>/<entry_prefix>/m/plugin/com.skiff.files/files`（2026-11-05 实测用 `https://10.10.15.4:18080/swn4nbhe`；当前值见 [TEST_ENV.md](TEST_ENV.md) §2）。

## 变更记录

| 日期 | 变更 |
|---|---|
| 2026-10-30 | 初版：随 MVP（M0–M4 代码面）交付，覆盖两种部署形态、7 组验收用例、未实现清单；补 Windows 冒烟记录与 4 个测试期缺陷 |
| 2026-10-30 | 补 release 构建实测与 §9.3 发布打包链路实测（三件套、确定性、正负验签）；修 2 处缺陷（`keygen --help` 落私钥、`pack-release.sh` 包名写错）；§1 内核行改为「不依赖 `openat2`」的实情 |
| 2026-10-31 | **上机就绪复查**：新增 §0 上机前闸门（编译/单测/结构）与三条取得发布物的路径、§1.1 权限口径表、§2.9 插件测试物准备、§2.10 权限链自检；修 9 个只会在 Linux 暴露的问题（3 个编译错误 + 6 个权限/打包缺陷，见 §9.4）；D8 改为「窗口仍在、不作阻断项」，§8 新增 TOCTOU / Linux 完整编译 / 备份一致性 / UID 槽位四项；测试 104 → 105 |
| 2026-10-31 | **同步第二轮执行（ARCH_REVIEW §10.4）后的实测值**：§9.1 全量单测 107 → **118**，逐包表补齐 `skiff-docker`（4）与 `skiff-compose`（6）并更新各包用例数（合计 118）；§0 闸门与 CI.md 的单测口径同步。测试链条：88 → 104 → 105 → 107 → 118 |
| 2026-11-03 | **覆盖轮**：§1.1 补 `versions/{ver}` 与 `bin/**` 的可穿越/可执行口径；§2.10 增加代码目录权限自检；§8 新增「插件代码目录可执行位（已修未复验）」与「Docker 生命周期未实测」两项；新增 §9.6 逐项结论 |
| 2026-11-03 | **内网直连**：新增 §2.4.1（明文直连配置、`trusted_proxies` 口径、Cookie/Secure 限制、`install.sh --listen`、iptables 白名单与持久化）；§9.6 增补实测行 |
| 2026-11-03 | **同端口 TLS**：§2.4 A 补自签证书步骤（SAN 必须含实际访问地址、私钥 0600 只放 `/etc/skiff/tls/`、自签仅限内网复核）；新增 §2.4.2（实测状态码、cookie-jar 登录/改密、指纹、重启与重装保留、HSTS 未实现、18081 说明）；C7 回填；§8 把原「明文浏览器登录不可用」替换为「HSTS 未实现」；§9.5/§9.6 同步 |
| 2026-11-03 | **官方插件落地**：新增 §2.11（空列表根因、导入公钥、5 包安装命令与授权、systemd `ProtectSystem` 缺口与 drop-in、UI/CSP/RPC 实测、`target:null` 内核缺陷修复）；§9.6 增补实测行 |
| 2026-11-04 | **网站寄宿迁 `/www`**：数据根改 `/www/skiff`、站点 `/www/sites`、vhost `/www/nginx/conf.d`、日志 `/www/logs/nginx`、socket `/www/php`；nginx/php 改 host 网络，镜像固定 OpenResty digest；防火墙 80/443 + 18080；§1.1 权限表与 §9.6 同步，迁移实测见本条 |
| 2026-11-04 | **闭环轮**：新增 §2.13 站点 HTTPS（证书目录/自签/上传/强制跳转/`nginx -t` 回滚）、§2.14 镜像源自动落盘、§0.2 路径 E（目标机 musl 静态包）、§9.7 逐项实测；同步 §9.6 插件由 5 包更正为 4 包 |
| 2026-11-04 | **round4/round5**：新增 §9.8；静态资源增加带安全入口前缀的别名路由 `/{p}/static/{*path}`（与根路径共用 handler），站点列表单表合并、插件 UI 主题兜底与 `skiff.theme.request` 实测通过；两轮备份与二进制 sha 记录在 §9.8 |
| 2026-11-04 | **round6（方案 A「舱线」+ 前缀收紧）**：新增 §9.9；根路径 `/static` 与 `/plugins/<id>/ui` 旁路删除后，33 条同源引用逐条 200、0 坏链；A 方案 SVG 精灵/48px 二级通栏与手风琴互斥/Escape 回焦浏览器实测通过；证书列按证书目录判定；二进制 sha `9b2158c9…`，备份 `20261104-214238-round6` |
| 2026-11-05 | **Release 覆盖安装**：新增 §9.10；覆盖 `v0.1.0` 资产（不新建 tag），测试机真实下载/校验/安装；不做旧布局迁移，新建 `demo.local` 即生成 `{index,log,ssl,conf}`，`nginx -t`/站点 200/默认站切换/四标签/越界 400/删除整目录均实测；文件管理 0.1.2 安装与 RPC/浏览器交互实测，修复插件 iframe `sandbox` 缺 `allow-forms` |
| 2026-11-08 | **端口/前缀/TLS 可配置 + 三态 Cookie**：§2.3 补「设置页修改监听端口/入口前缀」规则表、24h 旧前缀 302 防锁死、升级保持原入口；§2.4 改为 TLS 三态（`true`/`false`/不写）与三条正路 A/B/C，补 `install.sh --tls on/off` 映射与 `SKIFF_TLS_ENABLED`/`SKIFF_ENTRY_PREFIX` 环境变量、重启生效口径；§2.4.1 改写为显式 `tls_enabled=false` 可直接浏览器登录（`skiff_dev_session` 无 Secure）并写清明文安全边界。其余见 `docs/changes/2026-11-08.md` |
