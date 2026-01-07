# Bitcoin-Style Peer Connection Architecture Refactoring

## Overview

This refactoring transforms the networking layer from a TTL-based gossip system to a Bitcoin-style inventory-driven peer-to-peer architecture.

## Architecture Changes

### Before (Old Architecture)
- Global `NetworkHub` with inbound queue
- Central `dispatcher_loop` handling all protocol logic
- TTL-based message flooding
- Direct broadcast of full blocks/transactions
- Tight coupling between I/O and protocol logic

### After (New Architecture)
- **PeerConnection**: Pure I/O layer (TCP read/write only)
- **PeerState**: Per-peer protocol state machine
- **InventoryManager**: Inventory-driven propagation logic
- **PeerManager**: Central coordinator (replaces dispatcher)
- Inventory-driven protocol: `Inv → GetData → Block/Tx`

## New Modules

### `node/src/network/peer_connection.rs`
- Handles TCP I/O only
- Framing messages (Envelope encoding/decoding)
- Sends events upward via channels
- NO blockchain logic, NO protocol decisions

### `node/src/network/peer_state.rs`
- Tracks per-peer state:
  - Handshake completion
  - Known inventory
  - Requested inventory
  - Misbehavior score
  - Activity timestamps

### `node/src/network/inventory.rs`
- Global inventory tracking
- Decides which inventory to request
- Prefers blocks over transactions
- Prevents duplicate requests

### `node/src/network/peer_manager.rs`
- Replaces `dispatcher_loop`
- Coordinates all peer connections
- Implements inventory-driven protocol
- Routes messages between peers and validation

## Message Flow

```
PeerConnection (IO)
    ↓ PeerEvent
PeerManager
    ↓
InventoryManager
    ↓
Validation (Blockchain)
    ↓
PeerManager
    ↓ PeerCommand
PeerConnection (IO)
```

## Inventory-Driven Protocol

1. **Announcement**: Send `Inv` message with hashes
2. **Request**: Peer sends `GetData` for items it wants
3. **Response**: Send `Block` or `Tx` with actual data
4. **Propagation**: Recipient announces to other peers via `Inv`

## Key Features

- ✅ Inventory-driven propagation (no unsolicited data)
- ✅ Per-peer state machines
- ✅ Clear separation: I/O, state, protocol, validation
- ✅ Backpressure-aware (bounded channels)
- ✅ DoS protection (misbehavior scoring)
- ✅ No TTL-based flooding

## Remaining Work

1. **Validation Pool**: Isolate validation logic into separate module
2. **Unit Tests**: Add tests for inventory de-duplication
3. **Legacy Compatibility**: Remove or fully migrate old NetworkHub usage
4. **Error Handling**: Improve error handling and peer eviction

## Files Modified

- `lib/src/network.rs`: Added `Inv`, `GetData`, `Block`, `Tx` messages
- `node/src/network/`: New module structure
- `node/src/handler.rs`: Updated to use PeerConnection
- `node/src/main.rs`: Updated to use PeerManager
- `node/src/network_legacy.rs`: Old NetworkHub (kept for compatibility)

## Testing

To test inventory de-duplication:
```rust
// Test that the same inventory item is not requested twice
// Test that known inventory is not re-announced
// Test that blocks are preferred over transactions
```

