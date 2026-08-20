use async_trait::async_trait;

use crate::{
    application::{
        AccountingProvider, AccountingRecord, AccountingSource, AccountingSourceError,
        ProviderCapabilities, ProviderDescriptor,
    },
    config::SaldeoSettings,
    domain::Month,
};

#[derive(Debug, Clone)]
pub struct SaldeoAdapter {
    settings: SaldeoSettings,
}

impl SaldeoAdapter {
    #[must_use]
    pub const fn new(settings: SaldeoSettings) -> Self {
        Self { settings }
    }

    fn configured(&self) -> bool {
        self.settings.username.is_some() && self.settings.api_token.is_some()
    }
}

#[async_trait]
impl AccountingSource for SaldeoAdapter {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            provider: AccountingProvider::Saldeo,
            display_name: "SaldeoSMART",
            configured: self.configured(),
            read_only: true,
            sync_enabled: false,
            capabilities: ProviderCapabilities {
                revenues: true,
                expenses: true,
                payments: true,
                bank_transactions: true,
                taxes: false,
                webhooks: false,
            },
        }
    }

    async fn fetch_records(
        &self,
        _month: Month,
    ) -> Result<Vec<AccountingRecord>, AccountingSourceError> {
        if !self.configured() {
            return Err(AccountingSourceError::NotConfigured {
                provider: AccountingProvider::Saldeo,
                missing: "SALDEO_USERNAME and/or SALDEO_API_TOKEN".to_owned(),
            });
        }

        Err(AccountingSourceError::NotEnabled {
            provider: AccountingProvider::Saldeo,
            reason: "live XML command mapping is gated until the dedicated company-scoped API user is verified".to_owned(),
        })
    }
}
