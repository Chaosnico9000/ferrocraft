use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use ferrocraft::{Config, Server};
use tokio::net::TcpListener;
use tokio::signal::unix::SignalKind;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ferrocraft.toml"));
    let config = Config::load_or_create(&config_path)?;
    let listener = TcpListener::bind(&config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;

    let server = Server::new(config)?;
    let mut terminate = tokio::signal::unix::signal(SignalKind::terminate())?;
    tokio::select! {
        result = Arc::clone(&server).serve(listener) => return result,
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
    info!("shutting down");
    server.shutdown().await;
    // Give the connections a moment to deliver their disconnect message.
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(())
}
