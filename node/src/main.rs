use anyhow::Result;
use argh::FromArgs;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

mod context;
mod database;
mod handler;
mod network;
mod network_legacy;
mod util;

fn init_tracing() -> Result<()> {
    // Create a formatting layer for tracing output with a compact format
    let fmt_layer = fmt::layer().compact();

    // Create a filter layer to control the verbosity of logs
    // Try to get the filter configuration from the environment variables
    // If it fails, default to the "info" log level
    let filter_layer = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info"))?;

    // Build the tracing subscriber registry with the formatting layer,
    // the filter layer, and the error layer for enhanced error reporting
    tracing_subscriber::registry()
        .with(filter_layer) // Add the filter layer to control log verbosity
        .with(fmt_layer) // Add the formatting layer for compact log output
        .init(); // Initialize the tracing subscriber

    Ok(())
}

#[derive(FromArgs)]
/// A toy blockchain node
struct Args {
    #[argh(option, default = "9000")]
    /// port number
    port: u16,
    #[argh(option, default = "String::from(\"./blockchain_db\")")]
    /// blockchain database directory
    db_path: String,
    #[argh(positional)]
    /// addresses of initial nodes
    nodes: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing()?;
    
    let args: Args = argh::from_env();

    // Access the parsed arguments
    let port = args.port;
    let db_path = args.db_path;
    let nodes = args.nodes;

    // Initialize database and blockchain
    let ctx = context::NodeContext::new(&db_path, &nodes).await?;

    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    info!("Listening on {}", addr);

    // Clone context for background tasks
    let ctx_cleanup = ctx.clone();
    let ctx_save = ctx.clone();

    // start a task to periodically cleanup the mempool. Normally, you would want to keep and join the handle
    tokio::spawn(util::cleanup(ctx_cleanup));
    // and a task to periodically save the blockchain
    tokio::spawn(util::save(ctx_save));

    // Create PeerManager (Bitcoin-style architecture)
    use crate::network::peer_manager::PeerManager;
    const EVENT_BUFFER: usize = 256;
    let (peer_manager, event_rx) = PeerManager::new(ctx.clone(), EVENT_BUFFER);
    
    // Spawn PeerManager event loop (replaces dispatcher_loop)
    let peer_manager_clone = peer_manager.clone();
    tokio::spawn(async move {
        if let Err(err) = PeerManager::run(event_rx, peer_manager_clone).await {
            tracing::error!("PeerManager exited: {err}");
        }
    });

    // Get event sender for forwarding events from PeerConnections
    let event_tx = peer_manager.event_sender();

    loop {
        let (socket, peer_addr) = listener.accept().await?;
        let peer_manager_accept = peer_manager.clone();
        let event_tx_accept = event_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = handler::accept_peer(peer_manager_accept, event_tx_accept, socket, peer_addr).await {
                tracing::warn!("failed to accept peer: {err}");
            }
        });
    }
}
