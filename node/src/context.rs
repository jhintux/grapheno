use crate::database::BlockchainDB;
use crate::util::{
    download_blockchain, find_longest_chain_node, populate_connections, save_blockchain,
};
use anyhow::Result;
use btclib::types::Blockchain;
use dashmap::DashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tracing::info;

/// Shared context for the node containing blockchain, database, and peer connections
#[derive(Clone)]
pub struct NodeContext {
    pub blockchain: Arc<RwLock<Blockchain>>,
    pub db: Arc<BlockchainDB>,
    pub nodes: Arc<DashMap<String, TcpStream>>,
}

impl NodeContext {
    pub async fn new<P: AsRef<Path>>(db_path: P, nodes: &[String]) -> Result<Self> {
        info!("opening database at {}", db_path.as_ref().display());
        let db = Arc::new(BlockchainDB::open(db_path)?);
        
        // Load blockchain from database or initialize a new one
        let blockchain = match db.load_blockchain() {
            Ok(loaded_blockchain) => {
                info!("blockchain loaded from database");
                Arc::new(RwLock::new(loaded_blockchain))
            }
            Err(_) => {
                info!("no blockchain found in database, initializing...");
                Arc::new(RwLock::new(Blockchain::new()))
            }
        };

        // Populate node connections only if blockchain wasn't loaded from database
        let nodes_connections = if blockchain.read().await.block_height() == 0 {
            // New blockchain, need to connect to nodes
            let connections = populate_connections(nodes).await?;
            info!("total amount of known nodes: {}", connections.len());

            if nodes.is_empty() {
                info!("no initial nodes provided, starting as a seed node");
            } else {
                let (longest_name, longest_count) =
                    find_longest_chain_node(&connections).await?;

                download_blockchain(
                    &connections,
                    &blockchain,
                    &longest_name,
                    longest_count,
                )
                .await?;

                info!("blockchain downloaded, from {}", longest_name);

                {
                    let mut blockchain_guard = blockchain.write().await;
                    blockchain_guard.rebuild_utxos();
                    blockchain_guard.try_adjust_target();
                }

                // Save the downloaded blockchain to database
                save_blockchain(&db, &blockchain).await?;
            }
            
            connections
        } else {
            // Blockchain loaded from database, initialize empty connections map
            // Connections will be populated later as needed
            Arc::new(DashMap::new())
        };

        Ok(Self {
            blockchain,
            db,
            nodes: nodes_connections,
        })
    }
}
