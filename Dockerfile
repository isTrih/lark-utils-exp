# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.96.0
ARG APP_VERSION=0.0.0+dev
ARG BUILD_METADATA=dev
ARG GIT_COMMIT=unknown
ARG BUILD_DATE=unknown

# 构建阶段使用目标平台镜像。
# 在 M1 Darwin 上通过 buildx 构建 linux/amd64 时，这里会进入 amd64 Linux 环境，
# 避免把本机 arm64 产物错误打进服务器镜像。
FROM --platform=$TARGETPLATFORM rust:${RUST_VERSION}-bookworm AS builder

ARG BUILD_METADATA
ARG GIT_COMMIT
ARG BUILD_DATE

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
        pkg-config \
        build-essential \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY web ./web

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    LARK_EXP_BUILD_METADATA="${BUILD_METADATA}" \
    LARK_EXP_GIT_COMMIT="${GIT_COMMIT}" \
    LARK_EXP_BUILD_TIME="${BUILD_DATE}" \
    cargo build --release --bin server \
    && cp /app/target/release/server /app/server

# 运行阶段只保留服务二进制和证书。
FROM --platform=$TARGETPLATFORM debian:bookworm-slim AS runtime

ARG APP_VERSION
ARG GIT_COMMIT
ARG BUILD_DATE

LABEL org.opencontainers.image.title="lark-utils-exp" \
      org.opencontainers.image.version="${APP_VERSION}" \
      org.opencontainers.image.revision="${GIT_COMMIT}" \
      org.opencontainers.image.created="${BUILD_DATE}"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 app \
    && mkdir -p /app/logs \
    && chown app:app /app/logs

WORKDIR /app

COPY --from=builder /app/server /usr/local/bin/lark-exp-server

ENV SERVER_BIND_ADDR=0.0.0.0:8080
ENV RUST_LOG=info
ENV LOG_DIR=/app/logs
ENV TZ=Asia/Shanghai
ENV LARK_EXP_IMAGE_VERSION=${APP_VERSION}

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=3s --start-period=20s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "http://127.0.0.1:8080/live"]

USER app

CMD ["lark-exp-server"]
