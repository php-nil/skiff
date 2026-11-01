#!/bin/sh
# Skiff 发布打包（POSIX sh）
#
# 产出（与 §5.1 交付物一致）：
#   dist/skiff-<ver>-linux-<arch>.tar.gz
#   dist/skiff-<ver>-linux-<arch>.tar.gz.sha256     # 摘要，装机前先校验
#
# 发布物**不做 detached 签名**（§11 决策 12）：只用 SHA-256 摘要防传输损坏，
# 签名链路（--key / .sig / verify-file）已于 2026-11-01 整体移除。
#
# 官方插件（guard/monitor/nginx/files/cron）随包携带：调用 plugins/pack.sh 产出已签名
# `.wbpkg`，放进发布包的 official/；install.sh 再复制到 /var/lib/skiff/official/
# （不自动安装，权限确认不能跳过）。插件打包必须给 --plugin-key，否则本脚本拒绝
# 出包（要出「不带插件」的包请显式 --skip-plugins）。
#
# 安装顺序固定：先校验摘要，再解包执行 install.sh。
# 包内脚本不校验包含自己的压缩包（自校验悖论）。
#
# 用法:
#   deploy/pack-release.sh [--version 0.1.0] [--target x86_64-unknown-linux-musl] \
#                          [--plugin-key <私钥文件>] [--skip-plugins] [--out dist]
set -eu

VERSION=0.1.0
TARGET=""
OUT=dist
PLUGIN_KEY=""
SKIP_PLUGINS=0

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --target) TARGET="$2"; shift 2 ;;
        --plugin-key) PLUGIN_KEY="${2:?--plugin-key 需要参数}"; shift 2 ;;
        --plugin-key=*) PLUGIN_KEY="${1#*=}"; shift ;;
        --skip-plugins) SKIP_PLUGINS=1; shift ;;
        --out) OUT="$2"; shift 2 ;;
        -h|--help) sed -n '2,/^set -eu$/p' "$0" | sed '$d'; exit 0 ;;
        *) echo "未知参数: $1" >&2; exit 2 ;;
    esac
done

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

# 交叉编译时用 `CARGO=cross` 覆写（CI 里 aarch64 / musl 都走这条）。
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
    # 但这样打出来的包**不是 Linux 发行物**，只能用于校验脚本本身。
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

# 官方插件：与主程序同架构构建 → 发布私钥回填指纹 → 打包 → 自检验签。
if [ "$SKIP_PLUGINS" -eq 0 ]; then
    [ -n "$PLUGIN_KEY" ] || {
        echo "错误: 发布包必须携带官方插件（guard/monitor/nginx/files/cron）。" >&2
        echo "      请给 --plugin-key <发布私钥>；确实要出裸包请显式 --skip-plugins。" >&2
        exit 2
    }
    [ -f "$PLUGIN_KEY" ] || { echo "错误: 插件私钥不存在: $PLUGIN_KEY" >&2; exit 2; }
    # 先清掉上一轮产物：amd64 / arm64 是两台机器（或同一台的两轮）分别打，
    # 残留的别架构 .wbpkg 会被一起塞进发布包，装机时按 bin/<arch>/plugin 找载荷
    # 直接对不上，而且这是个静默的坏包。
    rm -rf "$ROOT/plugins/official"
    echo "== 打包官方插件 ($ARCH) =="
    # shellcheck disable=SC2086
    sh plugins/pack.sh --key "$PLUGIN_KEY" ${TARGET:+--target "$TARGET"} \
        --out "$ROOT/plugins/official"
fi

cp "$BIN" "$STAGE/skiff"
if [ -f "$TOOL" ]; then
    cp "$TOOL" "$STAGE/skiff-plugin"
fi
cp deploy/install.sh deploy/uninstall.sh deploy/docker-bootstrap.sh \
   deploy/systemd/skiff.service deploy/config.toml.example "$STAGE/"
chmod 0755 "$STAGE/skiff" "$STAGE/install.sh" "$STAGE/uninstall.sh" "$STAGE/docker-bootstrap.sh"
if [ -f "$STAGE/skiff-plugin" ]; then
    chmod 0755 "$STAGE/skiff-plugin"
fi

if [ "$SKIP_PLUGINS" -eq 0 ]; then
    mkdir -p "$STAGE/official"
    cp "$ROOT"/plugins/official/*.wbpkg "$STAGE/official/"
    chmod 0644 "$STAGE"/official/*.wbpkg
    # 公钥随包发出：装机时必须导入面板信任库才能装这些包
    if [ -f "$ROOT/plugins/official/public-key.txt" ]; then
        cp "$ROOT/plugins/official/public-key.txt" "$STAGE/official/public-key.txt"
        chmod 0644 "$STAGE/official/public-key.txt"
    fi
    echo "已放入 $(ls -1 "$STAGE"/official/*.wbpkg | wc -l) 个官方插件包 → official/"
fi

cat > "$STAGE/README.txt" <<EOF
Skiff（轻舟）$VERSION — linux/$ARCH

安装（先校验摘要，再解包）：
  sha256sum -c $NAME.tar.gz.sha256
  tar xzf $NAME.tar.gz && cd $NAME && sh install.sh

包内文件：
  skiff                 面板主程序（/usr/local/bin/skiff）
  skiff-plugin          插件打包/验签工具（可选，install.sh 会一并装上）
  official/             官方插件包（guard/monitor/nginx/files/cron，install.sh 复制到
                        /var/lib/skiff/official/，不自动安装）
  install.sh            安装器
  docker-bootstrap.sh   Docker 前置引导（install.sh 会调用；也可单独补跑）
  uninstall.sh / skiff.service / config.toml.example

Docker：装机时默认检测并补装（缺才装，已有 docker 绝不重装）。
  sh install.sh --registry-mirror <加速器地址>   # 配镜像加速（无默认值，按需给）
  sh install.sh --no-docker                      # 完全不碰 Docker
  sh docker-bootstrap.sh --check-only            # 只检测，不改动

升级：替换 /usr/local/bin/skiff 后 systemctl restart skiff（数据不动）。
卸载：sh uninstall.sh（默认保留 /var/lib/skiff，--purge 才删除）。

官方插件（随包携带，安装时才逐条确认权限）：
  导入公钥：把 official/public-key.txt 里的 hex 填进面板「设置 → 信任公钥」
            （TOFU 首次信任；不导入会被拒装，指纹见各包 manifest.key_id）
  ls /var/lib/skiff/official
  skiff plugin install /var/lib/skiff/official/com.skiff.<x>-<ver>.wbpkg \
      --resource path:/var/www          # 也可以直接在面板插件页上传安装
EOF

TARBALL="$OUT/$NAME.tar.gz"
echo "== 打包 =="
# 固定 mtime/属主，保证同输入产出同摘要
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='UTC 2026-01-01' \
    -czf "$TARBALL" -C "$OUT" "$NAME"

echo "== 摘要 =="
( cd "$OUT" && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256" )
cat "$OUT/$NAME.tar.gz.sha256"

echo
echo "完成: $TARBALL"
