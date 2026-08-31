#!/usr/bin/env bash
set -Eeuo pipefail

# Blue-green LedgerGuard deploy with zero-downtime Caddy cutover.
#
# Builds the green image, starts app-green alongside the current app,
# health-checks it directly, switches the Caddy proxy upstream to the
# green network alias, verifies public health, then stops the old app.
#
# On failure: reverts Caddy to blue, stops green, leaves the previous
# release running with no user-visible downtime.
#
# Usage (runs ON virya-home via SSH):
#   bash scripts/deploy-bluegreen.sh <target-sha> [root-dir]
#
# When shipped to /tmp by deploy.sh, BASH_SOURCE resolves to /tmp and the
# parent-dir heuristic breaks. Accept an explicit root-dir as the 2nd arg.

if [[ -n "${2:-}" ]]; then
  ROOT_DIR="$2"
else
  ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi
TARGET="${1:-}"
ENV_FILE="${ROOT_DIR}/.env"
CADDYFILE="${ROOT_DIR}/Caddyfile"
GREEN_APP="ledgerguard-app-green-1"
BLUE_APP="ledgerguard-app-1"
GREEN_ALIAS="ledgerguard-app-green"
BLUE_ALIAS="ledgerguard-app"
PROXY_CONTAINER="ledgerguard-proxy-1"
CADDY_BACKUP=""
GREEN_STARTED=false
CADDY_SWITCHED=false

fail() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

rollback() {
  local status="${1:-1}"
  trap - ERR INT TERM HUP

  if [[ "$CADDY_SWITCHED" == true ]]; then
    printf 'ROLLBACK=START reason=caddy-switched reverting upstream to %s\n' "$BLUE_ALIAS" >&2
    if [[ -n "$CADDY_BACKUP" && -f "$CADDY_BACKUP" ]]; then
      cp "$CADDY_BACKUP" "$CADDYFILE"
      docker exec "$PROXY_CONTAINER" caddy reload --config /etc/caddy/Caddyfile --force >/dev/null 2>&1 || true
      printf 'ROLLBACK=CADDY_REVERTED upstream=%s\n' "$BLUE_ALIAS" >&2
    fi
  fi

  if [[ "$GREEN_STARTED" == true ]]; then
    printf 'ROLLBACK=STOPPING_GREEN\n' >&2
    cd "$ROOT_DIR"
    docker compose --env-file "$ENV_FILE" -f compose.yaml -f compose.bluegreen.yaml \
      stop app-green >/dev/null 2>&1 || true
    docker compose --env-file "$ENV_FILE" -f compose.yaml -f compose.bluegreen.yaml \
      rm -f app-green >/dev/null 2>&1 || true
    printf 'ROLLBACK=GREEN_STOPPED\n' >&2
  fi

  printf 'ROLLBACK=COMPLETE status=%d\n' "$status" >&2
  exit "$status"
}

[[ "$TARGET" =~ ^[0-9a-f]{40}$ ]] || fail "usage: deploy-bluegreen.sh <sha>"
for command in docker curl; do command -v "$command" >/dev/null 2>&1 || fail "missing: $command"; done

cd "$ROOT_DIR"
[[ -f "$ENV_FILE" ]] || fail "missing .env"
[[ -f "$CADDYFILE" ]] || fail "missing Caddyfile"
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || fail 'worktree must be clean'
[[ "$(git rev-parse HEAD)" == "$TARGET" ]] || fail "HEAD must equal target"

# Verify blue is running and healthy
blue_health="$(docker inspect "$BLUE_APP" --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' 2>/dev/null || true)"
[[ "$blue_health" == "healthy" || "$blue_health" == "running" ]] || fail "blue app not healthy: $blue_health"
printf 'BLUE_BASELINE=PASS health=%s\n' "$blue_health"

# Verify proxy is running
docker inspect "$PROXY_CONTAINER" --format '{{.State.Status}}' 2>/dev/null | grep -q running || fail "Caddy proxy is not running"

# Snapshot the Caddyfile for rollback
CADDY_BACKUP="$(mktemp -t caddyfile-lg-blue.XXXXXX)"
cp "$CADDYFILE" "$CADDY_BACKUP"
printf 'CADDY_BACKUP=PASS file=%s\n' "$CADDY_BACKUP"

trap 'rollback $?' ERR INT TERM HUP

# --- 1. Build green image ---------------------------------------------------

printf '\n==> 1/5 — Build green image\n'
docker build --target runtime -t ledgerguard:sha-${TARGET:0:12} .
printf 'GREEN_IMAGE=PASS\n'

# --- 2. Start green app -----------------------------------------------------

printf '\n==> 2/5 — Start green app\n'
GREEN_STARTED=true

export LEDGERGUARD_GREEN_TAG="sha-${TARGET:0:12}"
docker compose --env-file "$ENV_FILE" -f compose.yaml -f compose.bluegreen.yaml \
  up -d --no-deps app-green

printf 'GREEN_APP=STARTED\n'

# --- 3. Health-check green app directly -------------------------------------

printf '\n==> 3/5 — Health-check green app\n'
green_health=""
for attempt in $(seq 1 30); do
  green_health="$(docker inspect "$GREEN_APP" --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' 2>/dev/null || true)"
  if [[ "$green_health" == "healthy" ]]; then
    break
  fi
  sleep 2
done
[[ "$green_health" == "healthy" ]] || fail "green app did not become healthy: $green_health"

# Direct health check via Docker network
docker run --rm --network ledgerguard_internal curlimages/curl:8.12.0 \
  --fail --silent --show-error --connect-timeout 3 --max-time 10 \
  "http://${GREEN_ALIAS}:8080/health" >/dev/null

printf 'GREEN_HEALTH=PASS\n'

# --- 4. Switch Caddy to green -----------------------------------------------

printf '\n==> 4/5 — Switch Caddy upstream to green\n'
sed -i "s|reverse_proxy ${BLUE_ALIAS}:8080|reverse_proxy ${GREEN_ALIAS}:8080|" "$CADDYFILE"

# Verify the sed changed something
grep -Fq "reverse_proxy ${GREEN_ALIAS}:8080" "$CADDYFILE" || fail "Caddyfile was not updated to green upstream"
grep -Fq "reverse_proxy ${BLUE_ALIAS}:8080" "$CADDYFILE" && fail "Caddyfile still contains blue upstream — ambiguous state"

# Graceful Caddy reload (zero-downtime: in-flight requests complete, new ones go to green)
docker exec "$PROXY_CONTAINER" caddy reload --config /etc/caddy/Caddyfile --force
CADDY_SWITCHED=true
printf 'CADDY_SWITCH=PASS upstream=%s\n' "$GREEN_ALIAS"

# --- 5. Verify public health + stop blue ------------------------------------

printf '\n==> 5/5 — Verify health and finalize\n'
LEDGERGUARD_PORT="$(grep LEDGERGUARD_PORT "$ENV_FILE" 2>/dev/null | cut -d= -f2 || echo "8088")"

# Verify the proxy is still serving
curl --fail --silent --show-error --connect-timeout 3 --max-time 10 \
  "http://127.0.0.1:${LEDGERGUARD_PORT}/health" >/dev/null
printf 'PUBLIC_HEALTH=PASS port=%s\n' "$LEDGERGUARD_PORT"

# Stop blue app
docker stop "$BLUE_APP" >/dev/null 2>&1 || true
docker rm "$BLUE_APP" >/dev/null 2>&1 || true

# Clean up
rm -f "$CADDY_BACKUP"
trap - ERR INT TERM HUP

printf '\nLEDGERGUARD_BLUEGREEN=PASS sha=%s cutover=zero-downtime blue=stopped green=active\n' "$TARGET"
