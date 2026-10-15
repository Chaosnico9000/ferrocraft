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
        _ = console(&server) => {}
    }
    info!("shutting down");
    server.shutdown().await;
    // Give the connections a moment to deliver their disconnect message.
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(())
}

/// The server console: each line of standard input is a command, run as
/// an operator; `stop` ends the server. Without input (`< /dev/null`) the
/// server just goes on. Standard input is read on a thread of its own: a
/// blocking read in tokio's pool would keep the runtime, and so the
/// process, from ending.
async fn console(server: &Server) {
    let (lines, mut incoming) = tokio::sync::mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("console".into())
        .spawn(move || {
            for line in std::io::stdin().lines() {
                let Ok(line) = line else { break };
                if lines.send(line).is_err() {
                    break;
                }
            }
        })
        .expect("spawning the console thread");
    while let Some(line) = incoming.recv().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if matches!(line.trim_start_matches('/'), "stop") {
            info!("Stopping the server");
            return;
        }
        server.console_line(line);
    }
    std::future::pending::<()>().await;
}
