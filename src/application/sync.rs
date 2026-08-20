use std::collections::HashSet;

use serde::Serialize;
use thiserror::Error;

use super::{
    AccountingProvider, AccountingSource, AccountingSourceError, LedgerRepository, RepositoryError,
};
use crate::domain::{LedgerEntry, Month};

const MAX_EXTERNAL_ID_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SyncReport {
    pub provider: AccountingProvider,
    pub month: Month,
    pub imported: usize,
}

#[derive(Debug, Error)]
pub enum SyncError {
    #[error(transparent)]
    Source(#[from] AccountingSourceError),
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error("accounting source returned an invalid batch: {0}")]
    InvalidBatch(String),
}

pub async fn sync_month(
    source: &dyn AccountingSource,
    repository: &dyn LedgerRepository,
    month: Month,
) -> Result<SyncReport, SyncError> {
    let descriptor = source.descriptor();
    let entries = source.fetch_entries(month).await?;
    validate_batch(descriptor.provider, month, &entries)?;
    repository.upsert_entries(&entries).await?;

    Ok(SyncReport {
        provider: descriptor.provider,
        month,
        imported: entries.len(),
    })
}

fn validate_batch(
    provider: AccountingProvider,
    month: Month,
    entries: &[LedgerEntry],
) -> Result<(), SyncError> {
    let mut external_ids = HashSet::with_capacity(entries.len());

    for entry in entries {
        let external_id = entry.external_id.trim();
        if external_id.is_empty() {
            return Err(SyncError::InvalidBatch(
                "external_id must not be empty".to_owned(),
            ));
        }
        if external_id.len() > MAX_EXTERNAL_ID_BYTES {
            return Err(SyncError::InvalidBatch(format!(
                "external_id exceeds {MAX_EXTERNAL_ID_BYTES} bytes"
            )));
        }
        if !external_ids.insert(external_id) {
            return Err(SyncError::InvalidBatch(format!(
                "duplicate external_id in one batch: {external_id}"
            )));
        }
        if !month.contains(entry.booked_on) {
            return Err(SyncError::InvalidBatch(format!(
                "entry {} is outside requested month",
                entry.external_id
            )));
        }
        if entry.source.as_str() != provider.as_str() {
            return Err(SyncError::InvalidBatch(format!(
                "entry {} claims source {}, expected {}",
                entry.external_id, entry.source, provider
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::NaiveDate;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use super::*;
    use crate::{
        application::{ProviderCapabilities, ProviderDescriptor},
        domain::{EntryKind, Money, SourceSystem},
    };

    struct FakeSource {
        entries: Vec<LedgerEntry>,
    }

    #[async_trait]
    impl AccountingSource for FakeSource {
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                provider: AccountingProvider::Saldeo,
                display_name: "fake Saldeo",
                configured: true,
                read_only: true,
                capabilities: ProviderCapabilities::invoices_only(),
            }
        }

        async fn fetch_entries(
            &self,
            _month: Month,
        ) -> Result<Vec<LedgerEntry>, AccountingSourceError> {
            Ok(self.entries.clone())
        }
    }

    #[derive(Default)]
    struct CaptureRepository {
        entries: Mutex<Vec<LedgerEntry>>,
    }

    #[async_trait]
    impl LedgerRepository for CaptureRepository {
        async fn upsert_entries(&self, entries: &[LedgerEntry]) -> Result<(), RepositoryError> {
            self.entries.lock().unwrap().extend_from_slice(entries);
            Ok(())
        }

        async fn entries_for_month(
            &self,
            _month: Month,
        ) -> Result<Vec<LedgerEntry>, RepositoryError> {
            Ok(self.entries.lock().unwrap().clone())
        }
    }

    fn entry(external_id: &str, source: &str, day: u32) -> LedgerEntry {
        LedgerEntry {
            id: Uuid::new_v4(),
            external_id: external_id.to_owned(),
            kind: EntryKind::Expense,
            booked_on: NaiveDate::from_ymd_opt(2026, 8, day).unwrap(),
            gross: Money::non_negative(dec!(123.45)).unwrap(),
            net: None,
            vat: None,
            category: None,
            counterparty: None,
            source: SourceSystem::new(source).unwrap(),
        }
    }

    #[tokio::test]
    async fn valid_batch_is_persisted_once() {
        let source = FakeSource {
            entries: vec![entry("42", "saldeo", 20)],
        };
        let repository = CaptureRepository::default();
        let month = Month::new(2026, 8).unwrap();

        let report = sync_month(&source, &repository, month).await.unwrap();

        assert_eq!(report.imported, 1);
        assert_eq!(repository.entries.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn provider_spoofing_is_rejected_before_persistence() {
        let source = FakeSource {
            entries: vec![entry("42", "infakt", 20)],
        };
        let repository = CaptureRepository::default();
        let month = Month::new(2026, 8).unwrap();

        let error = sync_month(&source, &repository, month).await.unwrap_err();

        assert!(error.to_string().contains("claims source infakt, expected saldeo"));
        assert!(repository.entries.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn duplicate_external_ids_are_rejected_before_persistence() {
        let source = FakeSource {
            entries: vec![entry("42", "saldeo", 20), entry("42", "saldeo", 21)],
        };
        let repository = CaptureRepository::default();
        let month = Month::new(2026, 8).unwrap();

        let error = sync_month(&source, &repository, month).await.unwrap_err();

        assert!(error.to_string().contains("duplicate external_id"));
        assert!(repository.entries.lock().unwrap().is_empty());
    }
}
