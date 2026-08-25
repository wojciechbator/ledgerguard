use async_trait::async_trait;
use thiserror::Error;

use crate::domain::{LedgerEntry, Month};

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
