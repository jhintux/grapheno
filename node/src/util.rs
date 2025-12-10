use anyhow::{Context, Result};
use btclib::network::Message;
use tokio::net::TcpStream;
use tokio::time;
use tracing::{info, debug, warn, error};

use crate::context::NodeContext;

pub async fn load_blockchain(ctx: &NodeContext) -> Result<()> {
    info!("loading blockchain from database...");

    let new_blockchain = ctx.db.load_blockchain()?;
    info!("blockchain loaded from database");

    let mut blockchain = ctx.blockchain.write().await;
    *blockchain = new_blockchain;

    debug!("rebuilding utxos...");
    blockchain.rebuild_utxos();
    debug!("utxos rebuilt");

    debug!("checking if target needs to be adjusted...");
    debug!("current target: {}", blockchain.target());
    blockchain.try_adjust_target();
    debug!("new target: {}", blockchain.target());

    // Save the updated blockchain back to database
    drop(blockchain);
    save_blockchain(ctx).await?;

    info!("initialization complete");
    Ok(())
}

pub async fn populate_connections(ctx: &NodeContext, nodes: &[String]) -> Result<()> {
    info!("trying to connect to other nodes...");
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
                    ctx.nodes.insert(child_node, new_stream);
                }
            }
            _ => {
                warn!("unexpected message from {}", node);
            }
        }
        ctx.nodes.insert(node.clone(), stream);
    }
    Ok(())
}

// TODO potential security problem, malicious node could return a very large number (321). Create a consensus mecanism and AskDifference message could be used to do that.
pub async fn find_longest_chain_node(ctx: &NodeContext) -> Result<(String, u32)> {
    debug!("finding nodes with the highest blockchain length...");
    let mut longest_name = String::new();
    let mut longest_count = 0;
    let all_nodes = ctx.nodes
        .iter()
        .map(|x| x.key().clone())
        .collect::<Vec<_>>();
    for node in all_nodes {
        debug!("asking {} for blockchain length", node);
        let mut stream = ctx.nodes.get_mut(&node).context("no node")?;
        let message = Message::AskDifference(0);
        message.send_async(&mut *stream).await.unwrap();
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
pub async fn download_blockchain(ctx: &NodeContext, node: &str, count: u32) -> Result<()> {
    let mut stream = ctx.nodes.get_mut(node).unwrap();
    for i in 0..count as usize {
        let message = Message::FetchBlock(i);
        message.send_async(&mut *stream).await?;
        let message = Message::receive_async(&mut *stream).await?;
        match message {
            Message::NewBlock(block) => {
                let mut blockchain = ctx.blockchain.write().await;
                blockchain.add_block(block)?;
            }
            _ => {
                println!("unexpected message from {}", node);
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
        if let Err(e) = save_blockchain(&ctx).await {
            error!("error saving blockchain to database: {}", e);
        }
    }
}

pub async fn save_blockchain(ctx: &NodeContext) -> Result<()> {
    debug!("saving blockchain to database...");

    let blockchain = ctx.blockchain.read().await;
    ctx.db.save_blockchain(&*blockchain)?;
    debug!("blockchain saved to database");
    Ok(())
}
