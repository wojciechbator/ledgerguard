#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
TARGET="${1:-}"
WAIT_SECONDS="${LEDGERGUARD_DEPLOY_WAIT_SECONDS:-3600}"
POLL_SECONDS="${LEDGERGUARD_DEPLOY_POLL_SECONDS:-3}"
REMOTE="${LEDGERGUARD_DEPLOY_HOST:-virya-home}"
REMOTE_DIR="${LEDGERGUARD_DEPLOY_REMOTE_DIR:-/srv/ledgerguard}"

fail() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

require() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

for command in git gh ssh bash; do require "$command"; done
[[ "$WAIT_SECONDS" =~ ^[1-9][0-9]*$ ]] || fail 'LEDGERGUARD_DEPLOY_WAIT_SECONDS must be a positive integer'
[[ "$POLL_SECONDS" =~ ^[1-9][0-9]*$ ]] || fail 'LEDGERGUARD_DEPLOY_POLL_SECONDS must be a positive integer'

cd "$ROOT_DIR"
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || fail 'local worktree must be clean'
branch="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
[[ "$branch" == "main" ]] || fail "make deploy must run from main, got=${branch:-detached}"
HEAD_SHA="$(git rev-parse HEAD)"
[[ -n "$TARGET" ]] || TARGET="$HEAD_SHA"
[[ "$TARGET" =~ ^[0-9a-f]{40}$ ]] || fail 'target must be a full lowercase 40-character SHA'
[[ "$TARGET" == "$HEAD_SHA" ]] || fail "target must equal local HEAD: target=$TARGET head=$HEAD_SHA"
REMOTE_MAIN="$(git ls-remote origin refs/heads/main | awk '{print $1}')"
[[ "$REMOTE_MAIN" == "$TARGET" ]] || fail "origin/main mismatch: remote=$REMOTE_MAIN local=$TARGET"
REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
[[ -n "$REPO" ]] || fail 'cannot resolve GitHub repository'

printf '==> Waiting for CI for %s\n' "$TARGET"
deadline=$((SECONDS + WAIT_SECONDS))
run_id=""
last_notice=0
while (( SECONDS < deadline )); do
  run_id="$(gh run list --repo "$REPO" --workflow "CI" --branch main --commit "$TARGET" --limit 1 --json databaseId --jq '.[0].databaseId // empty' 2>/dev/null || true)"
  if [[ -n "$run_id" ]]; then
    printf 'CI_RUN=%s\n' "$run_id"
    gh run watch "$run_id" --repo "$REPO" --exit-status
    printf 'CI=PASS sha=%s\n' "$TARGET"
    break
  fi
  if (( SECONDS - last_notice >= 15 )); then
    printf '... still waiting for CI run for %s\n' "$TARGET"
    last_notice=$SECONDS
  fi
  sleep "$POLL_SECONDS"
done
[[ -n "$run_id" ]] || fail "timed out waiting for CI for $TARGET"

[[ "$(git rev-parse HEAD)" == "$TARGET" ]] || fail 'local HEAD moved while waiting for CI'
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || fail 'local worktree changed while waiting for CI'
REMOTE_MAIN="$(git ls-remote origin refs/heads/main | awk '{print $1}')"
[[ "$REMOTE_MAIN" == "$TARGET" ]] || fail "origin/main moved while waiting: remote=$REMOTE_MAIN target=$TARGET"

printf '==> Deploying exact validated SHA %s to %s\n' "$TARGET" "$REMOTE"
ssh -T "$REMOTE" bash -s -- "$REMOTE_DIR" "$TARGET" <<'REMOTE_DEPLOY'
set -Eeuo pipefail
root="$1"
target="$2"
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
for command in git bash; do command -v "$command" >/dev/null 2>&1 || fail "missing Home deploy command: $command"; done
[[ -d "$root/.git" ]] || fail "LedgerGuard checkout missing on Home: $root"
cd "$root"
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || fail 'Home LedgerGuard worktree must be clean'
branch="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
[[ "$branch" == "main" ]] || fail "Home LedgerGuard must be on main, got=${branch:-detached}"
git fetch --quiet origin main
fetched="$(git rev-parse FETCH_HEAD)"
[[ "$fetched" == "$target" ]] || fail "origin/main moved before Home deploy: fetched=$fetched target=$target"
git merge --ff-only "$target"
[[ "$(git rev-parse HEAD)" == "$target" ]] || fail 'Home LedgerGuard did not converge to exact target'
exec bash scripts/deploy-home.sh "$target"
REMOTE_DEPLOY

printf 'MAKE_DEPLOY=PASS repo=ledgerguard sha=%s host=%s exact=true\n' "$TARGET" "$REMOTE"
