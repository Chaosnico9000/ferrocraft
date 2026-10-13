pub mod config;
mod connection;
mod login;
mod status;

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use tokio::net::TcpListener;
use tracing::{debug, info};

pub use config::Config;

/// State shared by all connections.
#[derive(Debug)]
pub struct Server {
    pub config: Config,
    pub online_players: AtomicU32,
}

impl Server {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self {
            config,
            online_players: AtomicU32::new(0),
        })
    }

    /// Accepts connections until the listener fails; each connection runs on
    /// its own task.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) -> anyhow::Result<()> {
        info!(addr = %listener.local_addr()?, "listening");
        loop {
            let (stream, peer) = listener.accept().await?;
            let server = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(err) = connection::handle(server, stream).await {
                    debug!(%peer, "connection closed: {err:#}");
                }
            });
        }
    }
}
