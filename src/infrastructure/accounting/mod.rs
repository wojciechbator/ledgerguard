mod fakturownia;
mod infakt;
mod saldeo;
mod saldeo_protocol;
pub mod saldeo_xml;
mod wfirma;

use std::sync::Arc;

use crate::{
    application::{AccountingProvider, AccountingSource},
    config::AccountingSettings,
};

pub use fakturownia::FakturowniaAdapter;
pub use infakt::InfaktAdapter;
pub use saldeo::SaldeoAdapter;
pub use wfirma::WfirmaAdapter;

#[must_use]
pub fn build_accounting_source(settings: &AccountingSettings) -> Arc<dyn AccountingSource> {
    match settings.provider {
        AccountingProvider::Saldeo => Arc::new(SaldeoAdapter::new(settings.saldeo.clone())),
        AccountingProvider::Fakturownia => {
            Arc::new(FakturowniaAdapter::new(settings.fakturownia.clone()))
        }
        AccountingProvider::Infakt => Arc::new(InfaktAdapter::new(settings.infakt.clone())),
        AccountingProvider::Wfirma => Arc::new(WfirmaAdapter::new(settings.wfirma.clone())),
    }
}
