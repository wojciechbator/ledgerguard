use std::collections::HashSet;

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use super::{
    AccountingProvider, AccountingRecord, AccountingSource, AccountingSourceError,
    LedgerRepository, RepositoryError,
};
use crate::domain::{LedgerEntry, Month, SourceSystem};

const MAX_EXTERNAL_ID_BYTES: usize = 256;
const MAX_CATEGORY_BYTES: usize = 256;
const MAX_COUNTERPARTY_BYTES: usize = 512;
const MAX_BATCH_ENTRIES: usize = 10_000;

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
    #[error("{0} credentials are not configured")]
    ProviderNotConfigured(AccountingProvider),
    #[error("{0} deterministic company/account scope is not configured")]
    ProviderScopeNotConfigured(AccountingProvider),
    #[error("{0} adapter is not read-only; synchronization refused")]
    WritableProviderRefused(AccountingProvider),
    #[error("{0} live normalization contract is not fixture-verified")]
    ProviderNotVerified(AccountingProvider),
    #[error("accounting source returned an invalid batch: {0}")]
    InvalidBatch(String),
}

pub async fn sync_month(
    source: &dyn AccountingSource,
    repository: &dyn LedgerRepository,
    month: Month,
) -> Result<SyncReport, SyncError> {
    let descriptor = source.descriptor();
    validate_provider_gate(descriptor)?;

    let records = source.fetch_records(month).await?;
    validate_batch(month, &records)?;

    let provenance = SourceSystem::new(descriptor.provider.as_str())
        .expect("built-in accounting provider names are valid source slugs");
    let entries = records
        .into_iter()
        .map(|record| LedgerEntry {
            id: Uuid::new_v4(),
            external_id: record.external_id.trim().to_owned(),
            kind: record.kind,
            booked_on: record.booked_on,
            gross: record.gross,
            net: record.net,
            vat: record.vat,
            category: normalize_optional_text(record.category),
            counterparty: normalize_optional_text(record.counterparty),
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

fn validate_provider_gate(descriptor: super::ProviderDescriptor) -> Result<(), SyncError> {
    if !descriptor.configured {
        return Err(SyncError::ProviderNotConfigured(descriptor.provider));
    }
    if !descriptor.scope_configured {
        return Err(SyncError::ProviderScopeNotConfigured(descriptor.provider));
    }
    if !descriptor.read_only {
        return Err(SyncError::WritableProviderRefused(descriptor.provider));
    }
    if !descriptor.sync_enabled {
        return Err(SyncError::ProviderNotVerified(descriptor.provider));
    }
    Ok(())
}

fn validate_batch(month: Month, records: &[AccountingRecord]) -> Result<(), SyncError> {
    if records.len() > MAX_BATCH_ENTRIES {
        return Err(SyncError::InvalidBatch(format!(
            "batch exceeds {MAX_BATCH_ENTRIES} records"
        )));
    }

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
        validate_optional_text("category", record.category.as_deref(), MAX_CATEGORY_BYTES)?;
        validate_optional_text(
            "counterparty",
            record.counterparty.as_deref(),
            MAX_COUNTERPARTY_BYTES,
        )?;
    }

    Ok(())
}

fn validate_optional_text(
    field: &'static str,
    value: Option<&str>,
    max_bytes: usize,
) -> Result<(), SyncError> {
    if value.is_some_and(|value| value.trim().len() > max_bytes) {
        return Err(SyncError::InvalidBatch(format!(
            "{field} exceeds {max_bytes} bytes"
        )));
    }
    Ok(())
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

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
        descriptor: ProviderDescriptor,
        fetched: AtomicBool,
    }

    impl FakeSource {
        fn verified(records: Vec<AccountingRecord>) -> Self {
            Self {
                records,
                descriptor: ProviderDescriptor {
                    provider: AccountingProvider::Saldeo,
                    display_name: "fake Saldeo",
                    configured: true,
                    scope_configured: true,
                    read_only: true,
                    sync_enabled: true,
                    capabilities: ProviderCapabilities::invoices_only(),
                },
                fetched: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl AccountingSource for FakeSource {
        fn descriptor(&self) -> ProviderDescriptor {
            self.descriptor
        }

        async fn fetch_records(
            &self,
            _month: Month,
        ) -> Result<Vec<AccountingRecord>, AccountingSourceError> {
            self.fetched.store(true, Ordering::SeqCst);
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
        let source = FakeSource::verified(vec![record(" 42 ", 20)]);
        let repository = CaptureRepository::default();
        let month = Month::new(2026, 8).unwrap();

        let report = sync_month(&source, &repository, month).await.unwrap();

        assert_eq!(report.imported, 1);
        let entries = repository.entries.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].external_id, "42");
        assert_eq!(entries[0].source.as_str(), "saldeo");
        assert_ne!(entries[0].id, Uuid::nil());
    }

    #[tokio::test]
    async fn unverified_adapter_is_rejected_before_fetching_any_data() {
        let mut source = FakeSource::verified(vec![record("42", 20)]);
        source.descriptor.sync_enabled = false;
        let repository = CaptureRepository::default();

        let error = sync_month(&source, &repository, Month::new(2026, 8).unwrap())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            SyncError::ProviderNotVerified(AccountingProvider::Saldeo)
        ));
        assert!(!source.fetched.load(Ordering::SeqCst));
        assert!(repository.entries.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn writable_adapter_is_rejected_before_fetching_any_data() {
        let mut source = FakeSource::verified(vec![record("42", 20)]);
        source.descriptor.read_only = false;
        let repository = CaptureRepository::default();

        let error = sync_month(&source, &repository, Month::new(2026, 8).unwrap())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            SyncError::WritableProviderRefused(AccountingProvider::Saldeo)
        ));
        assert!(!source.fetched.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn duplicate_external_ids_are_rejected_after_canonicalization() {
        let source = FakeSource::verified(vec![record("42", 20), record(" 42 ", 21)]);
        let repository = CaptureRepository::default();
        let month = Month::new(2026, 8).unwrap();

        let error = sync_month(&source, &repository, month).await.unwrap_err();

        assert!(error.to_string().contains("duplicate external_id"));
        assert!(repository.entries.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn out_of_period_records_are_rejected_before_persistence() {
        let source = FakeSource::verified(vec![AccountingRecord {
            booked_on: NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
            ..record("42", 20)
        }]);
        let repository = CaptureRepository::default();
        let month = Month::new(2026, 8).unwrap();

        let error = sync_month(&source, &repository, month).await.unwrap_err();

        assert!(error.to_string().contains("outside requested month"));
        assert!(repository.entries.lock().unwrap().is_empty());
    }
}
