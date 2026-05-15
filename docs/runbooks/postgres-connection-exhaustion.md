# Runbook: Postgres connection exhaustion

## Symptom

- arc-relay logs flood with `sqlx::Error::PoolTimedOut` or
  `PgConnection error: too many connections for role "arc"`.
- `/api/users/me` and other DB-backed endpoints return 500s.
- WebSocket connections may succeed (the upgrade doesn't hit DB) but
  the first JSON-RPC method that queries DB fails.

## Detection

- **Future:** Sentry alert when 5xx rate on `/api/*` crosses 2%.
- **Today:** arc-relay logs show repeated `PoolTimedOut` over a 1-min
  window:
  ```bash
  ssh root@31.220.59.147
  docker logs arc-relay --since 2m 2>&1 | grep -cE "PoolTimedOut|too many connections"
  ```

## Diagnosis

1. **Check Postgres' active connection count.**
   ```bash
   docker exec arc-postgres psql -U arc -d arc \
     -c "SELECT count(*), state FROM pg_stat_activity GROUP BY state;"
   ```
   - `active` count near `max_connections` (default 100 unless tuned) →
     pool is exhausted.
   - High `idle in transaction` count → a code path is holding a tx
     open without commit/rollback. That's the bug.

2. **Identify long-running queries.**
   ```bash
   docker exec arc-postgres psql -U arc -d arc -c "
     SELECT pid, age(clock_timestamp(), query_start) AS age, state, query
     FROM pg_stat_activity
     WHERE state != 'idle' AND query NOT LIKE '%pg_stat_activity%'
     ORDER BY age DESC LIMIT 10;"
   ```
   Anything older than 30s on a transactional endpoint is suspect.

3. **Identify the call site.** Recent commits to `arc-relay/src/api/`
   or `arc-relay/src/rpc.rs` that added new DB code are the most
   likely culprits. `git log -p --since=3.days arc-relay/src/ |
   grep -E "begin|transaction|\.acquire\(\)"` finds the candidates.

## Recovery

### Immediate relief

Kill the longest-running offending queries:

```bash
docker exec arc-postgres psql -U arc -d arc -c "
  SELECT pg_terminate_backend(pid) FROM pg_stat_activity
  WHERE state = 'idle in transaction'
    AND age(clock_timestamp(), state_change) > interval '60 seconds';"
```

If the relay's pool is wedged (sqlx sometimes doesn't recover cleanly
from a connection storm), restart arc-relay:

```bash
mcp__hostinger__VPS_updateProjectV1(virtualMachineId: 1147025, projectName: "arc")
```

(Restart is a side effect of the project refresh; for finer control,
`docker restart arc-relay` via SSH.)

### Followup

Verify in `arc-relay/src/`:

- All `pool.begin().await?` calls have a matching `.commit().await?`
  or `.rollback().await?` in every branch.
- `?` operators inside a transaction block aren't bypassing
  rollback — wrap the block so the transaction guard's `Drop` rolls
  back on early return.
- No `.acquire()` calls that hold a connection across an `await` that
  could take seconds (file I/O, external HTTP, etc.).

## Root-cause debug

- **Connection leak in error path.** A handler does `pool.acquire()`
  then `?`-bubbles an error before releasing. Fix: use `.transaction()`
  or rely on `PoolConnection`'s `Drop` to return the conn.
- **Connection pool too small for traffic.** Default sqlx pool is 10.
  If real concurrent demand is 20+ persistent WS connections each
  occasionally querying, the pool is undersized. Tune via
  `PoolOptions::max_connections(N)` in `arc-relay/src/db.rs`.
- **Postgres `max_connections` is the actual ceiling.** Per-database
  pools sum to ≤ `max_connections`. Bump the postgres config (in
  `docker-compose.prod.yml`, add `-c max_connections=200` to the
  postgres command) if multiple services share the box.

## Prevention

- Always prefer `pool.transaction(|tx| ...)` over manual `begin/commit`
  — it handles rollback automatically.
- Add a periodic metric for `pool.size()` and
  `pool.num_idle()` so saturation is visible before exhaustion.
- Health check (future): `/api/health` could include a `db` field that
  pings the DB with a 1s timeout; returns 503 when DB unreachable.
