#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="${ROOT_DIR}/.env"
BACKUP_DIR="${ROOT_DIR}/backups"
WAIT_SECONDS="${LEDGERGUARD_DEPLOY_WAIT_SECONDS:-90}"

cd "${ROOT_DIR}"

log() { printf '[ledgerguard] %s\n' "$*"; }
die() { printf '[ledgerguard] ERROR: %s\n' "$*" >&2; exit 1; }

require() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

random_hex_32() {
  od -An -N32 -tx1 /dev/urandom | tr -d ' \n'
}

read_env_value() {
  local key="$1"
  awk -F= -v key="$key" '$1 == key { sub(/^[^=]*=/, ""); print; exit }' "${ENV_FILE}"
}

bootstrap_env() {
  if [[ -f "${ENV_FILE}" ]]; then
    chmod 600 "${ENV_FILE}"
    return
  fi

  local postgres_password api_token
  postgres_password="$(random_hex_32)"
  api_token="$(random_hex_32)"

  umask 077
  cat > "${ENV_FILE}" <<EOF
POSTGRES_PASSWORD=${postgres_password}
LEDGERGUARD_API_TOKEN=${api_token}
LEDGERGUARD_AUTH_DISABLED=false
LEDGERGUARD_PORT=8088
RUST_LOG=ledgerguard=info,tower_http=info
LEDGERGUARD_ACCOUNTING_PROVIDER=saldeo
LEDGERGUARD_LIVE_SYNC_ENABLED=false
SALDEO_BASE_URL=https://saldeo.brainshare.pl
# SALDEO_USERNAME=
# SALDEO_API_TOKEN=
# SALDEO_COMPANY_PROGRAM_ID=
EOF
  chmod 600 "${ENV_FILE}"
  log "created ${ENV_FILE} with random PostgreSQL/API secrets"
  log "Saldeo credentials intentionally remain empty"
}

backup_database_if_running() {
  local postgres_id
  postgres_id="$(docker compose --env-file "${ENV_FILE}" ps -q postgres 2>/dev/null || true)"
  [[ -n "${postgres_id}" ]] || return 0

  local running
  running="$(docker inspect -f '{{.State.Running}}' "${postgres_id}" 2>/dev/null || true)"
  [[ "${running}" == "true" ]] || return 0

  mkdir -p "${BACKUP_DIR}"
  chmod 700 "${BACKUP_DIR}"
  local stamp target
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  target="${BACKUP_DIR}/ledgerguard-predeploy-${stamp}.sql.gz"
  log "creating pre-deploy PostgreSQL backup"
  docker compose --env-file "${ENV_FILE}" exec -T postgres \
    pg_dump -U ledgerguard -d ledgerguard | gzip -9 > "${target}"
  chmod 600 "${target}"
  find "${BACKUP_DIR}" -type f -name 'ledgerguard-predeploy-*.sql.gz' -mtime +30 -delete
  log "backup: ${target}"
}

update_checkout() {
  if [[ ! -d .git ]]; then
    return 0
  fi

  local branch
  branch="$(git branch --show-current)"
  if [[ "${branch}" != "main" ]]; then
    log "checkout is on ${branch}; skipping git pull (deploying current checkout)"
    return 0
  fi

  if ! git diff --quiet || ! git diff --cached --quiet; then
    die "working tree is dirty; commit/stash changes before deployment"
  fi

  log "updating main with fast-forward only"
  git pull --ff-only origin main
}

wait_for_health() {
  local app_id deadline status
  app_id="$(docker compose --env-file "${ENV_FILE}" ps -q app)"
  [[ -n "${app_id}" ]] || die "app container was not created"
  deadline=$((SECONDS + WAIT_SECONDS))

  while (( SECONDS < deadline )); do
    status="$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "${app_id}" 2>/dev/null || true)"
    case "${status}" in
      healthy) return 0 ;;
      unhealthy|exited|dead)
        docker compose --env-file "${ENV_FILE}" logs --tail=120 app >&2 || true
        die "app entered ${status} state"
        ;;
    esac
    sleep 2
  done

  docker compose --env-file "${ENV_FILE}" logs --tail=120 app >&2 || true
  die "health check did not become healthy within ${WAIT_SECONDS}s"
}

main() {
  require docker
  require git
  require od
  require awk
  require gzip

  docker compose version >/dev/null 2>&1 || die "Docker Compose v2 is required"

  bootstrap_env
  docker compose --env-file "${ENV_FILE}" config --quiet
  backup_database_if_running
  update_checkout

  log "building runtime image"
  docker compose --env-file "${ENV_FILE}" build --pull app

  log "starting LedgerGuard"
  docker compose --env-file "${ENV_FILE}" up -d --remove-orphans
  wait_for_health

  local port
  port="$(read_env_value LEDGERGUARD_PORT)"
  port="${port:-8088}"
  log "deployment healthy"
  docker compose --env-file "${ENV_FILE}" ps
  printf '\nDashboard: http://127.0.0.1:%s/\n' "${port}"
  printf 'API token:  %s (kept local; do not paste it into chat)\n' "${ENV_FILE}"
  printf 'Remote access: use your private reverse proxy/Tailscale path or an SSH tunnel.\n'
}

main "$@"
