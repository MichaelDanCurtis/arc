# ARC — relay service for Beacon + Beam

The Arc relay (Rust) + Arc web frontend (Next.js) + the six shared
wire-protocol crates that both Beacon and Beam consume. Deployed at:

- **`arc.ailuxis.com`** — Arc dashboard + JSON-RPC API + WebSocket relay
- **`beam.ailuxis.com`** — Plugin marketplace UI (same Next.js, different surface)

VPS at Hostinger (`thecurtis.cloud`, IP `31.220.59.147`), Traefik for
TLS + path routing, postgres for state. See `deploy/` for the production
compose file.

## Repo layout

```
arc-relay/                   Rust HTTP + WebSocket server (axum + sqlx)
arc-web/                     Next.js 15 frontend (dashboard + marketplace)
crates/                      Wire-protocol crates — consumed by Beacon + Beam
├── beacon-features          FeatureFlag enum
├── beacon-jsonrpc           JSON-RPC 2.0 message types + dispatcher
├── beacon-model             Shared data types (User, Session, Block, ...)
├── beacon-proto             Wire-format envelopes
├── beacon-blocks            OSC 133 + block detection
└── beacon-blocks-wasm       WASM wrapper of beacon-blocks (for arc-web)
deploy/                      Production compose + Hostinger setup notes
docker-compose.yml           Local dev (postgres + arc-relay + arc-web + traefik)
Cargo.toml                   Workspace manifest
rust-toolchain.toml          Pinned Rust channel (1.91, matches Docker)
REQUIREMENTS.md              Joint Beacon+Beam capability contract (stub)
```

The `crates/` directory's package names retain the `beacon-` prefix for
backwards-source-compat with downstream consumers; a rename pass to
`arc-*` can come later if desired.

## Lineage

This repo was extracted from `MichaelDanCurtis/Beacon` (working dir
`WarpClone/`) on 2026-05-13 via `git filter-repo` to preserve per-file
history. The full migration plan lives at
`WarpClone/docs/superpowers/specs/2026-05-13-arc-extraction-migration.md`
and the consolidation rationale at
`WarpClone/docs/superpowers/specs/2026-05-12-arc-consolidation.md`.

Both Beacon and Beam path-dep into this repo's `crates/` from their
own filesystem locations:

- **Beacon** (`MichaelDanCurtis/Beacon` in `WarpClone/`): `path = "../../../ARC/crates/<name>"` from each `beacon-core/crates/*/Cargo.toml` that needs a moved type.
- **Beam** (`MichaelDanCurtis/WarpMod`): same pattern from `app/` or wherever the consumer crate lives.

### Per-machine symlink prerequisite

The 3-dotdot path above resolves to `WarpClone/ARC/crates/<name>`,
which doesn't exist by default — there's no `ARC` directory inside
`WarpClone/`. Create a per-machine symlink so cargo can resolve the
deps without a 4-dotdot escape:

```bash
ln -snf /Volumes/Main\ External/Development/ARC WarpClone/ARC
```

Same pattern for Beam: `ln -snf /Volumes/Main\ External/Development/ARC WarpMod/ARC`.

Why a symlink instead of bumping to 4 dotdots: Claude-Code worktrees
under `WarpClone/.claude/worktrees/<name>/` would need a 7-dotdot
escape from there. The symlink at the WarpClone root makes the same
3-dotdot string work from any depth inside WarpClone — main checkout
or worktree alike. Not committed (per-machine); each developer
creates it once.

If either repo moves, both repos' Cargo.toml files need a `sed` to
retarget. Acceptable while there are 2 consumers; publish crates to a
registry when the count grows.

## Quickstart

### Local dev

```bash
docker compose up -d        # bring up postgres + arc-relay + arc-web + traefik
docker compose logs -f arc-relay
```

Or run a single piece without Docker:

```bash
# Cargo workspace
cargo build --workspace
cargo run -p arc-relay      # needs a local Postgres at $ARC_DATABASE_URL

# Next.js frontend
cd arc-web
NEXT_PUBLIC_ARC_RELAY_URL=http://localhost:9900 npm run dev
```

### Production deploy

See `deploy/docker-compose.prod.yml` and the deployment runbook at
`WarpClone/docs/operations/release-arc.md` (still housed in WarpClone for
now; can move here in a later cleanup pass).

Build + push images:

```bash
docker buildx build --platform linux/amd64 \
  -f arc-relay/Dockerfile \
  -t michaeldancurtis/arc-relay:latest \
  -t michaeldancurtis/arc-relay:$(git rev-parse --short HEAD) \
  --push .

docker buildx build --platform linux/amd64 \
  -f arc-web/Dockerfile \
  -t michaeldancurtis/arc-web:latest \
  -t michaeldancurtis/arc-web:$(git rev-parse --short HEAD) \
  --build-arg NEXT_PUBLIC_ARC_RELAY_URL=https://arc.ailuxis.com \
  --push .
```

Refresh the VPS via the Hostinger MCP:
`mcp__hostinger__VPS_updateProjectV1(virtualMachineId: 1147025, projectName: "arc")`

## Coordination

This service backs two clients with different needs. Joint requirements
in `REQUIREMENTS.md`; live coordination between sessions in
`/Volumes/Main External/Development/a2a-beam-beacon.jsonl` (file-based
A2A channel; protocol at the sibling `a2a-protocol.md`).

## License

Dual structure, to support both the proprietary service and the OSS Beam
client without a license cascade:

- **`arc-relay/` and `arc-web/`** (service binary + frontend): proprietary.
  Deployed as a service; not redistributable.
- **`crates/*`** (wire-protocol shared types — `beacon-features`,
  `beacon-jsonrpc`, `beacon-model`, `beacon-proto`, `beacon-blocks`,
  `beacon-blocks-wasm`): **`MIT OR Apache-2.0`**, declared at the
  workspace level in `Cargo.toml` and inherited by every crate via
  `license.workspace = true`. Both Beacon (proprietary) and Beam
  (`MIT OR Apache-2.0`) can path-dep or git-dep these crates without
  a license-cascade conflict.

License text files at the repo root: `LICENSE-MIT`, `LICENSE-APACHE`.
SPDX-compliant. Service-binary terms are governed by the deploy agreement
(TBD at `ailuxis.com/legal/arc-tos`), not the crate license.

If a future crate needs to ship inside an OSS client image, confirm it
inherits `license.workspace = true` (or sets its own MIT/Apache-2.0
declaration) before adding new dependencies that could cascade restrictively
— see the `agpl-license-blocks-commercial-sister-fork` skill for the
audit checklist.
