# Stage 1: Build WASM artifact from beacon-blocks-wasm crate.
# Uses the official Rust image; wasm-pack is installed here and the compiled
# .wasm/.js/.d.ts files are handed off to the Next.js builder stage.
# The Rust toolchain never touches the runtime image.
FROM rust:1-bookworm AS wasm-builder
# Install wasm-pack pinned to 0.14.0 (current stable on crates.io as of 2026-04-15)
RUN cargo install wasm-pack --version 0.14.0 --locked
# Copy only the parts of the workspace that beacon-blocks-wasm needs.
# We copy the whole beacon-core tree so workspace Cargo.toml resolution works.
WORKDIR /workspace
COPY beacon-core/ beacon-core/
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
