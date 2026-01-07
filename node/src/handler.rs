use crate::network::peer_connection::PeerConnection;
use crate::network::peer_manager::PeerManager;
use anyhow::Result;
use tokio::net::TcpStream;
use tracing::debug;
use std::net::SocketAddr;

const INBOUND_BUFFER: usize = 128;
const OUTBOUND_BUFFER: usize = 256;

/// Accept a new peer connection using Bitcoin-style architecture
///
/// Creates a PeerConnection (I/O only) and registers it with the PeerManager.
/// The PeerConnection handles all TCP I/O, and the PeerManager handles protocol logic.
pub async fn accept_peer(
    peer_manager: std::sync::Arc<PeerManager>,
    event_tx: tokio::sync::mpsc::Sender<crate::network::peer_connection::PeerEvent>,
    socket: TcpStream,
    peer_addr: SocketAddr,
) -> Result<()> {
    let peer_id = peer_addr.to_string();
    
    // Create PeerConnection (spawns I/O task)
    let (command_tx, mut event_rx) = PeerConnection::spawn(
        peer_id.clone(),
        socket,
        INBOUND_BUFFER,
        OUTBOUND_BUFFER,
    );
    
    // Register with PeerManager
    peer_manager.register_peer(peer_id.clone(), command_tx);
    
    // Forward events from PeerConnection to PeerManager
    while let Some(event) = event_rx.recv().await {
        if event_tx.send(event).await.is_err() {
            debug!("PeerManager event channel closed, disconnecting peer {}", peer_id);
            break;
        }
    }
    
    // Cleanup on disconnect
    peer_manager.remove_peer(&peer_id);
    Ok(())
}