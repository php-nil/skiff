# Skiff 镜像（可选形态，方案 §5.4）
#
# 构建（单架构，本机）：
#   docker build -f deploy/Dockerfile -t skiff:0.1.0 .
# 多架构（需要 buildx + QEMU）：
#   docker buildx build -f deploy/Dockerfile \
#     --platform linux/amd64,linux/arm64 -t ghcr.io/example/skiff:0.1.0 --push .
#
# 镜像构成：rust:1-alpine 构建 + alpine:3.23 运行。
# 不用 distroless（排障要 shell），不用 alpine 3.20（2026-04-01 已 EOL）。

ARG RUST_IMAGE=rust:1-alpine
ARG RUNTIME_IMAGE=alpine:3.23

# ── 构建阶段 ────────────────────────────────────────────────────
FROM ${RUST_IMAGE} AS builder

# rusqlite bundled 与 zstd-sys 需要 C 工具链；ring 需要 perl？——ring 1.x 自带汇编，
# 但构建期仍要 cc；musl-dev 提供 musl-gcc。
RUN apk add --no-cache musl-dev gcc make perl

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# workspace 的成员里有 xtask：少这一份 manifest，cargo 会直接崩在
# "failed to load manifest for workspace member xtask" —— 镜像根本构建不出来
COPY xtask ./xtask
# PHP 扩展安装器按 include_str!("../../../deploy/vendor/install-php-extensions") 编进
# skiff-app：构建上下文必须带上 deploy/vendor，否则镜像构建死在 couldn't read。
COPY deploy/vendor ./deploy/vendor
# 前端资源在 crates/web/{static,templates} 里，随上面的 COPY crates 进上下文。
# 不要再 COPY 顶层 web-assets/——那个目录 2026-10-31 已迁入 crates/web，
# 残留引用会让 docker build 直接死在 "/web-assets": not found（release 首跑踩过）。
COPY .cargo ./.cargo

# 只构建服务端二进制（包名是 skiff，目录是 crates/core），不构建 xtask / 插件 SDK CLI
RUN cargo build --release --locked -p skiff --bin skiff

# ── 运行阶段 ────────────────────────────────────────────────────
FROM ${RUNTIME_IMAGE}

RUN apk add --no-cache tzdata ca-certificates \
    && addgroup -S -g 200000 skiff-plugins 2>/dev/null || addgroup -S skiff-plugins

COPY --from=builder /src/target/release/skiff /skiff

ENV SKIFF_DATA_DIR=/data \
    SKIFF_RUN_DIR=/run/skiff \
    SKIFF_LISTEN=0.0.0.0:18080 \
    SKIFF_FORM=docker \
    SKIFF_LOG_LEVEL=info

RUN mkdir -p /data /run/skiff && chmod 0700 /data && chmod 0710 /run/skiff

VOLUME ["/data"]
EXPOSE 18080

# 自检用二进制自身（不引入 curl）
HEALTHCHECK --interval=30s --timeout=3s --retries=3 --start-period=10s \
    CMD ["/skiff", "health"]

ENTRYPOINT ["/skiff"]
CMD ["serve"]
