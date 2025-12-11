use anyhow::Result;
use argh::FromArgs;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

mod context;
mod database;
mod handler;
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

    loop {
        let (socket, _) = listener.accept().await?;
        let ctx_handle = ctx.clone();
        tokio::spawn(handler::handle_connection(ctx_handle, socket));
    }
}
