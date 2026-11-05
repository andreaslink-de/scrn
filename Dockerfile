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

# libssl is needed later by the passkey library; ca-certificates for TLS.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3t64 \
    && rm -rf /var/lib/apt/lists/*

# Unprivileged user. compose.yaml may override the UID/GID so that the
# bind-mounted ./data directory is writable.
RUN groupadd --system --gid 10001 scrn \
    && useradd --system --uid 10001 --gid scrn --home-dir /nonexistent --shell /usr/sbin/nologin scrn \
    && mkdir -p /data \
    && chown scrn:scrn /data

COPY --from=builder /usr/local/bin/scrn-server /usr/local/bin/scrn-server

ENV SCRN_BIND=0.0.0.0:8080 \
    SCRN_DATA_DIR=/data \
    RUST_LOG=info

USER scrn:scrn
VOLUME ["/data"]
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/scrn-server", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/scrn-server"]
