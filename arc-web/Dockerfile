# arc-web — production image.
#
# Build context is the ARC repo root. Multi-stage:
#   wasm-builder  Rust + wasm-pack → /wasm-out
#   deps          npm ci → node_modules
#   builder       next build (consumes the wasm)
#   runner        slim Node 22 alpine
#
# Big simplification vs the pre-extraction layout: ARC's workspace has
# exactly 6 crates, all real, so no stub-Cargo.toml dance is needed in
# the wasm-builder stage. (The old WarpClone version had to stub out
# 24+ unrelated beacon-* crates to satisfy `cargo fetch`.)

# ---------------------------------------------------------------------------
# Stage 1: build the WASM artifact
# ---------------------------------------------------------------------------
FROM rust:1.91-bookworm AS wasm-builder
RUN cargo install wasm-pack --version 0.14.0 --locked

WORKDIR /workspace

COPY Cargo.toml Cargo.toml
COPY rust-toolchain.toml rust-toolchain.toml
COPY crates crates

# arc-relay is the 7th workspace member but we don't compile it here.
# Stub its manifest + empty lib/main so cargo's workspace resolver is
# happy without bringing arc-relay/ into this layer's cache key.
RUN mkdir -p arc-relay/src && \
    printf '[package]\nname = "arc-relay"\nversion = "0.1.0"\nedition = "2021"\n\n[lib]\nname = "arc_relay"\npath = "src/lib.rs"\n\n[[bin]]\nname = "arc-relay"\npath = "src/main.rs"\n' > arc-relay/Cargo.toml && \
    echo "// stub" > arc-relay/src/lib.rs && \
    echo "fn main() {}" > arc-relay/src/main.rs

RUN rustup target add wasm32-unknown-unknown
RUN wasm-pack build \
      crates/beacon-blocks-wasm \
      --target bundler \
      --out-dir /wasm-out \
      --release

# ---------------------------------------------------------------------------
# Stage 2: Install Node dependencies
# ---------------------------------------------------------------------------
FROM node:22-alpine AS deps
WORKDIR /app
COPY arc-web/package.json arc-web/package-lock.json ./
RUN npm ci

# ---------------------------------------------------------------------------
# Stage 3: Build Next.js app
# ---------------------------------------------------------------------------
FROM node:22-alpine AS builder
WORKDIR /app
COPY --from=deps /app/node_modules ./node_modules
COPY arc-web/ .
# Inject the pre-built WASM artifact so `prebuild` (wasm-pack) is not
# needed inside this stage — no Rust toolchain in the Node image.
COPY --from=wasm-builder /wasm-out ./src/wasm/beacon-blocks

ENV NEXT_TELEMETRY_DISABLED=1

# NEXT_PUBLIC_* env vars are inlined at *build* time, not runtime — they
# become string literals in the JS bundle. Pulling this from
# docker-compose's `environment:` block at runtime is too late; the static
# HTML is already baked. Pass the production relay URL as a --build-arg.
#
# Default points at the live Hostinger deploy so a `docker buildx ...
# --push` without --build-arg still produces a working production image.
# Override for staging/preview builds.
ARG NEXT_PUBLIC_ARC_RELAY_URL=https://arc.ailuxis.com
ENV NEXT_PUBLIC_ARC_RELAY_URL=${NEXT_PUBLIC_ARC_RELAY_URL}

# Skip the prebuild script since the WASM artifact is already present.
RUN npm run build --ignore-scripts

# ---------------------------------------------------------------------------
# Stage 4: Production runtime
# ---------------------------------------------------------------------------
FROM node:22-alpine AS runner
WORKDIR /app
ENV NODE_ENV=production
ENV NEXT_TELEMETRY_DISABLED=1

COPY --from=builder /app/.next/standalone ./
COPY --from=builder /app/.next/static ./.next/static
COPY --from=builder /app/public ./public

EXPOSE 3000
ENV PORT=3000
CMD ["node", "server.js"]
