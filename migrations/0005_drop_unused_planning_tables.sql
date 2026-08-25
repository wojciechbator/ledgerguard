-- These three tables were created in 0001 but never referenced by the
-- service: budgets live in BudgetSettings (env), and sync outcomes are
-- reported synchronously by the API instead of being persisted.
DROP TABLE IF EXISTS monthly_budgets;
DROP TABLE IF EXISTS cash_snapshots;
DROP TABLE IF EXISTS sync_runs;
