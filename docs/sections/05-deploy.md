# 5. 部署：systemd 直装（主）+ 可选 Docker 包

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：§5 ｜ 上一章：[§4 扩展体系（二）](04-plugin-publish.md) ｜ 下一章：[§6 安全](06-security.md)

## 5.1 主形态：宿主机 systemd 直装

```
/usr/local/bin/skiff           # 单二进制（amd64/arm64）
/etc/skiff/config.toml         # 可选配置文件
/var/lib/skiff/                # 唯一数据根 SKIFF_DATA_DIR
  skiff.db / plugins/ / logs/ / initial-admin.txt / backup/
/run/skiff/                    # root:skiff-plugins 0711；kernel.sock 0660；plugins/{id}/ 0700
skiff.service                  # systemd unit：Type=simple, Restart=always
```

- **交付物**：多架构 tar.gz（二进制 + `install.sh` + `docker-bootstrap.sh` + `skiff.service` + `uninstall.sh`）+ detached `SHA-256` 摘要（**不做签名**，§11 决策 12：2026-11-01 起发布物只验摘要）。**安装顺序固定**：先校验 tarball 摘要，校验失败立即退出；再解包并执行包内 `install.sh`。包内脚本不负责校验包含自身的压缩包，避免自校验悖论。
- **install.sh**（摘要校验后执行）：创建 `skiff-plugins` 运行组与数据目录（数据根与 `plugins/` 均为 **0711 root**）→ 装 unit → `systemctl enable --now` → 打印 `initial-admin.txt` 路径。
- **目录为什么不是 0700**：插件降权到独立 UID 后，要打开自己的 `data/` 必须能穿越所有祖先目录（缺 x 位即不可达）。0711 表示「可穿越、不可列目录」，配合 `skiff.db` 0600 / `backup/` 0700，「看不见、打不开」两个目标都成立；写成 0700 会让插件一启动就 EACCES。同理 `/run/skiff` 取 0711，`kernel.sock` 仍是 0660 root:skiff-plugins，插件靠**附加组** `skiff-plugins` 拿到组权限（`setgroups` 在 `setgid/setuid` 之前执行）。
- **升级** = 换二进制 + `systemctl restart`；数据不动，SQLite 迁移**幂等、向前**，启动自动执行并写 schema 版本。
- **MVP 主形态内核固定以 root 运行**：需要创建独立插件 UID/GID、管理 nftables/cgroups，并访问 docker.sock；systemd unit 配独立插件运行组与 cgroup delegation。插件全部降权（§4.2）。不提供“专用管理 UID + docker 组”的半支持选项——它既已等价 root，又无法在未补 capabilities/目录属主规则前满足上述操作。
- **Docker 前置由安装脚本负责，不进内核**：`install.sh` 调包内 `docker-bootstrap.sh` 补装 Docker——**缺才装，已有可用 daemon 一律不动**。理由三条：① 内核是 musl 静态单二进制，把「下载远端脚本 + 调 apt/dnf/yum/apk」编进去要背 HTTP 客户端与数十种发行版适配，体积与攻击面都不划算；② 装完之后要能单独补跑（`sh docker-bootstrap.sh`），脚本比发版快；③ 必须发生在 `skiff.service` 启动**之前**——`DockerService` 只在进程启动时 connect 一次，先起面板再装 Docker 会一直停在「Docker 不可用」直到手动重启。
- **绝不重装已有的 Docker**：LinuxMirrors `docker.sh` 一旦发现已有 docker，会先 `systemctl disable --now docker` 再 `apt-get remove -y 'docker*'` 重装 CE。对一台正跑业务容器的机器，那是停机而不是安装。所以「有 docker 二进制但 daemon 没起来」时脚本只尝试拉起服务，拉不起来就退出 3 并提示，只有显式 `--force-reinstall` 才走重装。Alpine 不走该脚本（它只认 apt/dnf/yum），改 `apk add docker`。
- **网络与代理**：Docker CE 源默认 `mirrors.tencent.com/docker-ce`（`--docker-source` 可换）；镜像加速器**不设默认值**——国内公共加速器多数已停服，硬编码等于写死一个坏地址，要加速显式给 `--registry-mirror <host>`（可重复，脚本自动补 `https://`）。远端脚本先落盘、可用 `--docker-script-sha256` 固定摘要，**不用 `curl | bash`**：管道中断会执行半截脚本，也无法在中间插入校验。下载失败时打印备用地址（`edgeone.linuxmirrors.cn` / GitHub raw），不阻塞面板安装。
- **两种「代理」分开处理**：`--registry-mirror` 改的是 registry 地址（拉镜像快）；`--docker-proxy <url>` 改的是 daemon 进程出网方式（公司内网必须走代理时用，写 `/etc/systemd/system/docker.service.d/10-skiff-http-proxy.conf`）。后者只在新装 Docker 时自动写——已有 docker 的机器重启 daemon 会中断容器，脚本只打印配置片段让运维自己挑窗口。
- **加速器不覆盖用户配置**：已有 docker 且给了 `--registry-mirror` 时，默认只打印手工合并方式；`daemon.json` 可能含 `data-root`/`log-driver`/`insecure-registries`，整文件重写会丢掉它们。确认该文件只用于镜像加速时才 `--force-registry`（走 `docker.sh --only-registry`，由它自行备份）。
- **装不上不阻塞面板**：Docker 引导失败只 `warn`，安装继续——面板缺 Docker 时按 §3.3 优雅降级，只有 `docker` 能力面不可用，其余功能正常。

## 5.2 数据、配置与升级

- 唯一数据根 `/var/lib/skiff`：`skiff.db`、`plugins/`、`logs/`、`initial-admin.txt`、`backup/`。配置文件唯一规范路径是 `/etc/skiff/config.toml`，不在数据根重复维护；可选 Docker 形态以环境变量为主。

**插件存储分层**（`/var/lib/skiff/plugins/{id}/`；`versions/` 仅 root 写入、插件只读，`data/`/`cache/`/`run/` 插件可写；landlock 按层授权属设计目标，当前未实现）：

```
versions/   # 代码：root 写入、插件只读，N-1 回滚（§4.5）
data/       # 持久业务数据
cache/      # 可再生缓存
run/        # 运行时临时（锁/临时文件），重启即清
```

| 层 | 备份 | 卸载 | 清理 |
|---|---|---|---|
| `versions/` | 不备（源包归档另有） | 删 | — |
| `data/` | **必含** | 默认保留，勾选“删数据”才清 | 仅用户手动 |
| `cache/` | 不含 | 即删 | **随时可清**；软上限默认 200MB，超限告警不杀，占用计入插件状态页 |
| `run/` | 不含 | 即删 | 重启自动清 |

插件目录、配置与数据相关的其余约定：

- **插件配置两轨**：`config_schema` 声明项（含 `secret`）**内核代管于 `skiff.db`**——UI 统一渲染表单、卸载可导出；插件运行时经一次性 RPC 或继承 FD 取值，**secret 不写启动环境、不落插件可写目录**。插件管理的工作对象配置（如 Nginx 的 `nginx.conf`）属 `data/` 或被管理资源本身，不进 skiff.db。
- **配置优先级：环境变量 > `/etc/skiff/config.toml` > 内置默认**。
- **secret 配置项**：MVP 明文入 `skiff.db`（0600 root，插件降权后不可读，§4.2），运行时经一次性 RPC/继承 FD 交付插件、不落插件可写目录——**已知限制**，后续可加密落盘。
- **首启流程**：生成随机管理员密码写 `/var/lib/skiff/initial-admin.txt`（0600），首次登录强制改密，改密成功后删除该文件；安全入口随机前缀首启生成并持久化到库。
- **审计**：入 SQLite（操作者/IP/动作/结果/时间）；文件日志按大小轮转，保留 N 份（默认 5）。
- **备份**（升级前执行）：**当前实现不停止插件**——`VACUUM INTO` 保证库自身一致，但插件 `data/` 是直接复制，写入中的文件可能不一致；需要强一致时先停服再备（这是已实现的现状，不是本段原定的「先停插件再备份」）。方案目标仍是先停插件，未做到就写清楚，不产“成功”假象。SQLite 用 `VACUUM INTO /var/lib/skiff/backup/skiff-<ts>.db` 或 rusqlite backup API——**禁止对活动库直接 tar**（WAL 下拿到的是不一致快照）；插件停止后复制 `data/`，`cache/`/`run/` 不卷入。需要热备的后续插件可实现版本化 `backup`/`restore` RPC，但 MVP 不在线复制活动数据。备份目录 0700，记录版本与校验和，保留最近 3 份；M5 必须做真实恢复演练，备份成功不等于可恢复。

## 5.3 与 1Panel / 宝塔同机共存

- 不碰它们的容器、网络与数据目录；共用 `docker.sock` 时走 §6 三层策略（同一 Engine，各管各的）。
- 默认端口 18080 可配（避开 1Panel 默认 10086 / 宝塔 8888）。
- M3 验收含共存用例：面板自建容器不打 `createdBy: Apps` label，不污染 1Panel 托管视图。

## 5.4 可选形态：1Panel 商店 Docker 包（受限）

**受限声明**：该形态能力面收缩为 `docker`/`identity`（`fs`/`net` 不可用，需要它们的插件装前拒装）；插件在容器内运行。可承诺独立 UID/GID；**landlock / cgroups / project quota / nftables owner-match 均未实现**，因此该形态与主形态一样按 reduced sandbox 运行。`requires_full_sandbox=true` 的插件默认拒装，admin 显式开启 `allow_reduced_sandbox` 后才可降级安装；`network_egress` 的实际收口未实现，当前仅作 manifest 声明与展示（§4.2）。容器内 Supervisor fork 不出宿主进程——这是把它定位为受限形态的根本原因。

包结构（严格按官方规范）：

```
deploy/1panel/apps/skiff/
├── logo.png                    # 180×180，≤10KB（社区上架口径）
├── data.yml                    # 应用级声明
├── README.md                   # 中文说明
└── 0.1.0/                      # 版本目录，不带 v 前缀，与镜像 tag 一致
    ├── data.yml                # 版本级表单
    ├── docker-compose.yml
    ├── data/                   # 持久化目录（1Panel 映射到应用专属目录）
    └── scripts/{init.sh,upgrade.sh,uninstall.sh}
```

data.yml 字段要点（三源复核）：应用级 `key/name/tags/type/crossVersionUpdate/limit/recommend/website/github/document/memoryRequired/architectures`；版本级 formFields 字段集 `default/edit/disabled/envKey/key/labelEn/labelZh/required/type/random/rule/values`，`type: service|password|text|number|select`，`rule: paramPort|paramCommon|paramComplexity|paramExtUrl`。`type` 取 `tool`。`example.com` 为占位，发布前换真实地址。

**约束对齐要点**：
- **compose 里每个 `${VAR}` 必须在 formFields 有同名 `envKey`**，否则参数传不进去（最常见上架失败原因；`xtask` 用代码校验，§8）。
- `PANEL_APP_PORT_*` 前缀会被 1Panel 识别为端口并做占用校验，必须用这个前缀。`rule: paramPort` 限定 1–65535。

`docker-compose.yml` 要点：

```yaml
services:
  skiff:
    image: ghcr.io/example/skiff:0.1.0
    container_name: ${CONTAINER_NAME}
    restart: always
    ports:
      - ${PANEL_APP_PORT_HTTP}:18080
    environment:
      - TZ=${TZ}
      - SKIFF_LOG_LEVEL=${SKIFF_LOG_LEVEL}
      - SKIFF_DATA_DIR=/data
      - SKIFF_LISTEN=0.0.0.0:18080
    volumes:
      - ./data:/data
      - /var/run/docker.sock:/var/run/docker.sock
      - /etc/localtime:/etc/localtime:ro
    healthcheck:
      test: ["CMD", "/skiff", "health"]
      interval: 30s
      timeout: 3s
      retries: 3
      start_period: 10s
    labels:
      createdBy: "Apps"
    networks:
      - 1panel-network

networks:
  1panel-network:
    external: true
```

| 约定 | 本方案 | 说明 |
|---|---|---|
| `networks: 1panel-network (external)` | ✅ | 缺失则无法与同网络服务互通，且 1Panel 视其为非托管应用 |
| `container_name: ${CONTAINER_NAME}` | ✅ | 固定写法，1Panel 注入以保证多实例不冲突 |
| `labels: createdBy: "Apps"` | ✅ | 1Panel 识别“应用市场应用”的唯一标识 |
| `PANEL_APP_PORT_HTTP` | ✅ | 必须在版本级 data.yml 有同名 envKey |
| `./data` 相对挂载 | ✅ | 1Panel 映射到应用专属目录；**面板所有状态必须落在这里** |
| `restart: always` | ✅ | 商店惯例 |
| `architectures: [amd64, arm64]` | ✅ | 双架构镜像 |

**挂载最小化**：只挂 `docker.sock` 与 `./data`（+ 时区兜底）——不挂宿主任何目录；这里的 `./data` 仅用于面板自身持久化，不代表允许面板替用户创建宿主绑定挂载，后者按 §5.5 全部拒绝。docker.sock 等价宿主 root 的收敛见 §6。**镜像构成**：`rust:1-alpine` 构建 + `alpine:3.23` 运行（不用 distroless——排障要 shell；不用 3.20——2026-04-01 已 EOL），alpine ~8MB + 二进制 ~12MB + tzdata ~3MB ≈ 23MB（估算，实测回填）。

## 5.5 路径与卷校验（Docker 挂载）

Docker Engine 只认**宿主路径**（Portainer 等面板在此翻过车）。面板代用户创建 volume / 容器挂载时，路径授权与 Engine 下传必须使用同一种宿主语义：

- **主形态（native）**：绑定挂载只接受 admin 预先配置的宿主授权根。目标存在时 `canonicalize` 后校验仍在授权根内；目标不存在时规范化最近的已存在祖先并拒绝符号链接逃逸，随后按相对安全路径创建。禁止 `..`、绝对授权根外路径和检查—使用间的符号链接切换；授权根及路径各组件须由 root（或 skiff 进程属主，供非 root 开发/测试运行）管理且插件不可重命名/替换，不接受位于插件可写目录内的 bind 源。当前实现是 `canonicalize` + `openat(O_NOFOLLOW)` 的检查链；`open_dir_nofollow` 已实现但**尚未接入**判定链，检查—使用窗口仍在（§12.3），对外不得声称已消除。`openat2(RESOLVE_BENEATH)` 是后续目标，不是现状。
- **可选 Docker 形态**：面板容器看到的 `/data` 不等于 Engine 所在宿主的 `./data` 源路径，语法校验无法建立这个映射。因此该形态**关闭宿主绑定挂载**，Compose/容器 API 收到 bind 或带宿主绑定语义的 volume 一律 403；只允许通过 §6 检查的 named volume，禁止把面板容器内路径直接传给 Docker。
- 从 Docker 读到的 volume 宿主路径 → 展示给用户时保持原样，但“可展示”不代表“可挂载”；挂载仍须重新过对象与参数策略。

> 影响面：Compose/容器创建的挂载参数（风险 4、15）。转换层必须从一开始就写对，否则后面改不动。
