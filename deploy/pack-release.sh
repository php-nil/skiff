#!/bin/sh
# Skiff 发布打包（POSIX sh）
#
# 产出（与 §5.1 交付物一致；交付路径 = 从 Release 下载 → 解压 → 跑 install.sh）：
#   dist/skiff-<ver>-<target>.tar.gz
#   dist/skiff-<ver>-<target>.tar.gz.sha256     # 单包摘要，装机前先校验
#   （<target> 未指定时退化为 linux-<arch>）
#
# 安装包内至少含：skiff、skiff-plugin、install.sh、uninstall.sh、skiff.service、
# config.toml.example、official/*.wbpkg、official/public-key.txt、
# vendor/install-php-extensions、checksums.txt、README.txt。
# checksums.txt 覆盖包内除自身外的所有文件。
#
# 发布物**不做 detached 签名**（§11 决策 12）：只用 SHA-256 摘要防传输损坏，
# 签名链路（--key / .sig / verify-file）已于 2026-11-01 整体移除。
#
# 官方插件包 7 个（6 个插件 guard/monitor/files/cron/ftp/firewall + testkit）随包携带：
# 调用 plugins/pack.sh 产出已签名
# `.wbpkg`，放进发布包的 official/；install.sh 再复制到 /www/skiff/official/
# （不自动安装，权限确认不能跳过）。插件签名私钥按当前决定**非强制**：
# 给了 --plugin-key 用正式身份，没给则生成临时身份并告警（产物仅供测试）。
#
# 安装顺序固定：先校验 checksums.txt，再解包执行 install.sh。
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

# 命名带 target（与 CI 产物一致）：skiff-<ver>-x86_64-unknown-linux-musl.tar.gz
NAME="skiff-$VERSION-${TARGET:-linux-$ARCH}"
STAGE="$OUT/$NAME"
rm -rf "$STAGE"
mkdir -p "$STAGE"

TOOL="$ROOT/target/${TARGET:+$TARGET/}release/skiff-plugin"
[ -f "$TOOL" ] || TOOL="$TOOL.exe"

# 官方插件：与主程序同架构构建 → 发布私钥回填指纹 → 打包 → 自检验签。
if [ "$SKIP_PLUGINS" -eq 0 ]; then
    [ -f "$TOOL" ] || { echo "错误: 缺少打包工具 skiff-plugin: $TOOL" >&2; exit 2; }
    EPHEMERAL_KEY=""
    EPHEMERAL_DIR=""
    if [ -z "$PLUGIN_KEY" ]; then
        # 按当前决定：插件签名非强制，缺私钥不阻塞出包。临时身份可让包直接装，
        # 但签名人不稳定（每次出包都变），仅供测试/验证打包链路。
        # 注意：keygen 拒绝覆盖已存在的文件，所以用临时目录 + 不存在的文件名。
        EPHEMERAL_DIR=$(mktemp -d "${TMPDIR:-/tmp}/skiff-ephemeral-key.XXXXXX")
        EPHEMERAL_KEY="$EPHEMERAL_DIR/official.key"
        # shellcheck disable=SC2064
        trap 'rm -rf "$EPHEMERAL_DIR"' EXIT INT TERM
        "$TOOL" keygen --out "$EPHEMERAL_KEY" >/dev/null 2>&1 \
            || { echo "错误: 生成临时插件签名身份失败（$TOOL keygen --out <文件>）" >&2; exit 2; }
        PLUGIN_KEY="$EPHEMERAL_KEY"
        echo "警告: 未提供 --plugin-key，已生成临时签名身份；产物仅供测试，正式发布请配置正式私钥。" >&2
    fi
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

# 固定版本 PHP 扩展安装器（mlocati）：随包分发，install.sh 复制到
# /www/skiff/tools/，运行时不再从 GitHub 拉取。
[ -f deploy/vendor/install-php-extensions ] \
    || { echo "错误: 缺少 deploy/vendor/install-php-extensions（固定版本扩展安装器）" >&2; exit 2; }
grep -q "IPE_VERSION=2.13.2" deploy/vendor/install-php-extensions \
    || { echo "错误: 随包扩展安装器不是正式 release 2.13.2（禁止 master 副本）" >&2; exit 2; }
mkdir -p "$STAGE/vendor"
cp deploy/vendor/install-php-extensions "$STAGE/vendor/install-php-extensions"
chmod 0755 "$STAGE/vendor/install-php-extensions"

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
Skiff（轻舟）$VERSION — ${TARGET:-linux-$ARCH}

安装（交付路径固定：下载 → 校验 → 解压 → 安装）：
  sha256sum -c $NAME.tar.gz.sha256          # 也可用 Release 的 checksums.txt 一起校
  tar xzf $NAME.tar.gz
  cd $NAME && sh install.sh                 # 在解压目录内直接执行，不依赖仓库结构

包内文件：
  skiff                 面板主程序（/usr/local/bin/skiff）
  skiff-plugin          插件打包/验签工具（可选，install.sh 会一并装上）
  official/             官方插件包 7 个（guard/monitor/files/cron/ftp/firewall + testkit，
                        install.sh 复制到
                        /www/skiff/official/，不自动安装未装插件；已装插件由内核启动时
                        按版本/内容同步，保留 data/ 与既有授权）+ public-key.txt
  install.sh            安装器（支持 --datadir / --listen / SKIFF_LISTEN / 幂等重装）
  docker-bootstrap.sh   Docker 前置引导（install.sh 会调用；也可单独补跑）
  vendor/install-php-extensions
                       固定版本 PHP 扩展安装器（install.sh 复制到 /www/skiff/tools/）
  uninstall.sh / skiff.service / config.toml.example
  checksums.txt         本包内所有文件的 SHA-256（解压后 sha256sum -c 可复核）

Docker：装机时默认检测并补装（缺才装，已有 docker 绝不重装）。
  sh install.sh --registry-mirror <加速器地址>   # 配镜像加速（无默认值，按需给）
  sh install.sh --no-docker                      # 完全不碰 Docker
  sh docker-bootstrap.sh --check-only            # 只检测，不改动

升级：从 Release 取同架构新包 → 解压 → sh install.sh（数据与配置不动）。
卸载：sh uninstall.sh（默认保留 /www/skiff，--purge 才删除）。

官方插件（随包携带，安装时才逐条确认权限）：
  公钥由首启自动信任（official/public-key.txt，TOFU 首次信任），无需手动导入
  ls /www/skiff/official
  skiff plugin install /www/skiff/official/com.skiff.<x>-<ver>.wbpkg \\
      --resource path:/var/www          # 也可以直接在面板插件页上传安装
  注：若本次出包未提供正式签名私钥，official/ 内的包由临时身份签名，
      仅用于测试；正式发行请使用正式私钥重新出包。
EOF

# 包内摘要：覆盖包内除 checksums.txt 自身以外的所有文件（自校验悖论）
echo "== 包内摘要 =="
( cd "$STAGE" && find . -type f ! -name checksums.txt -print | sed 's|^\./||' | sort \
    | xargs sha256sum > checksums.txt )
cat "$STAGE/checksums.txt"

TARBALL="$OUT/$NAME.tar.gz"
echo "== 打包 =="
# 固定 mtime/属主，保证同输入产出同摘要
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='UTC 2026-01-01' \
    -czf "$TARBALL" -C "$OUT" "$NAME"
# 暂存目录不能留在 OUT：CI 会把 dist/* 当 Release 资产上传，目录会被 gh 当成
# "is a directory" 直接失败（run 41843228601 就是这个原因）。
rm -rf "$STAGE"

echo "== 摘要 =="
( cd "$OUT" && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256" )
cat "$OUT/$NAME.tar.gz.sha256"

echo
echo "完成: $TARBALL"
