# LedgerGuard

**Cash flow & cost planning service for small businesses.**

LedgerGuard answers one practical question: **how much can I safely spend right now?** It combines current cash, committed costs, tax/VAT/ZUS reserves, a minimum cash buffer and planned purchases into a deterministic planning result.

The service is intentionally a small Rust modular monolith. Accounting data stays source-of-truth in SaldeoSMART/accounting; LedgerGuard stores normalized read models and planning policy.

## Status

Bootstrap v0.1. The domain engine, PostgreSQL persistence boundary, HTTP API, migrations, Docker deployment and CI are present. The SaldeoSMART adapter is deliberately fail-closed until a dedicated, company-scoped API user/token is issued and its effective permissions are verified.

## Architecture

```text
SaldeoSMART (read-only)
        |
        v
 application ports
        |
        v
 normalized ledger ---- PostgreSQL
        |
        v
  planner domain
        |
        +---- /v1/planner/evaluate
        +---- /v1/planner/simulate
        +---- alerts/dashboard (next)
```

Boundaries:

- `src/domain` — pure business rules, money, periods and ledger concepts;
- `src/application` — ports/use-case boundaries;
- `src/infrastructure` — PostgreSQL and Saldeo adapters;
- `src/api` — Axum transport only.

See [`docs/architecture.md`](docs/architecture.md) for invariants and the Saldeo integration checklist.

## Run locally

```bash
cp .env.example .env
docker compose up --build
```

The app binds only to loopback in Compose by default:

```text
http://127.0.0.1:8088/healthz
http://127.0.0.1:8088/readyz
```

## Planner API

Evaluate the current position:

```bash
curl -s http://127.0.0.1:8088/v1/planner/evaluate \
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

Simulate an additional purchase by POSTing the same body to `/v1/planner/simulate` with:

```json
{ "purchase_gross": "8900" }
```

Money is serialized as decimal strings to avoid binary floating-point in financial calculations.

## Development gates

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
```

## Deployment model

The intended first deployment is a private home-server service behind the existing reverse proxy/Tailscale boundary. The Compose port is loopback-only; do not expose financial endpoints directly to the public Internet.

## Security

- never commit Saldeo credentials or financial exports;
- use a dedicated Saldeo API user restricted to the intended company;
- keep the integration read-only;
- verify effective API scope before enabling sync;
- secrets live in deployment environment/secrets, not in PostgreSQL rows.
