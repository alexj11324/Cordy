# --- Legacy auxiliary binaries ---
FROM golang:1.26-alpine AS builder

RUN apk add --no-cache git

WORKDIR /src

# Cache dependencies
COPY server/go.mod server/go.sum ./server/
RUN cd server && go mod download

# Copy server source
COPY server/ ./server/
RUN go version | awk '{print $3}' > /tmp/go-version

# Build binaries that still have Go-only consumers during the staged
# migration. Keep the old CLI under an explicit name while the default
# `cordy` binary is supplied by the Rust stage below.
ARG VERSION=dev
ARG COMMIT=unknown
ARG DATE=unknown
RUN cd server && CGO_ENABLED=0 go build -ldflags "-s -w -X main.version=${VERSION} -X main.commit=${COMMIT} -X main.date=${DATE}" -o bin/go-cordy ./cmd/cordy
RUN cd server && CGO_ENABLED=0 go build -ldflags "-s -w" -o bin/backfill_task_usage_hourly ./cmd/backfill_task_usage_hourly
RUN cd server && CGO_ENABLED=0 go build -ldflags "-s -w" -o bin/backfill_codex_usage_cache ./cmd/backfill_codex_usage_cache

# --- Rust HTTP server, migration runner, and CLI ---
FROM rust:1-alpine AS rust-server-builder

RUN apk add --no-cache build-base

WORKDIR /src/server-rs

# Keep the Rust build self-contained and lockfile-reproducible. The workspace
# has no generated source outside Cargo manifests, checked-in SQLx metadata,
# and its crate directories.
COPY server-rs/Cargo.toml server-rs/Cargo.lock ./
COPY server-rs/.sqlx/ ./.sqlx/
COPY server-rs/crates/ ./crates/
COPY --from=builder /tmp/go-version /tmp/go-version

# The Rust crates embed these Go-owned assets through paths relative to the
# repository root. Keep that root-level layout in the Rust build stage.
RUN mkdir -p /src/server/internal/service/builtin_agents/mika \
    /src/server/internal/service/builtin_skills \
    /src/server/internal/handler
COPY server/internal/service/builtin_agents/ /src/server/internal/service/builtin_agents/
COPY server/internal/service/builtin_skills/ /src/server/internal/service/builtin_skills/
COPY server/internal/handler/reserved_slugs.json /src/server/internal/handler/reserved_slugs.json

ARG VERSION=dev
ARG COMMIT=unknown
ARG DATE=unknown
ARG GO_VERSION=unknown
RUN go_version="${GO_VERSION}"; \
    if [ "$go_version" = "unknown" ]; then go_version="$(cat /tmp/go-version)"; fi; \
    CORDY_BUILD_VERSION="${VERSION}" \
    CORDY_BUILD_COMMIT="${COMMIT}" \
    CORDY_BUILD_DATE="${DATE}" \
    CORDY_BUILD_GO_VERSION="$go_version" \
    CORDY_GIT_COMMIT="${COMMIT}" \
    cargo build --release --locked -p cordy-server -p cordy-migrate -p cordy-cli

# --- Runtime stage ---
FROM alpine:3.21

RUN apk add --no-cache ca-certificates tzdata

WORKDIR /app

COPY --from=rust-server-builder /src/server-rs/target/release/cordy-server server
COPY --from=rust-server-builder /src/server-rs/target/release/cordy-migrate migrate
COPY --from=rust-server-builder /src/server-rs/target/release/cordy cordy
COPY --from=builder /src/server/bin/go-cordy .
COPY --from=builder /src/server/bin/backfill_task_usage_hourly .
COPY --from=builder /src/server/bin/backfill_codex_usage_cache .
COPY server/migrations/ ./migrations/
COPY LICENSE NOTICE ./
COPY docker/entrypoint.sh .
RUN sed -i 's/\r$//' entrypoint.sh && chmod +x entrypoint.sh

EXPOSE 8080

# /readyz is migration-aware; unlike a raw database ping it keeps the
# container unhealthy until the server's schema/readiness contract is true.
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=6 \
    CMD wget -q -O /dev/null http://127.0.0.1:8080/readyz || exit 1

ENTRYPOINT ["./entrypoint.sh"]
