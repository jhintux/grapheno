use std::sync::Arc;

use anyhow::Result;
use btclib::types::Blockchain;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::context::NodeContext;
use crate::database::BlockchainDB;
use crate::handler;

pub async fn populate_connections(ctx: NodeContext, nodes: &[String]) -> Result<()> {
    debug!("trying to connect to other nodes...");
    for node in nodes {
        debug!("connecting to {}", node);
        match TcpStream::connect(&node).await {
            Ok(stream) => {
                info!("connected to {}", node);
                let peer_addr = match stream.peer_addr() {
                    Ok(addr) => addr,
                    Err(err) => {
                        warn!("missing peer addr for {}: {err}", node);
                        continue;
                    }
                };
                let ctx_clone = ctx.clone();
                tokio::spawn(async move {
                    let _ = handler::accept_peer(ctx_clone, stream, peer_addr).await;
                });
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
