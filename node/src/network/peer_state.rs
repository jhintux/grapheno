// PeerState: Per-peer protocol state machine
//
// Tracks:
// - Known inventory (what this peer has announced)
// - Requested inventory (what we've requested from this peer)
// - Last activity timestamp
//
// This is the protocol state, NOT the I/O state.

use btclib::network::InvItem;
use std::collections::HashSet;
use std::time::Instant;

pub type PeerId = String;

/// Per-peer protocol state
///
/// Each peer connection has its own state machine tracking:
/// - What inventory the peer has announced
/// - What we've requested from the peer
#[derive(Debug)]
pub struct PeerState {
    /// Inventory items this peer has announced (via Inv messages)
    /// We use this to avoid requesting the same item twice.
    known_inventory: HashSet<InvItem>,
    
    /// Inventory items we've requested from this peer (via GetData)
    /// We use this to track pending requests and avoid duplicates.
    requested_inventory: HashSet<InvItem>,
    
    /// Last time we received a message from this peer
    last_activity: Instant,
}

impl PeerState {
    pub fn new(_peer_id: PeerId) -> Self {
        Self {
            known_inventory: HashSet::new(),
            requested_inventory: HashSet::new(),
            last_activity: Instant::now(),
        }
    }

    /// Record that we've seen this inventory from the peer
    ///
    /// Returns new inventory items (not seen before).
    pub fn record_inventory(&mut self, items: &[InvItem]) -> Vec<InvItem> {
        let mut new_items = Vec::new();
        for item in items {
            if self.known_inventory.insert(item.clone()) {
                new_items.push(item.clone());
            }
        }
        new_items
    }

    /// Record that we've requested this inventory from the peer
    ///
    /// Returns new requests (not already requested).
    pub fn record_request(&mut self, items: &[InvItem]) -> Vec<InvItem> {
        let mut new_requests = Vec::new();
        for item in items {
            if self.requested_inventory.insert(item.clone()) {
                new_requests.push(item.clone());
            }
        }
        new_requests
    }

    /// Mark that we've received data for a requested item
    pub fn fulfill_request(&mut self, item: &InvItem) {
        self.requested_inventory.remove(item);
    }

    /// Update last activity timestamp
    pub fn update_activity(&mut self) {
        self.last_activity = Instant::now();
    }
}

