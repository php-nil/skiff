#!/bin/sh
# Skiff 发布打包（POSIX sh）
#
# 产出（与 §5.1 交付物一致）：
#   dist/skiff-<ver>-linux-<arch>.tar.gz
#   dist/skiff-<ver>-linux-<arch>.tar.gz.sha256     # 摘要，装机前先校验
#   dist/skiff-<ver>-linux-<arch>.tar.gz.sig        # detached ed25519 签名（hex）
#
# 安装顺序固定：先校验摘要与签名，再解包执行 install.sh。
# 包内脚本不校验包含自己的压缩包（自校验悖论）。
#
# 用法:
#   deploy/pack-release.sh [--version 0.1.0] [--target x86_64-unknown-linux-musl] \
#                          [--key release.key] [--out dist]
set -eu

VERSION=0.1.0
TARGET=""
KEY=""
OUT=dist

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --target) TARGET="$2"; shift 2 ;;
        --key) KEY="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
        *) echo "未知参数: $1" >&2; exit 2 ;;
    esac
done

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

# 交叉编译时用 `CARGO=cross` 覆写（CI 里 aarch64 / musl 都走这条）。
# 注意：签名工具始终用**宿主**的 `cargo` 构建 —— 它要在本机签本机的文件，
# 交叉出来的 aarch64 二进制 runner 上跑不了。
CARGO_BIN=${CARGO:-cargo}

case "${TARGET:-host}" in
    *x86_64*|*amd64*) ARCH=amd64 ;;
    *aarch64*|*arm64*) ARCH=arm64 ;;
    *) ARCH=$(uname -m | sed 's/x86_64/amd64/; s/aarch64/arm64/') ;;
esac

BUILD_ARGS="--release --locked -p skiff --bin skiff"
if [ -n "$TARGET" ]; then
    BUILD_ARGS="$BUILD_ARGS --target $TARGET"
fi

echo "== 构建 skiff $VERSION (${TARGET:-host}) =="
# shellcheck disable=SC2086
"$CARGO_BIN" build $BUILD_ARGS

# 插件工具也进包：安装/打包/验签 .wbpkg 都靠它，缺了它 E 组用例在目标机上跑不起来。
TOOL_ARGS="--release --locked -p skiff-sdk --bin skiff-plugin"
if [ -n "$TARGET" ]; then
    TOOL_ARGS="$TOOL_ARGS --target $TARGET"
fi
# shellcheck disable=SC2086
"$CARGO_BIN" build $TOOL_ARGS

BIN="$ROOT/target/${TARGET:+$TARGET/}release/skiff"
if [ ! -f "$BIN" ] && [ -f "$BIN.exe" ]; then
    # 开发机（Windows/MSYS）上产物带 .exe：允许跑通打包流程做冒烟，
    # 但这样打出来的包**不是 Linux 发行物**，只能用于校验脚本与签名链路。
    BIN="$BIN.exe"
    echo "注意：当前是 $(uname -s) 主机，用 $(basename "$BIN") 做打包冒烟；正式发布请在 Linux 上执行。" >&2
fi
[ -f "$BIN" ] || { echo "未找到产物: $BIN" >&2; exit 1; }

NAME="skiff-$VERSION-linux-$ARCH"
STAGE="$OUT/$NAME"
rm -rf "$STAGE"
mkdir -p "$STAGE"

TOOL="$ROOT/target/${TARGET:+$TARGET/}release/skiff-plugin"
[ -f "$TOOL" ] || TOOL="$TOOL.exe"

cp "$BIN" "$STAGE/skiff"
[ -f "$TOOL" ] && cp "$TOOL" "$STAGE/skiff-plugin"
cp deploy/install.sh deploy/uninstall.sh deploy/docker-bootstrap.sh \
   deploy/systemd/skiff.service deploy/config.toml.example "$STAGE/"
chmod 0755 "$STAGE/skiff" "$STAGE/install.sh" "$STAGE/uninstall.sh" "$STAGE/docker-bootstrap.sh"
[ -f "$STAGE/skiff-plugin" ] && chmod 0755 "$STAGE/skiff-plugin"

cat > "$STAGE/README.txt" <<EOF
Skiff（轻舟）$VERSION — linux/$ARCH

安装（先校验，再解包）：
  sha256sum -c $NAME.tar.gz.sha256
  # 有发布公钥时：
  #   skiff-plugin verify-file $NAME.tar.gz --pubkey <发布公钥hex>
  tar xzf $NAME.tar.gz && cd $NAME && sh install.sh

包内文件：
  skiff                 面板主程序（/usr/local/bin/skiff）
  skiff-plugin          插件打包/验签工具（可选，install.sh 会一并装上）
  install.sh            安装器
  docker-bootstrap.sh   Docker 前置引导（install.sh 会调用；也可单独补跑）
  uninstall.sh / skiff.service / config.toml.example

Docker：装机时默认检测并补装（缺才装，已有 docker 绝不重装）。
  sh install.sh --registry-mirror <加速器地址>   # 配镜像加速（无默认值，按需给）
  sh install.sh --no-docker                      # 完全不碰 Docker
  sh docker-bootstrap.sh --check-only            # 只检测，不改动

升级：替换 /usr/local/bin/skiff 后 systemctl restart skiff（数据不动）。
卸载：sh uninstall.sh（默认保留 /var/lib/skiff，--purge 才删除）。
EOF

TARBALL="$OUT/$NAME.tar.gz"
echo "== 打包 =="
# 固定 mtime/属主，保证同输入产出同摘要
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='UTC 2026-01-01' \
    -czf "$TARBALL" -C "$OUT" "$NAME"

echo "== 摘要 =="
( cd "$OUT" && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256" )
cat "$OUT/$NAME.tar.gz.sha256"

if [ -n "$KEY" ]; then
    echo "== 签名 =="
    SIGNER="${SIGNER:-target/release/skiff-plugin}"
    if [ ! -x "$SIGNER" ] && [ ! -x "$SIGNER.exe" ]; then
        cargo build --release --locked -p skiff-sdk --bin skiff-plugin
    fi
    # 开发机上的可执行文件带 .exe 后缀
    [ -x "$SIGNER" ] || SIGNER="$SIGNER.exe"
    "$SIGNER" sign-file "$TARBALL" --key "$KEY"
    echo "签名: $TARBALL.sig"
else
    echo "跳过签名（未提供 --key）。**正式发布必须签名**：没有 .sig 的包属于未签名发布物。"
fi

echo
echo "完成: $TARBALL"
