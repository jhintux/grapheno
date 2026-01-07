// Bitcoin-style peer-to-peer networking architecture
//
// Architecture overview:
// - PeerConnection: Pure I/O layer (TCP read/write, framing)
// - PeerState: Per-peer protocol state machine
// - InventoryManager: Inventory-driven propagation logic
// - PeerManager: Central coordinator (replaces dispatcher)
//
// Message flow:
// PeerConnection (IO) -> PeerManager -> InventoryManager -> Validation Pool -> PeerManager -> PeerConnection

pub mod peer_connection;
pub mod peer_state;
pub mod inventory;
pub mod peer_manager;

