use async_trait::async_trait;
use reqwest::Client;
use uuid::Uuid;

use super::saldeo_protocol::{SaldeoHttpMethod, SaldeoRequest, signed_request};
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
    http: reqwest::Client,
}

impl SaldeoAdapter {
    #[must_use]
    pub fn new(settings: SaldeoSettings) -> Self {
        Self {
            settings,
            http: Client::new(),
        }
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
    pub(crate) fn company_list_probe_request(
        &self,
    ) -> Result<SaldeoRequest, AccountingSourceError> {
        let (username, token) = self.credentials()?;
        signed_request(
            SaldeoHttpMethod::Get,
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

    /// Saldeo API-XML v2.12 defines `document.list` as POST. The deliberately
    /// conservative `SALDEO` policy avoids the export-state semantics of LAST_10_DAYS variants.
    pub(crate) fn document_list_request(&self) -> Result<SaldeoRequest, AccountingSourceError> {
        let (username, token) = self.credentials()?;
        let company_program_id = self.settings.company_program_id.as_deref().ok_or_else(|| {
            AccountingSourceError::ScopeNotConfigured {
                provider: AccountingProvider::Saldeo,
                missing: "SALDEO_COMPANY_PROGRAM_ID".to_owned(),
            }
        })?;

        signed_request(
            SaldeoHttpMethod::Post,
            DOCUMENT_LIST_PATH,
            username,
            token,
            &Uuid::new_v4().to_string(),
            &[
                ("company_program_id", company_program_id),
                ("policy", "SALDEO"),
            ],
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
            // Sync stays disabled until the operator explicitly flips the
            // env flag; flipping it is a human decision recorded in config.
            sync_enabled: self.settings.sync_enabled,
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

    fn validate_configuration(&self) -> Result<(), AccountingSourceError> {
        let completely_unconfigured = self.settings.username.is_none()
            && self.settings.api_token.is_none()
            && self.settings.company_program_id.is_none();
        if completely_unconfigured {
            return Ok(());
        }

        self.company_list_probe_request()?;
        if self.scope_configured() {
            self.document_list_request()?;
        }
        Ok(())
    }

    async fn fetch_records(
        &self,
        month: Month,
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

        if !self.settings.sync_enabled {
            return Err(AccountingSourceError::NotEnabled {
                provider: AccountingProvider::Saldeo,
                reason: "SALDEO_SYNC_ENABLED not set; live document pulls stay gated until the operator verifies the contract".to_owned(),
            });
        }

        let request = self.document_list_request()?;
        let url = format!("{}{}", self.settings.base_url, request.path);
        let mut request_builder = match request.method {
            SaldeoHttpMethod::Get => self.http.get(url),
            SaldeoHttpMethod::Post => self.http.post(url),
        };
        for (key, value) in &request.parameters {
            request_builder = request_builder.query(&[(key.as_str(), value.as_str())]);
        }
        let response =
            request_builder
                .send()
                .await
                .map_err(|error| AccountingSourceError::Transport {
                    provider: AccountingProvider::Saldeo,
                    reason: error.to_string(),
                })?;
        if !response.status().is_success() {
            return Err(AccountingSourceError::Transport {
                provider: AccountingProvider::Saldeo,
                reason: format!("status {}", response.status()),
            });
        }
        let body = response
            .text()
            .await
            .map_err(|_| AccountingSourceError::InvalidData {
                provider: AccountingProvider::Saldeo,
                reason: "document.list returned non-text body".into(),
            })?;

        super::saldeo_xml::normalize_document_list(&body, month)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SecretString;

    fn configured_settings() -> SaldeoSettings {
        SaldeoSettings {
            base_url: "https://saldeo.brainshare.pl".to_owned(),
            sync_enabled: false,
            username: Some("user".to_owned()),
            api_token: Some(SecretString::for_test("token")),
            company_program_id: Some("company-123".to_owned()),
        }
    }

    #[test]
    fn company_probe_uses_the_official_get_operation() {
        let request = SaldeoAdapter::new(configured_settings())
            .company_list_probe_request()
            .unwrap();

        assert_eq!(request.method, SaldeoHttpMethod::Get);
        assert_eq!(request.path, COMPANY_LIST_PATH);
    }

    #[test]
    fn document_list_uses_v2_12_post_and_conservative_saldeo_policy() {
        let request = SaldeoAdapter::new(configured_settings())
            .document_list_request()
            .unwrap();

        assert_eq!(request.method, SaldeoHttpMethod::Post);
        assert_eq!(request.path, DOCUMENT_LIST_PATH);
        assert!(
            request
                .parameters
                .iter()
                .any(|(key, value)| key == "policy" && value == "SALDEO")
        );
        assert!(!request.parameters.iter().any(|(_, value)| value == "token"));
    }

    #[test]
    fn preflight_allows_absent_credentials_but_rejects_partial_configuration() {
        let empty = SaldeoSettings {
            base_url: "https://saldeo.brainshare.pl".to_owned(),
            sync_enabled: false,
            username: None,
            api_token: None,
            company_program_id: None,
        };
        assert!(SaldeoAdapter::new(empty).validate_configuration().is_ok());

        let partial = SaldeoSettings {
            base_url: "https://saldeo.brainshare.pl".to_owned(),
            sync_enabled: false,
            username: Some("user".to_owned()),
            api_token: None,
            company_program_id: None,
        };
        assert!(
            SaldeoAdapter::new(partial)
                .validate_configuration()
                .is_err()
        );
    }
}
