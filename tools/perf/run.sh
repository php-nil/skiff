#!/bin/bash
# 测试机站点压测驱动：quick = 首轮短跑（4 并发），full = 三路径 4/16/32 档位矩阵，
# php = 只跑 PHP 动态路径的 4/16/32 档位（配置类改动的 A/B 复测）。
# 依赖：测试机 python3；站点 audit-env.local（PHP）、audit-rust.local（Rust 反代）已 running。
# 环境变量：OUT / SITE_DIR / PHP_HOST / RUST_HOST / RATE。
set -u

# 位置断言：压测负载只能在测试机执行，开发机只允许通过 ssh 驱动测试机。
# 首行固定打印 hostname/uptime，便于在开发机侧核对实际执行位置。
HOST_NAME=$(hostname 2>/dev/null || echo unknown)
UP_TIME=$(uptime 2>/dev/null | tr -s ' ')
echo "[perf] host=$HOST_NAME time=$(date -Is) uptime=$UP_TIME"
if [ "$(uname -s 2>/dev/null)" != "Linux" ]; then
  echo "PERF-REFUSED: 非 Linux 主机，禁止在此执行压测（开发机请用 ssh 驱动测试机）" >&2
  exit 3
fi
if [ ! -d /root/perf-tools ]; then
  echo "PERF-REFUSED: 未发现 /root/perf-tools，当前主机不是约定测试机" >&2
  exit 3
fi

D=${OUT:-/root/perf-20261107}
SITE=${SITE_DIR:-/www/sites/audit-env.local/index}
PH=${PHP_HOST:-audit-env.local}
RH=${RUST_HOST:-audit-rust.local}
RATE=${RATE:-100}
MODE=${1:-quick}
LOG1=/www/sites/$PH/log/access.log
LOG2=/www/sites/$RH/log/access.log
mkdir -p "$D"

PRE1=$(stat -c %s "$LOG1" 2>/dev/null || echo 0)
PRE2=$(stat -c %s "$LOG2" 2>/dev/null || echo 0)
HAD1=0; HAD2=0
[ -e "$LOG1.1" ] && HAD1=1
[ -e "$LOG2.1" ] && HAD2=1

head -c 65536 /dev/urandom > "$SITE/perf-static-64k.bin"
printf '<?php echo "ok";' > "$SITE/perf.php"
chmod 644 "$SITE/perf-static-64k.bin" "$SITE/perf.php"

echo idle > "$D/phase"
rm -f "$D/stop"
python3 "$(dirname "$0")/observe.py" "$D" &
CPID=$!

cleanup() {
  touch "$D/stop"
  sleep 2
  kill "$CPID" 2>/dev/null || true
  rm -f "$SITE/perf-static-64k.bin" "$SITE/perf.php"
  pkill -f "$D/load.py" 2>/dev/null || true
  # 只回收本轮新增的轮转副本；原本存在的 .1 不动
  [ "$HAD1" = 0 ] && rm -f "$LOG1.1"
  [ "$HAD2" = 0 ] && rm -f "$LOG2.1"
}
trap cleanup EXIT

run() {
  echo "$1" > "$D/phase"
  python3 "$(dirname "$0")/load.py" "$2" "$3" "$4" "$5" "$D/$1.json" "$RATE" || echo "load-fail $1"
}

if [ "$MODE" = quick ]; then
  sleep 30
  run warmup-static "$PH" /perf-static-64k.bin 4 5
  run warmup-php    "$PH" /perf.php            4 5
  run warmup-rust   "$RH" /                    4 5
  run static-c4     "$PH" /perf-static-64k.bin 4 30
  run php-c4        "$PH" /perf.php            4 30
  run rust-c4       "$RH" /                    4 30
  echo idle-after > "$D/phase"
  sleep 30
elif [ "$MODE" = php ]; then
  sleep 60
  run warmup-php "$PH" /perf.php 4 5
  for c in 4 16 32; do
    run "php-c$c" "$PH" /perf.php "$c" 60
    sleep 5
  done
  echo idle-after > "$D/phase"
  sleep 60
else
  sleep 60
  for t in "static $PH /perf-static-64k.bin" "php $PH /perf.php" "rust $RH /"; do
    set -- $t
    run "warmup-$1" "$2" "$3" 4 5
  done
  for c in 4 16 32; do
    for t in "static $PH /perf-static-64k.bin" "php $PH /perf.php" "rust $RH /"; do
      set -- $t
      run "$1-c$c" "$2" "$3" "$c" 60
      sleep 5
    done
  done
  echo idle-after > "$D/phase"
  sleep 60
fi

touch "$D/stop"
wait "$CPID"
trap - EXIT
rm -f "$SITE/perf-static-64k.bin" "$SITE/perf.php"
[ "$HAD1" = 0 ] && rm -f "$LOG1.1"
[ "$HAD2" = 0 ] && rm -f "$LOG2.1"
# 站点日志截回压测前尺寸（保留原有前缀，去掉本轮追加的行）
[ "$(stat -c %s "$LOG1")" -gt "$PRE1" ] && truncate -s "$PRE1" "$LOG1"
[ "$(stat -c %s "$LOG2")" -gt "$PRE2" ] && truncate -s "$PRE2" "$LOG2"
python3 "$(dirname "$0")/report.py" "$D"
