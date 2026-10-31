#!/bin/sh
# Skiff（轻舟）卸载脚本（POSIX sh，方案 §5.1）
#
# 默认保留数据（/var/lib/skiff）。彻底删除必须显式 --purge。
# 用法: sh uninstall.sh [--purge] [--prefix /usr/local]
set -eu

PREFIX=/usr/local
PURGE=0

for arg in "$@"; do
    case "$arg" in
        --purge) PURGE=1 ;;
        --prefix) PREFIX="${2:?--prefix 需要参数}"; shift 2 ;;
        --prefix=*) PREFIX="${arg#*=}" ;;
        -h|--help)
            echo "用法: sh uninstall.sh [--purge] [--prefix /usr/local]"
            echo "  默认保留 /var/lib/skiff（数据库与插件数据），--purge 才会删除"
            exit 0 ;;
        *) echo "未知参数: $arg" >&2; exit 2 ;;
    esac
done

log() { printf '[uninstall] %s\n' "$*"; }
die() { printf '[uninstall] 错误: %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "需要 root 权限运行"

DATA_DIR=/var/lib/skiff
CONF_DIR=/etc/skiff
UNIT=/etc/systemd/system/skiff.service

if command -v systemctl >/dev/null 2>&1; then
    systemctl stop skiff 2>/dev/null || true
    systemctl disable skiff 2>/dev/null || true
fi

if [ -f "$UNIT" ]; then
    rm -f "$UNIT"
    command -v systemctl >/dev/null 2>&1 && systemctl daemon-reload
    log "已移除 systemd unit"
fi

rm -f "$PREFIX/bin/skiff"
log "已移除 $PREFIX/bin/skiff"
rm -f "$PREFIX/bin/skiff-plugin"
log "已移除 $PREFIX/bin/skiff-plugin（若安装过）"

rm -rf /run/skiff

if [ "$PURGE" -eq 1 ]; then
    echo
    echo "即将删除以下内容（不可恢复）："
    echo "  $DATA_DIR   （数据库 + 插件代码与数据）"
    echo "  $CONF_DIR   （配置文件）"
    echo "以及全部 skiff-plugins 组用户（UID 200000-200999）"
    echo
    printf '确认请输入大写 YES: '
    read -r answer
    [ "$answer" = "YES" ] || die "已取消，数据未删除"

    # 插件 UID 是面板分配的（200000-200999），按范围清理，不碰系统用户
    for uid in $(getent passwd | awk -F: '$3>=200000 && $3<=200999 {print $1}'); do
        userdel "$uid" 2>/dev/null || log "无法删除用户 $uid（可能仍有进程）"
    done

    rm -rf "$DATA_DIR" "$CONF_DIR"
    log "已删除数据与配置"
    if getent group skiff-plugins >/dev/null 2>&1; then
        groupdel skiff-plugins 2>/dev/null || true
    fi
else
    echo
    echo "已卸载程序，保留数据："
    echo "  $DATA_DIR （重新安装后可直接接续）"
    echo "  $CONF_DIR"
    echo "彻底清除请执行: sh uninstall.sh --purge"
fi
