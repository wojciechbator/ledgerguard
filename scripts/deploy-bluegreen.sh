#!/usr/bin/env bash
set -Eeuo pipefail

# Blue-green LedgerGuard deploy with exact-artifact verification.
#
# Pulls a pre-built image by tag, verifies its OCI revision label
# matches the target SHA, runs migrations, starts app-green alongside
# the current app, health-checks it directly, verifies /meta, reloads
# Caddy (not restart) to route to green, soaks with error-rate monitoring,
# then stops the old app.
#
# The Caddyfile uses a static dual-upstream format with lb_policy first:
#   reverse_proxy ledgerguard-app-1:8080 ledgerguard-app-green-1:8080 {
#       lb_policy first ...
#   }
# The active upstream is determined by order (first = primary) and the
# # LEDGERGUARD_ACTIVE= marker. Cutover reorders the upstreams and
# updates the marker, then gracefully reloads Caddy.
#
# On failure: reverts Caddy to blue, stops green, leaves the previous
# release running with no user-visible downtime.
#
# Usage (runs ON virya-home via SSH):
#   bash scripts/deploy-bluegreen.sh <target-sha> [root-dir]
#
# Environment:
#   LEDGERGUARD_REGISTRY  — registry host (e.g. ghcr.io/owner/ledgerguard)

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
RELEASE_STATE_DIR="/var/lib/ledgerguard/releases"
RECEIPT_HELPER="${ROOT_DIR}/scripts/release_receipt.py"
CADDY_BACKUP=""
GREEN_STARTED=false
CADDY_SWITCHED=false
RELEASE_ID=""
SOAK_SECONDS="${LEDGERGUARD_SOAK_SECONDS:-300}"
SOAK_ERROR_THRESHOLD_PERCENT="${LEDGERGUARD_SOAK_ERROR_THRESHOLD_PERCENT:-2}"
SOAK_MAX_ABSOLUTE_FAILURES="${LEDGERGUARD_SOAK_MAX_ABSOLUTE_FAILURES:-3}"

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
      docker exec "$PROXY_CONTAINER" caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile --address 127.0.0.1:2019 >/dev/null 2>&1 || true
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

  # Write failure receipt
  if [[ -n "$RELEASE_ID" ]]; then
    python3 "$RECEIPT_HELPER" rollback \
      --state-dir "$RELEASE_STATE_DIR" \
      --release-id "$RELEASE_ID" \
      --service ledgerguard \
      --reason "deploy-failure" >/dev/null 2>&1 || true
  fi

  printf 'ROLLBACK=COMPLETE status=%d\n' "$status" >&2
  exit "$status"
}

[[ "$TARGET" =~ ^[0-9a-f]{40}$ ]] || fail "usage: deploy-bluegreen.sh <sha>"
for command in docker curl jq; do command -v "$command" >/dev/null 2>&1 || fail "missing: $command"; done

cd "$ROOT_DIR"
[[ -f "$ENV_FILE" ]] || fail "missing .env"
[[ -f "$CADDYFILE" ]] || fail "missing Caddyfile"
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || fail 'worktree must be clean'
[[ "$(git rev-parse HEAD)" == "$TARGET" ]] || fail "HEAD must equal target"

# Verify Caddyfile is release-ready: static dual-upstream + active marker
grep -Fq '# LEDGERGUARD_ACTIVE=' "$CADDYFILE" || \
  fail 'Caddyfile is not release-ready: missing active release marker'
grep -Fq 'reverse_proxy ledgerguard-app-1:8080 ledgerguard-app-green-1:8080' "$CADDYFILE" \
  || grep -Fq 'reverse_proxy ledgerguard-app-green-1:8080 ledgerguard-app-1:8080' "$CADDYFILE" \
  || fail 'Caddyfile does not contain the static blue-green upstream pair'
printf 'PREFLIGHT=PASS caddy=blue-green-ready\n'

# Initialise release state and write pending receipt
python3 "$RECEIPT_HELPER" init --state-dir "$RELEASE_STATE_DIR" --service ledgerguard >/dev/null
RELEASE_ID="lg-${TARGET:0:12}-$(date -u +%Y%m%d%H%M%S)"
python3 "$RECEIPT_HELPER" pending \
  --state-dir "$RELEASE_STATE_DIR" \
  --service ledgerguard \
  --release-id "$RELEASE_ID" \
  --source-sha "$TARGET" >/dev/null
printf 'RELEASE=%s\n' "$RELEASE_ID"

# --- Pre-deploy: verify critical secrets are present and non-empty ----------
env_fail=0
for var in POSTGRES_PASSWORD LEDGERGUARD_API_TOKEN; do
  val="$(grep -E "^${var}=" "$ENV_FILE" 2>/dev/null | head -1 | cut -d= -f2- || true)"
  if [[ -z "$val" || "$val" == replace-with-* ]]; then
    printf 'ENV_CHECK=FAIL var=%s is missing or placeholder\n' "$var" >&2
    env_fail=1
  fi
done
imap_user="$(grep -E "^LEDGERGUARD_IMAP_USERNAME=" "$ENV_FILE" 2>/dev/null | head -1 | cut -d= -f2- || true)"
imap_pass="$(grep -E "^LEDGERGUARD_IMAP_PASSWORD=" "$ENV_FILE" 2>/dev/null | head -1 | cut -d= -f2- || true)"
if [[ -z "$imap_user" || -z "$imap_pass" ]]; then
  printf 'ENV_CHECK=FAIL LEDGERGUARD_IMAP_USERNAME/IMAP_PASSWORD missing — email ingest will break\n' >&2
  env_fail=1
fi
[[ "$env_fail" == 0 ]] || fail "pre-deploy env check failed — fix .env before deploying"
printf 'ENV_CHECK=PASS\n'

# Verify at least one app (blue or green) is running and healthy
blue_health="$(docker inspect "$BLUE_APP" --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' 2>/dev/null || true)"
green_health="$(docker inspect "$GREEN_APP" --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' 2>/dev/null || true)"
if [[ "$blue_health" == "healthy" || "$blue_health" == "running" ]]; then
  printf 'BLUE_BASELINE=PASS health=%s\n' "$blue_health"
elif [[ "$green_health" == "healthy" || "$green_health" == "running" ]]; then
  printf 'GREEN_BASELINE=PASS health=%s (blue not running)\n' "$green_health"
else
  fail "no healthy app found (blue=$blue_health green=$green_health)"
fi

# Verify proxy is running
docker inspect "$PROXY_CONTAINER" --format '{{.State.Status}}' 2>/dev/null | grep -q running || fail "Caddy proxy is not running"

# Snapshot the Caddyfile for rollback
CADDY_BACKUP="$(mktemp -t caddyfile-lg-blue.XXXXXX)"
cp "$CADDYFILE" "$CADDY_BACKUP"
printf 'CADDY_BACKUP=PASS file=%s\n' "$CADDY_BACKUP"

trap 'rollback $?' ERR INT TERM HUP

# --- 1. Build image from source and verify -----------------------------------

printf '\n==> 1/8 — Build image from source\n'
GREEN_TAG="sha-${TARGET:0:12}"
docker build \
  --build-arg LEDGERGUARD_GIT_SHA="$TARGET" \
  --label org.opencontainers.image.revision="$TARGET" \
  -t "ledgerguard:${GREEN_TAG}" \
  . >/dev/null 2>&1 || fail "cannot build ledgerguard image"
printf 'GREEN_IMAGE=PASS tag=ledgerguard:%s\n' "$GREEN_TAG"

python3 "$RECEIPT_HELPER" phase --state-dir "$RELEASE_STATE_DIR" \
  --release-id "$RELEASE_ID" --phase build --status pass >/dev/null

# --- 2. Verify OCI revision label matches target SHA ------------------------

printf '\n==> 2/8 — Verify OCI revision label\n'
label_sha="$(docker inspect "ledgerguard:${GREEN_TAG}" --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' 2>/dev/null || true)"
if [[ -z "$label_sha" ]]; then
  printf 'LABEL_CHECK=WARN no org.opencontainers.image.revision label — skipping verification\n' >&2
elif [[ "$label_sha" != "$TARGET" ]]; then
  fail "OCI revision label mismatch: label=$label_sha target=$TARGET"
else
  printf 'LABEL_CHECK=PASS label=%s\n' "$label_sha"
fi

# --- 3. Run migrate one-shot ------------------------------------------------

printf '\n==> 3/8 — Run migrations\n'
docker run --rm \
  --network ledgerguard_database \
  --env-file "$ENV_FILE" \
  -e DATABASE_URL="postgres://ledgerguard:$(grep -E '^POSTGRES_PASSWORD=' "$ENV_FILE" | head -1 | cut -d= -f2-)@postgres:5432/ledgerguard" \
  -e RUST_LOG="${RUST_LOG:-info}" \
  "ledgerguard:${GREEN_TAG}" migrate
printf 'MIGRATE=PASS\n'

python3 "$RECEIPT_HELPER" phase --state-dir "$RELEASE_STATE_DIR" \
  --release-id "$RELEASE_ID" --phase migration --status pass >/dev/null

# --- 4. Start green app ------------------------------------------------------

printf '\n==> 4/8 — Start green app\n'
GREEN_STARTED=true

export LEDGERGUARD_GREEN_TAG="$GREEN_TAG"
export LEDGERGUARD_GIT_SHA="$TARGET"
export LEDGERGUARD_BUILD_TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
docker compose --env-file "$ENV_FILE" -f compose.yaml -f compose.bluegreen.yaml \
  up -d --no-deps app-green

printf 'GREEN_APP=STARTED\n'

# --- 5. Health-check green app directly --------------------------------------

printf '\n==> 5/8 — Health-check green app\n'
green_health=""
for attempt in $(seq 1 30); do
  green_health="$(docker inspect "$GREEN_APP" --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' 2>/dev/null || true)"
  if [[ "$green_health" == "healthy" ]]; then
    break
  fi
  sleep 2
done
[[ "$green_health" == "healthy" ]] || fail "green app did not become healthy: $green_health"

docker run --rm --network ledgerguard_internal curlimages/curl:8.12.0 \
  --fail --silent --show-error --connect-timeout 3 --max-time 10 \
  "http://${GREEN_ALIAS}:8080/healthz" >/dev/null

printf 'GREEN_HEALTH=PASS\n'

# --- 6. Verify /meta gitSha matches target ----------------------------------

printf '\n==> 6/8 — Verify /meta gitSha\n'
meta_json="$(docker run --rm --network ledgerguard_internal curlimages/curl:8.12.0 \
  --fail --silent --show-error --connect-timeout 3 --max-time 10 \
  "http://${GREEN_ALIAS}:8080/meta")"
meta_sha="$(printf '%s' "$meta_json" | jq -r '.git_sha // empty')"
if [[ "$meta_sha" != "$TARGET" ]]; then
  fail "/meta gitSha mismatch: meta=$meta_sha target=$TARGET"
fi
printf 'META_VERIFY=PASS git_sha=%s\n' "$meta_sha"

python3 "$RECEIPT_HELPER" phase --state-dir "$RELEASE_STATE_DIR" \
  --release-id "$RELEASE_ID" --phase health --status pass >/dev/null

# --- 7. Gracefully reload Caddy to route to green ----------------------------

printf '\n==> 7/8 — Reload Caddy upstream to green\n'

# Detect which color is currently active from the Caddyfile marker.
active_color="$(sed -n 's/^[[:space:]]*# LEDGERGUARD_ACTIVE=//p' "$CADDYFILE" | head -n1)"
if [[ "$active_color" == "blue" ]]; then
  CURRENT_APP="$BLUE_APP"
  NEW_APP="$GREEN_APP"
  DEPLOY_COLOR="green"
  printf 'BASELINE=BLUE → deploying green\n'
elif [[ "$active_color" == "green" ]]; then
  CURRENT_APP="$GREEN_APP"
  NEW_APP="$BLUE_APP"
  DEPLOY_COLOR="blue"
  printf 'BASELINE=GREEN → deploying blue\n'
else
  fail "Caddyfile active marker is not 'blue' or 'green': got='${active_color:-empty}'"
fi

# Reorder the upstreams so the new color is primary (first) and update marker.
# The Caddyfile has both upstreams on one line with lb_policy first, so
# swapping the order makes the new color primary.
caddy_candidate="$(mktemp -t caddyfile-lg-candidate.XXXXXX)"
cp "$CADDYFILE" "$caddy_candidate"
if [[ "$DEPLOY_COLOR" == "green" ]]; then
  sed \
    -e 's/# LEDGERGUARD_ACTIVE=blue/# LEDGERGUARD_ACTIVE=green/' \
    -e 's|reverse_proxy ledgerguard-app-1:8080 ledgerguard-app-green-1:8080|reverse_proxy ledgerguard-app-green-1:8080 ledgerguard-app-1:8080|' \
    "$CADDYFILE" > "$caddy_candidate"
else
  sed \
    -e 's/# LEDGERGUARD_ACTIVE=green/# LEDGERGUARD_ACTIVE=blue/' \
    -e 's|reverse_proxy ledgerguard-app-green-1:8080 ledgerguard-app-1:8080|reverse_proxy ledgerguard-app-1:8080 ledgerguard-app-green-1:8080|' \
    "$CADDYFILE" > "$caddy_candidate"
fi

grep -Fq "# LEDGERGUARD_ACTIVE=${DEPLOY_COLOR}" "$caddy_candidate" || fail 'candidate Caddy marker was not updated'
if [[ "$DEPLOY_COLOR" == "green" ]]; then
  grep -Fq "reverse_proxy ledgerguard-app-green-1:8080 ledgerguard-app-1:8080" "$caddy_candidate" \
    || fail 'candidate Caddy upstream order was not updated'
else
  grep -Fq "reverse_proxy ledgerguard-app-1:8080 ledgerguard-app-green-1:8080" "$caddy_candidate" \
    || fail 'candidate Caddy upstream order was not updated'
fi

cp "$caddy_candidate" "$CADDYFILE"
rm -f "$caddy_candidate"

docker exec "$PROXY_CONTAINER" caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile --address 127.0.0.1:2019
CADDY_SWITCHED=true
printf 'CADDY_SWITCH=PASS primary=%s fallback=%s reload=graceful\n' "$NEW_APP" "$CURRENT_APP"

# --- 8. Soak with error-rate monitoring, then finalize -----------------------

printf '\n==> 8/8 — Soak and finalize\n'
LEDGERGUARD_PORT="$(grep LEDGERGUARD_PORT "$ENV_FILE" 2>/dev/null | cut -d= -f2 || echo "8088")"

# Verify the proxy is still serving
curl --fail --silent --show-error --connect-timeout 3 --max-time 10 \
  "http://127.0.0.1:${LEDGERGUARD_PORT}/healthz" >/dev/null
printf 'PUBLIC_HEALTH=PASS port=%s\n' "$LEDGERGUARD_PORT"

# Soak: poll /healthz for SOAK_SECONDS, track error rate
soak_deadline=$((SECONDS + SOAK_SECONDS))
soak_total=0
soak_errors=0
soak_interval=5
printf 'SOAK=START duration=%ss interval=%ss threshold=%s%% max_failures=%s\n' \
  "$SOAK_SECONDS" "$soak_interval" "$SOAK_ERROR_THRESHOLD_PERCENT" "$SOAK_MAX_ABSOLUTE_FAILURES"

while (( SECONDS < soak_deadline )); do
  soak_total=$((soak_total + 1))
  if ! curl --fail --silent --show-error --connect-timeout 3 --max-time 10 \
       "http://127.0.0.1:${LEDGERGUARD_PORT}/healthz" >/dev/null 2>&1; then
    soak_errors=$((soak_errors + 1))
    printf 'SOAK=ERROR total=%d errors=%d\n' "$soak_total" "$soak_errors" >&2
  fi
  sleep "$soak_interval"
done

if (( soak_total == 0 )); then
  fail "soak produced no samples — check proxy"
fi
soak_error_percent=$(( soak_errors * 100 / soak_total ))
printf 'SOAK=COMPLETE total=%d errors=%d error_rate=%d%%\n' "$soak_total" "$soak_errors" "$soak_error_percent"

if (( soak_errors >= SOAK_MAX_ABSOLUTE_FAILURES )); then
  fail "soak exceeded max absolute failures: $soak_errors >= $SOAK_MAX_ABSOLUTE_FAILURES"
fi
if (( soak_error_percent >= SOAK_ERROR_THRESHOLD_PERCENT )); then
  fail "soak exceeded error-rate threshold: ${soak_error_percent}% >= ${SOAK_ERROR_THRESHOLD_PERCENT}%"
fi
printf 'SOAK=PASS\n'

# --- Post-deploy smoke test: verify the actual dashboard endpoints work ---
API_TOKEN="$(grep -E "^LEDGERGUARD_API_TOKEN=" "$ENV_FILE" | head -1 | cut -d= -f2-)"
smoke_fail=0

dash_status="$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 --max-time 10 \
  "http://127.0.0.1:${LEDGERGUARD_PORT}/")"
if [[ "$dash_status" != "200" ]]; then
  printf 'SMOKE=FAIL dashboard status=%s\n' "$dash_status" >&2
  smoke_fail=1
fi

api_status="$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 --max-time 10 \
  -H "Authorization: Bearer ${API_TOKEN}" \
  "http://127.0.0.1:${LEDGERGUARD_PORT}/v1/system/status")"
if [[ "$api_status" != "200" ]]; then
  printf 'SMOKE=FAIL /v1/system/status status=%s (expected 200 — API token broken?)\n' "$api_status" >&2
  smoke_fail=1
fi

ledger_status="$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 --max-time 10 \
  -H "Authorization: Bearer ${API_TOKEN}" \
  "http://127.0.0.1:${LEDGERGUARD_PORT}/v1/ledger/month")"
if [[ "$ledger_status" != "200" ]]; then
  printf 'SMOKE=FAIL /v1/ledger/month status=%s (expected 200 — DB broken?)\n' "$ledger_status" >&2
  smoke_fail=1
fi

ingest_status="$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 --max-time 10 \
  -H "Authorization: Bearer ${API_TOKEN}" \
  "http://127.0.0.1:${LEDGERGUARD_PORT}/v1/ingest/documents?limit=1")"
if [[ "$ingest_status" != "200" ]]; then
  printf 'SMOKE=FAIL /v1/ingest/documents status=%s (expected 200)\n' "$ingest_status" >&2
  smoke_fail=1
fi

if [[ "$smoke_fail" == 1 ]]; then
  fail "post-deploy smoke test failed — green is live but features are broken, check .env"
fi
printf 'SMOKE=PASS dashboard api ledger ingest\n'

# Stop the old (now-fallback) app
docker stop "$CURRENT_APP" >/dev/null 2>&1 || true
docker rm "$CURRENT_APP" >/dev/null 2>&1 || true

# Clean up
rm -f "$CADDY_BACKUP"
trap - ERR INT TERM HUP

python3 "$RECEIPT_HELPER" finalize \
  --state-dir "$RELEASE_STATE_DIR" \
  --release-id "$RELEASE_ID" \
  --service ledgerguard \
  --source-sha "$TARGET" >/dev/null

printf '\nLEDGERGUARD_BLUEGREEN=PASS sha=%s cutover=graceful-reload old=%s stopped new=%s active\n' "$TARGET" "$CURRENT_APP" "$NEW_APP"
