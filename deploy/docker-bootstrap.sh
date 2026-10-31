#!/bin/sh
# Skiff（轻舟）Docker 前置引导（POSIX sh）
#
# 为什么是脚本而不是 Rust 内置：
#   1) 内核是 musl 静态单二进制（§5.1），把「下载安装脚本 + 调各发行版包管理器」编进去
#      要背上 HTTP 客户端与 27 种发行版的适配，体积和攻击面都不划算；
#   2) 它必须在面板已经装好之后能单独补跑（`sh docker-bootstrap.sh`），脚本比发版快。
#
# 默认只补缺、不重装。这条不是保守，是避免事故：
#   LinuxMirrors 的 docker.sh 在检测到已有 docker 时会先 `systemctl disable --now docker`，
#   再 `apt-get remove -y 'docker*'` 重装 CE。对一台正跑着业务容器的机器，那是停机，
#   不是安装。所以「已有 docker」的机器一律不交给它，除非显式 --force-reinstall。
#
# 用法:
#   sh docker-bootstrap.sh [--source <host>] [--registry-mirror <host>]...
#                          [--script-url <url>] [--script-sha256 <hex>]
#                          [--force-reinstall] [--check-only] [--no-registry]
set -eu

SOURCE="mirrors.tencent.com/docker-ce"
REGISTRY_MIRRORS=""
SCRIPT_URL="https://linuxmirrors.cn/docker.sh"
SCRIPT_SHA256=""
FORCE_REINSTALL=0
FORCE_REGISTRY=0
CHECK_ONLY=0
WANT_REGISTRY=0
DAEMON_PROXY=""
TMP=""

log() { printf '[docker] %s\n' "$*"; }
warn() { printf '[docker] 警告: %s\n' "$*" >&2; }
die() { printf '[docker] 错误: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
用法: sh docker-bootstrap.sh [选项]

  --source <host>            Docker CE 软件源（默认 mirrors.tencent.com/docker-ce）
  --registry-mirror <host>   镜像加速器，可重复，逗号拼接后交给 docker.sh
                             （脚本会自动补 https://，不要带协议头）
  --script-url <url>         LinuxMirrors 脚本地址（默认官方地址，可换备用域名/内网镜像）
  --script-sha256 <hex>      固定校验下载到的脚本摘要，防止远端被替换
  --force-reinstall          允许对「已有 docker」的机器重装（会停服并删除 docker* 包）
  --force-registry           已有 docker 时也允许重写 daemon.json 配加速器
                             （默认只打印手工合并方式，避免覆盖 data-root 等字段）
  --check-only               只检测并报告，不做任何改动
  --no-registry             不配置镜像加速器（默认也是不配，除非给了 --registry-mirror）
  --daemon-proxy <url>      给 docker daemon 配出网代理（写 systemd drop-in）。
                            仅在新装 Docker 时自动写；已有 docker 只打印配置片段，
                            因为重启 daemon 会中断正在跑的容器。
  -h, --help                显示本帮助

退出码: 0 就绪 / 2 daemon 不可用 / 3 已有 docker 但拒绝重装 / 4 安装失败
EOF
}

cleanup() {
    if [ -n "${TMP:-}" ] && [ -f "${TMP:-}" ]; then
        rm -f "$TMP"
    fi
    return 0
}
trap cleanup EXIT HUP INT TERM

add_registry() {
    WANT_REGISTRY=1
    if [ -z "$REGISTRY_MIRRORS" ]; then
        REGISTRY_MIRRORS="$1"
    else
        REGISTRY_MIRRORS="$REGISTRY_MIRRORS,$1"
    fi
}

while [ $# -gt 0 ]; do
    case "$1" in
        --source) SOURCE="${2:?--source 需要参数}"; shift 2 ;;
        --source=*) SOURCE="${1#*=}"; shift ;;
        --registry-mirror) add_registry "${2:?--registry-mirror 需要参数}"; shift 2 ;;
        --registry-mirror=*) add_registry "${1#*=}"; shift ;;
        --script-url) SCRIPT_URL="${2:?--script-url 需要参数}"; shift 2 ;;
        --script-url=*) SCRIPT_URL="${1#*=}"; shift ;;
        --script-sha256) SCRIPT_SHA256="${2:?--script-sha256 需要参数}"; shift 2 ;;
        --script-sha256=*) SCRIPT_SHA256="${1#*=}"; shift ;;
        --force-reinstall) FORCE_REINSTALL=1; shift ;;
        --force-registry) FORCE_REGISTRY=1; shift ;;
        --check-only) CHECK_ONLY=1; shift ;;
        --no-registry) WANT_REGISTRY=0; shift ;;
        --daemon-proxy) DAEMON_PROXY="${2:?--daemon-proxy 需要参数}"; shift 2 ;;
        --daemon-proxy=*) DAEMON_PROXY="${1#*=}"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "未知参数: $1（见 --help）" ;;
    esac
done

have() { command -v "$1" >/dev/null 2>&1; }

daemon_ok() {
    have docker && docker info >/dev/null 2>&1
}

docker_version() {
    docker --version 2>/dev/null || echo "未知版本"
}

start_service() {
    if have systemctl; then
        systemctl enable --now docker >/dev/null 2>&1 || systemctl start docker >/dev/null 2>&1 || true
    elif have rc-service; then
        rc-service docker start >/dev/null 2>&1 || true
    else
        warn "没有 systemctl / rc-service，无法自动拉起 daemon"
    fi
}

sha256_of() {
    if have sha256sum; then
        sha256sum "$1" | awk '{print $1}'
    elif have shasum; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        openssl dgst -sha256 "$1" | awk '{print $NF}'
    fi
}

fetch_to() {
    if have curl; then
        curl -fsSL --max-time 180 "$1" -o "$2"
    elif have wget; then
        wget -q -T 180 -O "$2" "$1"
    else
        return 1
    fi
}

# 装好之后如果面板已经在跑，必须提示重启：DockerService 只在进程启动时 connect 一次，
# 新出现的 socket 不会被已有进程看到，容器页会继续显示「Docker 不可用」。
finish_ok() {
    log "Docker 已就绪：$(docker_version)"
    if have systemctl && systemctl is-active skiff >/dev/null 2>&1; then
        warn "skiff.service 正在运行，但它在启动时只连接一次 Docker——"
        warn "请执行 systemctl restart skiff，否则容器页仍是空态"
    fi
}

# 下载远端脚本到 $TMP 并按需校验摘要。不用 `curl | bash`：管道执行在下载中途
# 断了会跑半截脚本，而且完全无法在中间插入校验。
fetch_script() {
    TMP=$(mktemp) || die "mktemp 失败"
    log "下载 LinuxMirrors 脚本：$SCRIPT_URL"
    if ! fetch_to "$SCRIPT_URL" "$TMP"; then
        warn "下载失败（DNS/超时/被拦截）。可换备用地址重试："
        warn "  --script-url https://edgeone.linuxmirrors.cn/docker.sh"
        warn "  --script-url https://raw.githubusercontent.com/SuperManito/LinuxMirrors/main/docker.sh"
        exit 4
    fi
    ACTUAL_SHA=$(sha256_of "$TMP")
    log "脚本摘要 sha256=$ACTUAL_SHA"
    if [ -n "$SCRIPT_SHA256" ]; then
        if [ "$ACTUAL_SHA" != "$SCRIPT_SHA256" ]; then
            die "摘要不符（期望 $SCRIPT_SHA256），拒绝执行未经校验的远端脚本"
        fi
        log "摘要校验通过"
    else
        log "未指定 --script-sha256，未固定摘要（建议发布时 pin 住上面这个值）"
    fi
}

# 只配加速器：交给 LinuxMirrors 的 --only-registry。它会整文件重写 daemon.json，
# 所以只允许在 --force-registry 下走这条。原文件由该脚本自行备份。
apply_registry_only() {
    have bash || die "配置加速器需要 bash（LinuxMirrors docker.sh 的 shell 方言）"
    fetch_script
    log "调用 docker.sh --only-registry（会重写 /etc/docker/daemon.json）"
    if ! bash "$TMP" --only-registry --source-registry "$REGISTRY_MIRRORS" \
        --pure-mode --ignore-backup-tips < /dev/null; then
        warn "加速器配置失败，daemon.json 可能需要手工恢复（原文件备份在 /etc/docker/daemon.json.bak）"
        exit 4
    fi
    log "镜像加速器：$(docker info --format '{{json .RegistryConfig.Mirrors}}' 2>/dev/null || echo '读取失败')"
}

# 已有 docker 且用户要加速器：默认不自动改配置。daemon.json 里可能还有
# data-root / log-driver / 私有 registry 的 insecure-registries，整文件重写会把它们抹掉。
print_registry_hint() {
    hosts=$(printf '%s' "$REGISTRY_MIRRORS" | sed 's| *, *|", "https://|g')
    warn "已有 Docker 在跑，默认不改它的 /etc/docker/daemon.json（可能含 data-root 等字段，整文件重写会丢）。"
    warn "要加加速器请手工合并后重启："
    warn "  { \"registry-mirrors\": [\"https://$hosts\"] }"
    warn "确认该文件只用于镜像加速时，也可直接重跑："
    warn "  sh docker-bootstrap.sh --registry-mirror <host> --force-registry"
}

# LinuxMirrors 把 daemon.json 重写成只有 registry-mirrors 的单行紧凑格式。
# 用户没要加速器时它会写一个无意义的 `https://registry.hub.docker.com`，
# 这会让 `docker info` 显示配了加速却实际没有。只在确认是它生成的文件时删掉那一行，
# 用户自己维护的 daemon.json 一律不碰。
drop_useless_registry() {
    cfg=/etc/docker/daemon.json
    [ -f "$cfg" ] || return 0
    lines=$(wc -l < "$cfg" | tr -d ' ')
    if [ "$lines" -le 3 ] && [ "$(grep -c 'registry-mirrors' "$cfg" || true)" = "1" ]; then
        printf '{\n}\n' > "$cfg"
        log "已清除无意义的默认 registry-mirrors（未指定加速器）"
        if daemon_ok; then
            systemctl restart docker >/dev/null 2>&1 || true
        fi
    else
        warn "$cfg 结构复杂，未自动改写；如需加速请手工合并 registry-mirrors"
    fi
}

# daemon 出网代理与「拉镜像加速器」是两件事：加速器改的是 registry 地址，
# 代理改的是 daemon 进程怎么连外网（公司内网必须走代理时用）。
# 只在新装路径自动写——已有 docker 的机器重启 daemon 会中断正在跑的容器。
proxy_conf_path() { printf '/etc/systemd/system/docker.service.d/10-skiff-http-proxy.conf'; }

apply_daemon_proxy() {
    [ -n "$DAEMON_PROXY" ] || return 0
    if ! have systemctl; then
        warn "--daemon-proxy 需要 systemd 写 drop-in，当前环境没有，已跳过"
        return 0
    fi
    _dir=$(dirname "$(proxy_conf_path)")
    mkdir -p "$_dir"
    {
        echo '[Service]'
        echo "Environment=\"HTTP_PROXY=$DAEMON_PROXY\""
        echo "Environment=\"HTTPS_PROXY=$DAEMON_PROXY\""
        echo 'Environment="NO_PROXY=localhost,127.0.0.1,::1"'
    } > "$(proxy_conf_path)"
    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl restart docker >/dev/null 2>&1 || true
    log "已写入 daemon 出网代理：$(proxy_conf_path)"
}

print_proxy_hint() {
    warn "已有 Docker 在跑，不自动写 daemon 代理（重启 daemon 会中断容器）。手工执行："
    warn "  mkdir -p /etc/systemd/system/docker.service.d"
    warn "  # 写入 $(proxy_conf_path)"
    warn "  # [Service]"
    warn "  # Environment=\"HTTP_PROXY=$DAEMON_PROXY\""
    warn "  # Environment=\"HTTPS_PROXY=$DAEMON_PROXY\""
    warn "  systemctl daemon-reload && systemctl restart docker"
}

# ── 1. 已有可用 daemon：直接返回 ─────────────────────────────────
if daemon_ok; then
    log "Docker 已就绪：$(docker_version)"
    if [ "$WANT_REGISTRY" -eq 1 ]; then
        if [ "$FORCE_REGISTRY" -eq 1 ]; then
            apply_registry_only
        else
            print_registry_hint
        fi
    fi
    if [ -n "$DAEMON_PROXY" ]; then
        print_proxy_hint
    fi
    exit 0
fi

# ── 2. 有二进制但 daemon 没起来：先尝试拉起 ──────────────────────
if have docker; then
    log "检测到 docker 二进制（$(docker_version)）但 daemon 未响应，尝试启动服务"
    start_service
    sleep 2
    if daemon_ok; then
        finish_ok
        exit 0
    fi
    if [ "$FORCE_REINSTALL" -ne 1 ]; then
        warn "daemon 仍不可用——可能是未装完、socket 权限不足，或这台机器没有 systemd。"
        warn "不会自动重装：docker.sh 会先停服再删除 docker* 包，正在跑的容器会断。"
        warn "确认可重装请加 --force-reinstall，或手工排查：systemctl status docker"
        exit 3
    fi
    log "已确认 --force-reinstall，进入重装流程"
fi

if [ "$CHECK_ONLY" -eq 1 ]; then
    log "Docker 未就绪（--check-only，不做改动）"
    exit 2
fi

# ── 3. Alpine：docker.sh 只认 apt/dnf/yum，这里走 apk ────────────
if [ -f /etc/alpine-release ]; then
    log "Alpine 环境：LinuxMirrors docker.sh 不支持 apk，改用 apk 安装"
    have apk || die "找不到 apk，无法安装 Docker"
    if ! apk add --no-cache docker docker-cli-compose; then
        warn "apk 安装失败（网络或源不可用）"
        exit 4
    fi
    rc-update add docker default >/dev/null 2>&1 || true
    start_service
    sleep 2
    if daemon_ok; then
        finish_ok
        exit 0
    fi
    warn "apk 装完了但 daemon 没起来：service docker start 后看 /var/log/docker.log"
    exit 2
fi

# ── 4. 其余发行版：LinuxMirrors docker.sh ───────────────────────
if ! have apt-get && ! have dnf && ! have yum; then
    warn "未识别的包管理器（既无 apt-get 也无 dnf/yum），请手工安装 Docker"
    exit 4
fi
if ! have bash; then
    warn "LinuxMirrors docker.sh 需要 bash，本机没有。请先装 bash 或手工安装 Docker"
    exit 4
fi

fetch_script

# docker.sh 在 SOURCE_REGISTRY 为空时会写死 registry.hub.docker.com 并重写 daemon.json，
# 所以这里必须显式取值，避免它进入任何交互分支。
if [ -n "$REGISTRY_MIRRORS" ]; then
    REGISTRY_ARG="$REGISTRY_MIRRORS"
else
    REGISTRY_ARG="registry.hub.docker.com"
fi

log "调用 docker.sh（源 $SOURCE，加速器 ${REGISTRY_MIRRORS:-无}）"
# 三个参数都是为了让它在无 tty 环境下不卡住：
#   --pure-mode          关掉 banner / 赞助商输出
#   --ignore-backup-tips 跳过 daemon.json 备份的 read -rp 询问
#   < /dev/null          兜底：任何残留的交互读到 EOF 而不是等键盘
if ! bash "$TMP" --source "$SOURCE" --source-registry "$REGISTRY_ARG" \
    --install-latest true --pure-mode --ignore-backup-tips < /dev/null; then
    warn "docker.sh 执行失败，详见上方输出"
    exit 4
fi

if [ "$WANT_REGISTRY" -eq 0 ]; then
    drop_useless_registry
fi

apply_daemon_proxy

if daemon_ok; then
    finish_ok
    if [ "$WANT_REGISTRY" -eq 1 ]; then
        log "镜像加速器：$(docker info --format '{{json .RegistryConfig.Mirrors}}' 2>/dev/null || echo '未生效')"
    fi
    exit 0
fi

warn "安装脚本返回成功但 daemon 未响应：systemctl status docker 查看原因"
exit 2
