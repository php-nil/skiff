#!/bin/sh
# Skiff（轻舟）宿主机直装脚本（POSIX sh，方案 §5.1）
#
# 由发布包内执行，前提：外层已校验 tar.gz 的 SHA-256 摘要（§11 决策 12：发布物不签名）。
# 本脚本不负责校验包含自身的压缩包（自校验悖论，§5.1）。
#
# 用法: sh install.sh [--prefix /usr/local] [--datadir /www/skiff] [--webroot /www]
#                      [--listen <addr:port>] [--no-start] [--no-systemd]
#                      [--no-docker] [--docker-source <host>] [--registry-mirror <host>]...
set -eu

PREFIX=/usr/local
DATADIR_OVERRIDE=""
WEBROOT_OVERRIDE=""
LISTEN_OVERRIDE="${SKIFF_LISTEN:-}"
DO_START=1
DO_SYSTEMD=1
WITH_DOCKER=1
DOCKER_SOURCE="mirrors.tencent.com/docker-ce"
REGISTRY_MIRRORS=""
DOCKER_SCRIPT_SHA=""
DOCKER_PROXY=""

# 用 while 而不是 `for arg in "$@"`：后者在 POSIX 里进入循环前就固定了参数列表，
# 循环体里的 `shift 2` 只改位置参数、不影响遍历，于是 `--prefix /opt` 的第二轮
# 会把 `/opt` 当未知参数退出 2。带值选项必须用 while 才能正确跳位。
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) PREFIX="${2:?--prefix 需要参数}"; shift 2 ;;
        --prefix=*) PREFIX="${1#*=}"; shift ;;
        --datadir) DATADIR_OVERRIDE="${2:?--datadir 需要参数}"; shift 2 ;;
        --datadir=*) DATADIR_OVERRIDE="${1#*=}"; shift ;;
        --webroot) WEBROOT_OVERRIDE="${2:?--webroot 需要参数}"; shift 2 ;;
        --webroot=*) WEBROOT_OVERRIDE="${1#*=}"; shift ;;
        --listen) LISTEN_OVERRIDE="${2:?--listen 需要参数}"; shift 2 ;;
        --listen=*) LISTEN_OVERRIDE="${1#*=}"; shift ;;
        --no-start) DO_START=0; shift ;;
        --no-systemd) DO_SYSTEMD=0; shift ;;
        --no-docker) WITH_DOCKER=0; shift ;;
        --docker-source) DOCKER_SOURCE="${2:?--docker-source 需要参数}"; shift 2 ;;
        --docker-source=*) DOCKER_SOURCE="${1#*=}"; shift ;;
        --registry-mirror)
            if [ -z "$REGISTRY_MIRRORS" ]; then
                REGISTRY_MIRRORS="${2:?--registry-mirror 需要参数}"
            else
                REGISTRY_MIRRORS="$REGISTRY_MIRRORS,${2:?--registry-mirror 需要参数}"
            fi
            shift 2 ;;
        --registry-mirror=*)
            if [ -z "$REGISTRY_MIRRORS" ]; then
                REGISTRY_MIRRORS="${1#*=}"
            else
                REGISTRY_MIRRORS="$REGISTRY_MIRRORS,${1#*=}"
            fi
            shift ;;
        --docker-script-sha256) DOCKER_SCRIPT_SHA="${2:?--docker-script-sha256 需要参数}"; shift 2 ;;
        --docker-proxy) DOCKER_PROXY="${2:?--docker-proxy 需要参数}"; shift 2 ;;
        --docker-proxy=*) DOCKER_PROXY="${1#*=}"; shift ;;
        -h|--help)
            cat <<'EOF'
用法: sh install.sh [选项]

  --prefix <dir>              安装前缀（默认 /usr/local）
  --datadir <dir>             数据根（默认 /www/skiff：库、config.toml、plugins、backup）
  --webroot <dir>             站点寄宿根（默认 /www：sites/nginx/logs/certs/php）
  --listen <addr:port>        面板监听地址（默认沿用示例配置 127.0.0.1:18080）；
                              可用环境变量 SKIFF_LISTEN 代替；内网直连需显式改，
                              并自行确认 allow_insecure_http 或 TLS 配置
  --no-start                  装完不启动服务
  --no-systemd                不装 systemd unit
  --no-docker                 跳过 Docker 自动安装（面板仍可装，容器页为空态）
  --docker-source <host>      Docker CE 源（默认 mirrors.tencent.com/docker-ce）
  --registry-mirror <host>    镜像加速器，可重复；脚本自动补 https://
  --docker-script-sha256 <h>  固定 LinuxMirrors docker.sh 的摘要（发布/审计用）
  --docker-proxy <url>        docker daemon 出网代理（写 systemd drop-in；仅新装时自动写）
EOF
            exit 0 ;;
        *) echo "未知参数: $1（见 --help）" >&2; exit 2 ;;
    esac
done

BIN_DIR="$PREFIX/bin"
UNIT_DIR=/etc/systemd/system
DATA_DIR=${DATADIR_OVERRIDE:-/www/skiff}
WEB_ROOT=${WEBROOT_OVERRIDE:-/www}
CONF_DIR=/etc/skiff
RUN_DIR=/run/skiff
PLUGIN_GROUP=skiff-plugins

log() { printf '[install] %s\n' "$*"; }
warn() { printf '[install] 警告: %s\n' "$*" >&2; }
die() { printf '[install] 错误: %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "需要 root 权限运行"

SRC_DIR=$(cd "$(dirname "$0")" && pwd)
[ -f "$SRC_DIR/skiff" ] || die "当前目录没有 skiff 二进制，请在解包后的发布目录内执行"

# 1. 运行组：插件以独立 UID 降权运行，需要这个组作为 /run/skiff 的属组
if getent group "$PLUGIN_GROUP" >/dev/null 2>&1; then
    log "运行组 $PLUGIN_GROUP 已存在"
else
    groupadd --system "$PLUGIN_GROUP" 2>/dev/null || addgroup -S "$PLUGIN_GROUP"
    log "已创建运行组 $PLUGIN_GROUP"
fi

# 2. 数据与配置目录
#
# 属性要分开看：数据根本身 0711（可穿越、不可列目录），backup 0700，
# skiff.db 由面板自己 chmod 0600。这里若把数据根设成 0700，降权到独立 UID
# 的插件无法到达自己的 data/（ancestor 缺 x 位 = 路径不可达），插件一跑就 EACCES。
mkdir -p "$DATA_DIR" "$CONF_DIR"
chmod 0711 "$DATA_DIR"
chown root:root "$DATA_DIR"
chmod 0755 "$CONF_DIR"
mkdir -p "$DATA_DIR/plugins" "$DATA_DIR/logs" "$DATA_DIR/backup"
chmod 0711 "$DATA_DIR/plugins"
chmod 0700 "$DATA_DIR/backup"
log "数据根 $DATA_DIR（0711 root，可穿越不可列）"

# 站点寄宿布局：公共 vhost 目录与 PHP socket 对容器内进程可读（0755）。
# 每个站点自己的 index/log/ssl/conf 建在 $WEB_ROOT/sites/<域名>/ 内，建站时创建。
mkdir -p "$WEB_ROOT/sites" "$WEB_ROOT/nginx/conf.d" \
         "$WEB_ROOT/php" "$WEB_ROOT/skiff/waf/rules"
chmod 0755 "$WEB_ROOT" "$WEB_ROOT/sites" "$WEB_ROOT/nginx" "$WEB_ROOT/nginx/conf.d" "$WEB_ROOT/php"
log "站点寄宿根 $WEB_ROOT（sites/<域名>/{index,log,ssl,conf}，公共配置 nginx/conf.d）"

# 配置文件里写死 data_dir/web_root：示例配置默认值就是 /www/skiff + /www，
# 自定义 --datadir/--webroot 时同步追加（环境变量优先级更高，但落盘更直观）。
ensure_path_config() {
    conf=$1
    data=$2
    web=$3
    [ -f "$conf" ] || return 0
    if grep -qE '^[[:space:]]*data_dir[[:space:]]*=' "$conf"; then
        sed -i "s|^[[:space:]]*data_dir[[:space:]]*=.*|data_dir = \"$data\"|" "$conf"
    else
        printf '\ndata_dir = "%s"\n' "$data" >> "$conf"
    fi
    if grep -qE '^[[:space:]]*web_root[[:space:]]*=' "$conf"; then
        sed -i "s|^[[:space:]]*web_root[[:space:]]*=.*|web_root = \"$web\"|" "$conf"
    else
        printf 'web_root = "%s"\n' "$web" >> "$conf"
    fi
    log "配置路径已写入：data_dir=$data web_root=$web"
}

if [ ! -f "$CONF_DIR/config.toml" ] && [ -f "$SRC_DIR/config.toml.example" ]; then
    cp "$SRC_DIR/config.toml.example" "$CONF_DIR/config.toml"
    chmod 0644 "$CONF_DIR/config.toml"
    log "已放置示例配置 $CONF_DIR/config.toml（请按需修改）"
fi

if [ -n "$DATADIR_OVERRIDE" ] || [ -n "$WEBROOT_OVERRIDE" ]; then
    ensure_path_config "$CONF_DIR/config.toml" "$DATA_DIR" "$WEB_ROOT"
fi

# 2.1 「示例配置装完即起不来」的收口
#
# 示例配置里 allow_insecure_http 是**注释态**（默认 false），而 listen 默认 loopback：
# 这个组合会被 Config::validate 直接 bail，服务装完就是 crash loop。2026-11-03 上机
# 实测确认（docs/ITERATION.md §6.1，restart counter 一路涨到 9）。
#
# 只在「loopback + 未配 TLS」这一种明确安全的前提下补上，并告警；非 loopback 明文
# 仍保持拒绝启动（§6.2 裸奔防护不许被安装脚本绕过）。
ensure_insecure_http() {
    conf=$1
    [ -f "$conf" ] || return 0
    # 配了 TLS 就不需要（且该项无意义）
    if grep -qE '^[[:space:]]*tls_cert[[:space:]]*=' "$conf"; then
        return 0
    fi
    if grep -qE '^[[:space:]]*allow_insecure_http[[:space:]]*=[[:space:]]*true' "$conf"; then
        return 0
    fi
    listen=$(sed -n 's/^[[:space:]]*listen[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$conf" | head -1)
    case "$listen" in
        127.0.0.1:*|localhost:*|localhost|\[::1\]:*) ;;
        *)
            warn "未配 TLS 且 listen=$listen 不是 loopback：面板启动会被拒绝（§6.2）。"
            warn "请配 tls_cert/tls_key（§2.4 A）或改回 loopback + 可信反代（§2.4 B）；未自动改写配置。"
            return 0
            ;;
    esac
    {
        printf '\n# ── 由 install.sh 追加 ──\n'
        printf '# 示例配置里本项是注释态（默认 false），而 loopback + 无 TLS 时不开它会拒绝启动。\n'
        printf '# 配好 tls_cert/tls_key 或改为可信反代后，建议删掉下面这行（见 DEPLOY_TEST.md §2.4）。\n'
        printf 'allow_insecure_http = true\n'
    } >> "$conf"
    warn "已在 $conf 启用明文 HTTP（仅 loopback）。对外提供访问前必须配 TLS 或可信反代（§2.4）。"
}
# 2.0 监听地址覆盖（--listen / SKIFF_LISTEN）
#
# 默认**不改**配置：示例配置是 127.0.0.1:18080（只回环）。内网直连等场景要显式给值，
# 这样重装/升级不会把已有的监听地址悄悄改回去，也不会把内部实例意外暴露到网卡上。
apply_listen_override() {
    conf=$1
    [ -n "$LISTEN_OVERRIDE" ] || return 0
    case "$LISTEN_OVERRIDE" in
        *:*) ;;
        *) die "--listen/SKIFF_LISTEN 需要 <addr:port> 形式，实际: $LISTEN_OVERRIDE" ;;
    esac
    [ -f "$conf" ] || return 0
    if grep -qE '^[[:space:]]*listen[[:space:]]*=' "$conf"; then
        sed -i "s|^[[:space:]]*listen[[:space:]]*=.*|listen = \"$LISTEN_OVERRIDE\"|" "$conf"
    else
        printf '\nlisten = "%s"\n' "$LISTEN_OVERRIDE" >> "$conf"
    fi
    log "监听地址已设为 $LISTEN_OVERRIDE（--listen/SKIFF_LISTEN）"
}
apply_listen_override "$CONF_DIR/config.toml"

ensure_insecure_http "$CONF_DIR/config.toml"

# 3. 二进制（先写临时文件再原子替换，避免升级瞬间的半个可执行文件）
mkdir -p "$BIN_DIR"
install -m 0755 "$SRC_DIR/skiff" "$BIN_DIR/.skiff.new"
mv -f "$BIN_DIR/.skiff.new" "$BIN_DIR/skiff"
log "已安装 $BIN_DIR/skiff"

# 插件开发工具（可选）：打 .wbpkg、生成密钥、验签都用它。发布包里带就装。
if [ -f "$SRC_DIR/skiff-plugin" ]; then
    install -m 0755 "$SRC_DIR/skiff-plugin" "$BIN_DIR/.skiff-plugin.new"
    mv -f "$BIN_DIR/.skiff-plugin.new" "$BIN_DIR/skiff-plugin"
    log "已安装 $BIN_DIR/skiff-plugin（插件打包/验签工具）"
fi

# 官方插件包（随发布包携带）：只放到本地官方源，**不自动安装**——
# 权限与对象授权必须由管理员逐条确认，跳过确认就等于装了个万能插件。
OFFICIAL_DIR="$DATA_DIR/official"
if [ -d "$SRC_DIR/official" ]; then
    mkdir -p "$OFFICIAL_DIR"
    chmod 0755 "$OFFICIAL_DIR"
    COPIED=0
    for pkg in "$SRC_DIR"/official/*.wbpkg; do
        [ -f "$pkg" ] || continue
        install -m 0644 "$pkg" "$OFFICIAL_DIR/"
        COPIED=$((COPIED + 1))
    done
    # 公钥只是公开信息：复制过去，首次启动会自动写入信任库（不构成泄密）
    if [ -f "$SRC_DIR/official/public-key.txt" ]; then
        install -m 0644 "$SRC_DIR/official/public-key.txt" "$OFFICIAL_DIR/public-key.txt"
        log "官方公钥已放入 $OFFICIAL_DIR，首启会自动信任（无需手动导入）"
    fi
    log "官方插件包已放入 $OFFICIAL_DIR（$COPIED 个，未自动安装）"
fi

# 4. Docker 前置
#
# 必须排在 systemd 之前：DockerService 只在进程启动时 connect 一次，先起面板再装
# Docker，容器页会一直停在「Docker 不可用」直到手动重启服务。
#
# 失败不阻塞安装：Docker 缺了面板仍能以 docker 之外的能力面运行（§3.3 优雅降级）。
if [ "$WITH_DOCKER" -eq 1 ]; then
    if [ -f "$SRC_DIR/docker-bootstrap.sh" ]; then
        DOCKER_ARGS="--source $DOCKER_SOURCE"
        if [ -n "$REGISTRY_MIRRORS" ]; then
            DOCKER_ARGS="$DOCKER_ARGS --registry-mirror $REGISTRY_MIRRORS"
        fi
        if [ -n "$DOCKER_SCRIPT_SHA" ]; then
            DOCKER_ARGS="$DOCKER_ARGS --script-sha256 $DOCKER_SCRIPT_SHA"
        fi
        if [ -n "$DOCKER_PROXY" ]; then
            DOCKER_ARGS="$DOCKER_ARGS --daemon-proxy $DOCKER_PROXY"
        fi
        log "检查 Docker 前置（CE 源 $DOCKER_SOURCE）"
        # shellcheck disable=SC2086
        if sh "$SRC_DIR/docker-bootstrap.sh" $DOCKER_ARGS; then
            log "Docker 就绪"
        else
            warn "Docker 未就绪；面板仍会启动，容器/镜像/网络/卷为空态（其余功能不受影响）"
            warn "稍后可单独补跑：sh docker-bootstrap.sh [--registry-mirror <host>]"
        fi
    else
        warn "发布包内缺少 docker-bootstrap.sh，跳过 Docker 安装"
    fi
else
    log "跳过 Docker 安装（--no-docker）"
fi

# 5. systemd unit
if [ "$DO_SYSTEMD" -eq 1 ] && command -v systemctl >/dev/null 2>&1 && [ -f "$SRC_DIR/skiff.service" ]; then
    install -m 0644 "$SRC_DIR/skiff.service" "$UNIT_DIR/skiff.service"
    systemctl daemon-reload
    systemctl enable skiff >/dev/null 2>&1 || true
    log "已安装并 enable skiff.service"
    if [ "$DO_START" -eq 1 ]; then
        systemctl restart skiff
        # 「起不起来的安装」不能报成功：以前这里无条件打印"已启动"，把 §6.1 那个 P0
        # 藏了一个多月——装完 crash loop，验收和环境都以为装好了。
        i=0
        while [ "$i" -lt 20 ]; do
            [ "$(systemctl is-active skiff 2>/dev/null)" = "active" ] && break
            i=$((i + 1))
            sleep 1
        done
        if [ "$(systemctl is-active skiff 2>/dev/null)" != "active" ]; then
            printf '[install] 错误: skiff.service 20 秒内没有进入 active\n' >&2
            journalctl -u skiff -n 20 --no-pager 2>/dev/null >&2 || true
            printf '[install] 修好 %s 后执行 systemctl restart skiff；排障见 docs/DEPLOY_TEST.md §7\n' "$CONF_DIR/config.toml" >&2
            exit 1
        fi
        log "已启动 skiff.service（active）"
    fi
else
    log "跳过 systemd 安装（--no-systemd 或无 systemctl）"
fi

# 运行目录：属组 skiff-plugins，0711（可穿越）。kernel.sock 由面板启动时创建，
# 属组同样改成 skiff-plugins、0660——插件靠附加组拿到组权限。
mkdir -p "$RUN_DIR"
chgrp "$PLUGIN_GROUP" "$RUN_DIR" 2>/dev/null || true
chmod 0711 "$RUN_DIR"

echo
echo "────────────────────────────────────────────────────────"
echo "安装完成。下一步："
echo "  1) 初始管理员口令："
echo "       cat $DATA_DIR/initial-admin.txt"
echo "     （初始口令可直接登录；建议尽快在面板「设置 → 口令」修改，改密成功后该文件自动删除）"
echo "  2) 安全入口前缀写在启动日志里："
echo "       journalctl -u skiff -n 30 --no-pager | grep -i 入口"
echo "     也可直接从数据库读取 settings.entry_prefix。"
echo "  3) 服务状态与自检："
echo "       systemctl status skiff --no-pager"
echo "       $BIN_DIR/skiff doctor"
echo "       $BIN_DIR/skiff health"
echo "  4) Docker 状态（面板的核心能力面之一）："
echo "       docker info"
echo "     未就绪时容器页显示空态，可补跑 sh docker-bootstrap.sh"
echo "     镜像拉不动多半是网络：用 --registry-mirror <host> 配加速器"
echo "  5) 默认只监听 127.0.0.1:18080；对外请配 TLS 或反代（见 config.toml.example）"
if [ -d "$OFFICIAL_DIR" ]; then
echo "  6) 官方插件（随包携带，安装时逐条确认权限）："
echo "       官方公钥已由首启自动信任（$OFFICIAL_DIR/public-key.txt），无需手动导入"
echo "       ls $OFFICIAL_DIR"
echo "       $BIN_DIR/skiff plugin install $OFFICIAL_DIR/com.skiff.<x>-<ver>.wbpkg \\"
echo "           --resource path:/var/www"
echo "     也可在面板「插件 → 安装」上传同一份 .wbpkg。"
fi
echo "────────────────────────────────────────────────────────"
