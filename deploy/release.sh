#!/usr/bin/env bash
# deploy/release.sh — build + push ARC images to Docker Hub.
#
# Tier-one plan T1.2: this is the build+push half of the deploy. The VPS
# refresh step is intentionally NOT in this script — it's an MCP tool
# call (`mcp__hostinger__VPS_updateProjectV1(1147025, "arc")`) executed
# from an arc-claude session after this script finishes. Reason for the
# split:
#   - Build can take 30+ min (linux/amd64 cross from Apple Silicon).
#   - VPS refresh is 30 sec.
#   - Splitting lets the session do other work while the build runs.
#
# Usage:
#   ./deploy/release.sh                    # arc-relay only, BUILD_SHA=HEAD short sha
#   ./deploy/release.sh --include-web      # also rebuild arc-web
#   ./deploy/release.sh --sha 26c25fc      # explicit sha tag (e.g. retag of an existing build)
#   ./deploy/release.sh --no-relay --include-web   # arc-web only
#
# Tags applied per image:
#   michaeldancurtis/arc-{relay,web}:latest
#   michaeldancurtis/arc-{relay,web}:<SHA>
#
# Smoke:
#   After your arc-claude session triggers VPS_updateProjectV1, run
#   `./deploy/verify.sh` (or curl /api/health directly) to confirm the
#   build_sha field matches what you just pushed.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ── defaults ────────────────────────────────────────────────────────────
BUILD_SHA="$(git rev-parse --short HEAD)"
DO_RELAY=1
DO_WEB=0
DOCKER_HUB_USER="${DOCKER_HUB_USER:-michaeldancurtis}"
PLATFORM="${PLATFORM:-linux/amd64}"
NEXT_PUBLIC_ARC_RELAY_URL="${NEXT_PUBLIC_ARC_RELAY_URL:-https://arc.ailuxis.com}"

# ── args ────────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --include-web) DO_WEB=1; shift ;;
    --no-relay)    DO_RELAY=0; shift ;;
    --sha)         BUILD_SHA="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 64 ;;
  esac
done

if [[ "$DO_RELAY" -eq 0 && "$DO_WEB" -eq 0 ]]; then
  echo "Nothing to do (both --no-relay and --include-web absent)." >&2
  exit 64
fi

# ── preflight ───────────────────────────────────────────────────────────
echo "▸ ARC release"
echo "  BUILD_SHA      = $BUILD_SHA"
echo "  arc-relay      = $([[ $DO_RELAY -eq 1 ]] && echo yes || echo no)"
echo "  arc-web        = $([[ $DO_WEB -eq 1 ]] && echo yes || echo no)"
echo "  platform       = $PLATFORM"
echo "  hub-user       = $DOCKER_HUB_USER"
echo "  web-relay-url  = $NEXT_PUBLIC_ARC_RELAY_URL"
echo

# Warn if BUILD_SHA doesn't match HEAD — usually a retag, not a fresh build.
HEAD_SHA="$(git rev-parse --short HEAD)"
if [[ "$BUILD_SHA" != "$HEAD_SHA" ]]; then
  echo "⚠  BUILD_SHA ($BUILD_SHA) != git HEAD ($HEAD_SHA)"
  echo "   /api/health will report build_sha=$BUILD_SHA but the source SHA differs."
  echo "   Acceptable for retag or explicit override; abort if surprising (Ctrl-C in 3s)."
  sleep 3
fi

# ── arc-relay ───────────────────────────────────────────────────────────
if [[ "$DO_RELAY" -eq 1 ]]; then
  echo "▸ Building arc-relay:$BUILD_SHA"
  docker buildx build \
    --platform "$PLATFORM" \
    -f arc-relay/Dockerfile \
    --build-arg BUILD_SHA="$BUILD_SHA" \
    -t "$DOCKER_HUB_USER/arc-relay:latest" \
    -t "$DOCKER_HUB_USER/arc-relay:$BUILD_SHA" \
    --push .
  echo "✅ Pushed $DOCKER_HUB_USER/arc-relay:{latest,$BUILD_SHA}"
fi

# ── arc-web ─────────────────────────────────────────────────────────────
if [[ "$DO_WEB" -eq 1 ]]; then
  echo "▸ Building arc-web:$BUILD_SHA"
  docker buildx build \
    --platform "$PLATFORM" \
    -f arc-web/Dockerfile \
    --build-arg NEXT_PUBLIC_ARC_RELAY_URL="$NEXT_PUBLIC_ARC_RELAY_URL" \
    -t "$DOCKER_HUB_USER/arc-web:latest" \
    -t "$DOCKER_HUB_USER/arc-web:$BUILD_SHA" \
    --push .
  echo "✅ Pushed $DOCKER_HUB_USER/arc-web:{latest,$BUILD_SHA}"
fi

# ── next steps ──────────────────────────────────────────────────────────
echo
echo "▸ Next step (run from arc-claude session):"
echo "  mcp__hostinger__VPS_updateProjectV1(virtualMachineId: 1147025, projectName: \"arc\")"
echo
echo "▸ Then verify:"
echo "  ./deploy/verify.sh --sha $BUILD_SHA"
