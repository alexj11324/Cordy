# syntax=docker/dockerfile:1

# --- Rust production binaries ---
FROM rust:1-alpine AS builder

RUN apk add --no-cache build-base

WORKDIR /src/server-rs

# Keep the Rust build self-contained and lockfile-reproducible.
COPY server-rs/Cargo.toml server-rs/Cargo.lock ./
COPY server-rs/.cargo/ ./.cargo/
COPY server-rs/.sqlx/ ./.sqlx/
COPY server-rs/crates/ ./crates/

# These Rust crates embed these source assets at compile time. The explicit
# copies keep Markdown assets available even when the Docker context excludes
# repository documentation.
COPY server-rs/crates/cordy-service/assets/ /src/server-rs/crates/cordy-service/assets/
COPY server-rs/crates/cordy-handler/assets/ /src/server-rs/crates/cordy-handler/assets/

ARG VERSION=dev
ARG COMMIT=unknown
ARG DATE=unknown
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/src/server-rs/target,sharing=locked \
    CORDY_BUILD_VERSION="${VERSION}" \
    CORDY_BUILD_COMMIT="${COMMIT}" \
    CORDY_BUILD_DATE="${DATE}" \
    CORDY_GIT_COMMIT="${COMMIT}" \
    cargo build --release --locked -p cordy-server -p cordy-cli -p cordy-migrate --bins && \
    mkdir -p /out && \
    cp target/release/cordy-server /out/server && \
    cp target/release/cordy /out/cordy && \
    cp target/release/cordy-migrate /out/migrate && \
    cp target/release/backfill_task_usage_hourly /out/ && \
    cp target/release/backfill_issue_last_activity /out/ && \
    cp target/release/backfill_codex_usage_cache /out/

# --- Runtime stage ---
FROM alpine:3.21

RUN apk add --no-cache ca-certificates tzdata

WORKDIR /app

COPY --from=builder /out/ ./
COPY migrations/ ./migrations/
COPY LICENSE NOTICE ./
COPY docker/entrypoint.sh .
RUN sed -i 's/\r$//' entrypoint.sh && chmod +x entrypoint.sh

EXPOSE 8080

# The entrypoint completes migrations before starting the server. /readyz then
# reports database connectivity, while the Helm liveness probe uses /health.
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=6 \
    CMD wget -q -O /dev/null "http://127.0.0.1:${PORT:-8080}/readyz" || exit 1

ENTRYPOINT ["./entrypoint.sh"]
