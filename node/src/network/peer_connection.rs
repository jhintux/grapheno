// PeerConnection: Pure I/O layer for TCP communication
//
// This module handles ONLY:
// - TCP read/write
// - Message framing (Envelope encoding/decoding)
// - Sending parsed messages upward via channels
//
// NO blockchain logic, NO protocol decisions, NO gossip logic.
// This is the transport layer only.

use btclib::network::Envelope;
use tokio::io::AsyncRead;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::select;
use tracing::{debug, warn};

pub type PeerId = String;

/// Events emitted by PeerConnection (inbound messages)
#[derive(Debug, Clone)]
pub struct PeerEvent {
    pub peer_id: PeerId,
    pub envelope: Envelope,
}

/// Commands sent to PeerConnection (outbound messages)
#[derive(Debug, Clone)]
pub struct PeerCommand {
    pub envelope: Envelope,
}

/// Pure I/O connection handler for a single peer
///
/// One instance per TCP connection. Runs a single async task that:
/// - Reads Envelopes from TCP and sends PeerEvents upward
/// - Receives PeerCommands and writes Envelopes to TCP
///
/// Uses tokio::select! to multiplex read/write operations.
pub struct PeerConnection {
    peer_id: PeerId,
    socket: TcpStream,
    inbound_tx: mpsc::Sender<PeerEvent>,
    outbound_rx: mpsc::Receiver<PeerCommand>,
}

impl PeerConnection {
    /// Create a new PeerConnection and spawn its I/O task
    ///
    /// Returns handles for sending commands and receiving events.
    pub fn spawn(
        peer_id: PeerId,
        socket: TcpStream,
        inbound_buffer: usize,
        outbound_buffer: usize,
    ) -> (
        mpsc::Sender<PeerCommand>,
        mpsc::Receiver<PeerEvent>,
    ) {
        let (inbound_tx, inbound_rx) = mpsc::channel(inbound_buffer);
        let (outbound_tx, outbound_rx) = mpsc::channel(outbound_buffer);

        let mut connection = Self {
            peer_id: peer_id.clone(),
            socket,
            inbound_tx,
            outbound_rx,
        };

        // Spawn the I/O task
        tokio::spawn(async move {
            if let Err(e) = connection.run().await {
                warn!("PeerConnection for {} exited: {}", peer_id, e);
            }
        });

        (outbound_tx, inbound_rx)
    }

    /// Main I/O loop: multiplex read and write operations
    ///
    /// This is the only place where TCP I/O happens for this peer.
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (mut reader, mut writer) = self.socket.split();

        loop {
            select! {
                // Read path: TCP -> Envelope -> PeerEvent -> upstream
                result = Self::read_envelope(&mut reader) => {
                    match result {
                        Ok(Some(env)) => {
                            let event = PeerEvent {
                                peer_id: self.peer_id.clone(),
                                envelope: env,
                            };
                            if self.inbound_tx.send(event).await.is_err() {
                                debug!("PeerConnection {}: inbound channel closed", self.peer_id);
                                break;
                            }
                        }
                        Ok(None) => {
                            debug!("PeerConnection {}: EOF", self.peer_id);
                            break;
                        }
                        Err(e) => {
                            warn!("PeerConnection {}: read error: {}", self.peer_id, e);
                            break;
                        }
                    }
                }
                // Write path: PeerCommand -> Envelope -> TCP
                cmd = self.outbound_rx.recv() => {
                    match cmd {
                        Some(cmd) => {
                            if let Err(e) = cmd.envelope.send_async(&mut writer).await {
                                warn!("PeerConnection {}: write error: {}", self.peer_id, e);
                                break;
                            }
                        }
                        None => {
                            debug!("PeerConnection {}: outbound channel closed", self.peer_id);
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Read a single Envelope from TCP
    ///
    /// Returns:
    /// - Ok(Some(env)) on success
    /// - Ok(None) on EOF
    /// - Err on I/O error
    async fn read_envelope(
        reader: &mut (impl AsyncRead + Unpin),
    ) -> Result<Option<Envelope>, Box<dyn std::error::Error + Send + Sync>> {
        match Envelope::receive_async(reader).await {
            Ok(env) => Ok(Some(env)),
            Err(e) => {
                // EOF is expected when peer disconnects
                if e.to_string().contains("UnexpectedEof") || e.to_string().contains("early eof") {
                    Ok(None)
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }
}

