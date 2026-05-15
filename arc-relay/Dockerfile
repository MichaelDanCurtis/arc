# arc-relay — production image.
#
# Build context is the ARC repo root (sibling to arc-relay/, crates/).
# Multi-stage: Rust builder → slim runtime.

# ---------------------------------------------------------------------------
# Stage 1: build
# ---------------------------------------------------------------------------
FROM rust:1.91-bookworm AS builder

# BUILD_SHA is read at compile time by `option_env!("BUILD_SHA")` in
# arc-relay/src/api/health_routes.rs and surfaced via GET /health's
# `build_sha` field. Default "unknown" so `cargo run` on a dev laptop
# (no build-arg) still produces a working binary; CI passes the real
# SHA via `--build-arg BUILD_SHA=$(git rev-parse --short HEAD)`.
ARG BUILD_SHA=unknown
ENV BUILD_SHA=${BUILD_SHA}

WORKDIR /build

# Copy the workspace + the binary's source. Workspace manifest at root
# brings in just the 6 shared crates + arc-relay — no stub-crate dance
# needed because ARC's workspace is intentionally small.
COPY Cargo.toml Cargo.toml
COPY rust-toolchain.toml rust-toolchain.toml
COPY crates crates
COPY arc-relay arc-relay

RUN cargo build --release -p arc-relay

# ---------------------------------------------------------------------------
# Stage 2: runtime
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/arc-relay /usr/local/bin/arc-relay
COPY --from=builder /build/arc-relay/migrations /app/migrations

WORKDIR /app
ENV RUST_LOG=arc_relay=info

EXPOSE 9900
CMD ["arc-relay"]
