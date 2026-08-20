use std::{env, net::SocketAddr, str::FromStr};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub database_url: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind_addr = env::var("LEDGERGUARD_BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
        let bind_addr = SocketAddr::from_str(&bind_addr)
            .with_context(|| format!("invalid LEDGERGUARD_BIND_ADDR: {bind_addr}"))?;

        let database_url = env::var("DATABASE_URL").context("DATABASE_URL is required")?;

        Ok(Self {
            bind_addr,
            database_url,
        })
    }
}
