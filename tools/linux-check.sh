#!/bin/sh
# 本地 Linux 目标编译级复核（ITERATION.md §8 / L1 门禁的一部分）
#
# 为什么需要它：开发机是 Windows，`#[cfg(unix)]` 分支在 Windows 三关里**根本不参与编译**。
# 2026-10-31 与 11-01 各踩一次：Windows 全绿、Linux 四个 job 同根因红。
# 这条命令把「改完 cfg(unix) 代码后推 CI 才发现」前移到本地，约 30~60 s。
#
# 口径说明：`cargo check` 只产元数据、**不链接**，所以 C 依赖（ring / zstd-sys /
# rusqlite）不需要真的编出 Linux 目标文件——在 Windows 上用 mingw 的 gcc 冒充交叉 cc
# 就能过 build script。这也意味着它**只挡编译错误，不证明能跑**：
# 单测执行交给 CI 的 `test` job，装机验证交给测试机。
#
# 用法: sh tools/linux-check.sh [--quick]
#   --quick   只查主 workspace（跳过 plugins/ 第二 workspace）
set -eu

TARGET=x86_64-unknown-linux-gnu
WITH_PLUGINS=1
while [ $# -gt 0 ]; do
    case "$1" in
        --quick) WITH_PLUGINS=0; shift ;;
        -h|--help) sed -n '2,/^set -eu$/p' "$0" | sed '$d'; exit 0 ;;
        *) echo "未知参数: $1" >&2; exit 2 ;;
    esac
done

cd "$(dirname "$0")/.."

# std 没装时别让 cargo 自己报一堆 E0463：直接给出可执行的下一步
if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
    cat >&2 <<EOF
[linux-check] 目标 std 未安装：$TARGET
  国内网络建议：
    RUSTUP_DIST_SERVER=https://rsproxy.cn rustup target add $TARGET
EOF
    exit 2
fi

# Windows（MSYS/MinGW/Cygwin）上为 Linux 目标编 C：用 mingw gcc 冒充交叉 cc。
# 见文件头：check 不链接，C 产物不被消费。
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        CC_BIN=""
        for c in /c/mingw/mingw64/bin/gcc.exe /mingw64/bin/gcc.exe; do
            [ -x "$c" ] && { CC_BIN=$c; break; }
        done
        if [ -z "$CC_BIN" ]; then
            echo "[linux-check] 找不到 mingw gcc，C 依赖会在 build script 阶段失败" >&2
            echo "[linux-check] 装一个（scoop install mingw 或 MSYS2 的 mingw-w64-gcc）后重试" >&2
            exit 2
        fi
        BIN_DIR=$(dirname "$CC_BIN")
        export CC_x86_64_unknown_linux_gnu="$CC_BIN"
        export AR_x86_64_unknown_linux_gnu="$BIN_DIR/ar.exe"
        export RANLIB_x86_64_unknown_linux_gnu="$BIN_DIR/ranlib.exe"
        # MSYS 的增量模式会刷「删除 lock 文件 拒绝访问」的假 warning，关掉才看得清真实输出
        export CARGO_INCREMENTAL=0
        echo "[linux-check] 交叉 cc: $CC_BIN"
        ;;
esac

echo "== 主 workspace（--workspace --all-targets，含 tests/ 与 examples）=="
cargo check --workspace --all-targets --locked --target "$TARGET"

if [ "$WITH_PLUGINS" -eq 1 ] && [ -f plugins/Cargo.toml ]; then
    echo "== plugins/（第二 workspace，插件侧 cfg(unix) 同样只在 Linux 编）=="
    cargo check --manifest-path plugins/Cargo.toml --workspace --all-targets \
        --locked --target "$TARGET"
fi

echo "[linux-check] 通过：$TARGET 编译级无错（运行行为仍需 CI test job 与测试机）"
