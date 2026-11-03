#!/bin/sh
# Skiff（轻舟）单轮迭代编排（在**开发机**上跑；测试机侧只跑 install.sh 与 deploy/accept.sh）
#
# 子命令：
#   gate                     L1 门禁：fmt / clippy / test / Linux target check
#   fetch [--run <id>]       从 GitHub Actions 取 amd64 产物到 dist/iter/x（默认取最近成功的 run）
#   run --pkg <目录> [--group a,b] [--rate-limit]
#                            装到测试机并出验收表：preflight → install.sh → accept.sh（计时）
#   rollback-wait [--expect T0|T1]
#                            §4.1 回滚协同：输出请求 → 轮询 SSH 与 /proc/uptime → 校时
#
# 凭据：source .workbuddy/server.env（gitignore 已覆盖；仓库是 public，密码绝不入库）
#
# 实跑状态（2026-11-03）：gate / fetch / run 三个子命令在首装轮实际用过；
# rollback-wait **尚未有真回滚可等**，逻辑按 §4.1 写、未实测，别当已验证。
#
# 已知环境限制：本机与测试机都只通 api.github.com，github.com:443 被拦
# （schannel 握手失败 / OpenSSL unexpected eof），所以 `git push` 走不通 ——
# 本脚本**不含** dispatch 子命令：包是从已有 run 的 artifact 取的。
# 恢复推送能力后，iter.yml 可直接 workflow_dispatch（见 docs/CI.md）。
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

# shellcheck source=/dev/null
[ -f .workbuddy/server.env ] && . .workbuddy/server.env

: "${SKIFF_TEST_HOST:?缺少 SKIFF_TEST_HOST（.workbuddy/server.env）}"
: "${SKIFF_TEST_USER:=root}"
: "${SKIFF_TEST_PORT:=22}"
: "${SKIFF_TEST_KEY:=$HOME/.ssh/skiff-iter}"
# env 文件里写的是 ~/.ssh/... —— 引号外的 ~ 不会展开，这里手动补上
# shellcheck disable=SC2088  # 这里的 ~ 是**要匹配的字面前缀**，不是要展开的路径
case "$SKIFF_TEST_KEY" in
    "~/"*) SKIFF_TEST_KEY="$HOME/${SKIFF_TEST_KEY#\~/}" ;;
esac
REMOTE_DIR=/root/iter

SSH="ssh -i $SKIFF_TEST_KEY -p $SKIFF_TEST_PORT -o BatchMode=yes -o StrictHostKeyChecking=no"
SCP="scp -i $SKIFF_TEST_KEY -P $SKIFF_TEST_PORT -o StrictHostKeyChecking=no"
TARGET="$SKIFF_TEST_USER@$SKIFF_TEST_HOST"

# GitHub API 需要代理时在这里指定（本机 github.com 不通、api 通，2026-11-03 实测）
GH_PROXY=${GH_PROXY:-}

gh_api() { # gh_api <path> [curl 额外参数...]
    _path=$1; shift
    curl -sS ${GH_PROXY:+-x "$GH_PROXY"} \
        -H "Authorization: Bearer $(gh_token)" \
        -H "Accept: application/vnd.github+json" "$@" \
        "https://api.github.com/repos/${GH_REPO:-php-nil/skiff}$_path"
}

gh_token() {
    if [ -n "${GITHUB_TOKEN:-}" ]; then printf '%s' "$GITHUB_TOKEN"; return; fi
    printf 'protocol=https\nhost=github.com\n\n' | git credential fill 2>/dev/null \
        | sed -n 's/^password=//p'
}

cmd_gate() {
    echo "== 1/4 fmt =="; cargo fmt --all --check
    echo "== 2/4 clippy =="
    CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    echo "== 3/4 test =="
    CARGO_INCREMENTAL=0 cargo test --workspace --locked
    echo "== 4/4 Linux target check =="
    sh tools/linux-check.sh
    echo "[gate] 全绿"
}

cmd_fetch() {
    run_id=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --run) run_id="$2"; shift 2 ;;
            --run=*) run_id="${1#*=}"; shift ;;
            *) echo "未知参数: $1" >&2; exit 2 ;;
        esac
    done
    if [ -z "$run_id" ]; then
        run_id=$(gh_api "/actions/runs?per_page=20" \
            | tr ',' '\n' | sed -n 's/.*"id":\([0-9]*\).*/\1/p' | head -1)
        [ -n "$run_id" ] || { echo "[fetch] 取不到 run id" >&2; exit 1; }
    fi
    echo "[fetch] run $run_id"
    art=$(gh_api "/actions/runs/$run_id/artifacts" \
        | tr '{' '\n' | grep 'release-amd64\|iter-amd64' \
        | sed -n 's/.*"id":\([0-9]*\).*/\1/p' | head -1)
    [ -n "$art" ] || { echo "[fetch] 该 run 没有 amd64 产物" >&2; exit 1; }

    mkdir -p dist/iter
    url=$(gh_api "/actions/artifacts/$art/zip" -o /dev/null -w '%{redirect_url}')
    echo "[fetch] 下载 artifact $art"
    # 走测试机下载更快（本机到 blob 只有十几 KB/s，13 MB 要十几分钟；2026-11-03 实测）
    $SSH "$TARGET" "mkdir -p $REMOTE_DIR && cd $REMOTE_DIR && curl -sS -L --max-time 600 -o art.zip '$url' && echo fetched"
    $SSH "$TARGET" "cd $REMOTE_DIR && rm -rf x && mkdir x && unzip -qo art.zip -d x && find x -maxdepth 2 -name skiff -o -maxdepth 2 -name 'install.sh' | sort"
    echo "[fetch] 包在测试机 $REMOTE_DIR/x 下；本地只留一份清单以避免 13 MB 跨境搬运"
}

cmd_run() {
    pkg=""
    group="all"
    extra=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --pkg) pkg="$2"; shift 2 ;;
            --pkg=*) pkg="${1#*=}"; shift ;;
            --group) group="$2"; shift 2 ;;
            --group=*) group="${1#*=}"; shift ;;
            --rate-limit) extra="$extra --with-rate-limit"; shift ;;
            *) echo "未知参数: $1" >&2; exit 2 ;;
        esac
    done
    [ -n "$pkg" ] || { echo "[run] 需要 --pkg <测试机上的解包目录，如 /root/iter/x/skiff-night-linux-amd64>" >&2; exit 2; }

    now=$(date +%s)
    $SCP deploy/accept.sh "$TARGET:$REMOTE_DIR/accept.sh"
    echo "== preflight（装前）=="
    $SSH "$TARGET" "sh $REMOTE_DIR/accept.sh --group preflight --pre-install --now $now" || {
        echo "[run] 前置断言未过，中止（不要带着不合格环境继续）" >&2; exit 1; }

    echo "== install.sh =="
    s=$(date +%s)
    # shellcheck disable=SC2086
    rc=$($SSH "$TARGET" "cd $pkg && sh install.sh >/root/iter/install.log 2>&1; echo \$?") || rc=1
    e=$(date +%s)
    echo "[run] INSTALL_EXIT=$rc 用时 $((e - s))s（日志 $REMOTE_DIR/install.log）"

    echo "== accept.sh =="
    s=$(date +%s)
    # shellcheck disable=SC2086
    rc2=0
    $SSH "$TARGET" "sh $REMOTE_DIR/accept.sh --group $group --install-exit $rc --pkg-dir $pkg --now $(date +%s)$extra" || rc2=$?
    e=$(date +%s)
    echo "[run] ACCEPT_EXIT=$rc2 用时 $((e - s))s"
    exit "$rc2"
}

cmd_rollback_wait() {
    expect=""
    [ "${1:-}" = "--expect" ] && expect="${2:-}"
    echo "【需要回滚】请把测试机恢复到 ${expect:-T0/T1}"
    echo "  判据：SSH 可连 且 /proc/uptime 首字段 < 180（连续 300 s 未恢复即报错，不静默继续）"
    waited=0
    while [ "$waited" -lt 300 ]; do
        if up=$($SSH -o ConnectTimeout=5 "$TARGET" 'cut -d" " -f1 /proc/uptime' 2>/dev/null); then
            up_i=${up%%.*}
            if [ "${up_i:-999999}" -lt 180 ]; then
                echo "[rollback] 已恢复，uptime ${up}s"
                # 强制校时：快照回退会把时钟拉回过去，TOTP 与 session 判定都会错乱
                $SSH "$TARGET" 'systemctl restart systemd-timesyncd 2>/dev/null || true; sleep 2; date -u +%FT%TZ'
                echo "[rollback] 请对照本机时间确认偏差 < 30 s，再继续"
                return 0
            fi
        fi
        sleep 5
        waited=$((waited + 5))
    done
    echo "[rollback] 300 s 内没恢复，中止（在旧状态上跑出的是假绿）" >&2
    exit 1
}

sub=${1:-}
[ $# -gt 0 ] && shift
case "$sub" in
    gate) cmd_gate ;;
    fetch) cmd_fetch "$@" ;;
    run) cmd_run "$@" ;;
    rollback-wait) cmd_rollback_wait "$@" ;;
    *) sed -n '2,/^set -eu$/p' "$0" | sed '$d'; exit 2 ;;
esac
