#!/usr/bin/env bash
# Interactive Saldeo + planning-policy setup for a deployed LedgerGuard.
#
# Run ON the host as root (it edits .env and recreates the app container):
#   cd /srv/ledgerguard/app && sudo bash scripts/configure-saldeo.sh
#
# Prompts happen on /dev/tty so the script can be piped (curl | sudo bash)
# without stealing the pipe, and typed secrets never land in shell history.
set -Eeuo pipefail

cd "$(dirname "$0")/.."

[[ -f .env ]] || { echo "ERROR: .env not found here — run this from the deployment checkout." >&2; exit 1; }
command -v docker >/dev/null || { echo "ERROR: docker not found" >&2; exit 1; }

tty_read() {
  local prompt="$1" silent="$2" value
  if [[ -r /dev/tty ]]; then
    if [[ "$silent" == "silent" ]]; then
      value="$(bash -c "read -rsp \"$prompt\" v </dev/tty; echo \"\$v\"" )"; printf '\n' >/dev/tty
    else
      value="$(bash -c "read -rp \"$prompt\" v </dev/tty; echo \"\$v\"")"
    fi
  else
    if [[ "$silent" == "silent" ]]; then read -rsp "$prompt" value; printf '\n'; else read -rp "$prompt" value; fi
  fi
  printf '%s' "$value"
}

echo "== LedgerGuard / SaldeoSMART configuration =="
echo "The API authenticates with your login plus an API TOKEN (not your web password)."
echo "Token lives in the Saldeo panel: Konfiguracja/Ustawienia -> API (Dane do API)."
echo

USERNAME="$(tty_read "Saldeo login (e-mail): " normal)"
[[ -n "$USERNAME" ]] || { echo "empty login" >&2; exit 1; }
API_TOKEN="$(tty_read "Saldeo API token: " silent)"
[[ ${#API_TOKEN} -ge 16 ]] || { echo "token looks too short" >&2; exit 1; }
COMPANY_ID="$(tty_read "SALDEO_COMPANY_PROGRAM_ID [enter to skip]: " normal)"
BUDGET="$(tty_read "Monthly cost budget in PLN, e.g. 12000.00 [enter to skip]: " normal)"

drop() { sed -i "/^$1=/d" .env; }
drop SALDEO_USERNAME; drop SALDEO_API_TOKEN; drop SALDEO_COMPANY_PROGRAM_ID
drop LEDGERGUARD_MONTHLY_COST_BUDGET; drop LEDGERGUARD_TIGHT_SHARE_BASIS_POINTS
drop LEDGERGUARD_LIVE_SYNC_ENABLED

{
  printf 'SALDEO_USERNAME=%s\n' "$USERNAME"
  printf 'SALDEO_API_TOKEN=%s\n' "$API_TOKEN"
  [[ -n "$COMPANY_ID" ]] && printf 'SALDEO_COMPANY_PROGRAM_ID=%s\n' "$COMPANY_ID"
  [[ -n "$BUDGET" ]] && printf 'LEDGERGUARD_MONTHLY_COST_BUDGET=%s\n' "$BUDGET"
  printf 'LEDGERGUARD_TIGHT_SHARE_BASIS_POINTS=%s\n' "$(tty_read 'Tight threshold bp [1000]: ' normal)"
  printf 'LEDGERGUARD_LIVE_SYNC_ENABLED=true\n'
} >> .env
chmod 600 .env

echo "==> restarting app"
docker compose --env-file .env up -d --force-recreate app >/dev/null
sleep 8
docker compose --env-file .env ps app
TOKEN="$(grep '^LEDGERGUARD_API_TOKEN=' .env | cut -d= -f2)"
PORT="$(grep '^LEDGERGUARD_PORT=' .env | cut -d= -f2); PORT=${PORT:-8088}"
echo "== provider status:"
curl -s -H "authorization: Bearer $TOKEN" "http://127.0.0.1:${PORT}/v1/accounting/provider"
echo
echo "CONFIGURE_SALDEO=DONE next=POST /v1/accounting/sync for the current month"
