#!/usr/bin/env bash
# deploy/rollback.sh — fast rollback by retagging a prior :SHA as :latest.
#
# Tier-one plan T1.2: rollback target is <2 min, single command.
#
# Algorithm:
#   1. Pull the prior :SHA image from Docker Hub to verify it exists.
#   2. Retag it as :latest locally.
#   3. Push :latest back to Docker Hub (overwriting the broken latest).
#   4. Print the VPS refresh command for the arc-claude session.
#
# This script does NOT touch the VPS — that step lives in
# `mcp__hostinger__VPS_updateProjectV1(1147025, "arc")` called from an
# arc-claude session after this script finishes. Same split as release.sh.
#
# Usage:
#   ./deploy/rollback.sh <prior-sha>
#
# Example:
#   ./deploy/rollback.sh 26c25fc
#
# To find a prior-SHA, look at recent build_sha values reported by
# `/api/health` history (Sentry release tags, your own logs), or
# `git log --oneline origin/main` for likely-good SHAs that have been
# built.

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <prior-sha>" >&2
  echo "" >&2
  echo "Rolls back arc-relay's :latest to point at the given prior tag." >&2
  echo "Find a prior-sha via /api/health history, Sentry releases, or git log." >&2
  exit 64
fi

PRIOR_SHA="$1"
DOCKER_HUB_USER="${DOCKER_HUB_USER:-michaeldancurtis}"
PLATFORM="${PLATFORM:-linux/amd64}"

echo "▸ ARC rollback"
echo "  target SHA   = $PRIOR_SHA"
echo "  retagging    = $DOCKER_HUB_USER/arc-relay:$PRIOR_SHA → :latest"
echo

# ── verify the prior image exists on Docker Hub ─────────────────────────
echo "▸ Verifying $DOCKER_HUB_USER/arc-relay:$PRIOR_SHA is pullable…"
if ! docker manifest inspect "$DOCKER_HUB_USER/arc-relay:$PRIOR_SHA" > /dev/null 2>&1; then
  echo "❌ Manifest not found on Docker Hub: $DOCKER_HUB_USER/arc-relay:$PRIOR_SHA" >&2
  echo "   Either the SHA is wrong, or that build was never pushed." >&2
  echo "   Available tags:" >&2
  curl -sS "https://hub.docker.com/v2/repositories/$DOCKER_HUB_USER/arc-relay/tags/?page_size=20" \
    | python3 -c "import json,sys; d=json.load(sys.stdin); [print(' ', t['name']) for t in d.get('results',[])]" 2>/dev/null >&2 || true
  exit 65
fi
echo "✅ Found."

# ── pull, retag, push ───────────────────────────────────────────────────
echo "▸ Pulling :$PRIOR_SHA"
docker pull --platform "$PLATFORM" "$DOCKER_HUB_USER/arc-relay:$PRIOR_SHA"

echo "▸ Retagging :$PRIOR_SHA → :latest"
docker tag "$DOCKER_HUB_USER/arc-relay:$PRIOR_SHA" "$DOCKER_HUB_USER/arc-relay:latest"

echo "▸ Pushing :latest"
docker push "$DOCKER_HUB_USER/arc-relay:latest"

# ── next steps ──────────────────────────────────────────────────────────
echo
echo "✅ Rollback prepared on Docker Hub."
echo
echo "▸ Next step (run from arc-claude session):"
echo "  mcp__hostinger__VPS_updateProjectV1(virtualMachineId: 1147025, projectName: \"arc\")"
echo
echo "▸ Then verify rollback landed:"
echo "  ./deploy/verify.sh --sha $PRIOR_SHA"
echo
echo "▸ Don't forget: track down the bug in the rolled-forward SHA before"
echo "  the next release, or you'll regress again. See"
echo "  docs/runbooks/arc-relay-crash-loop.md for the slow-fix workflow."
