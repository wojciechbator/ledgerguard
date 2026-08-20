use async_trait::async_trait;

use crate::{
    application::{
        AccountingProvider, AccountingRecord, AccountingSource, AccountingSourceError,
        ProviderCapabilities, ProviderDescriptor,
    },
    config::FakturowniaSettings,
    domain::Month,
};

#[derive(Debug, Clone)]
pub struct FakturowniaAdapter {
    settings: FakturowniaSettings,
}

impl FakturowniaAdapter {
    #[must_use]
    pub const fn new(settings: FakturowniaSettings) -> Self {
        Self { settings }
    }

    fn configured(&self) -> bool {
        self.settings.account_domain.is_some() && self.settings.api_token.is_some()
    }

    fn scope_configured(&self) -> bool {
        self.settings.department_id.is_some()
    }
}

#[async_trait]
impl AccountingSource for FakturowniaAdapter {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            provider: AccountingProvider::Fakturownia,
            display_name: "Fakturownia",
            configured: self.configured(),
            scope_configured: self.scope_configured(),
            read_only: true,
            sync_enabled: false,
            capabilities: ProviderCapabilities {
                revenues: true,
                expenses: true,
                payments: true,
                bank_transactions: false,
                taxes: false,
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
                provider: AccountingProvider::Fakturownia,
                missing: "FAKTUROWNIA_ACCOUNT_DOMAIN and/or FAKTUROWNIA_API_TOKEN".to_owned(),
            });
        }
        if !self.scope_configured() {
            return Err(AccountingSourceError::ScopeNotConfigured {
                provider: AccountingProvider::Fakturownia,
                missing: "FAKTUROWNIA_DEPARTMENT_ID".to_owned(),
            });
        }

        Err(AccountingSourceError::NotEnabled {
            provider: AccountingProvider::Fakturownia,
            reason: "read transport and pagination await redacted account fixtures before normalization is enabled"
                .to_owned(),
        })
    }
}
