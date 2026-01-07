// InventoryManager: Bitcoin-style inventory-driven propagation
//
// Rules:
// 1. Never push full data unsolicited
// 2. Always advertise hashes first (via Inv)
// 3. Never request the same object twice
// 4. Prefer blocks over transactions
// 5. Maintain global inventory tracking

use btclib::network::{InvItem, InvType};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Global inventory manager
///
/// Tracks:
/// - What inventory we've requested globally
/// - What inventory we have locally (in blockchain/mempool)
#[derive(Debug, Clone)]
pub struct InventoryManager {
    /// Inventory we've requested (across all peers)
    requested_inventory: Arc<RwLock<HashSet<InvItem>>>,
    
    /// Inventory we have locally (blocks in chain, txs in mempool)
    local_inventory: Arc<RwLock<HashSet<InvItem>>>,
}

impl InventoryManager {
    pub fn new() -> Self {
        Self {
            requested_inventory: Arc::new(RwLock::new(HashSet::new())),
            local_inventory: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Decide which inventory items we should request
    ///
    /// Bitcoin rules:
    /// - Prefer blocks over transactions
    /// - Don't request what we already have
    /// - Don't request what we've already requested
    pub async fn should_request(&self, items: &[InvItem]) -> Vec<InvItem> {
        let local = self.local_inventory.read().await;
        let requested = self.requested_inventory.read().await;
        
        let mut to_request = Vec::new();
        
        // First pass: collect blocks (higher priority)
        for item in items {
            if item.inv_type == InvType::Block {
                if !local.contains(item) && !requested.contains(item) {
                    to_request.push(item.clone());
                }
            }
        }
        
        // Second pass: collect transactions (lower priority)
        for item in items {
            if item.inv_type == InvType::Tx {
                if !local.contains(item) && !requested.contains(item) {
                    to_request.push(item.clone());
                }
            }
        }
        
        to_request
    }

    /// Record that we've requested these items
    pub async fn record_requested(&self, items: &[InvItem]) {
        let mut requested = self.requested_inventory.write().await;
        for item in items {
            requested.insert(item.clone());
        }
    }

    /// Mark that we've received and processed an item
    pub async fn mark_received(&self, item: &InvItem) {
        let mut requested = self.requested_inventory.write().await;
        requested.remove(item);
        
        let mut local = self.local_inventory.write().await;
        local.insert(item.clone());
    }

    /// Mark that we have this item locally (e.g., we created it)
    pub async fn mark_local(&self, item: &InvItem) {
        let mut local = self.local_inventory.write().await;
        local.insert(item.clone());
    }
}

impl Default for InventoryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use btclib::sha256::Hash;

    #[tokio::test]
    async fn test_inventory_deduplication() {
        let manager = InventoryManager::new();
        
        // Create test inventory items
        let hash1 = Hash::zero();
        let hash2 = Hash::hash(&[1, 2, 3]);
        let item1 = InvItem::block(hash1);
        let item2 = InvItem::tx(hash2);
        
        // Mark item1 as local
        manager.mark_local(&item1).await;
        
        // Should not request what we already have
        let to_request = manager.should_request(&[item1.clone(), item2.clone()]).await;
        assert!(!to_request.contains(&item1), "Should not request local inventory");
        assert!(to_request.contains(&item2), "Should request new inventory");
        
        // Record that we've requested item2
        manager.record_requested(&[item2.clone()]).await;
        
        // Should not request what we've already requested
        let to_request = manager.should_request(&[item2.clone()]).await;
        assert_eq!(to_request.len(), 0, "Should not request already-requested inventory");
    }
    
    #[tokio::test]
    async fn test_block_priority_over_tx() {
        let manager = InventoryManager::new();
        
        let block_hash = Hash::hash(&[1]);
        let tx_hash = Hash::hash(&[2]);
        let block_item = InvItem::block(block_hash);
        let tx_item = InvItem::tx(tx_hash);
        
        // Request should prioritize blocks
        let to_request = manager.should_request(&[tx_item.clone(), block_item.clone()]).await;
        assert_eq!(to_request.len(), 2);
        // Blocks should come first
        assert_eq!(to_request[0].inv_type, InvType::Block);
        assert_eq!(to_request[1].inv_type, InvType::Tx);
    }
}
