# Security policy

LedgerGuard processes financial metadata and is intended to run as a private service.

## Supported version

Only the current `main` branch is supported during pre-release development.

## Reporting a vulnerability

Do not open a public issue containing credentials, financial exports, API responses with personal data, or exploitable deployment details. Use GitHub's private vulnerability reporting when enabled, or contact the repository owner privately.

## Non-negotiable controls

- provider integrations are read-only;
- accounting credentials are deployment secrets and must never be committed or persisted in application tables;
- live provider normalization remains fail-closed until account scope and redacted fixtures are verified;
- HTTP is loopback/private-network by default;
- production exposure requires authentication at the reverse-proxy/private-network boundary;
- CI actions are pinned to immutable revisions;
- normalized imports must preserve source provenance and idempotency.
