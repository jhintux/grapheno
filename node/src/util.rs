use anyhow::{Context, Result};
use btclib::network::Message;
use tokio::net::TcpStream;
use tokio::time;

use crate::context::NodeContext;

pub async fn load_blockchain(ctx: &NodeContext) -> Result<()> {
    println!("loading blockchain from database...");

    let new_blockchain = ctx.db.load_blockchain()?;
    println!("blockchain loaded from database");

    let mut blockchain = ctx.blockchain.write().await;
    *blockchain = new_blockchain;

    println!("rebuilding utxos...");
    blockchain.rebuild_utxos();
    println!("utxos rebuilt");

    println!("checking if target needs to be adjusted...");
    println!("current target: {}", blockchain.target());
    blockchain.try_adjust_target();
    println!("new target: {}", blockchain.target());

    // Save the updated blockchain back to database
    drop(blockchain);
    save_blockchain(ctx).await?;

    println!("initialization complete");
    Ok(())
}

pub async fn populate_connections(ctx: &NodeContext, nodes: &[String]) -> Result<()> {
    println!("trying to connect to other nodes...");
    for node in nodes {
        println!("connecting to {}", node);
        let mut stream = TcpStream::connect(&node).await?;
        let message = Message::DiscoverNodes;
        message.send_async(&mut stream).await?;
        println!("sent DiscoverNodes to {}", node);
        let message = Message::receive_async(&mut stream).await?;
        match message {
            Message::NodeList(child_nodes) => {
                println!("received NodeList from {}", node);
                for child_node in child_nodes {
                    println!("adding node {}", child_node);
                    let new_stream = TcpStream::connect(&child_node).await?;
                    ctx.nodes.insert(child_node, new_stream);
                }
            }
            _ => {
                println!("unexpected message from {}", node);
            }
        }
        ctx.nodes.insert(node.clone(), stream);
    }
    Ok(())
}

// TODO potential security problem, malicious node could return a very large number (321). Create a consensus mecanism and AskDifference message could be used to do that.
pub async fn find_longest_chain_node(ctx: &NodeContext) -> Result<(String, u32)> {
    println!("finding nodes with the highest blockchain length...");
    let mut longest_name = String::new();
    let mut longest_count = 0;
    let all_nodes = ctx
        .nodes
        .iter()
        .map(|x| x.key().clone())
        .collect::<Vec<_>>();
    for node in all_nodes {
        println!("asking {} for blockchain length", node);
        let mut stream = ctx.nodes.get_mut(&node).context("no node")?;
        let message = Message::AskDifference(0);
        message.send_async(&mut *stream).await.unwrap();
        println!("sent AskDifference to {}", node);
        let message = Message::receive_async(&mut *stream).await?;
        match message {
            Message::Difference(count) => {
                println!("received Difference from {}", node);
                if count > longest_count {
                    println!("new longest blockchain: {} blocks from {node}", count);
                    longest_count = count;
                    longest_name = node;
                }
            }
            e => {
                println!("unexpected message from {}: {:?}", node, e);
            }
        }
    }
    Ok((longest_name, longest_count as u32))
}

// FetchAllBlocks message type returns an entire chain of blocks
pub async fn download_blockchain(ctx: &NodeContext, node: &str, _count: u32) -> Result<()> {
    let mut stream = ctx.nodes.get_mut(node).unwrap();
    let message = Message::FetchAllBlocks;
    message.send_async(&mut *stream).await?;
    let message = Message::receive_async(&mut *stream).await?;
    match message {
        Message::AllBlocks(blocks) => {
            println!("received {} blocks from {}", blocks.len(), node);
            let mut blockchain = ctx.blockchain.write().await;
            for block in blocks {
                blockchain.add_block(block)?;
            }
        }
        _ => {
            anyhow::bail!("unexpected message from {}", node);
        }
    }
    Ok(())
}

pub async fn cleanup(ctx: NodeContext) {
    let mut interval = time::interval(time::Duration::from_secs(30));
    loop {
        interval.tick().await;
        println!("cleaning the mempool from old transactions");
        let mut blockchain = ctx.blockchain.write().await;
        blockchain.cleanup_mempool();
    }
}

pub async fn save(ctx: NodeContext) {
    let mut interval = time::interval(time::Duration::from_secs(15));
    loop {
        interval.tick().await;
        if let Err(e) = save_blockchain(&ctx).await {
            println!("error saving blockchain to database: {}", e);
        }
    }
}

pub async fn save_blockchain(ctx: &NodeContext) -> Result<()> {
    println!("saving blockchain to database...");

    let blockchain = ctx.blockchain.read().await;
    ctx.db.save_blockchain(&*blockchain)?;
    println!("blockchain saved to database");
    Ok(())
}
