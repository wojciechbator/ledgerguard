# Architecture

LedgerGuard is a small modular monolith. DDD boundaries exist to protect financial rules, not to manufacture services.

## Boundaries

- `domain` — money, accounting periods, ledger entries, spending policy and planner rules. No I/O.
- `application` — ports used by use-cases. The domain never imports SQL, HTTP or Saldeo types.
- `infrastructure` — PostgreSQL and external accounting adapters.
- `api` — HTTP transport and DTO boundary.

## Source of truth

SaldeoSMART and the accountant remain the accounting source of truth. LedgerGuard stores normalized read models and produces planning estimates. It must never silently turn an estimate into an accounting fact.

## Safe-to-spend invariant

`available_cash` is a current cash snapshot. Historical expenses already reflected in that balance are not deducted again.

The planner subtracts only forward-looking obligations and explicit reserves:

```text
available cash
- committed costs
- tax reserve
- VAT reserve
- ZUS reserve
- minimum cash buffer
- planned spend
= headroom
```

Negative headroom is preserved in the result for diagnostics; `safe_to_spend` saturates at zero.

## Saldeo integration

The Saldeo adapter is intentionally fail-closed until a dedicated API user/token exists. The first live integration pass must verify:

1. the token can see only the intended company,
2. the exact read commands exposed to that user,
3. which document-list policy avoids mutating the accountant's export workflow,
4. stable external identifiers for idempotent upserts,
5. bank-statement visibility and semantics.

No Saldeo credential belongs in Git or the database.
