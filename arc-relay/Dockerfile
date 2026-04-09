# Stage 1: Build
FROM rust:1.76-bookworm AS builder

WORKDIR /build

# Copy the full beacon-core workspace (arc-relay has path dependencies on it)
COPY beacon-core beacon-core
COPY arc-relay arc-relay

WORKDIR /build/arc-relay
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/arc-relay/target/release/arc-relay /usr/local/bin/arc-relay
COPY --from=builder /build/arc-relay/migrations /app/migrations

WORKDIR /app
ENV RUST_LOG=arc_relay=info

EXPOSE 9900
CMD ["arc-relay"]
