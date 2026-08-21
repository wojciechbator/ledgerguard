# LedgerGuard

**Rust / Axum / PostgreSQL cash-planning service for small businesses.**

LedgerGuard answers one practical question: **how much can I safely spend right now?** It combines current cash, committed costs, tax/VAT/ZUS reserves, a minimum cash buffer and planned purchases into a deterministic planning result.

Accounting platforms remain the source of truth. LedgerGuard stores a normalized read model plus planning policy; it never silently turns a forecast into an accounting fact.

## Engineering snapshot

- **DDD boundary:** planner rules are pure domain code; accounting providers sit behind a provider-neutral `AccountingSource` port.
- **Provider isolation:** SaldeoSMART, Fakturownia/InvoiceOcean, inFakt and wFirma adapters map into neutral records instead of leaking vendor DTOs into the domain.
- **Money correctness:** decimal strings end-to-end, no binary floating point, checked scale/range rules and negative-value rejection at deserialization boundaries.
- **Idempotent sync:** `(source, external_id)` is the persistence identity, so corrected provider data updates the normalized record rather than duplicating it.
- **Fail-closed integration:** provider imports reject duplicates, invalid months, oversized batches and unverified live-account assumptions.
- **Production-minded small deploy:** loopback by default, non-root/read-only container, dropped capabilities, PostgreSQL contract tests and container smoke in CI.

The interesting part is not the dashboard. It is keeping **accounting truth, imported evidence and planning policy as three different concepts** so a convenience tool cannot quietly become a second accounting system.

## Status

Pre-API hardening. The planner domain, provider-neutral accounting port, PostgreSQL persistence, sync validation, HTTP API, migrations, container deployment, minimal dashboard and CI gates are present.

Four accounting adapter boundaries are wired:

- **SaldeoSMART** — default;
- **Fakturownia / InvoiceOcean**;
- **inFakt**;
- **wFirma**.

All provider adapters are read-only and fail closed until their real account contract is verified with redacted fixtures. SaldeoSMART remains the intended first live integration.

## Architecture

```text
             accounting providers
  Saldeo | Fakturownia | inFakt | wFirma
                    |
                    v
           AccountingSource port
                    |
                    v
          sync validation/use-case
                    |
                    v
 normalized ledger -------- PostgreSQL
                    |
                    v
              planner domain
                    |
        +-----------+-----------+
        |                       |
 /v1/planner/evaluate   /v1/planner/simulate
                    |
                    v
          embedded dashboard
```

Boundaries:

- `src/domain` — pure financial rules, money, periods, ledger concepts and provenance invariants;
- `src/application` — provider-neutral ports and orchestration/use-cases;
- `src/infrastructure` — PostgreSQL plus provider-specific adapters/factory;
- `src/api` — Axum transport, DTO boundary and a dependency-free embedded dashboard.

Adding a normal accounting provider does not require changing planner rules. See [`docs/providers.md`](docs/providers.md) and [`docs/architecture.md`](docs/architecture.md).

## Provider selection

SaldeoSMART is selected when `LEDGERGUARD_ACCOUNTING_PROVIDER` is omitted.

```text
LEDGERGUARD_ACCOUNTING_PROVIDER=saldeo
```

Supported values are `saldeo`, `fakturownia`, `infakt` and `wfirma`. Provider credentials are read only from environment/deployment secrets and are redacted from `Debug` output.

The active adapter can be inspected without exposing credentials:

```text
GET /v1/accounting/provider
```

It reports the provider, whether required credentials are present, read-only mode and declared capabilities.

## Home-server deploy

The canonical small deployment is Docker Compose on a private host. The deployment helper is idempotent, generates local secrets on first run, keeps Saldeo disabled until credentials are verified, makes a compressed PostgreSQL backup before upgrades when the database is already running, builds the current checkout and waits for the container health check.

```bash
git clone https://github.com/wojciechbator/ledgerguard.git
cd ledgerguard
./scripts/deploy-home.sh
```

On the first run it creates a mode-`0600` `.env` with random PostgreSQL and LedgerGuard API tokens. **Do not commit that file.**

The service remains loopback-only by default:

```text
Dashboard: http://127.0.0.1:8088/
Health:    http://127.0.0.1:8088/healthz
Ready:     http://127.0.0.1:8088/readyz
```

For remote use, expose it only through a private Tailscale/reverse-proxy boundary or an SSH tunnel. The dashboard is embedded in the Rust binary — there is no Node/Vite/React build and no CDN or external JavaScript dependency.

When Saldeo credentials arrive, put them in `.env` on the host only after company scope is verified, then explicitly enable `LEDGERGUARD_LIVE_SYNC_ENABLED=true`.

## Run locally

```bash
cp .env.example .env
# replace POSTGRES_PASSWORD before using the stack
docker compose up --build
```

Compose binds the HTTP service to loopback only. A directly-run binary also binds to loopback unless `LEDGERGUARD_BIND_ADDR` is explicitly overridden. The application container runs non-root, read-only, with all Linux capabilities dropped and `no-new-privileges` enabled.

## Planner API

Evaluate the current position:

```bash
curl -s http://127.0.0.1:8088/v1/planner/evaluate \
  -H 'authorization: Bearer YOUR_LOCAL_TOKEN' \
  -H 'content-type: application/json' \
  -d '{
    "input": {
      "available_cash": "30000",
      "committed_costs": "2000",
      "tax_reserve": "4000",
      "vat_reserve": "3000",
      "zus_reserve": "2000",
      "minimum_cash_buffer": "10000",
      "planned_spend": "1000"
    },
    "policy": { "tight_threshold": "2000" }
  }'
```

Simulate a purchase by adding `purchase_gross` to the same request shape and POSTing it to `/v1/planner/simulate`.

Money is represented with decimal arithmetic and transported as decimal strings, never binary floating point. Negative money values cannot be introduced through JSON deserialization.

## Accounting sync contract

Provider adapters return neutral accounting records rather than internal ledger entities. Before persistence, every provider batch is checked for bounded non-empty external IDs, duplicate IDs and strict requested-month boundaries. The application then assigns internal identity and trusted provider provenance.

Persistence uses `(source, external_id)` as the idempotency key. A corrected provider record updates the normalized row instead of creating a duplicate.

## Development gates

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked --release
```

CI additionally runs the real PostgreSQL repository contract against PostgreSQL 16, validates Compose and builds the runtime image. Third-party GitHub Actions are pinned to immutable commit SHAs.

## Deployment model

The intended first deployment is a private home-server service behind an authenticated reverse-proxy/Tailscale boundary. Do not expose financial endpoints directly to the public Internet.

## Security rules

- never commit provider credentials or financial exports;
- use the smallest available read-only scope;
- verify effective company/account scope before enabling live sync;
- never log or persist API secrets;
- do not guess provider-specific units, correction semantics or pagination behavior — verify them with redacted fixtures first;
- accounting systems and the accountant remain source of truth.
