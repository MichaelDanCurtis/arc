# ARC tier-one plan

> **Status:** draft. Owner: arc-claude (2026-05-14). For your review before
> commit. Scope is calibrated to "small but professional" — single VPS,
> one dev, two consumers (Beacon + Beam) — not big-tech-grade.

## What "tier one" means here

A service the user trusts during real work. If Beacon or Beam users notice
ARC at all, it's because something we shipped impressed them — never
because we broke them. Concrete:

1. Outages are rare, brief, and noticed-by-us-first.
2. Wire surfaces are contracted. A breaking change to a JSON-RPC method
   never lands without both clients ack'ing.
3. Deploys are mechanical. No "hope this works" pushes.
4. Recovery is documented. Any single-VPS-loss event is restorable from
   snapshots in under an hour.
5. Observability tells us what's happening without us asking.

## Tier-one bar (acceptance criteria)

### Reliability SLOs

| Surface | Metric | Target | Window |
|---|---|---|---|
| `arc.ailuxis.com` HTTP | Availability (200/2xx + 3xx) | **99.9%** | 30-day rolling |
| `wss://arc.ailuxis.com/ws` | Connect success rate | **99.5%** | 30-day rolling |
| `/api/*` HTTP | p95 latency | **<250 ms** | 7-day rolling |
| `/ws` round-trip notif | p95 added latency | **<50 ms** | 7-day rolling |
| 5xx rate (any surface) | Error rate | **<0.5%** | 7-day rolling |
| Postgres availability | Up | **99.95%** | 30-day rolling |

**Error budget:** 99.9% uptime = 43 minutes of downtime per 30-day window.
We burn that on planned maintenance + incidents. If we exhaust it, the
next two weeks are reliability-work-only (no new features).

### Mean Time To Recover

| Incident class | MTTR target |
|---|---|
| arc-relay crash loop | < 5 min (auto-restart + alert) |
| arc-web 5xx storm | < 15 min (rollback to prior tag) |
| Postgres connection storm | < 10 min (restart pool, kill long queries) |
| VPS-wide outage | < 60 min (snapshot restore to fresh VPS) |
| Bad image push | < 10 min (`docker compose up -d --rollback` or retag) |

### Wire-protocol invariants

- Every JSON-RPC method shipped on `arc.json-rpc` subprotocol is listed
  in [REQUIREMENTS.md](../REQUIREMENTS.md) with both Beacon and Beam
  columns filled.
- Breaking changes go through a deprecation window: new method ships,
  old method emits `arc.deprecation` notification with sunset date,
  removed after 30 days minimum AND both clients ack'd removal.
- Adding a method is a PR. Removing or changing a method is a PR + both
  clients' explicit ack on the A2A channel + Beacon-claude/beam-claude
  sign-off comment on the PR.

## Required pillars (what must exist)

### 1. Observability (P0 — does not exist today)

| Pillar | Mechanism | Status |
|---|---|---|
| Structured logs | Already emitted by `tracing` crate. Need: log aggregation off-VPS. | Partial |
| Error tracking | Sentry (free tier) or self-hosted Glitchtip. arc-relay + arc-web instrumented. | None |
| Metrics | `/metrics` Prometheus endpoint on arc-relay + Grafana Cloud free tier. | None |
| Uptime probes | UptimeRobot or Hostinger built-in pinging `/health` every 60s. | None |
| Alerts | Page-to-self via email or Telegram when SLO burn-rate is >2x normal for 1h. | None |
| Status page | `status.ailuxis.com` showing the SLO board. Optional but tier-one. | None |

**P0 deliverable:** at minimum, uptime probes + Sentry. Without these we
can't measure the SLOs we just committed to.

### 2. Deploy automation (P0 — half exists today)

| Step | Today | Tier-one target |
|---|---|---|
| Build images | Manual `docker buildx` from laptop | GitHub Actions on tag push |
| Push to Docker Hub | Manual | GHA, signed digests |
| Update VPS compose | SSH-vim (manual) | A `deploy/sync.sh` script + Hostinger MCP |
| Pull + restart | `mcp__hostinger__VPS_updateProjectV1` | Same, but called from GHA |
| Smoke verify | Eyeballing curl | GHA post-deploy script: hit `/health`, run JSON-RPC `arc.version`, confirm version string matches deployed tag |
| Rollback | Re-pull a prior tag manually | One-command: `deploy/rollback.sh <tag>` |

**Bottleneck:** GitHub repo doesn't exist yet (`github.com/MichaelDanCurtis/arc`).
This is the #1 blocker on the deploy automation pillar.

### 3. Wire-surface contracts (P1 — partial today)

- [REQUIREMENTS.md](../REQUIREMENTS.md) exists as the joint contract.
- Beacon and Beam columns are mostly empty (see arc-001 on A2A channel
  for the asks).
- No formal "PR template requires REQUIREMENTS.md row" gate yet.

**P1 deliverable:** when origin remote exists, ship a PR template under
`.github/pull_request_template.md` that requires a REQUIREMENTS.md row
update for any change touching `src/rpc.rs`, `src/ai_rpc.rs`, or
`src/api/`.

### 4. Disaster recovery (P1 — partial today)

| Asset | Backup mechanism | Restore-time target |
|---|---|---|
| Postgres data (sessions, blocks, users) | Hostinger snapshot daily + WAL shipping? | < 1 hour to a fresh VPS |
| Object storage (R2 plugin bundles) | Cloudflare R2's own redundancy | < 5 min (no action needed) |
| Compose files + .env | Tracked in this repo; .env in a 1Password vault | < 5 min |
| Docker Hub images | Multiple tags (`latest` + SHA) | Re-pullable from cache |

**P1 deliverable:** quarterly DR drill — pick a date, spin up a clean
Hostinger VPS, restore from the latest snapshot, confirm arc-relay
boots + Postgres comes online + both subdomains resolve. Documented
restoration runbook lives in `docs/runbooks/dr-restore.md`.

### 5. Runbooks (P1 — does not exist today)

Need plain-English procedures for the 4 most likely incidents:

- `docs/runbooks/arc-relay-crash-loop.md`
- `docs/runbooks/postgres-connection-exhaustion.md`
- `docs/runbooks/traefik-cert-renewal-stuck.md`
- `docs/runbooks/cold-vps-restore.md`

Each runbook: symptom (what user sees), detection (what alert fires),
diagnosis (commands to run), recovery (commands to fix), root-cause
debug (where to look). 1-2 pages each.

### 6. Integration smoke (P2 — does not exist today)

A cross-client smoke suite run on every release:

```
1. Beacon connects to wss://arc.ailuxis.com/ws → handshakes → opens a session
2. Beam connects same → handshakes → posts a chat message
3. Beacon attaches to Beam's session via arc.session.attach
4. Beam denies → both see arc.session.attach_denied
5. Both disconnect cleanly
```

Either run as a GHA job (test container with both clients vendored), or
as a CLI tool both teams can invoke against staging/prod. Lives in
`tests/integration/cross-client.rs`.

## Milestone plan

| M | Theme | Concrete deliverables | Acceptance |
|---|---|---|---|
| **T1.0** | Foundations | GitHub repo created; CI builds images; uptime probes live; Sentry hooked up | Build a known-good image from GHA; uptime dashboard shows `arc.ailuxis.com` 99.9%+ over 7 days; an intentionally-thrown error in dev shows up in Sentry within 60s |
| **T1.1** | Contracts | REQUIREMENTS.md columns filled by both clients; PR template enforces row-update; `arc.exec.*` + `arc.msg.*` wire ack'd; convergence rows decided | Every JSON-RPC method in `src/rpc.rs` has a row in REQUIREMENTS.md with both clients' ack |
| **T1.2** | Deploys | `deploy/release.sh` one-shot script; rollback script; GHA on tag push builds + pushes + triggers VPS refresh + smoke-verifies | Releasing `v0.2.0` is a `git tag` + `git push --tags` and nothing else; rollback to `v0.1.x` is one command |
| **T1.3** | Recovery | Runbooks for the 4 incidents; quarterly DR drill; `/health` returns DB + R2 + version JSON | Pick a random Saturday, kill arc-relay, time how long to recovery; must be < 30 min using only the runbook |
| **T1.4** | Smoke | Cross-client integration suite; runs on every prod deploy; auto-rollback if smoke fails | Intentionally regress the `/graphql/v2` handler; smoke must fail and deploy must auto-rollback |
| **T1.5** | Observability polish | Status page at status.ailuxis.com; SLO burn-rate alerts; Grafana dashboard linked from this doc | Burn 10% of monthly error budget in a test → page fires within 5 min |

## Out of scope for tier one (intentional)

- Multi-region active-active. Single VPS is fine for current load; revisit
  at >1k concurrent WS connections.
- On-call rotation. One-dev team; alerts go to self via email/Telegram,
  acceptable for 99.9% (not 99.99%).
- Hot-standby Postgres. Daily snapshot + WAL-ship to R2 is the bar.
- Mobile clients. ARC doesn't have any.
- SOC2 / audit logging compliance. Not regulatory-driven; defer until a
  customer requires it.

## Open items needing human input

| Item | Why blocks | Asks |
|---|---|---|
| Create `github.com/MichaelDanCurtis/arc` | T1.0 — no CI without it | Create repo, add as origin |
| OAuth client_id / secrets | T1.1 — auth flow can't be contracted without provider creds | GitHub OAuth app at minimum, callback `https://arc.ailuxis.com/api/auth/oauth/github/callback` |
| R2 creds | T1.3 DR — plugin uploads/restores need them | Create bucket + API token, populate `.env` |
| Status page domain | T1.5 — choose `status.ailuxis.com` or other | Pick subdomain + DNS |
| SLO numbers above | T1.0 — these are my guesses; you may want tighter/looser | Confirm or override the 99.9%/250ms/0.5% targets |

## Next-action recommendation

Land **T1.0 foundations** first — none of the others matter without
observability + a GitHub repo. Order:

1. You: create `github.com/MichaelDanCurtis/arc`, add me as collaborator (or
   give me a deploy key). 5 min.
2. arc-claude: push current state to origin, set up GHA workflow for image
   builds. 30 min once origin exists.
3. arc-claude: integrate Sentry into arc-relay + arc-web. 1 hour.
4. You: create Sentry project (free tier). 5 min.
5. arc-claude: add UptimeRobot probes (or Hostinger built-in) for the four
   surfaces. 15 min.

After T1.0, T1.1 (contracts) is the natural next milestone because both
sides of the A2A channel can fill columns in parallel.

---

**Edit log**

| Date | By | Change |
|---|---|---|
| 2026-05-14 | arc-claude | Initial draft. |
