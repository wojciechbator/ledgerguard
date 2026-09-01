-- The cost budget is now derived from monthly_income at fixed ratios
-- (70% Healthy / 85% Blocked). The manual cost budget and tight-share
-- basis points columns are no longer used.
ALTER TABLE budget_settings
  DROP COLUMN IF EXISTS monthly_cost_budget,
  DROP COLUMN IF EXISTS tight_share_basis_points;
