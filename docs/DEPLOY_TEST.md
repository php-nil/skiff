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
| 2 单测 | `cargo test --workspace` | 149 项全 ok、0 failed | 同上；失败项按输出定位，别整体重跑蒙过去 |
| 3 结构 | `cargo run -p xtask -- check` | 结尾「全部通过 ✅」 | 商店包变量/Compose 字段对齐被改坏了 |

三道都过再往下走。只拿预编译二进制、机器上没有 Rust 的话，跳过这三闸，但**必须**做 §2.1 的三件套校验和 §2.10 的权限链自检。

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

### 0.2 三条取得发布物的路径

**路径 A：目标机就地编译（首测推荐）**

```sh
cd /root/skiff            # git clone 或 scp 源码
cargo check --workspace   # 闸 1
cargo test  --workspace   # 闸 2
cargo run -p xtask -- check   # 闸 3

# 出包：tar.gz + .sha256 + .sig 三件套
./target/debug/skiff-plugin keygen --out /root/release-test.key
sh deploy/pack-release.sh --key /root/release-test.key
ls -l dist/
```

`pack-release.sh` 会把 `skiff`、`skiff-plugin`、`install.sh`、`uninstall.sh`、`skiff.service`、`config.toml.example` 打进 `dist/skiff-<ver>-linux-<arch>.tar.gz`。
在非 Linux 主机上跑也能出包，但产物是 PE/ELF 混合的假包，脚本会明确警告——**那种包不能装到 Linux 上**。

**路径 B：Linux 构建机出包，传到目标机**

```sh
# 构建机
sh deploy/pack-release.sh --key release.key
scp dist/skiff-0.1.0-linux-amd64.tar.gz* root@<目标机>:/root/

# 目标机（无需 Rust）
cd /root && sha256sum -c skiff-0.1.0-linux-amd64.tar.gz.sha256
```

**路径 C：已经有发布物** → 直接进 §2.1 校验，不要跳。

**路径 D：GitHub Actions 出包（推了 `v*` 标签之后）**

```sh
# 目标机上：拿 Release 里的三件套
curl -LO https://github.com/<owner>/<repo>/releases/download/v0.1.0/skiff-0.1.0-linux-amd64.tar.gz
curl -LO .../skiff-0.1.0-linux-amd64.tar.gz.sha256
curl -LO .../skiff-0.1.0-linux-amd64.tar.gz.sig
sha256sum -c skiff-0.1.0-linux-amd64.tar.gz.sha256
```

CI 出的包与 `deploy/pack-release.sh` 手工出包**是同一份脚本、同一套布局**（CI 只是把
`CARGO` 换成 `cross` 并传入架构与版本号），所以 §2 的装机步骤不用改。
注意区分两种产物：`release.yml`（标签触发）给的是完整三件套；`ci.yml` 的 `build` job
只给裸二进制（**没有 `.sha256` / `.sig`**），仅用于验证"能在 musl 下静态编出来"，
不要拿它装机。签名密钥没配 `RELEASE_SIGNING_KEY_HEX` 时 Release 里的包是未签名的，
日志里会有 `::warning::`，别当成已签。见 [CI.md](CI.md) §4。

### 0.3 传到目标机的文件

| 文件 | 必需 | 用途 |
|---|---|---|
| `skiff-<ver>-linux-<arch>.tar.gz` | 是 | 主程序与安装脚本 |
| 同名 `.sha256` | 是 | 安装前校验，缺了等于没有发布物 |
| 同名 `.sig` | 正式发布必需 | detached ed25519 签名（hex），用 `skiff-plugin verify-file` 验 |
| 源码目录（路径 A） | 视情况 | 就地编译、以及给插件测试物编 demo |
| `demo-0.1.0.wbpkg`（构建机产物） | 视情况 | 目标机没 Rust 时，用它跑 E 组 |

## 1. 环境与前置条件

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
| `/var/lib/skiff` | 0711 | root:root | 可穿越、**不可列目录**。设成 0700 会让降权后的插件走不到自己的 `data/`（ancestor 缺 x 位 = 路径不可达） |
| `/var/lib/skiff/plugins` | 0711 | root:root | 同上 |
| `/var/lib/skiff/plugins/{id}` | 0755 | root:root | 目录名可见无妨，插件代码在下一层 |
| `/var/lib/skiff/plugins/{id}/versions` | 0755 | root:root | 代码目录**只读**：插件不得改自己的二进制 |
| `/var/lib/skiff/plugins/{id}/data` | 0700 | 插件 UID | 业务数据，只有该插件能进 |
| `/var/lib/skiff/plugins/{id}/cache` | 0700 | 插件 UID | 缓存 |
| `/var/lib/skiff/plugins/{id}/run` | 0700 | 插件 UID | pid/临时文件；**每次重启重建后要重新 chown** |
| `/var/lib/skiff/backup` | 0700 | root:root | 库快照 |
| `/var/lib/skiff/skiff.db` | 0600 | root:root | 插件读不到库（E8） |
| `/etc/skiff/config.toml` | 0644 | root:root | 不含密钥；TLS 私钥另放 `/etc/skiff/tls/` |
| `/run/skiff` | 0711 | root:skiff-plugins | 可穿越；socket 本身仍按组判权 |
| `/run/skiff/kernel.sock` | 0660 | root:skiff-plugins | 插件靠**附加组**进来，不是靠 umask |
| `/run/skiff/plugins/{id}/plugin.sock` | 0600 | 插件 UID | 只允许内核连这个插件的 RPC |

> 插件 UID 是 `sha256(id)` 取模分配的 200000–200999，**不是**系统 passwd 里的用户；`getent passwd` 查不到是正常的。

## 2. 形态 A：宿主机 systemd 直装（主形态）

### 2.1 取得发布物并校验

发布物三件套：`skiff-<ver>-linux-<arch>.tar.gz`、同名 `.sha256`、同名 `.sig`（detached ed25519，hex）。

**安装顺序固定：先验摘要与签名，失败立即停手。** 包内 `install.sh` 不校验包含自己的压缩包（自校验悖论）。

```sh
# 1) 摘要
sha256sum -c skiff-0.1.0-linux-amd64.tar.gz.sha256

# 2) 签名（用发布说明里给出的公钥 hex 与指纹，逐位核对）
skiff-plugin verify-file skiff-0.1.0-linux-amd64.tar.gz --pubkey <发布公钥hex>
```

没有 `skiff-plugin` 时，用任意一台装了 Rust 的机器 `cargo build --release -p skiff-sdk --bin skiff-plugin` 生成；
只有 `sha256sum` 时能验摘要、**不能**验签名——签名验不了就等于这包来源不明，别装。

### 2.2 解包安装

```sh
tar xzf skiff-0.1.0-linux-amd64.tar.gz
cd skiff-0.1.0-linux-amd64
sh install.sh
```

`install.sh` 做的事（幂等）：

| 步骤 | 结果 |
|---|---|
| 建运行组 | `skiff-plugins`（system 组，已存在则跳过） |
| 建数据目录 | `/var/lib/skiff` 0711 root:root，含 `plugins/ logs/ backup/` |
| 放示例配置 | `/etc/skiff/config.toml`（仅当不存在时） |
| 装二进制 | `/usr/local/bin/skiff`（先写临时文件再原子替换） |
| 装插件工具 | `/usr/local/bin/skiff-plugin`（包内有就装，用于打包/验签） |
| 装 unit | `/etc/systemd/system/skiff.service` + `daemon-reload` + `enable` |
| 启动 | `systemctl restart skiff`（`--no-start` 可跳过） |

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
  数据根    : /var/lib/skiff
  安全入口  : /a1b2c3d4
  配置      : /etc/skiff/config.toml
  登录地址  : http://127.0.0.1:18080/a1b2c3d4/login
  初始口令  : /var/lib/skiff/initial-admin.txt（用户 admin，首次登录后强制改密）
```

`grep` 关键字是「安全入口」：`journalctl -u skiff --no-pager | grep 安全入口`。
监听写成 `0.0.0.0` 时日志里的地址不能直接点，换成真实 IP/域名访问。

初始口令在 `/var/lib/skiff/initial-admin.txt`（0600），首登强制改密，改密成功后该文件自动删除。安全入口前缀首启随机生成并持久化到库（`settings.security.entry_prefix`）；**非该前缀的路径返回 404**（不是 403）。

### 2.4 打开 TLS（对外前必做）

默认监听 `127.0.0.1:18080`。对外有两条正路，选一条：

**A. 本进程终止 TLS**

```toml
listen   = "0.0.0.0:18080"
tls_cert = "/etc/skiff/tls/fullchain.pem"
tls_key  = "/etc/skiff/tls/privkey.pem"
```

证书文件须 root 可读、非全局可写。启动时校验文件存在性，缺失直接退出。

**B. 可信反代终止 TLS**

```toml
listen              = "127.0.0.1:18080"
allow_insecure_http = true          # 显式确认「明文只到本机/可信反代」
trusted_proxies     = ["127.0.0.1/32", "::1/128"]
external_url        = "https://panel.example.com"
```

规则（§6.2，代码里就是这么判的）：

| 监听 | TLS 配置 | allow_insecure_http | trusted_proxies | 结果 |
|---|---|---|---|---|
| loopback | 无 | false | 任意 | 拒绝启动 |
| loopback | 无 | true | 任意 | 启动，`dev_http` 模式（Cookie 降级为 `skiff_dev_session`） |
| 非 loopback | 有 | 任意 | 任意 | 启动，HTTPS |
| 非 loopback | 无 | true | **非空** | 启动，视为“反代已终止 TLS”，日志告警 |
| 非 loopback | 无 | true | 空 | **拒绝启动**（等于裸奔） |
| 非 loopback | 无 | false | 任意 | 拒绝启动 |

只有直连 peer 命中 `trusted_proxies` 才采信 `X-Forwarded-For/Proto/Host`，否则一律清除伪造头。

### 2.5 升级

```sh
systemctl stop skiff     # 停机备份最稳；不停也能备，但插件 data/ 可能被写一半（§6.6 F1）
skiff backup
sh install.sh --no-systemd      # 只换二进制
systemctl start skiff
systemctl status skiff --no-pager
```

数据目录不动；SQLite 迁移幂等、向前，启动自动执行并写 `PRAGMA user_version`。若库的 schema 版本高于二进制支持版本，启动会拒绝并报 `SchemaTooNew`——这是防降级写坏数据，不是 bug。

### 2.6 回滚

```sh
systemctl stop skiff
# 1) 换回旧二进制
install -m 0755 skiff-<旧版本>/skiff /usr/local/bin/skiff
# 2) 数据库若已被新版本迁移过，需要回滚备份（备份里带版本与校验和）
ls /var/lib/skiff/backup/
# 3) 用备份替换（停机状态下操作，禁止对活动库直接覆盖）
cp /var/lib/skiff/backup/skiff-<ts>.db /var/lib/skiff/skiff.db
rm -f /var/lib/skiff/skiff.db-wal /var/lib/skiff/skiff.db-shm
systemctl start skiff
```

**注意**：SQLite 迁移只向前，不提供降级脚本。跨 schema 版本回滚必须走备份；备份恢复演练属 M5，**本轮未实测**（§8）。

### 2.7 卸载

```sh
sh uninstall.sh            # 保留 /var/lib/skiff 与 /etc/skiff
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
| `skiff plugin install <pkg> --yes --grant docker:containers.list --resource container:abc` | 非交互安装；`--yes` 下权限**默认全拒**，必须逐条 `--grant` | 0 |
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

### 2.10 权限链自检（装完必查）

装完不查权限，插件测试会以「EACCES」「连不上」的形式随机失败，且报错发生在插件侧，很难反推。逐条跑：

```sh
# 目录与 socket
ls -ld /var/lib/skiff /var/lib/skiff/plugins /run/skiff
stat -c '%a %U:%G %n' /run/skiff/kernel.sock
# 期望：drwx--x--x root root …；660 root skiff-plugins

# 插件装好后，逐段看路径可达性
namei -l /var/lib/skiff/plugins/com.example.demo/data

# 插件跑起来后看它的真实身份（不是 passwd 用户，是哈希 UID）
pid=$(pgrep -f 'plugins/com.example.demo/versions' | head -1)
grep -E '^(Uid|Gid|Groups)' /proc/$pid/status
# 期望：Uid/Gid 都是 200xxx；Groups 里含 skiff-plugins 的 gid
getent group skiff-plugins | cut -d: -f3

# 主进程仍是 root（它要能 setuid）
ps -o user,group,args -p $(systemctl show -p MainPID --value skiff)
```

## 3. 形态 B：Docker 受限形态（1Panel 商店包，可选）

**先读能力面收缩表，不能接受就别用这个形态。**

| 能力 | 容器形态 | 后果 |
|---|---|---|
| docker | 可用 | 仍走动词/对象/参数三层策略 |
| identity | 可用 | 登录/会话/TOTP/RBAC/审计 |
| fs | 不可用 | 需要 fs 的插件装前拒装 |
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
| `SKIFF_DATA_DIR` | `data_dir` | `/var/lib/skiff` | 唯一数据根 |
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
| `SKIFF_EXTERNAL_URL` | `external_url` | 无 | 对外地址（绝对链接兜底） |
| `SKIFF_LOG_LEVEL` | — | `info` | tracing 过滤；排障用 `RUST_LOG` 风格，如 `debug,skiff_plugin=trace` |
| `SKIFF_CONFIG` | — | `/etc/skiff/config.toml` | 配置文件路径 |

> `login_concurrency` 只能写在配置文件里，**没有**对应环境变量。

## 6. 验收测试用例

记录方式：每条填「实测」「结论」两列；失败即阻断该里程碑，不要“先放过去”。角色列 `v/o/a` 依次为 viewer / operator / admin。

### 6.1 A 组：安装与启动

| 编号 | 前置 | 步骤 | 预期 | 实测 | 结论 |
|---|---|---|---|---|---|
| A1 | 干净机器 | 改一个字节后跑 `sha256sum -c` | 校验失败并退出 | | |
| A2 | 发布物 | `skiff-plugin verify-file` 用错误公钥 | 验签失败 | | |
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
| B2 | 首启 | 用 `initial-admin.txt` 口令登录 | 成功，强制跳转改密 | | |
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
| C7 | 配了 `tls_cert/tls_key` | `curl -I https://<host>:18080/healthz` | 200；HTTP 访问同端口被拒 | | |

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
| E8 | 已装插件 | 插件尝试读 `/var/lib/skiff/skiff.db` | 权限拒绝（独立 UID，库 0600 root） | | |
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
| F4 | F1 | 新环境恢复备份并登录 | 用户/插件/配置齐全（M5 项，本轮未做） | | |
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
| **装包报“已安装版本不低于包内版本”** | 版本号没升 | 改 manifest 的 `version`；升级只接受更高版本 |
| **`pack` 报“指纹不一致”** | `--key` 用的私钥与 manifest.key_id 不是一对 | 用 `new` 生成的那把 `<dir>/skiff-signing.key`，或改 manifest.key_id |
| **`skiff-plugin new` 后 `build` 找不到 skiff-sdk** | SDK 未发布到 crates.io，脚手架写成了版本依赖 | 加 `--sdk-path <仓库>/crates/sdk` 重新生成，或手改 Cargo.toml 为 path 依赖 |
| **编译报 `x86_64-linux-gnu-gcc not found`** | 缺 C 工具链 | `apt install build-essential pkg-config` |
| **systemd 起不来，手跑却正常** | `ExecStart` 路径与实际安装前缀不一致（用了 `--prefix`），或 `ProtectSystem=full` 让某路径只读 | `systemctl cat skiff` 核对路径；需要写 `/etc` 的功能改到 `/var/lib` 或调 unit |
| 反代后 Cookie 不生效 | scheme 判定为非 HTTPS，`__Host-` 前缀被浏览器拒收 | 确认 `trusted_proxies` 命中且反代传 `X-Forwarded-Proto: https` |
| 表单提交 403 | CSRF 令牌与当前会话不匹配（多标签/会话续期） | 刷新页面重取令牌 |

## 8. 未实现 / 未验证清单（不得当事实引用）

| 项 | 现状 | 影响 | 对应 §12 |
|---|---|---|---|
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
| 发布包（Linux）实跑 | 打包链路**已验证**，Linux 产物**未产出** | `pack-release.sh` 的构建→打包→摘要→签名→验签链路已在开发机跑通（§9.3）；但产物是 Windows 二进制，**不是可发布物** | §5.1 |
| 多架构发布包（arm64） | 脚本支持，**未实测** | `deploy/pack-release.sh --target aarch64-...` 未跑过 | §12 |
| 1Panel 商店上架 | 包结构就绪，**未上架** | `xtask check-1panel` 只校验变量对齐 | §5.4 |
| 插件配置的管理界面 | **未实现**（`config_schema` 渲染表单没有） | 只能用 `skiff plugin config <id> <key> <value>` 写；插件侧 `Plugin::config()` 与内核 `capability/config` 路由已通 | §5.2 |

**写文档/对外宣讲时的口径**：上表任意一项都不能说成“已支持”。要么写“已实现”，要么写“设计目标/未实现”。

## 9. 本轮实测记录

### 9.1 开发机（Windows 11 + rustc 1.98.1，MSYS/Git Bash）

只覆盖编译、单测与 HTTP 冒烟；Linux 专属路径（setuid 降权、Unix socket、docker.sock、systemd 安装）在真机上才可测。

| 项 | 命令 | 结果 |
|---|---|---|
| 工作区编译 | `cargo check --workspace` | 通过：0 error、0 warning |
| 全量单测 | `cargo test --workspace` | **118 passed / 0 failed**（P0/P1/P2 全执行后的最近一次） |
| Linux 目标类型检查（纯 Rust 部分） | `cargo check -p skiff-model -p skiff-policy -p skiff-wire --target x86_64-unknown-linux-gnu` | 通过（0 warning）；含 C 依赖的 crate 无法在 Windows 上为该目标编译 |
| 上架与包校验 | `cargo run -p xtask -- check` | 商店包变量 ↔ formFields 对齐；Compose 样例字段全在 MVP 子集内 |
| 二进制构建（release） | `cargo build --release -p skiff` | 通过；**6.65 MB**（Windows PE） |
| 插件脚手架全链路 | `skiff-plugin new/build/pack/verify` | 通过：demo 插件编出 1.45MB 二进制，包 461KB，验签通过 |

逐包用例数：

| 包 | 用例数 | 覆盖内容 |
|---|---|---|
| `skiff`（core） | 10 | CLI 解析（含 `plugin config`）、配置校验矩阵（非 loopback 明文拒绝、可信反代白名单、非法 CIDR 拒启） |
| `skiff-model` | 8 | 角色序、grant kind、Form 拼写、Permission 映射、CIDR 解析与拒绝 |
| `skiff-policy` | 20 | 三层策略、受保护对象（含改状态动词）、bind 授权根、项目名规范化、路径校验 |
| `skiff-store` | 10 | 迁移幂等、用户/会话续期、API Token 联表、TOFU 首用、插件配置 secret、备份快照 |
| `skiff-identity` | 11 | argon2id、TOTP RFC 6238 向量、RBAC、Cookie 名切换、备份码 |
| `skiff-pkgfmt` | 19 | 签名输入编码、篡改载荷拦截、未签名/缺架构拒装、staging 清理、**打包只收载荷目录** |
| `skiff-plugin` | 6 | 版本比较、UID 分配与范围、IPC 帧上限、TOFU 矩阵 |
| `skiff-wire` | 6 | 帧编解码、请求头解析、RPC 封装 |
| `skiff-web` | 14 | 8 单元（gzip 协商与变体、CatchPanic→500+审计、表单/CSRF/CIDR/转义/URI 编码）+ 6 集成（`tests/main_flow.rs` 主干） |
| `skiff-docker` | 4 | 创建参数只映射白名单字段、Docker 不可用降级、保护对象标签、env 排序可复现 |
| `skiff-sdk` | 4 | SDK 侧 grant 查询与 JSON-RPC 错误码；`skiff-plugin` 的插件 id 取末段、crate 名合法性 |
| `skiff-compose` | 6 | MVP 子集解析、未支持字段报错、依赖成环、项目名规范化 |

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
| `POST /{prefix}/login`（正确口令） | `303 → /{prefix}/settings?force=1`，下发 `skiff_dev_session` | 通过（首登强制改密） |
| 未登录访问 `/{prefix}/containers?a=1&b=2` | `303 → /{prefix}/login?next=%2F{prefix}%2Fcontainers%3Fa%3D1%26b%3D2` | 通过（percent-encode，不再被 HTML 转义污染） |
| 改密后各页面 | `/{prefix}`、`/containers`、`/plugins`、`/audit` 全 `200`；容器页显示「Docker 不可用」空态 | 通过（优雅降级） |
| `GET /api/v1/docker/containers`（无/错 token） | `401` | 通过 |

### 9.3 发布打包链路（开发机，产物不可发布）

```sh
./target/release/skiff-plugin keygen --out dist/release-test.key   # 仅测试用
sh deploy/pack-release.sh --key dist/release-test.key
```

| 步骤 | 观测 | 结论 |
|---|---|---|
| 打包 | `dist/skiff-0.1.0-linux-amd64.tar.gz` 内含 `skiff` + `skiff-plugin` + 4 个安装脚本/样例 | 通过（2026-10-31 起含插件工具） |
| 摘要 | `sha256sum -c` → `OK` | 通过 |
| 确定性 | 同一输入重复打包，`sha256` 前后一致 | 通过（`--sort=name --mtime` 生效） |
| 验签（正确公钥） | `✓ 验签通过`，退出码 0 | 通过 |
| 验签（错误公钥 / 追加 1 字节） | 失败，退出码 **1** | 通过（不是「警告后继续」） |

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

- 全部 Linux 运行期行为：setuid 链、`SO_PEERCRED`、`kernel.sock` 组权限、systemd 安装、docker.sock 管理。**这一轮的目标就是把它们测掉**。
- 含 C 依赖 crate 的 Linux 编译（`plugin` / `sdk` / `core` / `web` / `store` / `docker`）：只能在目标机 `cargo check --workspace` 时确认。
- 资源数字（RSS、CPU、Linux 二进制体积）、备份恢复演练、与 1Panel 共存。

## 变更记录

| 日期 | 变更 |
|---|---|
| 2026-10-30 | 初版：随 MVP（M0–M4 代码面）交付，覆盖两种部署形态、7 组验收用例、未实现清单；补 Windows 冒烟记录与 4 个测试期缺陷 |
| 2026-10-30 | 补 release 构建实测与 §9.3 发布打包链路实测（三件套、确定性、正负验签）；修 2 处缺陷（`keygen --help` 落私钥、`pack-release.sh` 包名写错）；§1 内核行改为「不依赖 `openat2`」的实情 |
| 2026-10-31 | **上机就绪复查**：新增 §0 上机前闸门（编译/单测/结构）与三条取得发布物的路径、§1.1 权限口径表、§2.9 插件测试物准备、§2.10 权限链自检；修 9 个只会在 Linux 暴露的问题（3 个编译错误 + 6 个权限/打包缺陷，见 §9.4）；D8 改为「窗口仍在、不作阻断项」，§8 新增 TOCTOU / Linux 完整编译 / 备份一致性 / UID 槽位四项；测试 104 → 105 |
| 2026-10-31 | **同步第二轮执行（ARCH_REVIEW §10.4）后的实测值**：§9.1 全量单测 107 → **118**，逐包表补齐 `skiff-docker`（4）与 `skiff-compose`（6）并更新各包用例数（合计 118）；§0 闸门与 CI.md 的单测口径同步。测试链条：88 → 104 → 105 → 107 → 118 |
