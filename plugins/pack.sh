#!/bin/sh
# 官方插件打包（POSIX sh）：build → stamp → pack → verify → official/
#
# 用法:
#   sh plugins/pack.sh --key <发布私钥> [--target x86_64-unknown-linux-musl] [--out plugins/official]
#
# 约定：
# - 源码里的 manifest.key_id 是 64 个 0 的占位；本脚本在**临时目录**里用发布私钥
#   回填指纹再打包。这样仓库里不含任何一把密钥的指纹，打包也不会弄脏工作树。
# - 多架构是发布者责任：--target 给交叉目标时，产物只带该架构的 bin/<arch>/plugin。
#   装机侧内核按目标机架构找 `bin/<arch>/plugin`，缺就对不上。
# - 打包机上的 skiff-plugin 必须跑在**打包机**（host）上；插件载荷才是目标架构。
# - 私钥只从参数或 SKIFF_SIGNING_KEY 读，绝不落到产物里（.wbpkg 只含 bin/ui/
#   migrations + manifest + files.sha256 + sig）。
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

KEY="${SKIFF_SIGNING_KEY:-}"
TARGET=""
OUT="$ROOT/plugins/official"
HOST_CARGO=${HOST_CARGO:-cargo}

while [ $# -gt 0 ]; do
    case "$1" in
        --key) KEY="${2:?--key 需要参数}"; shift 2 ;;
        --target) TARGET="${2:?--target 需要参数}"; shift 2 ;;
        --out) OUT="${2:?--out 需要参数}"; shift 2 ;;
        -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
        *) echo "未知参数: $1" >&2; exit 2 ;;
    esac
done

die() { printf '[plugins] 错误: %s\n' "$*" >&2; exit 1; }
log() { printf '[plugins] %s\n' "$*"; }

[ -n "$KEY" ] || die "需要发布私钥：--key <文件> 或环境变量 SKIFF_SIGNING_KEY"
[ -f "$KEY" ] || die "私钥文件不存在: $KEY"

case "${TARGET:-host}" in
    *x86_64*|*amd64*) ARCH=amd64 ;;
    *aarch64*|*arm64*) ARCH=arm64 ;;
    *) ARCH=$(uname -m | sed 's/x86_64/amd64/; s/aarch64/arm64/') ;;
esac

# 插件构建走交叉工具链（CI 里 CARGO=cross）；打包工具本身必须能在打包机上跑。
PLUGIN_CARGO=${CARGO:-cargo}
TOOL_DIR="$ROOT/target/release"
TOOL="$TOOL_DIR/skiff-plugin"
if [ ! -f "$TOOL" ] && [ ! -f "$TOOL.exe" ]; then
    log "构建 skiff-plugin（打包工具，host 架构）"
    "$HOST_CARGO" build --release --locked -p skiff-sdk --bin skiff-plugin
fi
[ -f "$TOOL" ] || TOOL="$TOOL.exe"
[ -f "$TOOL" ] || die "未找到打包工具: $TOOL"

mkdir -p "$OUT"
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT INT TERM

PLUGINS="guard monitor nginx files cron"
BUILT=0
PUBKEY=""
for name in $PLUGINS; do
    dir="plugins/$name"
    [ -f "$dir/manifest.toml" ] || die "缺少 $dir/manifest.toml"

    log "构建 skiff-plugin-$name（${TARGET:-host}）"
    (
        cd plugins
        # shellcheck disable=SC2086
        "$PLUGIN_CARGO" build --release --locked -p "skiff-plugin-$name" ${TARGET:+--target "$TARGET"}
    )

    src="plugins/target${TARGET:+/$TARGET}/release/plugin"
    [ -f "$src" ] || src="$src.exe"
    [ -f "$src" ] || die "未找到构建产物: $src"

    # 只拷进包的三类载荷（bin/ui/migrations）+ manifest，避免把 target/ 或私钥带进去。
    work="$STAGE/$name"
    mkdir -p "$work"
    cp "$dir/manifest.toml" "$work/"
    cp -R "$dir/ui" "$work/"
    if [ -d "$dir/migrations" ]; then
        cp -R "$dir/migrations" "$work/"
    fi
    mkdir -p "$work/bin/$ARCH"
    cp "$src" "$work/bin/$ARCH/plugin"
    chmod 0755 "$work/bin/$ARCH/plugin"

    version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$dir/manifest.toml" | head -1)
    [ -n "$version" ] || die "$dir/manifest.toml 缺少 version"

    log "回填 key_id 并打包 $name $version ($ARCH)"
    pub=$("$TOOL" stamp --dir "$work" --key "$KEY" | sed -n 's/^公钥(hex): //p')
    [ -n "$pub" ] || die "stamp 未输出公钥（$name）"

    pkg="$OUT/com.skiff.$name-$version.wbpkg"
    "$TOOL" pack --dir "$work" --key "$KEY" --arch "$ARCH" --out "$pkg"
    # 交叉打包时打包机架构与载荷架构不同，必须显式 --arch，否则 verify 会按主机
    # 架构找 bin/<arch>/plugin 而误报「缺载荷」。
    "$TOOL" verify "$pkg" --pubkey "$pub" --arch "$ARCH" >/dev/null
    log "已验签自检通过: $(basename "$pkg")"
    PUBKEY="$pub"
    BUILT=$((BUILT + 1))
done

[ "$BUILT" -gt 0 ] || die "一个插件都没打出来"
# 公钥落一份在 official/：装机侧必须把它导入面板「设置 → 信任公钥」才能装包
# （TOFU 首次信任）。公钥本来就是公开信息，放在发布包里不构成泄密。
printf '%s\n' "$PUBKEY" > "$OUT/public-key.txt"
log "完成：$BUILT 个官方插件 → $OUT"
# 日志里也给一次，省得再回打包机翻私钥
log "官方插件签名公钥(hex): $PUBKEY"
ls -1 "$OUT"/*.wbpkg
