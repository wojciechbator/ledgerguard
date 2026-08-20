use async_trait::async_trait;

use crate::{
    application::{
        AccountingProvider, AccountingSource, AccountingSourceError, ProviderCapabilities,
        ProviderDescriptor,
    },
    config::WfirmaSettings,
    domain::{LedgerEntry, Month},
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
}

#[async_trait]
impl AccountingSource for WfirmaAdapter {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            provider: AccountingProvider::Wfirma,
            display_name: "wFirma",
            configured: self.configured(),
            read_only: true,
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

    async fn fetch_entries(&self, _month: Month) -> Result<Vec<LedgerEntry>, AccountingSourceError> {
        if !self.configured() {
            return Err(AccountingSourceError::NotConfigured {
                provider: AccountingProvider::Wfirma,
                missing: "WFIRMA_ACCESS_KEY, WFIRMA_SECRET_KEY and/or WFIRMA_APP_KEY".to_owned(),
            });
        }

        Err(AccountingSourceError::NotEnabled {
            provider: AccountingProvider::Wfirma,
            reason: "API-key and OAuth2 auth are documented; read normalization awaits account fixtures".to_owned(),
        })
    }
}
