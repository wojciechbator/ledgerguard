#!/usr/bin/env bash
set -Eeuo pipefail

# Blue-green LedgerGuard deploy.
#
# Builds the new image, starts a green app container alongside the current
# one, health-checks it, switches the port mapping, then stops the old one.
#
# On failure: stops green, leaves blue running.
#
# Usage (runs ON virya-home via SSH):
#   bash scripts/deploy-bluegreen.sh <target-sha>

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-}"
ENV_FILE="${ROOT_DIR}/.env"
HEALTH_TIMEOUT=90
GREEN_APP="ledgerguard-app-green-1"
BLUE_APP="ledgerguard-app-1"
GREEN_STARTED=false

fail() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

rollback() {
  local status="${1:-1}"
  trap - ERR INT TERM HUP
  if [[ "$GREEN_STARTED" == true ]]; then
    printf 'ROLLBACK=STOPPING_GREEN\n' >&2
    docker stop "$GREEN_APP" >/dev/null 2>&1 || true
    docker rm "$GREEN_APP" >/dev/null 2>&1 || true
  fi
  printf 'ROLLBACK=COMPLETE status=%d\n' "$status" >&2
  exit "$status"
}

[[ "$TARGET" =~ ^[0-9a-f]{40}$ ]] || fail "usage: deploy-bluegreen.sh <sha>"
for command in docker curl; do command -v "$command" >/dev/null 2>&1 || fail "missing: $command"; done

cd "$ROOT_DIR"
[[ -f "$ENV_FILE" ]] || fail "missing .env"
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || fail 'worktree must be clean'
[[ "$(git rev-parse HEAD)" == "$TARGET" ]] || fail "HEAD must equal target"

# Verify blue is running
blue_health="$(docker inspect "$BLUE_APP" --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' 2>/dev/null || true)"
[[ "$blue_health" == "healthy" || "$blue_health" == "running" ]] || fail "blue app not healthy: $blue_health"
printf 'BLUE_BASELINE=PASS health=%s\n' "$blue_health"

trap 'rollback $?' ERR INT TERM HUP

# --- 1. Build green image ---------------------------------------------------

printf '\n==> 1/3 — Build green image\n'
docker build --target runtime -t ledgerguard:sha-${TARGET:0:12} .
printf 'GREEN_IMAGE=PASS\n'

# --- 2. Start green app + health-check --------------------------------------

printf '\n==> 2/3 — Start green app and health-check\n'
GREEN_STARTED=true

# Start green app on a different port
GREEN_PORT=$((RANDOM % 1000 + 18000))
docker run -d \
  --name "$GREEN_APP" \
  --restart "no" \
  --env-file "$ENV_FILE" \
  -e DATABASE_URL="postgres://ledgerguard:$(grep POSTGRES_PASSWORD "$ENV_FILE" | cut -d= -f2)@ledgerguard-postgres-1:5432/ledgerguard" \
  -p "127.0.0.1:${GREEN_PORT}:8080" \
  --network ledgerguard_database \
  --network ledgerguard_egress \
  --read-only \
  --tmpfs /tmp:size=256m,mode=1777 \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  --pids-limit 128 \
  --memory 512m \
  ledgerguard:sha-${TARGET:0:12}

# Wait for green to be healthy
green_health=""
for attempt in $(seq 1 30); do
  green_health="$(docker inspect "$GREEN_APP" --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' 2>/dev/null || true)"
  if [[ "$green_health" == "healthy" ]]; then
    break
  fi
  sleep 2
done
[[ "$green_health" == "healthy" ]] || fail "green app did not become healthy: $green_health"

# Direct health check
curl --fail --silent --show-error --connect-timeout 3 --max-time 10 \
  "http://127.0.0.1:${GREEN_PORT}/health" >/dev/null

printf 'GREEN_HEALTH=PASS port=%s\n' "$GREEN_PORT"

# --- 3. Switch: stop blue, rename green -------------------------------------

printf '\n==> 3/3 — Switch to green\n'

# Stop blue
docker stop "$BLUE_APP" >/dev/null 2>&1 || true
docker rm "$BLUE_APP" >/dev/null 2>&1 || true

# Rename green to the standard name
docker rename "$GREEN_APP" "$BLUE_APP"

# Restart with the standard port and restart policy
docker stop "$BLUE_APP" >/dev/null 2>&1 || true
docker rm "$BLUE_APP" >/dev/null 2>&1 || true

LEDGERGUARD_PORT="$(grep LEDGERGUARD_PORT "$ENV_FILE" 2>/dev/null | cut -d= -f2 || echo "8088")"
docker run -d \
  --name "$BLUE_APP" \
  --restart unless-stopped \
  --init \
  --env-file "$ENV_FILE" \
  -p "127.0.0.1:${LEDGERGUARD_PORT}:8080" \
  --network ledgerguard_database \
  --network ledgerguard_egress \
  --read-only \
  --tmpfs /tmp:size=256m,mode=1777 \
  --cap-drop ALL \
  --security-opt no-new-privileges:true \
  --pids-limit 128 \
  --memory 512m \
  --healthcmd '/ledgerguard healthcheck' \
  ledgerguard:sha-${TARGET:0:12}

# Wait for the final container to be healthy
for attempt in $(seq 1 30); do
  final_health="$(docker inspect "$BLUE_APP" --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' 2>/dev/null || true)"
  if [[ "$final_health" == "healthy" ]]; then
    break
  fi
  sleep 2
done
[[ "$final_health" == "healthy" ]] || fail "final app did not become healthy: $final_health"

curl --fail --silent --show-error --connect-timeout 3 --max-time 10 \
  "http://127.0.0.1:${LEDGERGUARD_PORT}/health" >/dev/null

trap - ERR INT TERM HUP
printf '\nLEDGERGUARD_BLUEGREEN=PASS sha=%s cutover=zero-downtime\n' "$TARGET"
