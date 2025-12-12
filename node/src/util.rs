use std::sync::Arc;

use anyhow::{Context, Result};
use btclib::network::Message;
use btclib::types::Blockchain;
use dashmap::DashMap;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::context::NodeContext;
use crate::database::BlockchainDB;

pub async fn populate_connections(nodes: &[String]) -> Result<Arc<DashMap<String, TcpStream>>> {
    let node_connections = Arc::new(DashMap::new());
    debug!("trying to connect to other nodes...");
    for node in nodes {
        debug!("connecting to {}", node);
        let mut stream = TcpStream::connect(&node).await?;
        let message = Message::DiscoverNodes;
        message.send_async(&mut stream).await?;
        debug!("sent DiscoverNodes to {}", node);
        let message = Message::receive_async(&mut stream).await?;
        match message {
            Message::NodeList(child_nodes) => {
                debug!("received NodeList from {}", node);
                for child_node in child_nodes {
                    debug!("adding node {}", child_node);
                    let new_stream = TcpStream::connect(&child_node).await?;
                    node_connections.insert(child_node, new_stream);
                }
            }
            _ => {
                warn!("unexpected message from {}", node);
            }
        }
        node_connections.insert(node.clone(), stream);
    }
    Ok(node_connections)
}

// TODO potential security problem, malicious node could return a very large number (321). Create a consensus mecanism and AskDifference message could be used to do that.
pub async fn find_longest_chain_node(
    nodes_connections: &Arc<DashMap<String, TcpStream>>,
) -> Result<(String, u32)> {
    debug!("finding nodes with the highest blockchain length...");
    let mut longest_name = String::new();
    let mut longest_count = 0;
    let all_nodes = nodes_connections
        .iter()
        .map(|x| x.key().clone())
        .collect::<Vec<_>>();
    for node in all_nodes {
        debug!("asking {} for blockchain length", node);
        let mut stream = nodes_connections.get_mut(&node).context("no node")?;
        let message = Message::AskDifference(0);
        message
            .send_async(&mut *stream)
            .await
            .context(format!("Failed to send AskDifference message to {}", node))?;
        debug!("sent AskDifference to {}", node);
        let message = Message::receive_async(&mut *stream).await?;
        match message {
            Message::Difference(count) => {
                debug!("received Difference from {}", node);
                if count > longest_count {
                    info!("new longest blockchain: {} blocks from {node}", count);
                    longest_count = count;
                    longest_name = node;
                }
            }
            e => {
                warn!("unexpected message from {}: {:?}", node, e);
            }
        }
    }
    Ok((longest_name, longest_count as u32))
}

// TODO add another message type that would return an entire chain of blocks
pub async fn download_blockchain(
    nodes_connections: &Arc<DashMap<String, TcpStream>>,
    blockchain: &Arc<RwLock<Blockchain>>,
    node: &str,
    count: u32,
) -> Result<()> {
    let mut stream = nodes_connections.get_mut(node).unwrap();
    for i in 0..count as usize {
        let message = Message::FetchBlock(i);
        message.send_async(&mut *stream).await?;
        let message = Message::receive_async(&mut *stream).await?;
        match message {
            Message::NewBlock(block) => {
                let mut blockchain = blockchain.write().await;
                blockchain.add_block(block)?;
            }
            _ => {
                warn!("unexpected message from {}", node);
            }
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
