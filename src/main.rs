use std::time::Duration;

use anyhow::{Context, Result};
use ledgerguard::{
    api::{AppState, router},
    config::Config,
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
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .context("failed to connect to PostgreSQL")?;

    sqlx::migrate!()
        .run(&pool)
        .await
        .context("failed to apply database migrations")?;

    let app = router(AppState { pool }).layer(TraceLayer::new_for_http());

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    info!(address = %config.bind_addr, "LedgerGuard listening");

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
