#!/bin/sh
# 1Panel 应用卸载脚本（POSIX sh）
# 1Panel 调用：`sh uninstall.sh <安装目录>`
#
# 不在这里删数据：1Panel 卸载流程会让用户单独勾选「是否删除应用数据」，
# 脚本自作主张删 /data 会让「卸载保数据」这条退路消失。
set -eu

APP_DIR="${1:-$(pwd)}"
echo "[skiff] 已卸载容器；数据保留在: $APP_DIR/data"
echo "[skiff] 如确认不再使用，可手动删除该目录（含 skiff.db 与插件数据）。"
