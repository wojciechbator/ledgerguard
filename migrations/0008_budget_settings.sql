-- Durable budget settings. Single-row table (CHECK id = 1) so the
-- operator's "Zapisz budżet" action survives restarts. Env vars
-- (LEDGERGUARD_MONTHLY_COST_BUDGET, LEDGERGUARD_MONTHLY_INCOME,
-- LEDGERGUARD_TIGHT_SHARE_BASIS_POINTS) remain as bootstrap defaults
-- when no row exists yet; once saved, DB values take priority.
CREATE TABLE budget_settings (
    id INT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    monthly_cost_budget NUMERIC(20, 2) CHECK (monthly_cost_budget IS NULL OR monthly_cost_budget > 0),
    monthly_income NUMERIC(20, 2) NOT NULL CHECK (monthly_income >= 0),
    tight_share_basis_points SMALLINT NOT NULL CHECK (tight_share_basis_points >= 0 AND tight_share_basis_points <= 5000),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
