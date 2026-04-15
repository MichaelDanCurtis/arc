# Stage 1: Build WASM artifact from beacon-blocks-wasm crate.
# Uses the official Rust image; wasm-pack is installed here and the compiled
# .wasm/.js/.d.ts files are handed off to the Next.js builder stage.
# The Rust toolchain never touches the runtime image.
FROM rust:1-bookworm AS wasm-builder
# Install wasm-pack pinned to 0.14.0 (current stable on crates.io as of 2026-04-15)
RUN cargo install wasm-pack --version 0.14.0 --locked

WORKDIR /workspace

# ── Pass 1: dependency-fetch layer ─────────────────────────────────────────
# Copy workspace manifest + lock so `cargo fetch` can resolve the full dep
# graph and download all third-party crates.  Only the three first-party
# crates that beacon-blocks-wasm actually needs get real Cargo.toml files;
# the remaining 11 workspace members are represented by stub Cargo.toml +
# src/lib.rs so the workspace resolver is satisfied without pulling in any
# of their source.  This layer is only invalidated by Cargo.toml / Cargo.lock
# changes — NOT by edits to src/ files in unrelated crates.
COPY beacon-core/Cargo.toml beacon-core/Cargo.toml
COPY beacon-core/Cargo.lock beacon-core/Cargo.lock

# Real Cargo.toml files for the three crates in the build path.
COPY beacon-core/crates/beacon-blocks-wasm/Cargo.toml \
     beacon-core/crates/beacon-blocks-wasm/Cargo.toml
COPY beacon-core/crates/beacon-blocks/Cargo.toml \
     beacon-core/crates/beacon-blocks/Cargo.toml
COPY beacon-core/crates/beacon-model/Cargo.toml \
     beacon-core/crates/beacon-model/Cargo.toml

# Stub Cargo.toml files for the remaining 11 workspace members.
# Each crate just needs a package table that satisfies the workspace resolver.
RUN for crate in \
      beacon-proto \
      beacon-pty \
      beacon-vt \
      beacon-shadow \
      beacon-ffi \
      beacon-tests \
      beacon-net \
      beacon-session \
      beacon-hostd \
      beacon-complete \
      beacon-ai \
    ; do \
      mkdir -p beacon-core/crates/${crate}/src && \
      printf '[package]\nname = "%s"\nversion.workspace = true\nedition.workspace = true\n' "${crate}" \
        > beacon-core/crates/${crate}/Cargo.toml && \
      echo "// stub" > beacon-core/crates/${crate}/src/lib.rs ; \
    done

# Stub src/lib.rs for the three real crates so `cargo fetch` can complete
# without their actual source being present yet.
RUN for crate in beacon-blocks-wasm beacon-blocks beacon-model; do \
      mkdir -p beacon-core/crates/${crate}/src && \
      echo "// stub" > beacon-core/crates/${crate}/src/lib.rs ; \
    done

# Copy the rust-toolchain so the exact channel is resolved before fetching.
COPY beacon-core/rust-toolchain.toml beacon-core/rust-toolchain.toml

# Add the wasm32 target and fetch all third-party dependencies.
# This layer is cached as long as Cargo.toml / Cargo.lock are unchanged.
RUN rustup target add wasm32-unknown-unknown
RUN cargo fetch --manifest-path beacon-core/Cargo.toml

# ── Pass 2: real source for beacon-blocks* + beacon-model ──────────────────
# Overwrite the stubs with actual source.  Only changes to these three crates
# (or to Cargo.toml / Cargo.lock) will invalidate the layers below.
COPY beacon-core/crates/beacon-blocks/src     beacon-core/crates/beacon-blocks/src
COPY beacon-core/crates/beacon-blocks-wasm/src beacon-core/crates/beacon-blocks-wasm/src
COPY beacon-core/crates/beacon-model/src      beacon-core/crates/beacon-model/src

RUN wasm-pack build \
      beacon-core/crates/beacon-blocks-wasm \
      --target bundler \
      --out-dir /wasm-out \
      --release

# Stage 2: Install Node dependencies
FROM node:22-alpine AS deps
WORKDIR /app
COPY arc-web/package.json arc-web/package-lock.json ./
RUN npm ci

# Stage 3: Build Next.js app
FROM node:22-alpine AS builder
WORKDIR /app
COPY --from=deps /app/node_modules ./node_modules
COPY arc-web/ .
# Inject the pre-built WASM artifact so `prebuild` (wasm-pack) is not needed
# inside this stage — no Rust toolchain in the Node image.
COPY --from=wasm-builder /wasm-out ./src/wasm/beacon-blocks
ENV NEXT_TELEMETRY_DISABLED=1
# Skip the prebuild script since the WASM artifact is already present.
RUN npm run build --ignore-scripts

# Stage 4: Production
FROM node:22-alpine AS runner
WORKDIR /app
ENV NODE_ENV=production
ENV NEXT_TELEMETRY_DISABLED=1

# Copy build output
COPY --from=builder /app/.next/standalone ./
COPY --from=builder /app/.next/static ./.next/static
COPY --from=builder /app/public ./public

EXPOSE 3000
ENV PORT=3000
CMD ["node", "server.js"]
