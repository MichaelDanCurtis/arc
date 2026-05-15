# Runbook: Cold VPS restore (full disaster recovery)

**Scope:** VPS 1147025 (Hostinger) is unreachable, deleted, corrupted, or
the operator wants to migrate to a clean box. Goal: restore
`arc.ailuxis.com` + `beam.ailuxis.com` to serving traffic in under 60
minutes from a known-good Hostinger snapshot.

## Symptom

- `mcp__hostinger__VPS_getVirtualMachineDetailsV1(1147025)` returns
  `state: stopped` AND can't be started, OR returns 404.
- Both subdomains DNS-resolve but TCP-connect fails (port 443
  unreachable).
- Hostinger dashboard shows the VPS in a non-recoverable state.

## Detection

- UptimeRobot fires on both `https://arc.ailuxis.com/api/health` and
  `https://beam.ailuxis.com/` for >5 minutes.
- Hostinger sends a maintenance notification (rare but possible).
- Operator runs `ssh root@31.220.59.147` and the host is gone.

## Pre-recovery checklist

- [ ] Latest Hostinger snapshot date — note it.
- [ ] Docker Hub images current — `michaeldancurtis/arc-relay:latest`
  and `michaeldancurtis/arc-web:latest` should be pullable.
- [ ] DNS provider login (where `ailuxis.com` zone lives — Cloudflare?
  Hostinger?).
- [ ] R2 credentials backed up offline (1Password or equivalent).
- [ ] `.env` for arc-relay backed up offline.

## Recovery (target: < 60 min)

### Option A: Restore from Hostinger snapshot (preferred)

1. **List available snapshots:**
   ```python
   mcp__hostinger__VPS_getBackupsV1(virtualMachineId: 1147025)
   # OR
   mcp__hostinger__VPS_getSnapshotV1(virtualMachineId: 1147025)
   ```
   Pick the most recent snapshot. Note the snapshot ID.

2. **Restore to a new VM:**
   ```python
   # If 1147025 is salvageable:
   mcp__hostinger__VPS_restoreSnapshotV1(virtualMachineId: 1147025, snapshotId: <id>)
   # If 1147025 is unrecoverable: purchase a new VPS via
   # mcp__hostinger__VPS_purchaseNewVirtualMachineV1(...) then
   # mcp__hostinger__VPS_restoreBackupV1(...) onto it.
   ```

3. **Wait for boot** (~5-10 min). Confirm via
   `VPS_getVirtualMachineDetailsV1` → `state: running`.

4. **Update DNS if the IP changed.** The new VPS may have a new
   public IP. Update both A records (`arc.ailuxis.com` and
   `beam.ailuxis.com`) to the new IP. Snapshot the old + new IPs.

5. **Wait for DNS propagation** (~1-5 min usually). Verify:
   ```bash
   dig +short arc.ailuxis.com
   # Should return the new IP.
   ```

6. **Verify services started.** SSH in and:
   ```bash
   docker ps
   # Expect: arc-relay, arc-web, arc-postgres, root-traefik-1 all up
   docker exec arc-postgres pg_isready
   # Expect: arc-postgres accepting connections
   ```

7. **Pull latest images** (snapshot may have older tags):
   ```bash
   docker compose -f /root/arc-compose.yml pull
   docker compose -f /root/arc-compose.yml up -d
   ```

8. **Smoke test:**
   ```bash
   curl https://arc.ailuxis.com/api/health  # Expect 200 + JSON
   curl -sI https://beam.ailuxis.com/      # Expect 200
   ```

### Option B: Rebuild from scratch (snapshot too old or unavailable)

This path takes ~2-3 hours but uses no Hostinger-specific state.

1. Purchase a new VPS at the desired tier:
   ```python
   mcp__hostinger__VPS_purchaseNewVirtualMachineV1(...)
   mcp__hostinger__VPS_setupPurchasedVirtualMachineV1(virtualMachineId: <new-id>, ...)
   ```

2. SSH in. Install Docker:
   ```bash
   curl -fsSL https://get.docker.com | sh
   ```

3. Clone the ARC repo (private — needs GitHub access):
   ```bash
   git clone https://github.com/MichaelDanCurtis/arc.git /root/arc
   cd /root/arc/deploy
   ```

4. Restore `.env` from offline backup:
   ```bash
   cp /path/to/offline/backup/.env .env
   ```

5. Start the stack:
   ```bash
   docker compose -f docker-compose.prod.yml up -d
   ```

6. **Restore Postgres data.** This is the critical step — without it,
   sessions, users, plugins are gone.
   - If Postgres dumps were shipped to R2 (T1.3 deliverable), pull the
     latest dump and `pg_restore`.
   - If only Hostinger snapshots have it, see Option A.

7. Update DNS A records to the new IP. Wait propagation.

8. Smoke test (same as Option A step 8).

## Verification

- `https://arc.ailuxis.com/api/health` → 200 with valid `build_sha`.
- `https://beam.ailuxis.com/marketplace` → renders, lists plugins.
- A test JSON-RPC call against `wss://arc.ailuxis.com/ws` succeeds.
- Postgres has the user/session/plugin data you expect (`docker exec
  arc-postgres psql -U arc -d arc -c "SELECT count(*) FROM users;"`).
- `build_sha` in `/api/health` matches `git rev-parse --short HEAD`
  in the repo (proves the deployed image is current).

## Root-cause debug

DR drills are about practice, not blame. After a real or drill
restore, write a short retrospective in `docs/postmortems/`:
- What broke?
- What did we restore?
- What was the gap (snapshot age, data loss, downtime)?
- What changes prevent or mitigate next time? (more frequent snapshots,
  WAL shipping, replica, etc.)

## Prevention

- **Snapshot policy:** at least daily Hostinger snapshot of the VPS.
  Cheap insurance.
- **Postgres dumps to R2:** part of T1.3 — a nightly `pg_dump | aws
  s3 cp` (or R2 equivalent) so DB data isn't VPS-bound.
- **Quarterly DR drill:** pick a Saturday, intentionally tear down
  the VPS, restore on a new one, time the operation. Update this
  runbook with surprises.
- **DNS TTL low.** Set both A records to TTL 300 (5 min) so DNS
  failover is fast. Bump to higher TTL only in steady state if
  Cloudflare bills per query.
