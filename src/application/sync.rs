use std::collections::HashSet;

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use super::{
    AccountingProvider, AccountingRecord, AccountingSource, AccountingSourceError, LedgerRepository,
    RepositoryError,
};
use crate::domain::{LedgerEntry, Month, SourceSystem};

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
    let records = source.fetch_records(month).await?;
    validate_batch(month, &records)?;

    let provenance = SourceSystem::new(descriptor.provider.as_str())
        .expect("built-in accounting provider names are valid source slugs");
    let entries = records
        .into_iter()
        .map(|record| LedgerEntry {
            id: Uuid::new_v4(),
            external_id: record.external_id,
            kind: record.kind,
            booked_on: record.booked_on,
            gross: record.gross,
            net: record.net,
            vat: record.vat,
            category: record.category,
            counterparty: record.counterparty,
            source: provenance.clone(),
        })
        .collect::<Vec<_>>();

    repository.upsert_entries(&entries).await?;

    Ok(SyncReport {
        provider: descriptor.provider,
        month,
        imported: entries.len(),
    })
}

fn validate_batch(month: Month, records: &[AccountingRecord]) -> Result<(), SyncError> {
    let mut external_ids = HashSet::with_capacity(records.len());

    for record in records {
        let external_id = record.external_id.trim();
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
        if !month.contains(record.booked_on) {
            return Err(SyncError::InvalidBatch(format!(
                "record {} is outside requested month",
                record.external_id
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

    use super::*;
    use crate::{
        application::{ProviderCapabilities, ProviderDescriptor},
        domain::{EntryKind, Money},
    };

    struct FakeSource {
        records: Vec<AccountingRecord>,
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

        async fn fetch_records(
            &self,
            _month: Month,
        ) -> Result<Vec<AccountingRecord>, AccountingSourceError> {
            Ok(self.records.clone())
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

    fn record(external_id: &str, day: u32) -> AccountingRecord {
        AccountingRecord {
            external_id: external_id.to_owned(),
            kind: EntryKind::Expense,
            booked_on: NaiveDate::from_ymd_opt(2026, 8, day).unwrap(),
            gross: Money::non_negative(dec!(123.45)).unwrap(),
            net: None,
            vat: None,
            category: None,
            counterparty: None,
        }
    }

    #[tokio::test]
    async fn valid_batch_gets_application_owned_identity_and_provenance() {
        let source = FakeSource {
            records: vec![record("42", 20)],
        };
        let repository = CaptureRepository::default();
        let month = Month::new(2026, 8).unwrap();

        let report = sync_month(&source, &repository, month).await.unwrap();

        assert_eq!(report.imported, 1);
        let entries = repository.entries.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source.as_str(), "saldeo");
        assert_ne!(entries[0].id, Uuid::nil());
    }

    #[tokio::test]
    async fn duplicate_external_ids_are_rejected_before_persistence() {
        let source = FakeSource {
            records: vec![record("42", 20), record("42", 21)],
        };
        let repository = CaptureRepository::default();
        let month = Month::new(2026, 8).unwrap();

        let error = sync_month(&source, &repository, month).await.unwrap_err();

        assert!(error.to_string().contains("duplicate external_id"));
        assert!(repository.entries.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn out_of_period_records_are_rejected_before_persistence() {
        let source = FakeSource {
            records: vec![AccountingRecord {
                booked_on: NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
                ..record("42", 20)
            }],
        };
        let repository = CaptureRepository::default();
        let month = Month::new(2026, 8).unwrap();

        let error = sync_month(&source, &repository, month).await.unwrap_err();

        assert!(error.to_string().contains("outside requested month"));
        assert!(repository.entries.lock().unwrap().is_empty());
    }
}
