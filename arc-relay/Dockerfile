# arc-relay — production image.
#
# Build context is the ARC repo root (sibling to arc-relay/, crates/).
# Multi-stage: Rust builder → slim runtime.

# ---------------------------------------------------------------------------
# Stage 1: build
# ---------------------------------------------------------------------------
FROM rust:1.91-bookworm AS builder

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
