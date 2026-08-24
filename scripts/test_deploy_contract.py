from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
JUSTFILE = (ROOT / "justfile").read_text()
WAITER = ROOT / "scripts/deploy.sh"
HOME = ROOT / "scripts/deploy-home.sh"
WAITER_TEXT = WAITER.read_text()
HOME_TEXT = HOME.read_text()

subprocess.run(["bash", "-n", str(WAITER)], check=True)
subprocess.run(["bash", "-n", str(HOME)], check=True)

assert "deploy:\n    bash scripts/deploy.sh" in JUSTFILE
assert "check: fmt lint panics deploy-contract test" in JUSTFILE
assert '--workflow "CI"' in WAITER_TEXT
assert "origin/main mismatch" in WAITER_TEXT
assert "still waiting for CI run" in WAITER_TEXT
assert "Deploying exact validated SHA" in WAITER_TEXT
assert "virya-home" in WAITER_TEXT
assert "/srv/ledgerguard" in WAITER_TEXT
assert 'fetched="$(git rev-parse FETCH_HEAD)"' in WAITER_TEXT
assert '[[ "$fetched" == "$target" ]]' in WAITER_TEXT
assert 'exec bash scripts/deploy-home.sh "$target"' in WAITER_TEXT
assert 'TARGET="${1:-}"' in HOME_TEXT
assert 'git fetch --quiet origin main' in HOME_TEXT
assert '[[ "${fetched}" == "${TARGET}" ]]' in HOME_TEXT
assert 'git merge --ff-only "${TARGET}"' in HOME_TEXT
assert "LEDGERGUARD_DEPLOY=PASS" in HOME_TEXT

print("LEDGERGUARD_DEPLOY_CONTRACT=PASS ci=exact visible-wait=true home=exact")
