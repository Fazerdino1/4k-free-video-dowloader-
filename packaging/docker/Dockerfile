# Multi-stage immutable container image for headless Linux servers & containers (Alpine musl)
# Build from repository root:
#   docker build -f packaging/docker/Dockerfile -t vidgrab-cli:latest .
FROM rust:1.83-alpine3.21 AS builder

RUN apk add --no-cache musl-dev openssl-dev sqlite-dev pkgconfig

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/vidgrab-shared crates/vidgrab-shared
COPY crates/vidgrab-core crates/vidgrab-core
COPY crates/vidgrab-cli crates/vidgrab-cli
COPY crates/vidgrab-ui crates/vidgrab-ui
COPY src-tauri src-tauri

RUN cargo build --release --locked -p vidgrab-cli \
    && strip target/release/vidgrab-cli

FROM alpine:3.21 AS runtime

RUN apk add --no-cache \
      ca-certificates \
      ffmpeg \
      python3 \
      yt-dlp \
    && addgroup -S -g 10001 vidgrab \
    && adduser -S -u 10001 -G vidgrab -h /data vidgrab \
    && mkdir -p /downloads \
    && chown -R vidgrab:vidgrab /data /downloads

COPY --from=builder /src/target/release/vidgrab-cli /usr/local/bin/vidgrab-cli

USER 10001:10001
WORKDIR /downloads
ENV HOME=/data \
    XDG_CONFIG_HOME=/data/.config \
    XDG_DATA_HOME=/data/.local/share

ENTRYPOINT ["vidgrab-cli"]
CMD ["--help"]
