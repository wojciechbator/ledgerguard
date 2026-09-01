use std::{
    env,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use ledgerguard::{
    api::{ApiAuth, AppState, router},
    config::Config,
    infrastructure::{
        accounting::build_accounting_source,
        email_ingest::{self, store::IngestStore},
        postgres::PgLedgerRepository,
    },
};
use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::{Level, info};

const DEFAULT_HEALTHCHECK_PORT: u16 = 8080;
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> Result<()> {
    if run_utility_command().await? {
        return Ok(());
    }

    init_tracing();

    let config = Config::from_env()?;
    let accounting = build_accounting_source(&config.accounting);
    accounting
        .validate_configuration()
        .context("accounting provider configuration preflight failed")?;
    let provider = accounting.descriptor();

    if config.runtime.live_sync_enabled
        && !(provider.configured
            && provider.scope_configured
            && provider.read_only
            && provider.sync_enabled)
    {
        bail!(
            "LEDGERGUARD_LIVE_SYNC_ENABLED=true but {} is not fully configured, scoped, read-only and fixture-verified",
            provider.provider
        );
    }

    let pool = PgPoolOptions::new()
        .min_connections(1)
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(300))
        .max_lifetime(Duration::from_secs(1_800))
        .connect(config.database_url.expose())
        .await
        .context("failed to connect to PostgreSQL")?;

    let migrations_dir =
        env::var("LEDGERGUARD_MIGRATIONS_DIR").unwrap_or_else(|_| "migrations".to_owned());
    let migrator = Migrator::new(Path::new(&migrations_dir))
        .await
        .with_context(|| format!("failed to load database migrations from {migrations_dir}"))?;
    migrator
        .run(&pool)
        .await
        .context("failed to apply database migrations")?;

    let ledger_repo = Arc::new(PgLedgerRepository::new(pool.clone()));

    // Load durable budget settings from DB. On first startup (no row yet),
    // seed the table with env-derived defaults so the operator can edit
    // them via the UI. After that, DB values take priority over env vars.
    let budget = match ledger_repo.load_budget_settings().await {
        Ok(Some(stored)) => {
            info!("budget: loaded durable settings from DB");
            stored
        }
        Ok(None) => {
            info!("budget: no DB row yet, seeding env-derived defaults");
            if let Err(error) = ledger_repo.save_budget_settings(&config.budget).await {
                tracing::warn!(%error, "budget: failed to seed defaults to DB");
            }
            config.budget.clone()
        }
        Err(error) => {
            tracing::warn!(%error, "budget: failed to load from DB, falling back to env defaults");
            config.budget.clone()
        }
    };

    // Email ingest is enabled only when IMAP credentials are present.
    let email_ingest = if config.email_ingest.imap_username.is_some()
        && config.email_ingest.imap_password.is_some()
    {
        Some(config.email_ingest.clone())
    } else {
        None
    };

    let state = AppState {
        pool,
        accounting,
        ledger: ledger_repo.clone(),
        live_sync_enabled: config.runtime.live_sync_enabled,
        budget: std::sync::Arc::new(std::sync::RwLock::new(budget)),
        email_ingest,
        tax: std::sync::Arc::new(config.tax.clone()),
    };

    // Spawn auto-sync background task for email-OCR ingest. Runs every
    // N hours (default 24). The first run is delayed 60s to let the
    // server bind and migrations complete. Set interval to 0 to disable.
    if let Some(ref ingest_config) = state.email_ingest {
        let interval_hours = ingest_config.auto_sync_interval_hours;
        if interval_hours > 0 {
            let pool_clone = state.pool.clone();
            let ingest_config_clone = ingest_config.clone();
            tokio::spawn(async move {
                let initial_delay = Duration::from_secs(60);
                let interval = Duration::from_secs(u64::from(interval_hours) * 3600);
                tokio::time::sleep(initial_delay).await;
                loop {
                    info!("auto-sync: starting scheduled email ingest");
                    match run_auto_ingest(&pool_clone, &ingest_config_clone).await {
                        Ok(report) => {
                            info!(
                                "auto-sync: complete — {} scanned, {} invoices, {} skipped, {} unparseable, {} errors",
                                report.scanned,
                                report.invoices_imported,
                                report.bank_confirmations_skipped,
                                report.unparseable,
                                report.errors
                            );
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "auto-sync: email ingest failed");
                        }
                    }
                    tokio::time::sleep(interval).await;
                }
            });
        }
    }

    let auth = match config.runtime.api_token.as_ref() {
        Some(token) => ApiAuth::Bearer(Arc::from(token.expose())),
        // Config validation only allows a missing token together with
        // LEDGERGUARD_AUTH_DISABLED on a loopback bind.
        None => ApiAuth::Disabled,
    };
    let app = router(state, auth).layer(TraceLayer::new_for_http());

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    info!(
        version = env!("CARGO_PKG_VERSION"),
        address = %config.bind_addr,
        accounting_provider = %provider.provider,
        accounting_configured = provider.configured,
        accounting_scope_configured = provider.scope_configured,
        accounting_sync_verified = provider.sync_enabled,
        live_sync_enabled = config.runtime.live_sync_enabled,
        auth_disabled = config.runtime.auth_disabled,
        "LedgerGuard listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")?;
    Ok(())
}

async fn run_utility_command() -> Result<bool> {
    let Some(command) = env::args().nth(1) else {
        return Ok(false);
    };

    match command.as_str() {
        "--version" | "version" => {
            println!("ledgerguard {}", env!("CARGO_PKG_VERSION"));
            Ok(true)
        }
        "healthcheck" => {
            run_healthcheck()?;
            Ok(true)
        }
        "ingest-email" => {
            init_tracing();
            run_email_ingest().await?;
            Ok(true)
        }
        "migrate" => {
            init_tracing();
            run_migrate().await?;
            Ok(true)
        }
        _ => bail!("unknown command: {command}"),
    }
}

fn run_healthcheck() -> Result<()> {
    let port = env::var("LEDGERGUARD_BIND_ADDR")
        .ok()
        .and_then(|value| value.parse::<SocketAddr>().ok())
        .map_or(DEFAULT_HEALTHCHECK_PORT, |address| address.port());
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, HEALTHCHECK_TIMEOUT)
        .with_context(|| format!("failed to connect to LedgerGuard at {address}"))?;
    stream.set_read_timeout(Some(HEALTHCHECK_TIMEOUT))?;
    stream.set_write_timeout(Some(HEALTHCHECK_TIMEOUT))?;
    stream.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;

    let mut response = [0_u8; 64];
    let read = stream.read(&mut response)?;
    let response = &response[..read];
    ensure!(
        response.starts_with(b"HTTP/1.1 200") || response.starts_with(b"HTTP/1.0 200"),
        "LedgerGuard health endpoint did not return HTTP 200"
    );
    Ok(())
}

async fn run_email_ingest() -> Result<()> {
    let config = Config::from_env()?;

    let username = config
        .email_ingest
        .imap_username
        .as_ref()
        .context("LEDGERGUARD_IMAP_USERNAME is required for email ingestion")?;
    let password = config
        .email_ingest
        .imap_password
        .as_ref()
        .context("LEDGERGUARD_IMAP_PASSWORD is required for email ingestion")?;

    let pool = PgPoolOptions::new()
        .min_connections(1)
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(config.database_url.expose())
        .await
        .context("failed to connect to PostgreSQL")?;

    let migrations_dir =
        env::var("LEDGERGUARD_MIGRATIONS_DIR").unwrap_or_else(|_| "migrations".to_owned());
    let migrator = Migrator::new(Path::new(&migrations_dir))
        .await
        .with_context(|| format!("failed to load database migrations from {migrations_dir}"))?;
    migrator
        .run(&pool)
        .await
        .context("failed to apply database migrations")?;

    let store = IngestStore::new(pool);
    let imap_config = email_ingest::imap_client::ImapConfig {
        host: config.email_ingest.imap_host.clone(),
        port: config.email_ingest.imap_port,
        username: username.clone(),
        password: password.expose().to_owned(),
        sent_folder: config.email_ingest.sent_folder.clone(),
        recipient_filter: config.email_ingest.recipient_filter.clone(),
        subject_filter: config.email_ingest.subject_filter.clone(),
        lookback_days: config.email_ingest.lookback_days,
    };

    let report = email_ingest::run_ingest(imap_config, &store).await?;

    info!(
        "email ingest report: {} scanned, {} invoices imported, {} bank confirmations skipped, {} unparseable, {} errors",
        report.scanned,
        report.invoices_imported,
        report.bank_confirmations_skipped,
        report.unparseable,
        report.errors
    );

    Ok(())
}

async fn run_migrate() -> Result<()> {
    let config = Config::from_env()?;
    let pool = PgPoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(config.database_url.expose())
        .await
        .context("failed to connect to PostgreSQL")?;

    let migrations_dir =
        env::var("LEDGERGUARD_MIGRATIONS_DIR").unwrap_or_else(|_| "migrations".to_owned());
    let migrator = Migrator::new(Path::new(&migrations_dir))
        .await
        .with_context(|| format!("failed to load database migrations from {migrations_dir}"))?;
    migrator
        .run(&pool)
        .await
        .context("failed to apply database migrations")?;
    info!("migrations applied successfully");
    Ok(())
}

fn init_tracing() {
    let level = env::var("RUST_LOG")
        .ok()
        .and_then(|value| value.parse::<Level>().ok())
        .unwrap_or(Level::INFO);
    tracing_subscriber::fmt()
        .with_max_level(level)
        .compact()
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        // A failed handler install must not take the service down; the loop
        // simply never resolves and systemd's stop timeout does the rest.
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::warn!(%error, "SIGTERM handler unavailable"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

/// Runs a single email-OCR ingest cycle for the auto-sync background task.
/// Reuses the same pipeline as the manual HTTP trigger and the CLI command.
async fn run_auto_ingest(
    pool: &sqlx::PgPool,
    config: &ledgerguard::config::EmailIngestSettings,
) -> anyhow::Result<email_ingest::IngestReport> {
    let username = config
        .imap_username
        .as_ref()
        .context("IMAP username not configured")?;
    let password = config
        .imap_password
        .as_ref()
        .context("IMAP password not configured")?;

    let store = IngestStore::new(pool.clone());
    let imap_config = email_ingest::imap_client::ImapConfig {
        host: config.imap_host.clone(),
        port: config.imap_port,
        username: username.clone(),
        password: password.expose().to_owned(),
        sent_folder: config.sent_folder.clone(),
        recipient_filter: config.recipient_filter.clone(),
        subject_filter: config.subject_filter.clone(),
        lookback_days: config.lookback_days,
    };

    email_ingest::run_ingest(imap_config, &store)
        .await
        .map_err(|e| anyhow::anyhow!(e))
}
