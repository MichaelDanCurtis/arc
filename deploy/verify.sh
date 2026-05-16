#!/usr/bin/env bash
# deploy/verify.sh — smoke-check that a deploy landed.
#
# Hits `/api/health` and asserts the `build_sha` field matches what was
# expected. Retries with backoff so it can be invoked immediately after
# a VPS refresh and ride out the container restart window.
#
# Usage:
#   ./deploy/verify.sh                     # expect build_sha=HEAD short sha
#   ./deploy/verify.sh --sha 26c25fc       # expect a specific sha (e.g. post-rollback)
#   ./deploy/verify.sh --host arc.ailuxis.com    # override host
#
# Exit codes:
#   0  /api/health responded 200 with build_sha matching expected
#   1  /api/health never returned matching build_sha within timeout
#   2  /api/health returned 200 but JSON was malformed

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

EXPECTED_SHA="$(git rev-parse --short HEAD)"
HOST="arc.ailuxis.com"
MAX_TRIES=20
DELAY=5

while [[ $# -gt 0 ]]; do
  case "$1" in
    --sha)  EXPECTED_SHA="$2"; shift 2 ;;
    --host) HOST="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 64 ;;
  esac
done

URL="https://$HOST/api/health"

echo "▸ Verifying $URL reports build_sha=$EXPECTED_SHA"
for attempt in $(seq 1 "$MAX_TRIES"); do
  body=$(curl -sS --max-time 10 "$URL" 2>/dev/null || true)

  if [[ -z "$body" ]]; then
    echo "  attempt $attempt/$MAX_TRIES: no response (relay restarting?), retrying in ${DELAY}s"
    sleep "$DELAY"
    continue
  fi

  # Pull build_sha out of the JSON. Trust nothing — body could be HTML or
  # a plain "404 page not found" if Traefik is mid-routing.
  reported_sha=$(echo "$body" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    print(d.get('build_sha', ''))
except Exception:
    pass
" 2>/dev/null || true)

  if [[ -z "$reported_sha" ]]; then
    echo "  attempt $attempt/$MAX_TRIES: response not JSON (probably mid-restart), retrying in ${DELAY}s"
    sleep "$DELAY"
    continue
  fi

  if [[ "$reported_sha" == "$EXPECTED_SHA" ]]; then
    echo "✅ $URL → build_sha=$reported_sha"
    echo "$body"
    exit 0
  fi

  echo "  attempt $attempt/$MAX_TRIES: build_sha=$reported_sha (expected $EXPECTED_SHA), retrying in ${DELAY}s"
  sleep "$DELAY"
done

echo "❌ $URL never reported build_sha=$EXPECTED_SHA after $MAX_TRIES tries (~$(( MAX_TRIES * DELAY ))s)" >&2
echo "   Last body:" >&2
echo "$body" >&2
exit 1
