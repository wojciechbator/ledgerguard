use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use ledgerguard::{
    api::{AppState, router},
    config::Config,
    infrastructure::{accounting::build_accounting_source, postgres::PgLedgerRepository},
};
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
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

    sqlx::migrate!()
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

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
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
