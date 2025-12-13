use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow,Result};
use btclib::network::Message;
use btclib::types::Blockchain;
use dashmap::DashMap;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, RwLock};
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::context::NodeContext;
use crate::database::BlockchainDB;

pub async fn populate_connections(nodes: &[String]) -> Result<Arc<DashMap<String, Arc<Mutex<TcpStream>>>>> {
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
                    node_connections.insert(child_node, Arc::new(Mutex::new(new_stream)));
                }
            }
            _ => {
                warn!("unexpected message from {}", node);
            }
        }
        node_connections.insert(node.clone(), Arc::new(Mutex::new(stream)));
    }
    Ok(node_connections)
}

/// Calculate consensus chain length using majority rule (50%+1)
/// Returns (consensus_length, list of nodes with that length)
fn calculate_consensus_chain_length(
    responses: Vec<(String, i32)>,
) -> Result<(u32, Vec<String>)> {
    if responses.is_empty() {
        return Err(anyhow!("no node responses received"));
    }

    let total_nodes = responses.len();
    let majority_threshold = (total_nodes / 2) + 1;

    // Group nodes by their claimed chain length
    let mut length_groups: HashMap<i32, Vec<String>> = HashMap::new();
    for (node_name, chain_length) in responses {
        length_groups
            .entry(chain_length)
            .or_insert_with(Vec::new)
            .push(node_name);
    }

    // Find chain length with majority consensus
    //let mut consensus_length: Option<i32> = None;
    //let mut consensus_nodes: Vec<String> = Vec::new();
    let mut consensus: Option<(i32, &Vec<String>)> = None;
    let mut max_group_size = 0;

    for (length, nodes) in &length_groups {
        let group_size = nodes.len();
        if group_size >= majority_threshold {
            // Found majority consensus
            consensus = Some((*length, nodes));
            //consensus_length = Some(*length);
            //consensus_nodes = nodes.clone();
            break;
        }
        // Track the most common length as fallback
        if group_size > max_group_size {
            max_group_size = group_size;
            //consensus_length = Some(*length);
            //consensus_nodes = nodes.clone();
        }
    }

    // If no majority, use the most common length (already set above)
    let (consensus_len, consensus_nodes) = consensus.ok_or_else(|| {
        anyhow!("failed to determine consensus chain length")
    })?;

    let consensus_count = consensus_nodes.len();
    if consensus_count < majority_threshold {
        warn!(
            "no majority consensus found ({} nodes agree on length {}, need {}). using most common length",
            consensus_count, consensus_len, majority_threshold
        );
    } else {
        info!(
            "consensus reached: {} nodes agree on chain length {}",
            consensus_count, consensus_len
        );
    }

    // Log outliers for security monitoring
    for (length, nodes) in &length_groups {
        if *length != consensus_len {
            warn!(
                "outlier chain length detected: {} nodes claim length {} (consensus: {})",
                nodes.len(),
                length,
                consensus_len
            );
            for node in nodes {
                warn!("  - outlier node: {}", node);
            }
        }
    }

    // Ensure non-negative length
    if consensus_len < 0 {
        return Err(anyhow!(
            "consensus chain length is negative: {}",
            consensus_len
        ));
    }

    Ok((consensus_len as u32, consensus_nodes.clone()))
}

/// Perform AskDifference request/response with a single node
/// Locks the Mutex, performs I/O, and releases the lock before returning
async fn ask_node_difference(
    stream_mutex: Arc<Mutex<TcpStream>>,
) -> Result<i32> {
    let mut stream = stream_mutex.lock().await;
    
    // Send AskDifference message
    let message = Message::AskDifference(0);
    message.send_async(&mut *stream).await?;
    
    debug!("sent AskDifference");
    
    // Receive response
    let response = Message::receive_async(&mut *stream).await?;
    
    match response {
        Message::Difference(count) => {
            debug!("received Difference: {}", count);
            Ok(count)
        }
        other => {
            Err(anyhow!("unexpected message type: {:?}", other))
        }
    }
}

/// Find the node with the longest valid chain using consensus mechanism
/// Implements Bitcoin-like consensus: requires majority (50%+1) of nodes to agree on chain length
pub async fn find_longest_chain_node(
    nodes_connections: &Arc<DashMap<String, Arc<Mutex<TcpStream>>>>,
) -> Result<(String, u32)> {
    debug!("finding nodes with the highest blockchain length using consensus...");
    
    if nodes_connections.is_empty() {
        return Err(anyhow::anyhow!("no nodes connected"));
    }

    // Snapshot all node keys first (no .await, so safe to hold DashMap references)
    // This ensures we don't hold any DashMap locks across async boundaries
    let node_keys: Vec<String> = nodes_connections
        .iter()
        .map(|entry| entry.key().clone())
        .collect();

    // Collect all node responses
    let mut responses: Vec<(String, i32)> = Vec::new();
    let mut failed_nodes: Vec<String> = Vec::new();

    // Process each node sequentially
    for node_key in &node_keys {
        debug!("asking {} for blockchain length", node_key);
        
        // Get a clone of the Arc<Mutex<TcpStream>> while holding DashMap lock briefly
        // Then release the DashMap lock before doing any async I/O
        let stream_mutex = match nodes_connections.get(node_key) {
            Some(entry) => entry.value().clone(),
            None => {
                warn!("node {} no longer in connections map", node_key);
                failed_nodes.push(node_key.clone());
                continue;
            }
        };
        
        // Now perform async I/O outside of any DashMap lock
        match ask_node_difference(stream_mutex).await {
            Ok(count) => {
                debug!("received Difference from {}: {}", node_key, count);
                responses.push((node_key.clone(), count));
            }
            Err(e) => {
                warn!("failed to get response from {}: {}", node_key, e);
                failed_nodes.push(node_key.clone());
            }
        }
    }

    // Log failed nodes
    if !failed_nodes.is_empty() {
        warn!(
            "{} nodes failed to respond or returned invalid messages: {:?}",
            failed_nodes.len(),
            failed_nodes
        );
    }

    // Calculate consensus
    let (consensus_length, consensus_nodes) = calculate_consensus_chain_length(responses)?;

    // Select a node from the consensus group (prefer first one, or could randomize)
    let selected_node = consensus_nodes[0].clone();

    if consensus_nodes.len() == 1 && nodes_connections.len() > 1 {
        warn!(
            "only one node agrees on chain length {} (out of {} total nodes). proceeding with caution.",
            consensus_length,
            nodes_connections.len()
        );
    }

    info!(
        "consensus chain length: {} blocks. selected node: {} ({} nodes agree)",
        consensus_length,
        selected_node,
        consensus_nodes.len()
    );

    Ok((selected_node, consensus_length))
}

pub async fn download_blockchain(
    nodes_connections: &Arc<DashMap<String, Arc<Mutex<TcpStream>>>>,
    blockchain: &Arc<RwLock<Blockchain>>,
    node: &str,
) -> Result<()> {
    // Get a clone of the Arc<Mutex<TcpStream>> while holding DashMap lock briefly
    // Then release the DashMap lock before doing any async I/O
    let stream_mutex = nodes_connections
        .get(node)
        .ok_or_else(|| anyhow!("node {} not found in connections", node))?
        .value()
        .clone();
    
    // Now perform async I/O outside of any DashMap lock
    let mut stream = stream_mutex.lock().await;
    let message = Message::FetchAllBlocks;
    message.send_async(&mut *stream).await?;
    let message = Message::receive_async(&mut *stream).await?;
    match message {
        Message::AllBlocks(blocks) => {
            for block in blocks {
                let mut blockchain = blockchain.write().await;
                blockchain.add_block(block)?;
            }
        }
        _ => {
            warn!("unexpected message from {}", node);
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
