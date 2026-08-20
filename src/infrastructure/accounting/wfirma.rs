use async_trait::async_trait;

use crate::{
    application::{
        AccountingProvider, AccountingRecord, AccountingSource, AccountingSourceError,
        ProviderCapabilities, ProviderDescriptor,
    },
    config::WfirmaSettings,
    domain::Month,
};

#[derive(Debug, Clone)]
pub struct WfirmaAdapter {
    settings: WfirmaSettings,
}

impl WfirmaAdapter {
    #[must_use]
    pub const fn new(settings: WfirmaSettings) -> Self {
        Self { settings }
    }

    fn configured(&self) -> bool {
        self.settings.access_key.is_some()
            && self.settings.secret_key.is_some()
            && self.settings.app_key.is_some()
    }

    fn scope_configured(&self) -> bool {
        self.settings.company_id.is_some()
    }
}

#[async_trait]
impl AccountingSource for WfirmaAdapter {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            provider: AccountingProvider::Wfirma,
            display_name: "wFirma",
            configured: self.configured(),
            scope_configured: self.scope_configured(),
            read_only: true,
            sync_enabled: false,
            capabilities: ProviderCapabilities {
                revenues: true,
                expenses: true,
                payments: true,
                bank_transactions: false,
                taxes: true,
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
                provider: AccountingProvider::Wfirma,
                missing: "WFIRMA_ACCESS_KEY, WFIRMA_SECRET_KEY and/or WFIRMA_APP_KEY".to_owned(),
            });
        }
        if !self.scope_configured() {
            return Err(AccountingSourceError::ScopeNotConfigured {
                provider: AccountingProvider::Wfirma,
                missing: "WFIRMA_COMPANY_ID".to_owned(),
            });
        }

        Err(AccountingSourceError::NotEnabled {
            provider: AccountingProvider::Wfirma,
            reason: "API-key and OAuth2 auth are documented; read normalization awaits verified company scope and redacted fixtures".to_owned(),
        })
    }
}
