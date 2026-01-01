use anyhow::Result;
use cursive::views::TextContent;
use tracing::*;
use clap::{Parser, Subcommand};
use core::Core;
use kanal;
use std::path::PathBuf;
use std::sync::Arc;
use util::{generate_dummy_config, init_tracing, setup_panic_hook, big_mode_btc};
use tasks::{update_utxos, handle_transactions, ui_task, update_balance};

mod core;
mod util;
mod tasks;
mod ui;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[arg(short, long, value_name = "FILE", default_value = "wallet_config.toml")]
    config: PathBuf,
    #[arg(short, long, value_name = "ADDRESS")]
    node: Option<String>,
}
#[derive(Subcommand)]
enum Commands {
    GenerateConfig {
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing()?;
    setup_panic_hook();
    info!("Starting wallet app");

    let cli = Cli::parse();
    match &cli.command {
        Some(Commands::GenerateConfig { output }) => {
            return generate_dummy_config(output);
        }
        None => {}
    }

    info!("Loading config from: {:?}", cli.config);

    let mut core = Core::load(cli.config.clone()).await?;
    if let Some(node) = cli.node {
        info!("Overriding default node with: {}", node);
        let mut config = core.config.write().unwrap();
        config.default_node = node;
    }

    let (tx_sender, tx_receiver) = kanal::bounded(10);
    core.tx_sender = tx_sender.clone();

    let core = Arc::new(core);
    info!("Starting background tasks");
    
    // Fetch UTXOs immediately on startup
    info!("Fetching initial UTXOs...");
    if let Err(e) = core.fetch_utxos().await {
        warn!("Failed to fetch initial UTXOs: {}", e);
    } else {
        info!("Initial UTXOs fetched successfully");
    }

    let balance_content = TextContent::new(big_mode_btc(&core));
    tokio::select! {
        _ = ui_task(core.clone(), balance_content.clone()) => (),
        _ = update_utxos(core.clone()) => (),
        _ = handle_transactions(tx_receiver.clone_async(), core.clone()) => (),
        _ = update_balance(core.clone(), balance_content.clone()) => (),
    }
    info!("App shutting down");
    Ok(())
}
