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
}
