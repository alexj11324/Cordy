# --- Rust HTTP server, migration runner, and CLI ---
FROM rust:1-alpine AS rust-server-builder

RUN apk add --no-cache build-base

WORKDIR /src/server-rs

# Keep the Rust build lockfile-reproducible. A few crates embed the product
# prompt/skills and reserved slugs from the Go-side source tree, so those
# compile-time inputs are copied below at their repository-relative paths.
COPY server-rs/Cargo.toml server-rs/Cargo.lock ./
COPY server-rs/.sqlx/ ./.sqlx/
COPY server-rs/crates/ ./crates/
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
    cargo build --release --locked -p cordy-server -p cordy-migrate -p cordy-cli

# --- Runtime stage ---
FROM alpine:3.21

RUN apk add --no-cache ca-certificates tzdata

WORKDIR /app

COPY --from=rust-server-builder /src/server-rs/target/release/cordy-server server
COPY --from=rust-server-builder /src/server-rs/target/release/cordy-migrate migrate
COPY --from=rust-server-builder /src/server-rs/target/release/cordy cordy
COPY server/migrations/ ./migrations/
COPY LICENSE NOTICE ./
COPY docker/entrypoint.sh .
RUN sed -i 's/\r$//' entrypoint.sh && chmod +x entrypoint.sh
RUN ln -s migrate backfill_task_usage_hourly \
    && ln -s migrate backfill_issue_last_activity \
    && ln -s migrate backfill_codex_usage_cache

EXPOSE 8080

# /readyz is migration-aware; unlike a raw database ping it keeps the
# container unhealthy until the server's schema/readiness contract is true.
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=6 \
    CMD wget -q -O /dev/null http://127.0.0.1:8080/readyz || exit 1

ENTRYPOINT ["./entrypoint.sh"]
