# Performance and footprint

LedgerGuard is intentionally optimized as a small, network-bound service rather than as a compute-heavy application.

## Runtime invariants

- Accounting imports are persisted in bounded multi-row PostgreSQL statements, never one round-trip per ledger entry.
- No-op conflict updates are skipped to avoid unnecessary WAL and `updated_at` churn.
- Monthly reads are backed by `(booked_on, id)` to satisfy both the date range and stable ordering.
- The write-heavy ledger table does not maintain speculative secondary indexes: the former category index is removed until a category-filtered read path actually exists.
- The embedded dashboard shell may use a short private browser cache, while API and health responses remain `no-store`.
- Money validation uses a precomputed `NUMERIC(20,2)` ceiling; it does not parse the same decimal limit per record.
- The HTTP healthcheck is implemented by the LedgerGuard binary itself so the runtime image does not need a shell, curl, or a package manager.

## Build and size policy

- Production uses Rust 1.97 with a committed `Cargo.lock` and locked builds.
- Release builds use thin LTO, one codegen unit, symbol stripping, abort-on-panic, and no debug info.
- `release-size` is available for footprint experiments and uses `opt-level = "z"` plus fat LTO; it is not the default because the standard release profile preserves maximum runtime performance.
- The container runtime is a non-root `scratch` image containing only the static executable, CA certificates, and SQL migrations.
- CI enforces explicit binary and container image size budgets so footprint regressions are visible immediately.

The project avoids allocator swaps and other benchmark-only tuning unless measurements show a real workload bottleneck. The dominant live path is expected to be provider I/O plus PostgreSQL persistence.
