// PeerManager: Central coordinator (replaces dispatcher_loop)
//
// Responsibilities:
// - Owns all PeerState instances
// - Decides when to request data (inventory-driven)
// - Decides when to announce inventory
// - Enforces peer diversity rules
// - Schedules validation jobs
// - Routes messages between peers and validation layer
//
// This is the "brain" that replaces the old dispatcher_loop.

use crate::context::NodeContext;
use crate::network::inventory::InventoryManager;
use crate::network::peer_connection::{PeerCommand, PeerEvent};
use crate::network::peer_state::PeerState;
use btclib::network::{Envelope, InvItem, InvType, Message};
use btclib::sha256::Hash;
use btclib::types::{Block, BlockHeader, Transaction, TransactionOutput};
use btclib::util::MerkleRoot;
use chrono::Utc;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

pub type PeerId = String;

/// Handle to send commands to a peer connection
#[derive(Clone)]
pub struct PeerHandle {
    pub command_tx: mpsc::Sender<PeerCommand>,
}

/// Central peer manager (replaces dispatcher_loop)
///
/// Coordinates all peer connections, manages peer state,
/// and implements inventory-driven propagation.
pub struct PeerManager {
    /// All connected peers
    peers: Arc<DashMap<PeerId, PeerHandle>>,
    
    /// Per-peer state machines
    peer_states: Arc<DashMap<PeerId, PeerState>>,
    
    /// Inventory manager
    inventory: Arc<InventoryManager>,
    
    /// Node context (for blockchain access)
    ctx: NodeContext,
    
    /// Channel for receiving events from peer connections
    event_tx: mpsc::Sender<PeerEvent>,
}

impl PeerManager {
    /// Create a new PeerManager
    pub fn new(ctx: NodeContext, event_buffer: usize) -> (Arc<PeerManager>, mpsc::Receiver<PeerEvent>) {
        let (event_tx, event_rx) = mpsc::channel(event_buffer);
        
        let manager = Arc::new(PeerManager {
            peers: Arc::new(DashMap::new()),
            peer_states: Arc::new(DashMap::new()),
            inventory: Arc::new(InventoryManager::new()),
            ctx,
            event_tx,
        });
        
        (manager, event_rx)
    }
    
    /// Get the event sender (for forwarding events from PeerConnections)
    pub fn event_sender(&self) -> mpsc::Sender<PeerEvent> {
        self.event_tx.clone()
    }

    /// Register a new peer connection
    ///
    /// Returns a handle that can be used to send commands to the peer.
    pub fn register_peer(&self, peer_id: PeerId, command_tx: mpsc::Sender<PeerCommand>) -> PeerHandle {
        let handle = PeerHandle {
            command_tx,
        };
        
        self.peers.insert(peer_id.clone(), handle.clone());
        self.peer_states.insert(peer_id.clone(), PeerState::new(peer_id));
        
        handle
    }

    /// Remove a peer (on disconnect)
    pub fn remove_peer(&self, peer_id: &PeerId) {
        self.peers.remove(peer_id);
        self.peer_states.remove(peer_id);
        debug!("Removed peer: {}", peer_id);
    }

    /// Main event loop (replaces dispatcher_loop)
    ///
    /// Processes events from peer connections and coordinates
    /// inventory-driven propagation.
    pub async fn run(mut event_rx: mpsc::Receiver<PeerEvent>, manager: Arc<PeerManager>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        loop {
            tokio::select! {
                // Process events from peer connections
                event = event_rx.recv() => {
                    match event {
                        Some(event) => {
                            if let Err(e) = manager.handle_peer_event(event).await {
                                warn!("Error handling peer event: {}", e);
                            }
                        }
                        None => {
                            debug!("PeerManager: event channel closed");
                            break;
                        }
                    }
                }
            }
        }
        
        Ok(())
    }

    /// Handle an event from a peer connection
    pub async fn handle_peer_event(&self, event: PeerEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let PeerEvent { peer_id, envelope } = event;
        
        // Update peer activity
        if let Some(mut state) = self.peer_states.get_mut(&peer_id) {
            state.update_activity();
        }
        
        // Handle message based on type
        match &envelope.msg {
            // Inventory-driven messages
            Message::Inv(items) => {
                self.handle_inv(&peer_id, items).await?;
            }
            Message::GetData(items) => {
                self.handle_getdata(&peer_id, items).await?;
            }
            Message::Block(block) => {
                self.handle_block(&peer_id, block).await?;
            }
            Message::Tx(tx) => {
                self.handle_tx(&peer_id, tx).await?;
            }
            
            // Legacy messages (for backward compatibility during transition)
            Message::NewBlock(block) => {
                // Convert to inventory-driven: announce hash first
                let hash = block.hash();
                let item = InvItem::block(hash);
                self.announce_inventory(&[item]).await;
            }
            Message::NewTransaction(tx) => {
                // Convert to inventory-driven: announce hash first
                let hash = tx.hash();
                let item = InvItem::tx(hash);
                self.announce_inventory(&[item]).await;
            }
            
            // Other messages (handled by existing logic)
            _ => {
                // Delegate to existing handler logic for non-inventory messages
                // This maintains compatibility during transition
                self.handle_legacy_message(&peer_id, &envelope).await?;
            }
        }
        
        Ok(())
    }

    /// Handle Inv message (inventory announcement)
    ///
    /// Bitcoin protocol: When we receive Inv, we:
    /// 1. Record the inventory in peer state
    /// 2. Check if we should request any items
    /// 3. Send GetData for items we want
    async fn handle_inv(&self, peer_id: &PeerId, items: &[InvItem]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Update peer state with known inventory
        if let Some(mut state) = self.peer_states.get_mut(peer_id) {
            let new_items = state.record_inventory(items);
            
            // Check which items we should request
            let to_request = self.inventory.should_request(&new_items).await;
            
            if !to_request.is_empty() {
                // Record that we're requesting these
                state.record_request(&to_request);
                self.inventory.record_requested(&to_request).await;
                
                // Send GetData
                self.send_getdata(peer_id, &to_request).await?;
            }
        }
        
        Ok(())
    }

    /// Handle GetData message (peer requesting data)
    ///
    /// Bitcoin protocol: When we receive GetData, we:
    /// 1. Look up the requested items in our blockchain/mempool
    /// 2. Send Block or Tx messages with the actual data
    async fn handle_getdata(&self, peer_id: &PeerId, items: &[InvItem]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let blockchain = self.ctx.blockchain.read().await;
        
        for item in items {
            match item.inv_type {
                InvType::Block => {
                    // Find block by hash
                    if let Some(block) = blockchain.blocks().find(|b| b.hash() == item.hash) {
                        self.send_block(peer_id, block.clone()).await?;
                    }
                }
                InvType::Tx => {
                    // Check mempool first, then blockchain
                    let tx = blockchain.mempool()
                        .iter()
                        .find(|(_, tx)| tx.hash() == item.hash)
                        .map(|(_, tx)| tx.clone());
                    
                    if let Some(tx) = tx {
                        self.send_tx(peer_id, tx).await?;
                    } else {
                        // Could also search in blockchain transactions
                        // For now, we skip if not in mempool
                    }
                }
            }
        }
        
        Ok(())
    }

    /// Handle Block message (received data)
    ///
    /// Bitcoin protocol: When we receive Block, we:
    /// 1. Validate the block
    /// 2. Add to blockchain if valid
    /// 3. Announce to other peers (via Inv)
    async fn handle_block(&self, peer_id: &PeerId, block: &Block) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let hash = block.hash();
        let item = InvItem::block(hash);
        
        // Mark as received in inventory manager
        self.inventory.mark_received(&item).await;
        
        // Update peer state
        if let Some(mut state) = self.peer_states.get_mut(peer_id) {
            state.fulfill_request(&item);
        }
        
        // Validate and add to blockchain
        let mut blockchain = self.ctx.blockchain.write().await;
        match blockchain.add_block(block.clone()) {
            Ok(_) => {
                info!("Block accepted: {}", hash);
                
                // Announce to other peers (inventory-driven)
                self.announce_inventory(&[item]).await;
            }
            Err(e) => {
                warn!("Block rejected: {} - {}", hash, e);
            }
        }
        
        Ok(())
    }

    /// Handle Tx message (received data)
    ///
    /// Bitcoin protocol: When we receive Tx, we:
    /// 1. Validate the transaction
    /// 2. Add to mempool if valid
    /// 3. Announce to other peers (via Inv)
    async fn handle_tx(&self, peer_id: &PeerId, tx: &Transaction) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let hash = tx.hash();
        let item = InvItem::tx(hash);
        
        // Mark as received in inventory manager
        self.inventory.mark_received(&item).await;
        
        // Update peer state
        if let Some(mut state) = self.peer_states.get_mut(peer_id) {
            state.fulfill_request(&item);
        }
        
        // Validate and add to mempool
        let mut blockchain = self.ctx.blockchain.write().await;
        match blockchain.add_to_mempool(tx.clone()) {
            Ok(_) => {
                info!("Transaction accepted: {}", hash);
                
                // Announce to other peers (inventory-driven)
                self.announce_inventory(&[item]).await;
            }
            Err(e) => {
                warn!("Transaction rejected: {} - {}", hash, e);
            }
        }
        
        Ok(())
    }

    /// Announce inventory to all peers (via Inv message)
    ///
    /// Bitcoin protocol: We announce hashes first, peers request data if needed.
    async fn announce_inventory(&self, items: &[InvItem]) {
        // Mark as local inventory
        for item in items {
            self.inventory.mark_local(item).await;
        }
        
        // Send Inv to all peers
        let inv_msg = Message::Inv(items.to_vec());
        let envelope = Envelope::new(
            self.ctx.network.self_id.clone(),
            0, // TTL not used in inventory-driven protocol
            inv_msg,
        );
        
        for peer in self.peers.iter() {
            let cmd = PeerCommand { envelope: envelope.clone() };
            if let Err(e) = peer.value().command_tx.send(cmd).await {
                debug!("Failed to send Inv to {}: {}", peer.key(), e);
            }
        }
    }

    /// Send GetData message to a peer
    async fn send_getdata(&self, peer_id: &PeerId, items: &[InvItem]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(peer) = self.peers.get(peer_id) {
            let msg = Message::GetData(items.to_vec());
            let envelope = Envelope::new(
                self.ctx.network.self_id.clone(),
                0,
                msg,
            );
            let cmd = PeerCommand { envelope };
            peer.command_tx.send(cmd).await?;
        }
        Ok(())
    }

    /// Send Block message to a peer
    async fn send_block(&self, peer_id: &PeerId, block: Block) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(peer) = self.peers.get(peer_id) {
            let msg = Message::Block(block);
            let envelope = Envelope::new(
                self.ctx.network.self_id.clone(),
                0,
                msg,
            );
            let cmd = PeerCommand { envelope };
            peer.command_tx.send(cmd).await?;
        }
        Ok(())
    }

    /// Send Tx message to a peer
    async fn send_tx(&self, peer_id: &PeerId, tx: Transaction) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(peer) = self.peers.get(peer_id) {
            let msg = Message::Tx(tx);
            let envelope = Envelope::new(
                self.ctx.network.self_id.clone(),
                0,
                msg,
            );
            let cmd = PeerCommand { envelope };
            peer.command_tx.send(cmd).await?;
        }
        Ok(())
    }

    /// Handle legacy messages (for backward compatibility)
    ///
    /// This maintains compatibility with existing message types
    /// that aren't part of the inventory-driven protocol.
    async fn handle_legacy_message(&self, peer_id: &PeerId, envelope: &Envelope) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match &envelope.msg {
            Message::FetchBlock(height) => {
                let blockchain = self.ctx.blockchain.read().await;
                if let Some(block) = blockchain.blocks().nth(*height as usize).cloned() {
                    self.send_block(peer_id, block).await?;
                }
            }
            Message::DiscoverNodes => {
                let nodes: Vec<String> = self.peers.iter().map(|p| p.key().clone()).collect();
                let reply = Envelope::new(
                    self.ctx.network.self_id.clone(),
                    0,
                    Message::NodeList(nodes),
                );
                if let Some(peer) = self.peers.get(peer_id) {
                    let cmd = PeerCommand { envelope: reply };
                    peer.command_tx.send(cmd).await?;
                }
            }
            Message::FetchTemplate(pubkey) => {
                self.handle_fetch_template(peer_id, pubkey).await?;
            }
            Message::ValidateTemplate(block_template) => {
                self.handle_validate_template(peer_id, block_template).await?;
            }
            Message::SubmitTemplate(block) => {
                self.handle_submit_template(peer_id, block).await?;
            }
            Message::FetchUTXOs(key) => {
                self.handle_fetch_utxos(peer_id, key).await?;
            }
            Message::SubmitTransaction(tx) => {
                self.handle_submit_transaction(peer_id, tx).await?;
            }
            _ => {
                debug!("Unhandled legacy message: {:?}", envelope.msg);
            }
        }
        
        Ok(())
    }

    /// Handle FetchTemplate request from miner
    async fn handle_fetch_template(&self, peer_id: &PeerId, pubkey: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let blockchain = self.ctx.blockchain.read().await;

        // Build transactions list: coinbase first, then mempool transactions
        let mut transactions: Vec<Transaction> = blockchain
            .mempool()
            .iter()
            .take(btclib::BLOCK_TRANSACTION_CAP)
            .map(|(_, tx)| tx)
            .cloned()
            .collect();

        // Insert coinbase transaction at the beginning
        let coinbase = Transaction {
            inputs: vec![],
            outputs: vec![TransactionOutput {
                address: pubkey.to_string(),
                value: 0,
                unique_id: Uuid::new_v4(),
            }],
        };
        transactions.insert(0, coinbase);

        // Get last block hash
        let prev_block_hash = blockchain
            .blocks()
            .last()
            .map(|b| b.hash())
            .unwrap_or(Hash::zero());

        // Create block with placeholder merkle root
        let mut block = Block::new(
            BlockHeader {
                timestamp: Utc::now(),
                nonce: 0,
                prev_block_hash,
                merkle_root: MerkleRoot::calculate(&[]),
                target: blockchain.target(),
            },
            transactions,
        );

        // Calculate miner fees and update coinbase value
        let miner_fees = match block.calculate_miner_fees(blockchain.utxos()) {
            Ok(fees) => fees,
            Err(e) => {
                error!("Error calculating miner fees: {e}");
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to calculate miner fees: {}", e),
                )));
            }
        };

        let reward = blockchain.calculate_block_reward();
        block.transactions[0].outputs[0].value = reward + miner_fees;

        // Calculate merkle root once after coinbase value is finalized
        block.header.merkle_root = MerkleRoot::calculate(&block.transactions);

        let reply = Envelope::new(
            self.ctx.network.self_id.clone(),
            0,
            Message::Template(block),
        );
        if let Some(peer) = self.peers.get(peer_id) {
            let cmd = PeerCommand { envelope: reply };
            peer.command_tx.send(cmd).await?;
        }
        Ok(())
    }

    /// Handle ValidateTemplate request from miner
    async fn handle_validate_template(&self, peer_id: &PeerId, block_template: &Block) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let blockchain = self.ctx.blockchain.read().await;
        let last_block_hash = blockchain
            .blocks()
            .last()
            .map(|b| b.hash())
            .unwrap_or(Hash::zero());
        
        let status = block_template.header.prev_block_hash == last_block_hash;
        
        let reply = Envelope::new(
            self.ctx.network.self_id.clone(),
            0,
            Message::TemplateValidity(status),
        );
        if let Some(peer) = self.peers.get(peer_id) {
            let cmd = PeerCommand { envelope: reply };
            peer.command_tx.send(cmd).await?;
        } else {
            debug!("Peer {} not found when sending TemplateValidity response", peer_id);
        }
        Ok(())
    }

    /// Handle SubmitTemplate (mined block) from miner
    async fn handle_submit_template(&self, _peer_id: &PeerId, block: &Block) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Received mined block from miner: {}", block.hash());
        let mut blockchain = self.ctx.blockchain.write().await;
        
        match blockchain.add_block(block.clone()) {
            Ok(_) => {
                blockchain.rebuild_utxos();
                info!("Mined block accepted: {}", block.hash());
                
                // Announce to other peers via inventory-driven protocol
                let hash = block.hash();
                let item = InvItem::block(hash);
                self.announce_inventory(&[item]).await;
            }
            Err(e) => {
                warn!("Mined block rejected: {} - {}", block.hash(), e);
            }
        }
        
        Ok(())
    }

    /// Handle FetchUTXOs request from wallet
    async fn handle_fetch_utxos(&self, peer_id: &PeerId, key: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Handling FetchUTXOs request from peer {} for address {}", peer_id, key);
        let blockchain = self.ctx.blockchain.read().await;
        let utxos = blockchain
            .utxos()
            .iter()
            .filter(|(_, (_, txout))| txout.address == *key)
            .map(|(_, (marked, txout))| (txout.clone(), *marked))
            .collect::<Vec<_>>();
        
        info!("Found {} UTXOs for address {}", utxos.len(), key);
        let reply = Envelope::new(
            self.ctx.network.self_id.clone(),
            0,
            Message::UTXOs(utxos),
        );
        if let Some(peer) = self.peers.get(peer_id) {
            let cmd = PeerCommand { envelope: reply };
            peer.command_tx.send(cmd).await?;
            info!("Sent UTXOs response to peer {}", peer_id);
        } else {
            warn!("Peer {} not found when trying to send UTXOs response", peer_id);
        }
        Ok(())
    }

    /// Handle SubmitTransaction (transaction submission) from wallet
    async fn handle_submit_transaction(&self, _peer_id: &PeerId, tx: &Transaction) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let hash = tx.hash();
        info!("Received transaction from wallet: {}", hash);
        let mut blockchain = self.ctx.blockchain.write().await;
        
        match blockchain.add_to_mempool(tx.clone()) {
            Ok(_) => {
                info!("Transaction accepted: {}", hash);
                
                // Announce to other peers via inventory-driven protocol
                let item = InvItem::tx(hash);
                self.announce_inventory(&[item]).await;
            }
            Err(e) => {
                warn!("Transaction rejected: {} - {}", hash, e);
            }
        }
        
        Ok(())
    }
}

