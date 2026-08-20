use async_trait::async_trait;

use crate::{
    application::{AccountingGateway, AccountingGatewayError},
    domain::{LedgerEntry, Month},
};

/// SaldeoSMART adapter boundary.
///
/// The concrete command mapping is intentionally not guessed before the
/// dedicated API user and token are available. Saldeo's API surface is
/// permission-dependent; keeping this adapter fail-closed prevents an
/// accidental write-capable or cross-company integration.
#[derive(Debug, Clone)]
pub struct SaldeoGateway;

impl SaldeoGateway {
    #[must_use]
    pub const fn pending_credentials() -> Self {
        Self
    }
}

#[async_trait]
impl AccountingGateway for SaldeoGateway {
    async fn fetch_entries(
        &self,
        _month: Month,
    ) -> Result<Vec<LedgerEntry>, AccountingGatewayError> {
        Err(AccountingGatewayError::NotConfigured(
            "dedicated Saldeo API credentials are pending".to_owned(),
        ))
    }
}
