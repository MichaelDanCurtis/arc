# Runbook: arc-relay crash loop

**On-call: Michael.** Pager target: future Sentry / UptimeRobot. For now,
user-noticed or `mcp__hostinger__VPS_getProjectContainersV1` shows restart count climbing.

## Symptom

- Users report intermittent failures: dashboard 502s, marketplace pages
  blank, WS connections immediately drop.
- `curl https://arc.ailuxis.com/api/health` returns 502 or 504, OR
  returns 200 but `uptime_seconds < 60` repeatedly (proving the relay
  keeps restarting).
- `https://beam.ailuxis.com/marketplace` may still render (it's
  DB-backed via arc-relay; if arc-relay is down, plugins list shows
  loading-then-error).

## Detection

- **Future:** Sentry breadcrumb burst + UptimeRobot alert on
  `/api/health`.
- **Today:** Hostinger dashboard shows the `arc-relay` container's
  restart count incrementing.

```bash
mcp__hostinger__VPS_getProjectContainersV1(virtualMachineId: 1147025, projectName: "arc")
# Look for arc-relay's `state` and `restart_count`.
```

## Diagnosis

1. **Pull the last 200 lines of logs.** Either via Hostinger console or
   SSH:
   ```bash
   ssh root@31.220.59.147
   docker logs arc-relay --tail 200 2>&1 | less
   ```

2. **Look for the panic source.** Rust panics in `arc-relay` start with
   `thread '<name>' panicked at <file>:<line>:<col>`. Note:
   - **`thread 'main' panicked` at startup** — config error, bad
     `.env`, or migration failure. The process never accepted traffic;
     fix the env/config.
   - **`thread 'tokio-runtime-worker' panicked` after some traffic** —
     a request handler triggered the panic. The panic message names the
     `.unwrap()` / `.expect()` / `panic!()` call site. Go look at it.

3. **Cross-check the deployed SHA against the source.** `curl
   https://arc.ailuxis.com/api/health` → `build_sha` field. The
   panic's file:line in logs refers to that commit's source. Open it
   on github.com (`https://github.com/MichaelDanCurtis/arc/blob/<sha>/<file>#L<line>`).

4. **Is this an ongoing storm or a one-shot?** Run:
   ```bash
   docker logs arc-relay --since 5m 2>&1 | grep -c "thread '"
   ```
   - `> 5` panics in 5 min → ongoing storm; recovery is rollback (next
     section).
   - `< 5` → likely a one-off (cosmic ray, transient DB blip);
     investigate but don't auto-roll-back.

## Recovery

### Fast rollback (ongoing storm)

Pick the prior known-good SHA from `git log --oneline origin/main` or
the inventory file's `build_sha` history.

```bash
# From an arc-claude session, repoint :latest at the prior SHA:
docker pull michaeldancurtis/arc-relay:<prior-sha>
docker tag michaeldancurtis/arc-relay:<prior-sha> michaeldancurtis/arc-relay:latest
docker push michaeldancurtis/arc-relay:latest

# Then trigger refresh:
mcp__hostinger__VPS_updateProjectV1(virtualMachineId: 1147025, projectName: "arc")
```

`/api/health` should now report the rolled-back `build_sha`. Crashes
should stop within 30s.

### Slow fix (one-shot or post-storm)

1. Identify the file:line from logs.
2. Open a worktree, write a failing test that reproduces, fix, verify
   green, commit.
3. Build + push the new image (`BUILD_SHA=<new-sha>`).
4. `VPS_updateProjectV1` to deploy.
5. Smoke `/api/health` for the new SHA.

## Root-cause debug

Common arc-relay panic origins:

- **`.unwrap()` on a DB query result** — Postgres returned no rows or
  a serialization error. Look for `.unwrap()` near a `sqlx::query!` or
  `sqlx::query_as!`. Fix by `.map_err(...)` and a real error response.
- **WebSocket message deserialization** — a malformed client sent
  garbage that doesn't fit the JSON-RPC shape. Wrap deserialization in
  a `match` and emit `arc.error` notification rather than panicking.
- **A `RwLock` deadlock** — symptom is "thread X panicked at 'rwlock
  poisoned'" after another thread panicked while holding the lock.
  The earlier-panic thread is the real bug; this thread is just the
  symptom.

## Prevention

- Don't ship new `.unwrap()` / `.expect()` on hot paths. Use
  `?` + a domain error.
- Add a test for any panic that occurred — TDD-style, write the
  reproduction before fixing.
- Eventually: a CI gate that fails the build if `cargo build` produces
  new `.unwrap()` calls in `src/rpc.rs` or `src/api/`.
