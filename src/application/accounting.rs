use std::{fmt, str::FromStr};

use async_trait::async_trait;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{EntryKind, Money, Month};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingProvider {
    #[default]
    Saldeo,
    Fakturownia,
    Infakt,
    Wfirma,
}

impl AccountingProvider {
    pub const ALL: [Self; 4] = [Self::Saldeo, Self::Fakturownia, Self::Infakt, Self::Wfirma];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Saldeo => "saldeo",
            Self::Fakturownia => "fakturownia",
            Self::Infakt => "infakt",
            Self::Wfirma => "wfirma",
        }
    }
}

impl fmt::Display for AccountingProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("unsupported accounting provider: {0}")]
pub struct ParseAccountingProviderError(String);

impl FromStr for AccountingProvider {
    type Err = ParseAccountingProviderError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "saldeo" | "saldeosmart" => Ok(Self::Saldeo),
            "fakturownia" | "invoiceocean" => Ok(Self::Fakturownia),
            "infakt" => Ok(Self::Infakt),
            "wfirma" | "w-firma" => Ok(Self::Wfirma),
            other => Err(ParseAccountingProviderError(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderCapabilities {
    pub revenues: bool,
    pub expenses: bool,
    pub payments: bool,
    pub bank_transactions: bool,
    pub taxes: bool,
    pub webhooks: bool,
}

impl ProviderCapabilities {
    #[must_use]
    pub const fn invoices_only() -> Self {
        Self {
            revenues: true,
            expenses: true,
            payments: false,
            bank_transactions: false,
            taxes: false,
            webhooks: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderDescriptor {
    pub provider: AccountingProvider,
    pub display_name: &'static str,
    /// Required credentials are present. This does not mean the credentials were verified live.
    pub configured: bool,
    /// A deterministic company/account scope has been configured where the provider requires one.
    pub scope_configured: bool,
    pub read_only: bool,
    /// True only after the provider's real account contract has been fixture-verified.
    pub sync_enabled: bool,
    pub capabilities: ProviderCapabilities,
}

/// Provider-neutral record returned by an accounting adapter.
///
/// Adapters do not choose internal UUIDs or provenance. The application layer
/// attaches those after validating the batch, keeping infrastructure details
/// out of the domain identity model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountingRecord {
    pub external_id: String,
    pub kind: EntryKind,
    pub booked_on: NaiveDate,
    pub gross: Money,
    pub net: Option<Money>,
    pub vat: Option<Money>,
    pub category: Option<String>,
    pub counterparty: Option<String>,
}

#[derive(Debug, Error)]
pub enum AccountingSourceError {
    #[error("{provider} is not configured; missing: {missing}")]
    NotConfigured {
        provider: AccountingProvider,
        missing: String,
    },
    #[error("{provider} company/account scope is not configured; missing: {missing}")]
    ScopeNotConfigured {
        provider: AccountingProvider,
        missing: String,
    },
    #[error("{provider} adapter operation is not enabled yet: {reason}")]
    NotEnabled {
        provider: AccountingProvider,
        reason: String,
    },
    #[error("{provider} request failed: {reason}")]
    Transport {
        provider: AccountingProvider,
        reason: String,
    },
    #[error("{provider} returned invalid data: {reason}")]
    InvalidData {
        provider: AccountingProvider,
        reason: String,
    },
}

#[async_trait]
pub trait AccountingSource: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn fetch_records(
        &self,
        month: Month,
    ) -> Result<Vec<AccountingRecord>, AccountingSourceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saldeo_is_the_default_provider() {
        assert_eq!(AccountingProvider::default(), AccountingProvider::Saldeo);
    }

    #[test]
    fn provider_aliases_are_stable() {
        assert_eq!("saldeosmart".parse(), Ok(AccountingProvider::Saldeo));
        assert_eq!("invoiceocean".parse(), Ok(AccountingProvider::Fakturownia));
        assert_eq!("w-firma".parse(), Ok(AccountingProvider::Wfirma));
    }
}
