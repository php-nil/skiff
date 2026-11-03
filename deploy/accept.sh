#!/bin/sh
# Skiff（轻舟）验收脚本 —— docs/ITERATION.md §5 的可执行版
#
# 在**目标机**上跑，root 身份，只读断言 + 少量必要的有状态操作（改密、重启）。
# 每轮回归的命令入口；`tools/iter.sh` 负责编排，本脚本只负责"判"。
#
# 用法: sh accept.sh [选项]
#   分组选择（逗号分隔，默认 all = preflight,a,b,d0）
#     --group <列表>         preflight | a | b | d0 | all
#   输入（由 iter.sh 传入）
#     --install-exit <n>     install.sh 的退出码（A3 断言用；不传则该条 SKIP）
#     --port <端口>          默认 18080（与 config 的 listen 一致）
#     --data-dir <目录>      默认 /var/lib/skiff
#     --prefix <入口前缀>     不传则从 journal 日志/库中取
#     --now <epoch>          调用方当前时间，用于断言时钟偏差 < 30 s
#     --pre-install          preflight 时检查"端口空闲"（装之前跑）
#     --with-rate-limit      跑 B4 登录限流断言（会把 admin 锁 5 分钟，首装轮才建议开）
#     --pkg-dir <目录>       解包后的发布目录（D0 用它找 docker-bootstrap.sh）
#
# 退出码: 0 全 PASS（SKIP 不算通过，但不算失败）｜1 有 FAIL｜2 前置不满足
#
# 设计口径（§5.2）：**每条断言都要断具体文案或数值，不能只看状态码。**
# 历史教训：Multipart 超限被 map_err 包成 400，与业务校验的 400 撞在一起，
# 只看状态码分辨不出是哪一层拒的。
set -u

PORT=18080
DATA_DIR=/var/lib/skiff
CONF=/etc/skiff/config.toml
GROUPS="all"
INSTALL_EXIT=""
PREFIX=""
NOW_EPOCH=""
PRE_INSTALL=0
WITH_RATE_LIMIT=0
PKG_DIR=""

while [ $# -gt 0 ]; do
    case "$1" in
        --group) GROUPS="${2:?--group 需要参数}"; shift 2 ;;
        --group=*) GROUPS="${1#*=}"; shift ;;
        --install-exit) INSTALL_EXIT="${2:?}"; shift 2 ;;
        --install-exit=*) INSTALL_EXIT="${1#*=}"; shift ;;
        --port) PORT="${2:?}"; shift 2 ;;
        --port=*) PORT="${1#*=}"; shift ;;
        --data-dir) DATA_DIR="${2:?}"; shift 2 ;;
        --data-dir=*) DATA_DIR="${1#*=}"; shift ;;
        --prefix) PREFIX="${2:?}"; shift 2 ;;
        --prefix=*) PREFIX="${1#*=}"; shift ;;
        --now) NOW_EPOCH="${2:?}"; shift 2 ;;
        --now=*) NOW_EPOCH="${1#*=}"; shift ;;
        --pre-install) PRE_INSTALL=1; shift ;;
        --with-rate-limit) WITH_RATE_LIMIT=1; shift ;;
        --pkg-dir) PKG_DIR="${2:?--pkg-dir 需要参数}"; shift 2 ;;
        --pkg-dir=*) PKG_DIR="${1#*=}"; shift ;;
        -h|--help) sed -n '2,/^set -u$/p' "$0" | sed '$d'; exit 0 ;;
        *) echo "未知参数: $1" >&2; exit 2 ;;
    esac
done

BASE="http://127.0.0.1:$PORT"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

PASS_N=0; FAIL_N=0; SKIP_N=0; FAILED=""
print_row() { printf '%-4s %-6s %-44s %s\n' "$1" "$2" "$3" "$4"; }
pass() { PASS_N=$((PASS_N + 1)); print_row PASS "$1" "$2" "${3:-}"; }
fail() { FAIL_N=$((FAIL_N + 1)); FAILED="$FAILED $1"; print_row FAIL "$1" "$2" "${3:-}"; }
skip() { SKIP_N=$((SKIP_N + 1)); print_row SKIP "$1" "$2" "${3:-}"; }
section() { printf '\n── %s ──\n' "$*"; }

# assert_eq <编号> <说明> <实得> <期望>
assert_eq() {
    if [ "$3" = "$4" ]; then pass "$1" "$2" "$3"; else fail "$1" "$2" "期望[$4] 实得[$3]"; fi
}
# assert_contains <编号> <说明> <文件> <子串>
assert_contains() {
    if grep -qF -- "$4" "$3" 2>/dev/null; then pass "$1" "$2" "含『$4』"; else
        fail "$1" "$2" "缺『$4』：$(head -c 200 "$3" 2>/dev/null | tr '\n' ' ')"
    fi
}

# §1.1 权限口径：<路径> <模式> <属主:属组>
# 数据根与 plugins/ 必须是 0711 —— 可穿越不可列；0700 会让降权插件全线 EACCES。
check_perm() {
    id=$1; path=$2; want_mode=$3; want_owner=$4
    if [ ! -e "$path" ]; then fail "$id" "权限位 $path" "文件不存在"; return; fi
    got=$(stat -c '%a %U:%G' "$path" 2>/dev/null || echo "?")
    assert_eq "$id" "权限位 $path" "$got" "$want_mode $want_owner"
}

# 安全入口前缀：优先调用方传入，其次启动 banner，最后库（journal 里有中文，取 token 用纯 ASCII）
entry_prefix() {
    if [ -n "$PREFIX" ]; then echo "$PREFIX"; return; fi
    p=$(journalctl -u skiff --no-pager 2>/dev/null | tr -d '\r' \
        | sed -n 's#.*/\([A-Za-z0-9_-]\{4,\}\)/login.*#\1#p' | tail -1)
    if [ -n "$p" ]; then echo "$p"; return; fi
    # 兜底：直接读库（settings 表；sqlite3 不一定装了，装不上就留空）
    if command -v sqlite3 >/dev/null 2>&1 && [ -f "$DATA_DIR/skiff.db" ]; then
        sqlite3 "$DATA_DIR/skiff.db" \
            "select value from settings where key='security.entry_prefix'" 2>/dev/null
    fi
}

wait_health() { # $1 超时秒数
    n=0
    while [ "$n" -lt "$1" ]; do
        code=$(curl -s -o "$TMP/health.json" -w '%{http_code}' --max-time 3 "$BASE/healthz" 2>/dev/null || true)
        [ "$code" = "200" ] && return 0
        n=$((n + 1)); sleep 1
    done
    return 1
}

dev_mode() { # 明文 + loopback + allow_insecure_http → dev_http，Cookie 降级为 skiff_dev_session
    grep -q '^[[:space:]]*tls_cert' "$CONF" 2>/dev/null && return 1
    grep -q '^[[:space:]]*allow_insecure_http[[:space:]]*=[[:space:]]*true' "$CONF" 2>/dev/null
}

# ───────────────────────── preflight ─────────────────────────
preflight() {
    section "前置断言（§5.1）"
    PF_HARD=0

    if [ "$(id -u)" = "0" ]; then pass PF1 "root 身份" "id -u=0"; else fail PF1 "root 身份" "id -u=$(id -u)"; PF_HARD=1; fi

    init=$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')
    assert_eq PF2 "PID 1 是 systemd" "$init" "systemd"

    if [ -n "$NOW_EPOCH" ]; then
        d=$(( $(date +%s) - NOW_EPOCH ))
        [ "$d" -lt 0 ] && d=$(( -d ))
        if [ "$d" -lt 30 ]; then pass PF3 "与调用方时钟偏差 < 30 s" "${d}s"; else fail PF3 "与调用方时钟偏差 < 30 s" "${d}s（>30s，TOTP/session 会错乱）"; fi
    else
        skip PF3 "与调用方时钟偏差 < 30 s" "未传 --now，无参考时钟"
    fi

    if [ "$PRE_INSTALL" -eq 1 ]; then
        if ss -lnt 2>/dev/null | grep -q ":$PORT "; then fail PF4 "端口 $PORT 空闲" "已被占用"; else pass PF4 "端口 $PORT 空闲" "free"; fi
    else
        skip PF4 "端口 $PORT 空闲" "装后阶段不适用（服务应当占着）"
    fi

    avail=$(df -Pk "$DATA_DIR" 2>/dev/null | awk 'NR==2{print $4}')
    [ -n "${avail:-}" ] || avail=$(df -Pk /var | awk 'NR==2{print $4}')
    if [ "$avail" -ge 2097152 ]; then pass PF5 "磁盘可用 ≥ 2 GB" "$((avail / 1024)) MB"; else fail PF5 "磁盘可用 ≥ 2 GB" "$((avail / 1024)) MB"; fi

    kv=$(uname -r | sed 's/^\([0-9]*\)\.\([0-9]*\).*/\1 \2/')
    set -- $kv
    if [ "$1" -gt 5 ] || { [ "$1" -eq 5 ] && [ "$2" -ge 13 ]; }; then
        pass PF6 "内核 ≥ 5.13（landlock 前提）" "$(uname -r)"
    else
        fail PF6 "内核 ≥ 5.13（landlock 前提）" "$(uname -r)"
    fi

    if grep -q landlock /sys/kernel/security/lsm 2>/dev/null; then
        pass PF7 "landlock 在 LSM 列表内" "$(cat /sys/kernel/security/lsm 2>/dev/null)"
    else
        fail PF7 "landlock 在 LSM 列表内" "$(cat /sys/kernel/security/lsm 2>/dev/null || echo 无)"
    fi

    # cgroups v2 只记录：限额未实现（§8），不能当通过项
    if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
        pass PF8 "cgroups v2 已挂载（仅记录）" "$(cat /sys/fs/cgroup/cgroup.controllers)"
    else
        fail PF8 "cgroups v2 已挂载（仅记录）" "未挂载"
    fi

    if [ "$PF_HARD" -eq 1 ]; then
        echo "[accept] 前置硬条件不满足（root），后续分组无意义，中止" >&2
        summary
        exit 2
    fi
}

# ───────────────────────── A 组：安装与启动 ─────────────────────────
group_a() {
    section "A 组：安装与启动"

    if [ -n "$INSTALL_EXIT" ]; then
        assert_eq A3 "install.sh 退出码" "$INSTALL_EXIT" "0"
    else
        skip A3 "install.sh 退出码" "未传 --install-exit（本组不是紧跟安装跑的）"
    fi

    en=$(systemctl is-enabled skiff 2>/dev/null || true)
    assert_eq A3a "unit 已 enable" "$en" "enabled"
    ac=$(systemctl is-active skiff 2>/dev/null || true)
    assert_eq A3b "服务 active" "$ac" "active"

    # /healthz 20 s 内 200 —— 断言 body 字段，不只看状态码
    if wait_health 20; then
        pass A4a "GET /healthz 20 s 内 200" "$(cat "$TMP/health.json")"
        assert_contains A4b "healthz 报 status=ok" "$TMP/health.json" '"status":"ok"'
    else
        fail A4a "GET /healthz 20 s 内 200" "20 s 未就绪（服务没起来？先看 journalctl -u skiff）"
    fi

    P=$(entry_prefix)
    if [ -z "$P" ]; then
        fail A5a "安全入口前缀可从启动日志取得" "日志里没找到 /<前缀>/login"
    else
        pass A5a "安全入口前缀" "/$P"
        code=$(curl -s -o "$TMP/entry.html" -D "$TMP/entry.hdr" -w '%{http_code}' "$BASE/$P" 2>/dev/null || true)
        assert_eq A5b "GET /<入口> 未登录跳转" "$code" "303"
        loc=$(sed -n 's/^[Ll]ocation: *//p' "$TMP/entry.hdr" | tr -d '\r')
        case "$loc" in
            */login*) pass A5c "跳转到登录页" "$loc" ;;
            *) fail A5c "跳转到登录页" "Location=$loc" ;;
        esac
        # 裸 / 必须 404：跳转会把随机前缀泄给扫描器（§6.3 隐蔽入口）
        code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/" 2>/dev/null || true)
        assert_eq A5d "GET / 必须 404（不是跳转）" "$code" "404"
    fi

    # 首启产物与权限位（§1.1）
    check_perm A6a "$DATA_DIR" 711 "root:root"
    check_perm A6b "$DATA_DIR/plugins" 711 "root:root"
    check_perm A6c "$DATA_DIR/backup" 700 "root:root"
    check_perm A6d "$DATA_DIR/skiff.db" 600 "root:root"
    check_perm A6e "$CONF" 644 "root:root"
    check_perm A6f "/run/skiff" 711 "root:skiff-plugins"
    [ -S /run/skiff/kernel.sock ] && check_perm A6g "/run/skiff/kernel.sock" 660 "root:skiff-plugins" \
        || skip A6g "kernel.sock 0660 root:skiff-plugins" "socket 不存在（未启或未创建）"
    if [ -f "$DATA_DIR/initial-admin.txt" ]; then
        check_perm A6h "$DATA_DIR/initial-admin.txt" 600 "root:root"
    else
        skip A6h "initial-admin.txt 0600" "文件不在（本机已改过密？）"
    fi

    if skiff health >/dev/null 2>&1; then pass A7 "skiff health 退出码 0" "ok"; else fail A7 "skiff health 退出码 0" "非 0"; fi
    skiff doctor >"$TMP/doctor.log" 2>&1
    rc=$?
    assert_eq A8a "skiff doctor 退出码" "$rc" "0"
    if grep -qi '^.*ERROR' "$TMP/doctor.log" 2>/dev/null; then
        fail A8b "doctor 无不期望 ERROR" "$(grep -i ERROR "$TMP/doctor.log" | head -2 | tr '\n' ' ')"
    else
        pass A8b "doctor 无不期望 ERROR" "-"
    fi

    # 重启恢复，且入口前缀不变（前缀持久化在库里，不该每次变）
    before=$(systemctl show -p MainPID --value skiff)
    systemctl restart skiff >/dev/null 2>&1
    wait_health 20 && pass A9a "systemctl restart 后自恢复" "health 200" \
        || fail A9a "systemctl restart 后自恢复" "20 s 未就绪"
    p2=$(journalctl -u skiff --no-pager -n 200 2>/dev/null | tr -d '\r' \
         | sed -n 's#.*/\([A-Za-z0-9_-]\{4,\}\)/login.*#\1#p' | tail -1)
    if [ -n "$P" ] && [ -n "$p2" ]; then
        assert_eq A9b "重启后入口前缀不变" "$p2" "$P"
    else
        skip A9b "重启后入口前缀不变" "取不到前缀"
    fi

    # kill -9 → Restart=always（RestartSec=3）
    pid=$(systemctl show -p MainPID --value skiff)
    if [ -n "$pid" ] && [ "$pid" != "0" ]; then
        kill -9 "$pid" 2>/dev/null
        n=0; newpid=0
        while [ "$n" -lt 25 ]; do
            sleep 1
            newpid=$(systemctl show -p MainPID --value skiff)
            [ -n "$newpid" ] && [ "$newpid" != "0" ] && [ "$newpid" != "$pid" ] && \
                [ "$(systemctl is-active skiff)" = "active" ] && break
            n=$((n + 1))
        done
        if [ "$newpid" != "$pid" ] && [ "$newpid" != "0" ]; then
            pass A10a "kill -9 后 systemd 拉起" "旧 $pid → 新 $newpid（${n}s）"
        else
            fail A10a "kill -9 后 systemd 拉起" "25 s 内没有新 PID"
        fi
        wait_health 20 >/dev/null 2>&1
    else
        fail A10a "kill -9 后 systemd 拉起" "MainPID 取不到"
    fi

    # 帮助命令不得在当前目录落文件
    mkdir -p "$TMP/empty"
    ( cd "$TMP/empty" && skiff --help >/dev/null 2>&1 && skiff version >/dev/null 2>&1 )
    if [ -z "$(ls -A "$TMP/empty" 2>/dev/null)" ]; then
        pass A11 "--help / version 不落文件" "空目录仍为空"
    else
        fail A11 "--help / version 不落文件" "落了：$(ls -A "$TMP/empty" | tr '\n' ' ')"
    fi
}

# ───────────────────────── B 组：认证与会话 ─────────────────────────
group_b() {
    section "B 组：认证与会话"
    if ! command -v curl >/dev/null 2>&1; then
        skip B0 "curl 可用" "目标机没有 curl，整组无法执行"; return
    fi
    if ! dev_mode; then
        skip B0 "明文 dev 档（Cookie 可回传）" "配置为 TLS/prod 档：__Host- cookie 带 Secure，明文 HTTP 下 curl 不回传（§6.2）；需 --dev 或自签 TLS 档" ; return
    fi
    P=$(entry_prefix)
    if [ -z "$P" ]; then skip B0 "入口前缀" "取不到，整组无法执行"; return; fi
    [ -f "$DATA_DIR/initial-admin.txt" ] || { skip B0 "initial-admin.txt 尚在" "已改过密，首登链路无法重复验证"; return; }

    PW=$(sed -n 's/^口令: *//p' "$DATA_DIR/initial-admin.txt" | head -1 | tr -d '\r')
    JAR="$TMP/jar"; rm -f "$JAR"

    code=$(curl -s -o "$TMP/login.html" -D "$TMP/login.hdr" -c "$JAR" -w '%{http_code}' "$BASE/$P/login")
    assert_eq B1 "GET 登录页" "$code" "200"
    csrf=$(sed -n 's/.*name="csrf" value="\([^"]*\)".*/\1/p' "$TMP/login.html" | head -1)
    if [ -z "$csrf" ]; then fail B1a "登录页含 csrf 隐藏域" "没抓到 csrf"; return; fi
    pass B1a "登录页含 csrf 隐藏域" "len=${#csrf}"

    # B2：首登必须强制改密（跳 settings?force=1）
    code=$(curl -s -o "$TMP/b2.html" -D "$TMP/b2.hdr" -b "$JAR" -c "$JAR" \
        -d "csrf=$csrf" -d "username=admin" --data-urlencode "password=$PW" \
        -w '%{http_code}' "$BASE/$P/login")
    assert_eq B2 "初始口令登录" "$code" "303"
    loc=$(sed -n 's/^[Ll]ocation: *//p' "$TMP/b2.hdr" | tr -d '\r')
    case "$loc" in
        *settings*force=1*) pass B2a "首登强制改密跳转" "$loc" ;;
        *) fail B2a "首登强制改密跳转" "Location=$loc" ;;
    esac
    cp "$JAR" "$TMP/jar.old"

    # B5：缺 CSRF 必须拒（放在改密之前，避免误改状态）
    code=$(curl -s -o "$TMP/b5.html" -b "$JAR" -w '%{http_code}' \
        -d "csrf=bogus" -d "old_password=$PW" --data-urlencode "new_password=whatever-123" \
        --data-urlencode "confirm_password=whatever-123" "$BASE/$P/password")
    assert_eq B5 "改密缺/错 CSRF 被拒" "$code" "403"

    code=$(curl -s -o "$TMP/settings.html" -b "$JAR" -c "$JAR" -w '%{http_code}' "$BASE/$P/settings?force=1")
    assert_eq B3a "GET 设置页（改密入口）" "$code" "200"
    csrf2=$(sed -n 's/.*name="csrf" value="\([^"]*\)".*/\1/p' "$TMP/settings.html" | head -1)
    [ -n "$csrf2" ] || { fail B3b "设置页含 csrf" "没抓到"; return; }

    NEW="skiff-accept-$(date +%s)"
    code=$(curl -s -o "$TMP/b3.html" -D "$TMP/b3.hdr" -b "$JAR" -c "$JAR" \
        -d "csrf=$csrf2" --data-urlencode "old_password=$PW" \
        --data-urlencode "new_password=$NEW" --data-urlencode "confirm_password=$NEW" \
        -w '%{http_code}' "$BASE/$P/password")
    assert_eq B3c "改密成功" "$code" "303"
    if [ -f "$DATA_DIR/initial-admin.txt" ]; then
        fail B3d "改密后 initial-admin.txt 删除" "文件仍在"
    else
        pass B3d "改密后 initial-admin.txt 删除" "已删"
    fi

    # 旧会话失效：用改密前的 cookie 副本访问首页，应被踢回登录页
    code=$(curl -s -o /dev/null -D "$TMP/b3e.hdr" -b "$TMP/jar.old" -w '%{http_code}' "$BASE/$P")
    loc=$(sed -n 's/^[Ll]ocation: *//p' "$TMP/b3e.hdr" | tr -d '\r')
    case "$code:$loc" in
        303:*login*) pass B3e "改密后旧会话失效" "$code → $loc" ;;
        *) fail B3e "改密后旧会话失效" "$code → $loc" ;;
    esac

    # 新会话可用
    code=$(curl -s -o /dev/null -b "$JAR" -w '%{http_code}' "$BASE/$P")
    assert_eq B3f "新会话可用（GET 首页）" "$code" "200"

    if [ "$WITH_RATE_LIMIT" -eq 1 ]; then
        rm -f "$TMP/jar2"
        csrf3=$(curl -s -c "$TMP/jar2" "$BASE/$P/login" \
            | sed -n 's/.*name="csrf" value="\([^"]*\)".*/\1/p' | head -1)
        last_code=""; last_body=""
        i=1
        while [ "$i" -le 6 ]; do
            last_code=$(curl -s -o "$TMP/b4.html" -b "$TMP/jar2" -c "$TMP/jar2" -w '%{http_code}' \
                -d "csrf=$csrf3" -d "username=admin" -d "password=wrong-$i" "$BASE/$P/login")
            last_body=$(tr -d '\n' < "$TMP/b4.html" | head -c 300)
            i=$((i + 1))
        done
        assert_eq B4 "第 6 次错误口令被限流" "$last_code" "503"
        case "$last_body" in
            *秒后重试*) pass B4a "限流文案含剩余秒数" "$(echo "$last_body" | grep -o '请 [0-9]* 秒后重试' | head -1)" ;;
            *) fail B4a "限流文案含剩余秒数" "$last_body" ;;
        esac
        echo "[accept] 注意：B4 已把 admin 锁 5 分钟，后续登录断言会一直被拒" >&2
    else
        skip B4 "登录限流（5 次/5 min）" "未开 --with-rate-limit（会锁 admin 5 分钟）"
    fi
}

# ───────────────────────── D0：Docker 安装链路 ─────────────────────────
group_d0() {
    section "D0：Docker 安装链路"
    if command -v docker >/dev/null 2>&1; then
        if docker info >"$TMP/docker.log" 2>&1; then
            pass D0a "docker 已装且 daemon 可用" "$(docker version --format '{{.Server.Version}}' 2>/dev/null)"
        else
            fail D0a "docker 已装且 daemon 可用" "$(tail -2 "$TMP/docker.log" | tr '\n' ' ')"
        fi
    else
        fail D0a "docker 已装" "docker 命令不存在（install.sh 默认应装；--no-docker 时属预期）"
        skip D0b "docker-bootstrap.sh --check-only" "docker 未装，装前分支不在本轮范围"
        return
    fi

    # --check-only：就绪 exit 0 / 未就绪 exit 2，且不改动任何东西
    boot=""
    if [ -n "$PKG_DIR" ] && [ -f "$PKG_DIR/docker-bootstrap.sh" ]; then
        boot="$PKG_DIR/docker-bootstrap.sh"
    else
        for c in /root/iter/pkg/docker-bootstrap.sh /root/skiff/deploy/docker-bootstrap.sh; do
            [ -f "$c" ] && { boot=$c; break; }
        done
    fi
    if [ -n "$boot" ]; then
        before_sum=$(ls -la /etc/docker 2>/dev/null | md5sum)
        sh "$boot" --check-only >"$TMP/chk.log" 2>&1
        rc=$?
        after_sum=$(ls -la /etc/docker 2>/dev/null | md5sum)
        assert_eq D0b "--check-only 就绪时退出 0" "$rc" "0"
        assert_eq D0c "--check-only 不改动 daemon.json" "$after_sum" "$before_sum"
        # 已装不重装：日志里不能出现重装动作
        if grep -qE 'apt-get (remove|install)|systemctl disable' "$TMP/chk.log"; then
            fail D0d "已装不重装" "日志出现安装/卸载动作"
        else
            pass D0d "已装不重装" "无安装/卸载动作"
        fi
    else
        skip D0b "--check-only 退出 0" "找不到 docker-bootstrap.sh（用 --pkg-dir 指定解包目录）"
    fi
}

# ───────────────────────── 汇总 ─────────────────────────
summary() {
    printf '\n────────────────────────────────────────────\n'
    printf '合计: PASS %d  FAIL %d  SKIP %d\n' "$PASS_N" "$FAIL_N" "$SKIP_N"
    [ -n "$FAILED" ] && printf '失败项:%s\n' "$FAILED"
    printf '────────────────────────────────────────────\n'
    if [ "$SKIP_N" -gt 0 ]; then
        printf '提示：SKIP 不算通过。§5.1 口径是"不满足即 FAIL/SKIP，不能当通过项"。\n'
    fi
}

case ",$GROUPS," in
    *,all,*) RUN="preflight a b d0" ;;
    *) RUN=$(echo "$GROUPS" | tr ',' ' ') ;;
esac

for g in $RUN; do
    case "$g" in
        preflight) preflight ;;
        a) group_a ;;
        b) group_b ;;
        d0) group_d0 ;;
        *) echo "未知分组: $g" >&2; exit 2 ;;
    esac
done

summary
[ "$FAIL_N" -eq 0 ] || exit 1
exit 0
