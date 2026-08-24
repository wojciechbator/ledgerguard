use std::{env, fmt, fs, net::SocketAddr, str::FromStr};

use rust_decimal::Decimal;

use anyhow::{Context, Result, bail};

use crate::{application::AccountingProvider, domain::Money};

const MIN_API_TOKEN_BYTES: usize = 32;

#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    fn from_env_or_file(name: &'static str) -> Result<Option<Self>> {
        let direct = env::var(name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let file_var = format!("{name}_FILE");
        let file_path = env::var(&file_var)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());

        match (direct, file_path) {
            (Some(_), Some(_)) => bail!("set only one of {name} or {file_var}"),
            (Some(value), None) => Ok(Some(Self(value))),
            (None, Some(path)) => {
                let value = fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {file_var} secret file: {path}"))?;
                let value = value.trim().to_owned();
                if value.is_empty() {
                    bail!("{file_var} secret file is empty: {path}");
                }
                Ok(Some(Self(value)))
            }
            (None, None) => Ok(None),
        }
    }

    fn required_from_env_or_file(name: &'static str) -> Result<Self> {
        Self::from_env_or_file(name)?.with_context(|| {
            format!("{name} is required (or provide {name}_FILE pointing at a secret file)")
        })
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_owned())
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
pub struct BudgetSettings {
    /// Planned cost ceiling per calendar month. `None` disables affordability
    /// verdicts instead of guessing one.
    pub monthly_cost_budget: Option<Money>,
    /// Remaining-budget share (basis points) at which the verdict turns Tight.
    pub tight_share_basis_points: u16,
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
    pub budget: BudgetSettings,
    pub accounting: AccountingSettings,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind_addr =
            env::var("LEDGERGUARD_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
        let bind_addr = SocketAddr::from_str(&bind_addr)
            .with_context(|| format!("invalid LEDGERGUARD_BIND_ADDR: {bind_addr}"))?;

        let database_url = SecretString::required_from_env_or_file("DATABASE_URL")?;
        let auth_disabled = bool_env("LEDGERGUARD_AUTH_DISABLED", false)?;
        let api_token = SecretString::from_env_or_file("LEDGERGUARD_API_TOKEN")?;
        validate_runtime_security(bind_addr, auth_disabled, api_token.as_ref())?;

        let provider = env::var("LEDGERGUARD_ACCOUNTING_PROVIDER")
            .unwrap_or_else(|_| AccountingProvider::default().to_string())
            .parse()
            .context("invalid LEDGERGUARD_ACCOUNTING_PROVIDER")?;

        let saldeo_base_url = env::var("SALDEO_BASE_URL")
            .unwrap_or_else(|_| "https://saldeo.brainshare.pl".to_owned());
        if !saldeo_base_url.starts_with("https://") {
            bail!("SALDEO_BASE_URL must use https://");
        }

        let budget = BudgetSettings {
            monthly_cost_budget: match optional_env("LEDGERGUARD_MONTHLY_COST_BUDGET") {
                Some(raw) => Some(
                    Decimal::from_str(&raw)
                        .ok()
                        .and_then(|value| Money::non_negative(value).ok())
                        .with_context(|| {
                            format!("LEDGERGUARD_MONTHLY_COST_BUDGET must be a non-negative decimal, got {raw:?}")
                        })?,
                ),
                None => None,
            },
            tight_share_basis_points: match optional_env("LEDGERGUARD_TIGHT_SHARE_BASIS_POINTS") {
                Some(raw) => raw.parse::<u16>().map_err(|error| {
                    anyhow::anyhow!("invalid LEDGERGUARD_TIGHT_SHARE_BASIS_POINTS {raw:?}: {error}")
                })?,
                None => 1_000,
            },
        };
        if let Some(budget) = budget.monthly_cost_budget {
            anyhow::ensure!(
                budget.amount() > Decimal::ZERO,
                "LEDGERGUARD_MONTHLY_COST_BUDGET must be greater than zero"
            );
        }
        anyhow::ensure!(
            budget.tight_share_basis_points <= 5_000,
            "LEDGERGUARD_TIGHT_SHARE_BASIS_POINTS must be at most 5000"
        );

        Ok(Self {
            bind_addr,
            database_url,
            runtime: RuntimeSettings {
                api_token,
                auth_disabled,
                live_sync_enabled: bool_env("LEDGERGUARD_LIVE_SYNC_ENABLED", false)?,
            },
            budget,
            accounting: AccountingSettings {
                provider,
                saldeo: SaldeoSettings {
                    base_url: saldeo_base_url.trim_end_matches('/').to_owned(),
                    username: optional_env("SALDEO_USERNAME"),
                    api_token: SecretString::from_env_or_file("SALDEO_API_TOKEN")?,
                    company_program_id: optional_env("SALDEO_COMPANY_PROGRAM_ID"),
                },
                fakturownia: FakturowniaSettings {
                    account_domain: optional_env("FAKTUROWNIA_ACCOUNT_DOMAIN"),
                    department_id: optional_env("FAKTUROWNIA_DEPARTMENT_ID"),
                    api_token: SecretString::from_env_or_file("FAKTUROWNIA_API_TOKEN")?,
                },
                infakt: InfaktSettings {
                    api_key: SecretString::from_env_or_file("INFAKT_API_KEY")?,
                },
                wfirma: WfirmaSettings {
                    access_key: SecretString::from_env_or_file("WFIRMA_ACCESS_KEY")?,
                    secret_key: SecretString::from_env_or_file("WFIRMA_SECRET_KEY")?,
                    app_key: SecretString::from_env_or_file("WFIRMA_APP_KEY")?,
                    company_id: optional_env("WFIRMA_COMPANY_ID"),
                },
            },
        })
    }
}

fn validate_runtime_security(
    bind_addr: SocketAddr,
    auth_disabled: bool,
    api_token: Option<&SecretString>,
) -> Result<()> {
    if auth_disabled {
        if !bind_addr.ip().is_loopback() {
            bail!("LEDGERGUARD_AUTH_DISABLED=true is allowed only on a loopback bind address");
        }
        return Ok(());
    }

    let api_token = api_token
        .context("LEDGERGUARD_API_TOKEN is required unless LEDGERGUARD_AUTH_DISABLED=true")?;
    if api_token.expose().len() < MIN_API_TOKEN_BYTES {
        bail!("LEDGERGUARD_API_TOKEN must contain at least {MIN_API_TOKEN_BYTES} bytes");
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthenticated_runtime_is_loopback_only() {
        let loopback = SocketAddr::from_str("127.0.0.1:8080").unwrap();
        assert!(validate_runtime_security(loopback, true, None).is_ok());

        let exposed = SocketAddr::from_str("0.0.0.0:8080").unwrap();
        assert!(validate_runtime_security(exposed, true, None).is_err());
    }

    #[test]
    fn authenticated_runtime_requires_strong_token() {
        let exposed = SocketAddr::from_str("0.0.0.0:8080").unwrap();
        assert!(validate_runtime_security(exposed, false, None).is_err());
        assert!(
            validate_runtime_security(exposed, false, Some(&SecretString::for_test("short")))
                .is_err()
        );
        assert!(
            validate_runtime_security(
                exposed,
                false,
                Some(&SecretString::for_test("0123456789abcdef0123456789abcdef"))
            )
            .is_ok()
        );
    }
}
