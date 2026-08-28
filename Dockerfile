# --- Rust production binaries ---
FROM rust:1-alpine AS builder

RUN apk add --no-cache build-base

WORKDIR /src/server-rs

# Keep the Rust build self-contained and lockfile-reproducible. The workspace
# embeds only the narrow Go-owned asset paths copied below; no Go toolchain or
# Go runtime binary is part of the production image.
COPY server-rs/Cargo.toml server-rs/Cargo.lock ./
COPY server-rs/.cargo/ ./.cargo/
COPY server-rs/.sqlx/ ./.sqlx/
COPY server-rs/crates/ ./crates/

# These Rust crates embed these source assets at compile time. Preserve their
# repository-relative locations so local and container builds use the same
# inputs without copying the entire Go tree into the image builder.
COPY server/internal/service/builtin_agents/ /src/server/internal/service/builtin_agents/
COPY server/internal/service/builtin_skills/ /src/server/internal/service/builtin_skills/
COPY server/internal/handler/reserved_slugs.json /src/server/internal/handler/reserved_slugs.json

ARG VERSION=dev
ARG COMMIT=unknown
ARG DATE=unknown
ARG GO_VERSION=unknown
RUN CORDY_BUILD_VERSION="${VERSION}" \
    CORDY_BUILD_COMMIT="${COMMIT}" \
    CORDY_BUILD_DATE="${DATE}" \
    CORDY_BUILD_GO_VERSION="${GO_VERSION}" \
    CORDY_GIT_COMMIT="${COMMIT}" \
    cargo build --release --locked -p cordy-server -p cordy-cli -p cordy-migrate --bins

# --- Runtime stage ---
FROM alpine:3.21

RUN apk add --no-cache ca-certificates tzdata

WORKDIR /app

COPY --from=builder /src/server-rs/target/release/cordy-server server
COPY --from=builder /src/server-rs/target/release/cordy cordy
COPY --from=builder /src/server-rs/target/release/cordy-migrate migrate
COPY --from=builder /src/server-rs/target/release/backfill_task_usage_hourly .
COPY --from=builder /src/server-rs/target/release/backfill_issue_last_activity .
COPY --from=builder /src/server-rs/target/release/backfill_codex_usage_cache .
COPY server/migrations/ ./migrations/
COPY LICENSE NOTICE ./
COPY docker/entrypoint.sh .
RUN sed -i 's/\r$//' entrypoint.sh && chmod +x entrypoint.sh

EXPOSE 8080

# The entrypoint completes migrations before starting the server. /readyz then
# reports database connectivity, while the Helm liveness probe uses /health.
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=6 \
    CMD wget -q -O /dev/null "http://127.0.0.1:${PORT:-8080}/readyz" || exit 1

ENTRYPOINT ["./entrypoint.sh"]
