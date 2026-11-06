# syntax=docker/dockerfile:1

# --- Build stage -----------------------------------------------------------
FROM rust:1-slim-trixie AS builder
WORKDIR /build

# All database queries are checked at runtime, so the build needs no database.
ENV SQLX_OFFLINE=true

COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked -p scrn-server \
    && cp target/release/scrn-server /usr/local/bin/scrn-server

# --- Runtime stage ---------------------------------------------------------
FROM debian:trixie-slim

# The binary only needs the C runtime of the base image: no OpenSSL, and
# the server makes no outgoing TLS connections, so no CA certificates.

LABEL org.opencontainers.image.title="scrn" \
      org.opencontainers.image.description="Self-hosted screenshot sharing server" \
      org.opencontainers.image.source="https://github.com/andreaslink-de/scrn" \
      org.opencontainers.image.licenses="MIT"

# Unprivileged user. compose.yaml may override the UID/GID so that the
# bind-mounted ./data directory is writable.
RUN groupadd --system --gid 10001 scrn \
    && useradd --system --uid 10001 --gid scrn --home-dir /nonexistent --shell /usr/sbin/nologin scrn \
    && mkdir -p /data \
    && chown scrn:scrn /data

COPY --from=builder /usr/local/bin/scrn-server /usr/local/bin/scrn-server

ENV SCRN_BIND=0.0.0.0:8081 \
    SCRN_DATA_DIR=/data \
    RUST_LOG=info

USER scrn:scrn
VOLUME ["/data"]
EXPOSE 8081

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/scrn-server", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/scrn-server"]
