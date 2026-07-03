FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cc-switch-server /usr/local/bin/cc-switch-server
COPY --from=builder /app/web-dist /opt/cc-switch-server/web-dist
VOLUME ["/data/cc-switch-server"]
EXPOSE 15721
ENV CC_SWITCH_SERVER_CONFIG_DIR=/data/cc-switch-server
ENV CC_SWITCH_SERVER_WEB_DIST_DIR=/opt/cc-switch-server/web-dist
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:15721/health >/dev/null || exit 1
CMD ["cc-switch-server", "--host", "0.0.0.0", "--port", "15721"]
