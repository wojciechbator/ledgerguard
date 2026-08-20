# Architecture

LedgerGuard is a small modular monolith. DDD boundaries exist to protect financial rules and integration contracts, not to manufacture services.

## Boundaries

- `domain` — money, accounting periods, ledger entries, provider-agnostic provenance and planner rules. No I/O and no vendor API types.
- `application` — accounting/repository ports plus orchestration such as validated monthly sync.
- `infrastructure` — PostgreSQL and vendor-specific accounting adapters.
- `api` — HTTP transport and DTO boundary.

The dependency direction is inward: infrastructure and transport know the application/domain; the domain does not know Axum, SQLx, SaldeoSMART, Fakturownia, inFakt or wFirma.

## Source of truth

The selected accounting platform and the accountant remain the accounting source of truth. LedgerGuard stores normalized read models and produces planning estimates. It must never silently turn an estimate into an accounting fact.

## Provider boundary

`AccountingSource` is the anti-corruption layer between vendor APIs and the normalized ledger. Provider selection is an application/infrastructure concern. The planner does not branch on vendor names.

The default provider is SaldeoSMART. Fakturownia, inFakt and wFirma use the same port. A new adapter should require provider registration, typed configuration, an infrastructure implementation and fixture tests — not a new planner/domain branch.

Provider provenance is stored as a validated lowercase slug (`SourceSystem`) rather than a closed enum. This prevents a new vendor from forcing a domain/database schema redesign. Adapters return provider-neutral `AccountingRecord` values; the application layer owns internal UUIDs and attaches the selected provider as provenance, so an adapter cannot spoof another source.

## Sync invariants

A fetched batch is rejected before persistence when:

1. an external ID is empty or unreasonably large;
2. the same external ID occurs twice in one provider batch;
3. a record falls outside the requested month.

After validation, the application assigns internal identity and provider provenance. Persistence is idempotent on `(source, external_id)` so provider corrections update normalized data instead of duplicating it.

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

Negative headroom is preserved for diagnostics; `safe_to_spend` saturates at zero.

`Money` rejects negative values at construction and during deserialization. The HTTP contract accepts monetary values only as decimal strings and returns decimal strings, avoiding binary floating-point ambiguity at the transport boundary.

## SaldeoSMART first-live contract

The Saldeo adapter remains fail-closed until a dedicated API user/token exists. The first live integration pass must verify:

1. the token can see only the intended company;
2. the exact read commands exposed to that user;
3. which document-list policy does not unexpectedly mutate the accountant's export workflow;
4. stable external identifiers for idempotent upserts;
5. gross/net/VAT units and correction semantics from real redacted fixtures;
6. bank-statement visibility and semantics.

No provider credential belongs in Git or PostgreSQL.

## Runtime boundary

The first deployment target is a private home server. A direct binary binds to loopback by default. Compose binds the container internally to all interfaces but publishes HTTP to host loopback only; the app container runs non-root with a read-only root filesystem, drops Linux capabilities and enables `no-new-privileges`. Public exposure should happen only behind an explicit authenticated reverse-proxy/Tailscale boundary.
