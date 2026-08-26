# --- Legacy auxiliary binaries ---
FROM golang:1.26-alpine AS builder

RUN apk add --no-cache git

WORKDIR /src

# Cache dependencies
COPY server/go.mod server/go.sum ./server/
RUN cd server && go mod download

# Copy server source
COPY server/ ./server/

# Build binaries that still have Go-only consumers during the staged
# migration. The HTTP server is built by the Rust stage below.
ARG VERSION=dev
ARG COMMIT=unknown
ARG DATE=unknown
RUN cd server && CGO_ENABLED=0 go build -ldflags "-s -w -X main.version=${VERSION} -X main.commit=${COMMIT} -X main.date=${DATE}" -o bin/cordy ./cmd/cordy
RUN cd server && CGO_ENABLED=0 go build -ldflags "-s -w" -o bin/migrate ./cmd/migrate
RUN cd server && CGO_ENABLED=0 go build -ldflags "-s -w" -o bin/backfill_task_usage_hourly ./cmd/backfill_task_usage_hourly
RUN cd server && CGO_ENABLED=0 go build -ldflags "-s -w" -o bin/backfill_codex_usage_cache ./cmd/backfill_codex_usage_cache

# --- Rust HTTP server ---
FROM rust:1-alpine AS rust-server-builder

RUN apk add --no-cache build-base

WORKDIR /src/server-rs

# Keep the Rust build self-contained and lockfile-reproducible. The workspace
# has no generated source outside Cargo manifests, checked-in SQLx metadata,
# and its crate directories.
COPY server-rs/Cargo.toml server-rs/Cargo.lock ./
COPY server-rs/.sqlx/ ./.sqlx/
COPY server-rs/crates/ ./crates/

ARG COMMIT=unknown
RUN CORDY_GIT_COMMIT="${COMMIT}" cargo build --release --locked -p cordy-server

# --- Runtime stage ---
FROM alpine:3.21

RUN apk add --no-cache ca-certificates tzdata

WORKDIR /app

COPY --from=rust-server-builder /src/server-rs/target/release/cordy-server server
COPY --from=builder /src/server/bin/cordy .
COPY --from=builder /src/server/bin/migrate .
COPY --from=builder /src/server/bin/backfill_task_usage_hourly .
COPY --from=builder /src/server/bin/backfill_codex_usage_cache .
COPY server/migrations/ ./migrations/
COPY LICENSE NOTICE ./
COPY docker/entrypoint.sh .
RUN sed -i 's/\r$//' entrypoint.sh && chmod +x entrypoint.sh

EXPOSE 8080

ENTRYPOINT ["./entrypoint.sh"]
