use std::{env, fmt, net::SocketAddr, str::FromStr};

use anyhow::{Context, Result};

use crate::application::AccountingProvider;

#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    fn from_env(name: &'static str) -> Option<Self> {
        env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(Self)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

#[derive(Debug, Clone)]
pub struct SaldeoSettings {
    pub username: Option<String>,
    pub api_token: Option<SecretString>,
}

#[derive(Debug, Clone)]
pub struct FakturowniaSettings {
    pub account_domain: Option<String>,
    pub api_token: Option<SecretString>,
}

#[derive(Debug, Clone)]
pub struct InfaktSettings {
    pub api_key: Option<SecretString>,
}

#[derive(Debug, Clone)]
pub struct WfirmaSettings {
    pub access_key: Option<SecretString>,
    pub secret_key: Option<SecretString>,
    pub app_key: Option<SecretString>,
}

#[derive(Debug, Clone)]
pub struct AccountingSettings {
    pub provider: AccountingProvider,
    pub saldeo: SaldeoSettings,
    pub fakturownia: FakturowniaSettings,
    pub infakt: InfaktSettings,
    pub wfirma: WfirmaSettings,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub accounting: AccountingSettings,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind_addr = env::var("LEDGERGUARD_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
        let bind_addr = SocketAddr::from_str(&bind_addr)
            .with_context(|| format!("invalid LEDGERGUARD_BIND_ADDR: {bind_addr}"))?;

        let database_url = env::var("DATABASE_URL").context("DATABASE_URL is required")?;
        let provider = env::var("LEDGERGUARD_ACCOUNTING_PROVIDER")
            .unwrap_or_else(|_| AccountingProvider::default().to_string())
            .parse()
            .context("invalid LEDGERGUARD_ACCOUNTING_PROVIDER")?;

        Ok(Self {
            bind_addr,
            database_url,
            accounting: AccountingSettings {
                provider,
                saldeo: SaldeoSettings {
                    username: optional_env("SALDEO_USERNAME"),
                    api_token: SecretString::from_env("SALDEO_API_TOKEN"),
                },
                fakturownia: FakturowniaSettings {
                    account_domain: optional_env("FAKTUROWNIA_ACCOUNT_DOMAIN"),
                    api_token: SecretString::from_env("FAKTUROWNIA_API_TOKEN"),
                },
                infakt: InfaktSettings {
                    api_key: SecretString::from_env("INFAKT_API_KEY"),
                },
                wfirma: WfirmaSettings {
                    access_key: SecretString::from_env("WFIRMA_ACCESS_KEY"),
                    secret_key: SecretString::from_env("WFIRMA_SECRET_KEY"),
                    app_key: SecretString::from_env("WFIRMA_APP_KEY"),
                },
            },
        })
    }
}

fn optional_env(name: &'static str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}
