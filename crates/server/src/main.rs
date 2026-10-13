use std::path::PathBuf;

use anyhow::Context;
use ferrocraft::{Config, Server};
use tokio::net::TcpListener;
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

    let server = Server::new(config);
    tokio::select! {
        result = server.serve(listener) => result,
        _ = tokio::signal::ctrl_c() => {
            info!("shutting down");
            Ok(())
        }
    }
}
