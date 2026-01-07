use std::sync::Arc;

use anyhow::Result;
use btclib::types::Blockchain;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::context::NodeContext;
use crate::database::BlockchainDB;

pub async fn populate_connections(_ctx: NodeContext, nodes: &[String]) -> Result<()> {
    debug!("trying to connect to other nodes...");
    for node in nodes {
        debug!("connecting to {}", node);
        match TcpStream::connect(&node).await {
            Ok(_stream) => {
                info!("connected to {}", node);
                // Note: populate_connections is called during initialization before PeerManager exists
                // In the new architecture, connections are handled by PeerManager in main.rs
                // This function is kept for API compatibility but connections should be initiated
                // through the PeerManager after it's created
                warn!("populate_connections: connections should be initiated through PeerManager in new architecture, skipping {}", node);
            }
            Err(err) => warn!("failed to connect to {}: {}", node, err),
        }
    }
    Ok(())
}

pub async fn cleanup(ctx: NodeContext) {
    let mut interval = time::interval(time::Duration::from_secs(30));
    loop {
        interval.tick().await;
        debug!("cleaning the mempool from old transactions");
        let mut blockchain = ctx.blockchain.write().await;
        blockchain.cleanup_mempool();
    }
}

pub async fn save(ctx: NodeContext) {
    let mut interval = time::interval(time::Duration::from_secs(15));
    loop {
        interval.tick().await;
        if let Err(e) = save_blockchain(&ctx.db, &ctx.blockchain).await {
            error!("error saving blockchain to database: {}", e);
        }
    }
}

pub async fn save_blockchain(
    db: &Arc<BlockchainDB>,
    blockchain: &Arc<RwLock<Blockchain>>,
) -> Result<()> {
    debug!("saving blockchain to database...");

    let blockchain = blockchain.read().await;
    db.save_blockchain(&*blockchain)?;
    debug!("blockchain saved to database");
    Ok(())
}
