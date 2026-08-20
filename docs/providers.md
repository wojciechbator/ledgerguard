# Accounting providers

LedgerGuard uses an application-layer `AccountingSource` port. Provider-specific APIs stay in infrastructure adapters and never leak into the planner domain.

## Supported adapter shells

| Provider | Selector | Auth model | Planned read surface | Default |
| --- | --- | --- | --- | --- |
| SaldeoSMART | `saldeo` | dedicated user + API token | revenue, cost documents, settlements/bank data when scope permits | yes |
| Fakturownia | `fakturownia` | account domain + API token | revenue/cost invoices, payments | no |
| inFakt | `infakt` | `X-inFakt-ApiKey` with read-only scopes | invoices, costs, accounting/tax data | no |
| wFirma | `wfirma` | API keys initially; OAuth2 is compatible with the same adapter boundary | invoices, expenses, payments/accounting data | no |

All adapters are deliberately read-only. LedgerGuard does not need invoice creation, mutation, payment marking or accounting writes.

## Safety contract

An adapter must not normalize live data until its real account contract has been verified with redacted fixtures. This avoids silently guessing vendor-specific money units, date semantics, correction handling or tenant/company scope.

Every adapter must satisfy these invariants before `fetch_entries` is enabled:

1. credentials are loaded only from deployment secrets/environment;
2. effective account/company scope is verified;
3. only read operations are used;
4. stable external IDs exist for idempotent upserts;
5. gross/net/VAT units and currency semantics are fixture-tested;
6. corrections/credit notes do not double-count;
7. pagination and period boundaries are deterministic;
8. provider errors are translated into `AccountingSourceError` without leaking secrets.

## Adding another provider

1. add a variant to `AccountingProvider` and aliases to `FromStr`;
2. add typed settings in `config.rs`;
3. implement `AccountingSource` under `src/infrastructure/accounting/<provider>.rs`;
4. register it in `build_accounting_source`;
5. add redacted contract fixtures and normalization tests;
6. update this matrix.

No planner/domain changes should be required for a normal accounting-provider integration.
