use async_trait::async_trait;

use crate::{
    application::{
        AccountingProvider, AccountingRecord, AccountingSource, AccountingSourceError,
        ProviderCapabilities, ProviderDescriptor,
    },
    config::InfaktSettings,
    domain::Month,
};

#[derive(Debug, Clone)]
pub struct InfaktAdapter {
    settings: InfaktSettings,
}

impl InfaktAdapter {
    #[must_use]
    pub const fn new(settings: InfaktSettings) -> Self {
        Self { settings }
    }

    fn configured(&self) -> bool {
        self.settings.api_key.is_some()
    }
}

#[async_trait]
impl AccountingSource for InfaktAdapter {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            provider: AccountingProvider::Infakt,
            display_name: "inFakt",
            configured: self.configured(),
            read_only: true,
            sync_enabled: false,
            capabilities: ProviderCapabilities {
                revenues: true,
                expenses: true,
                payments: false,
                bank_transactions: false,
                taxes: true,
                webhooks: true,
            },
        }
    }

    async fn fetch_records(
        &self,
        _month: Month,
    ) -> Result<Vec<AccountingRecord>, AccountingSourceError> {
        if !self.configured() {
            return Err(AccountingSourceError::NotConfigured {
                provider: AccountingProvider::Infakt,
                missing: "INFAKT_API_KEY".to_owned(),
            });
        }

        Err(AccountingSourceError::NotEnabled {
            provider: AccountingProvider::Infakt,
            reason: "API v3 read scopes are modeled; normalization awaits redacted invoice/cost fixtures".to_owned(),
        })
    }
}
