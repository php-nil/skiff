#!/bin/sh
# 1Panel 应用升级前脚本（POSIX sh）
# 1Panel 会在停旧容器、起新容器之间调用：`sh upgrade.sh <安装目录>`
#
# 这里做一件事：在容器停止（无 WAL 写入者）时把 skiff.db 拍一份快照。
# 容器形态下不执行 `skiff backup`（那是宿主进程的命令），所以退化为文件级快照 +
# 显式说明：**该快照只用于升级回滚，不替代 §5.2 的备份演练。**
set -eu

APP_DIR="${1:-$(pwd)}"
DATA_DIR="$APP_DIR/data"
TS=$(date +%Y%m%d-%H%M%S)
BK="$DATA_DIR/backup"

if [ -f "$DATA_DIR/skiff.db" ]; then
    mkdir -p "$BK"
    chmod 0700 "$BK" 2>/dev/null || true
    cp "$DATA_DIR/skiff.db" "$BK/skiff-upgrade-$TS.db"
    # WAL/SHM 一并留档：容器已停，但文件内容对排障仍有价值
    [ -f "$DATA_DIR/skiff.db-wal" ] && cp "$DATA_DIR/skiff.db-wal" "$BK/skiff-upgrade-$TS.db-wal" || true
    echo "[skiff] 升级前快照: $BK/skiff-upgrade-$TS.db"
    # 只保留最近 3 份（§5.2）
    ls -1t "$BK"/skiff-upgrade-*.db 2>/dev/null | tail -n +4 | while read -r old; do
        rm -f "$old" "$old-wal"
    done
else
    echo "[skiff] 未发现已有数据库，跳过快照"
fi
