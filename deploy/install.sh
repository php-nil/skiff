#!/bin/sh
# Skiff（轻舟）宿主机直装脚本（POSIX sh，方案 §5.1）
#
# 由发布包内执行，前提：外层已校验 tar.gz 的 SHA-256 与 ed25519 签名。
# 本脚本不负责校验包含自身的压缩包（自校验悖论，§5.1）。
#
# 用法: sh install.sh [--prefix /usr/local] [--no-start] [--no-systemd]
#                      [--no-docker] [--docker-source <host>] [--registry-mirror <host>]...
set -eu

PREFIX=/usr/local
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
DATA_DIR=/var/lib/skiff
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

if [ ! -f "$CONF_DIR/config.toml" ] && [ -f "$SRC_DIR/config.toml.example" ]; then
    cp "$SRC_DIR/config.toml.example" "$CONF_DIR/config.toml"
    chmod 0644 "$CONF_DIR/config.toml"
    log "已放置示例配置 $CONF_DIR/config.toml（请按需修改）"
fi

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
        log "已启动 skiff.service"
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
echo "     （首登强制改密，改密成功后该文件自动删除）"
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
echo "────────────────────────────────────────────────────────"
