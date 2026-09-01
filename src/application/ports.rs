use async_trait::async_trait;
use rust_decimal::Decimal;
use thiserror::Error;

use crate::{
    config::BudgetSettings,
    domain::{LedgerEntry, Money, Month},
};

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("repository operation failed: {0}")]
    Storage(String),
}

#[async_trait]
pub trait LedgerRepository: Send + Sync {
    async fn upsert_entries(&self, entries: &[LedgerEntry]) -> Result<(), RepositoryError>;
    async fn entries_for_month(&self, month: Month) -> Result<Vec<LedgerEntry>, RepositoryError>;

    /// The `limit` most recent entries of `month`, newest first, for dashboard
    /// previews. Ordered in SQL so the caller never materializes the whole
    /// month just to show ten rows.
    async fn recent_entries_for_month(
        &self,
        month: Month,
        limit: i64,
    ) -> Result<Vec<LedgerEntry>, RepositoryError>;
}

/// Converts a `Decimal` to a non-negative `Money`, returning a storage
/// error on failure. Shared by the postgres implementation and tests.
fn decimal_to_money(value: Decimal) -> Result<Money, RepositoryError> {
    Money::non_negative(value).map_err(|err| RepositoryError::Storage(err.to_string()))
}

/// Parses a `budget_settings` row into `BudgetSettings`. Shared by the
/// postgres implementation and tests.
pub fn parse_budget_row(
    monthly_cost_budget: Option<rust_decimal::Decimal>,
    monthly_income: rust_decimal::Decimal,
    tight_share_basis_points: i16,
) -> Result<BudgetSettings, RepositoryError> {
    Ok(BudgetSettings {
        monthly_cost_budget: monthly_cost_budget.map(decimal_to_money).transpose()?,
        monthly_income: decimal_to_money(monthly_income)?,
        tight_share_basis_points: u16::try_from(tight_share_basis_points)
            .map_err(|err| RepositoryError::Storage(err.to_string()))?,
    })
}
