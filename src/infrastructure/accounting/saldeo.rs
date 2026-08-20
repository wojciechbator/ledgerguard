use async_trait::async_trait;
use uuid::Uuid;

use super::saldeo_protocol::{SaldeoGetRequest, signed_get_request};
use crate::{
    application::{
        AccountingProvider, AccountingRecord, AccountingSource, AccountingSourceError,
        ProviderCapabilities, ProviderDescriptor,
    },
    config::SaldeoSettings,
    domain::Month,
};

const COMPANY_LIST_PATH: &str = "/api/xml/1.0/company/list";
const DOCUMENT_LIST_PATH: &str = "/api/xml/2.12/document/list";

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

    fn scope_configured(&self) -> bool {
        self.settings.company_program_id.is_some()
    }

    /// Builds the first safe live request used after credentials are issued.
    /// `company.list` lets us verify exactly which companies the dedicated user can see
    /// before any document synchronization is allowed.
    pub fn company_list_probe_request(&self) -> Result<SaldeoGetRequest, AccountingSourceError> {
        let (username, token) = self.credentials()?;
        signed_get_request(
            COMPANY_LIST_PATH,
            username,
            token,
            &Uuid::new_v4().to_string(),
            &[],
        )
        .map_err(|error| AccountingSourceError::InvalidData {
            provider: AccountingProvider::Saldeo,
            reason: error.to_string(),
        })
    }

    /// Builds the document-list request with the deliberately conservative `SALDEO` policy.
    /// Policies such as LAST_10_DAYS can alter export state and therefore are not used by default.
    pub fn document_list_request(&self) -> Result<SaldeoGetRequest, AccountingSourceError> {
        let (username, token) = self.credentials()?;
        let company_program_id = self.settings.company_program_id.as_deref().ok_or_else(|| {
            AccountingSourceError::ScopeNotConfigured {
                provider: AccountingProvider::Saldeo,
                missing: "SALDEO_COMPANY_PROGRAM_ID".to_owned(),
            }
        })?;

        signed_get_request(
            DOCUMENT_LIST_PATH,
            username,
            token,
            &Uuid::new_v4().to_string(),
            &[("company_program_id", company_program_id), ("policy", "SALDEO")],
        )
        .map_err(|error| AccountingSourceError::InvalidData {
            provider: AccountingProvider::Saldeo,
            reason: error.to_string(),
        })
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.settings.base_url
    }

    fn credentials(&self) -> Result<(&str, &str), AccountingSourceError> {
        let username = self.settings.username.as_deref().ok_or_else(|| {
            AccountingSourceError::NotConfigured {
                provider: AccountingProvider::Saldeo,
                missing: "SALDEO_USERNAME".to_owned(),
            }
        })?;
        let token = self.settings.api_token.as_ref().ok_or_else(|| {
            AccountingSourceError::NotConfigured {
                provider: AccountingProvider::Saldeo,
                missing: "SALDEO_API_TOKEN".to_owned(),
            }
        })?;
        Ok((username, token.expose()))
    }
}

#[async_trait]
impl AccountingSource for SaldeoAdapter {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            provider: AccountingProvider::Saldeo,
            display_name: "SaldeoSMART",
            configured: self.configured(),
            scope_configured: self.scope_configured(),
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
        if !self.scope_configured() {
            return Err(AccountingSourceError::ScopeNotConfigured {
                provider: AccountingProvider::Saldeo,
                missing: "SALDEO_COMPANY_PROGRAM_ID".to_owned(),
            });
        }

        Err(AccountingSourceError::NotEnabled {
            provider: AccountingProvider::Saldeo,
            reason: "live XML response normalization stays gated until company.list scope and redacted document fixtures are verified".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SecretString;

    fn configured_settings() -> SaldeoSettings {
        SaldeoSettings {
            base_url: "https://saldeo.brainshare.pl".to_owned(),
            username: Some("user".to_owned()),
            api_token: Some(SecretString::for_test("token")),
            company_program_id: Some("company-123".to_owned()),
        }
    }

    #[test]
    fn document_list_is_hard_wired_to_non_surprising_saldeo_policy() {
        let request = SaldeoAdapter::new(configured_settings())
            .document_list_request()
            .unwrap();

        assert_eq!(request.path, DOCUMENT_LIST_PATH);
        assert!(
            request
                .query
                .iter()
                .any(|(key, value)| key == "policy" && value == "SALDEO")
        );
        assert!(!request.query.iter().any(|(_, value)| value == "token"));
    }
}
