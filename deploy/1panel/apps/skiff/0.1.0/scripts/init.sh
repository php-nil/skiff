#!/bin/sh
# 1Panel 应用安装后置脚本（POSIX sh）
# 1Panel 会以 `sh init.sh <安装目录>` 调用；脚本必须幂等，重装/修复时可能重复执行。
set -eu

APP_DIR="${1:-$(pwd)}"
DATA_DIR="$APP_DIR/data"

mkdir -p "$DATA_DIR"
chmod 0700 "$DATA_DIR" 2>/dev/null || true

# 容器内 root 即宿主 root 的映射（挂了 docker.sock），这里只落数据目录权限。
echo "[skiff] 数据目录就绪: $DATA_DIR"
echo "[skiff] 首次启动后："
echo "  1) 初始口令: docker exec \${CONTAINER_NAME} cat /data/initial-admin.txt"
echo "  2) 安全入口前缀: docker logs \${CONTAINER_NAME} 2>&1 | grep 入口"
echo "[skiff] 受限形态声明：fs/net 能力不可用；仅 docker/identity 能力面可用（§5.4）。"
