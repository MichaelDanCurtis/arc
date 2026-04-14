# Arc Deployment Guide

## Multi-container (recommended for production)

Uses separate Postgres, arc-relay, and arc-web containers managed by Docker Compose.

```sh
docker compose up --build
```

Services:
- `arc-relay` — Rust relay server on port 9900
- `postgres` — Postgres 16 database
- `arc-web` — Next.js UI (defined separately in `arc-web/`)

## All-in-one (simple self-hosting)

Bundles Postgres 16 + arc-relay + arc-web into a single container supervised
by [s6-overlay](https://github.com/just-containers/s6-overlay). Suitable for
single-server or homelab deployments where `docker compose` feels like overkill.

```sh
# Build and run
docker compose -f arc-relay/docker-compose.all-in-one.yml up --build

# Or build and run directly
docker build -f arc-relay/Dockerfile.all-in-one . -t arc-all-in-one
docker run -d \
  -p 9900:9900 -p 3000:3000 \
  -v arc_pgdata:/var/lib/postgresql/data \
  -e ARC_JWT_SECRET=<strong-secret-min-32-chars> \
  arc-all-in-one
```

Ports exposed:
- `9900` — arc-relay (API + WebSocket)
- `3000` — arc-web (Next.js UI)

### Environment variables (all-in-one)

| Variable | Default | Notes |
|---|---|---|
| `POSTGRES_USER` | `arc` | Postgres superuser |
| `POSTGRES_PASSWORD` | `arc` | **Change in production** |
| `POSTGRES_DB` | `arc` | Database name |
| `ARC_JWT_SECRET` | *(insecure default)* | **Must change in production** (≥32 chars) |
| `ARC_BIND_ADDR` | `0.0.0.0:9900` | Relay listen address |
| `ARC_WEB_PORT` | `3000` | arc-web listen port |
| `NEXT_PUBLIC_RELAY_URL` | `http://localhost:9900` | URL arc-web uses to reach the relay |
| `RUST_LOG` | `arc_relay=info` | Relay log filter |

### Service startup order (s6-overlay)

1. `postgres` — initializes cluster on first boot, then listens on `127.0.0.1:5432`
2. `arc-relay` — waits for `pg_isready`, runs SQLx migrations, then binds `:9900`
3. `arc-web` — depends on `arc-relay`; serves Next.js standalone on `:3000`

All three processes run as PID children of s6-overlay's `/init`. A `SIGTERM` to
the container shuts all three down gracefully.
