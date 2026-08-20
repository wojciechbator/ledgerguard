use std::{env, fmt, net::SocketAddr, str::FromStr};

use anyhow::{Context, Result, bail};

use crate::application::AccountingProvider;

#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    fn from_env(name: &'static str) -> Option<Self> {
        env::var(name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .map(Self)
    }

    fn required_from_env(name: &'static str) -> Result<Self> {
        Self::from_env(name).with_context(|| format!("{name} is required"))
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
pub struct RuntimeSettings {
    pub api_token: Option<SecretString>,
    pub auth_disabled: bool,
    pub live_sync_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct SaldeoSettings {
    pub base_url: String,
    pub username: Option<String>,
    pub api_token: Option<SecretString>,
    pub company_program_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FakturowniaSettings {
    pub account_domain: Option<String>,
    pub department_id: Option<String>,
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
    pub company_id: Option<String>,
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
    pub database_url: SecretString,
    pub runtime: RuntimeSettings,
    pub accounting: AccountingSettings,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind_addr =
            env::var("LEDGERGUARD_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
        let bind_addr = SocketAddr::from_str(&bind_addr)
            .with_context(|| format!("invalid LEDGERGUARD_BIND_ADDR: {bind_addr}"))?;

        let database_url = SecretString::required_from_env("DATABASE_URL")?;
        let auth_disabled = bool_env("LEDGERGUARD_AUTH_DISABLED", false)?;
        let api_token = SecretString::from_env("LEDGERGUARD_API_TOKEN");
        if !auth_disabled && api_token.is_none() {
            bail!(
                "LEDGERGUARD_API_TOKEN is required unless LEDGERGUARD_AUTH_DISABLED=true; never disable auth on an exposed deployment"
            );
        }

        let provider = env::var("LEDGERGUARD_ACCOUNTING_PROVIDER")
            .unwrap_or_else(|_| AccountingProvider::default().to_string())
            .parse()
            .context("invalid LEDGERGUARD_ACCOUNTING_PROVIDER")?;

        let saldeo_base_url = env::var("SALDEO_BASE_URL")
            .unwrap_or_else(|_| "https://saldeo.brainshare.pl".to_owned());
        if !saldeo_base_url.starts_with("https://") {
            bail!("SALDEO_BASE_URL must use https://");
        }

        Ok(Self {
            bind_addr,
            database_url,
            runtime: RuntimeSettings {
                api_token,
                auth_disabled,
                live_sync_enabled: bool_env("LEDGERGUARD_LIVE_SYNC_ENABLED", false)?,
            },
            accounting: AccountingSettings {
                provider,
                saldeo: SaldeoSettings {
                    base_url: saldeo_base_url.trim_end_matches('/').to_owned(),
                    username: optional_env("SALDEO_USERNAME"),
                    api_token: SecretString::from_env("SALDEO_API_TOKEN"),
                    company_program_id: optional_env("SALDEO_COMPANY_PROGRAM_ID"),
                },
                fakturownia: FakturowniaSettings {
                    account_domain: optional_env("FAKTUROWNIA_ACCOUNT_DOMAIN"),
                    department_id: optional_env("FAKTUROWNIA_DEPARTMENT_ID"),
                    api_token: SecretString::from_env("FAKTUROWNIA_API_TOKEN"),
                },
                infakt: InfaktSettings {
                    api_key: SecretString::from_env("INFAKT_API_KEY"),
                },
                wfirma: WfirmaSettings {
                    access_key: SecretString::from_env("WFIRMA_ACCESS_KEY"),
                    secret_key: SecretString::from_env("WFIRMA_SECRET_KEY"),
                    app_key: SecretString::from_env("WFIRMA_APP_KEY"),
                    company_id: optional_env("WFIRMA_COMPANY_ID"),
                },
            },
        })
    }
}

fn optional_env(name: &'static str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn bool_env(name: &'static str, default: bool) -> Result<bool> {
    let Ok(raw) = env::var(name) else {
        return Ok(default);
    };

    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => bail!("{name} must be one of true/false, 1/0, yes/no, on/off"),
    }
}
