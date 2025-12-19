use crate::database::BlockchainDB;
use crate::network::NetworkHub;
use crate::util::populate_connections;
use anyhow::Result;
use btclib::types::Blockchain;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{RwLock};
use tracing::info;
use uuid::Uuid;

/// Shared context for the node containing blockchain, database, and peer connections
#[derive(Clone)]
pub struct NodeContext {
    pub blockchain: Arc<RwLock<Blockchain>>,
    pub db: Arc<BlockchainDB>,
    pub network: Arc<NetworkHub>,
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

        let self_id = Uuid::new_v4().to_string();
        let network = NetworkHub::new(self_id);

        let ctx = Self {
            blockchain,
            db,
            network,
        };

        if !nodes.is_empty() {
            populate_connections(ctx.clone(), nodes).await?;
        }

        Ok(ctx)
    }
}
