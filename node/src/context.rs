use crate::database::BlockchainDB;
use btclib::types::Blockchain;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::RwLock;

/// Shared context for the node containing blockchain, database, and peer connections
#[derive(Clone)]
pub struct NodeContext {
    pub blockchain: Arc<RwLock<Blockchain>>,
    pub db: Arc<BlockchainDB>,
    pub nodes: Arc<DashMap<String, TcpStream>>,
}

impl NodeContext {
    pub fn new(db: BlockchainDB) -> Self {
        Self {
            blockchain: Arc::new(RwLock::new(Blockchain::new())),
            db: Arc::new(db),
            nodes: Arc::new(DashMap::new()),
        }
    }
}

