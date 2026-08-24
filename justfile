# LedgerGuard — command runner. `just` alone lists recipes.
default:
    @just --list

fmt:
    cargo fmt --all -- --check

lint:
    cargo clippy --locked --all-targets --all-features -- -D warnings

test:
    cargo test --locked --all-targets --all-features

# A financial service must not die on an unwrap; runtime code is panic-free.
panics:
    python3 scripts/check_runtime_panics.py

# Deployment contract gate (pins compose + deploy script invariants).
deploy-contract:
    python3 scripts/test_deploy_contract.py

# Everything CI runs.
check: fmt lint panics deploy-contract test

# Ship a validated SHA to virya-home (CI-gated).
deploy:
    bash scripts/deploy.sh

# Local convenience alias.
deploy-home:
    bash scripts/deploy-home.sh
