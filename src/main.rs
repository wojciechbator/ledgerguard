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
    api::{AppState, router},
    config::Config,
    infrastructure::{accounting::build_accounting_source, postgres::PgLedgerRepository},
};
use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::{Level, info};

const DEFAULT_HEALTHCHECK_PORT: u16 = 8080;
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> Result<()> {
    if run_utility_command()? {
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

    let ledger = Arc::new(PgLedgerRepository::new(pool.clone()));
    let state = AppState {
        pool,
        accounting,
        ledger,
        live_sync_enabled: config.runtime.live_sync_enabled,
    };
    let app = router(
        state,
        config
            .runtime
            .api_token
            .as_ref()
            .map(|token| token.expose()),
        config.runtime.auth_disabled,
    )
    .layer(TraceLayer::new_for_http());

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

fn run_utility_command() -> Result<bool> {
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
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
