# LedgerGuard

Polish freelance tax/ledger dashboard. Rust + Axum + Postgres + Docker.

## Deploy

```bash
bash scripts/deploy.sh          # from main, CI must pass
```

Blue-green zero-downtime via Caddy cutover. Deploys to `virya-home`.

### Critical: never break .env on the server

The `.env` file on `/srv/ledgerguard/.env` is gitignored and contains
secrets that are NOT in `.env.example`. A deploy (`git merge --ff-only`)
never touches it, but a manual server rebuild will silently drop them.

The deploy script now has guards:
- **Pre-deploy env check**: fails if `POSTGRES_PASSWORD`,
  `LEDGERGUARD_API_TOKEN`, `LEDGERGUARD_IMAP_USERNAME`, or
  `LEDGERGUARD_IMAP_PASSWORD` are missing or placeholder.
- **Post-deploy smoke test**: hits `/`, `/v1/system/status`,
  `/v1/ledger/month`, `/v1/ingest/documents` after Caddy cutover.
  If any fail, rollback reverts Caddy to blue.

### Production secrets (virya-home .env)

These must be present and non-empty:
- `POSTGRES_PASSWORD`
- `LEDGERGUARD_API_TOKEN` (production value differs from local)
- `LEDGERGUARD_IMAP_USERNAME=wojciech.jan.bator@gmail.com`
- `LEDGERGUARD_IMAP_PASSWORD` (Gmail app password)

`LEDGERGUARD_LIVE_SYNC_ENABLED=false` — accounting sync is disabled,
email ingest is the active path.

## Gates

```bash
cargo fmt --all
cargo clippy --locked --all-targets --all-features -D warnings
cargo test --locked --all-targets --all-features
```
