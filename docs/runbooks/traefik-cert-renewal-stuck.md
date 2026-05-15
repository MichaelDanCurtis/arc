# Runbook: Traefik cert renewal stuck

## Symptom

- Browser shows "Your connection is not private" / `NET::ERR_CERT_DATE_INVALID`
  on `arc.ailuxis.com` or `beam.ailuxis.com`.
- `curl -v https://arc.ailuxis.com/` shows `* SSL certificate problem:
  certificate has expired` or similar.
- API clients fail TLS handshake; `arc-relay` connections from Beacon /
  Beam break.

Let's Encrypt certs are valid for 90 days. Traefik tries to renew at
30 days remaining. Renewal failure that goes unnoticed for 30+ days =
prod outage.

## Detection

- **Future:** UptimeRobot probe with cert-expiry alert (alerts when
  cert has <14 days left).
- **Today:** check cert expiry manually:
  ```bash
  echo | openssl s_client -connect arc.ailuxis.com:443 -servername arc.ailuxis.com 2>/dev/null \
    | openssl x509 -noout -dates
  ```
  Compare `notAfter` to today. If <30 days and not actively renewing,
  something is wrong.

## Diagnosis

1. **Check Traefik logs.**
   ```bash
   ssh root@31.220.59.147
   docker logs root-traefik-1 --tail 200 2>&1 | grep -iE "acme|certificate|renew|error"
   ```
   Common ACME failures:
   - `unable to obtain certificates: invalid response from
     <challenge-url>` — challenge URL not reachable.
   - `urn:ietf:params:acme:error:rateLimited` — Let's Encrypt rate
     limit (5 certs per domain per week).
   - `Could not authorize <domain>` — DNS or HTTP-01 / TLS-ALPN
     challenge path blocked.

2. **Verify DNS still resolves.**
   ```bash
   dig +short arc.ailuxis.com
   dig +short beam.ailuxis.com
   # Both should resolve to 31.220.59.147 (or whatever the current VPS IP is).
   ```

3. **Verify port 80 + 443 are reachable.** Traefik uses TLS-ALPN-01 by
   default in our setup (port 443), which doesn't require port 80, but
   some setups fall back to HTTP-01.
   ```bash
   curl -sI -o /dev/null -w "%{http_code}\n" http://arc.ailuxis.com/.well-known/acme-challenge/test
   # Should be 404 (path not found, but port is reachable). Connection
   # refused = port blocked.
   ```

4. **Check Traefik's ACME storage.**
   ```bash
   ssh root@31.220.59.147
   docker exec root-traefik-1 ls -la /letsencrypt/
   docker exec root-traefik-1 cat /letsencrypt/acme.json | jq '.[].Account' 2>&1 | head
   ```
   The `acme.json` file persists certs across container restarts. If
   it's missing or corrupted, Traefik re-issues from scratch (slow if
   rate-limited).

## Recovery

### Force renewal (when ACME challenges work but Traefik is being lazy)

1. Delete the specific cert entry from `acme.json` (back up first):
   ```bash
   docker exec root-traefik-1 cp /letsencrypt/acme.json /letsencrypt/acme.json.bak
   # Edit acme.json to remove the entry for arc.ailuxis.com / beam.ailuxis.com
   # Then restart traefik:
   docker restart root-traefik-1
   ```
   Traefik will request a new cert on next request.

### Rate-limit hit (5/week exceeded)

Wait. Let's Encrypt's primary rate limit is 5 duplicate certs per
domain set per week, and resets weekly. If you've burned the quota by
deleting and re-issuing, you must wait until the 7-day rolling window
clears.

While waiting, the existing cert (still valid until expiry) keeps
serving. New ACME requests will fail with `429` but cached certs
work.

### Challenge unreachable

If Let's Encrypt can't reach the challenge endpoint:

- **HTTP-01:** Port 80 must be open from the Internet to Traefik's
  HTTP entrypoint.
  ```bash
  ssh root@31.220.59.147
  ss -tlnp | grep ':80 '
  # Should show traefik listening on 0.0.0.0:80
  ```
  Check Hostinger firewall rules.

- **TLS-ALPN-01:** Port 443 must be open AND Traefik's TLS entrypoint
  must be using `TLS-ALPN-01` resolver. Check
  `docker inspect root-traefik-1 --format '{{json .Args}}'` for
  `--certificatesresolvers.<name>.acme.tlschallenge=true`.

- **DNS-01 (if used):** Cloudflare API token must be valid; check
  `--certificatesresolvers.<name>.acme.dnschallenge.provider=cloudflare`
  and the `CLOUDFLARE_DNS_API_TOKEN` env var.

## Root-cause debug

- **`acme.json` permissions.** Traefik refuses to write the file if
  perms aren't `600`. Symptom: ACME succeeds but cert isn't persisted;
  next restart re-issues; eventually rate-limited.
  ```bash
  docker exec root-traefik-1 chmod 600 /letsencrypt/acme.json
  ```
- **Wrong cert resolver name in labels.** A docker label like
  `traefik.http.routers.foo.tls.certresolver=lets-encrypt` only works
  if Traefik knows a resolver named exactly `lets-encrypt`. Typo →
  silent fallback to self-signed → browser warning.
- **Wildcard cert via DNS-01 expected but HTTP-01 requested.** If
  Traefik is configured for TLS-ALPN-01 / HTTP-01, it can only issue
  per-host certs (not `*.ailuxis.com`). To get a wildcard, switch the
  resolver to DNS-01.

## Prevention

- Add an UptimeRobot or external monitor that alerts at `cert valid <
  14 days`.
- Document the Traefik command-line args + resolver name in
  `deploy/docker-compose.prod.yml` so the configuration is in the
  repo, not just on the VPS.
- Test renewal in staging quarterly (force-delete a staging cert and
  watch it re-issue).
