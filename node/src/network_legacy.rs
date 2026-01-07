use btclib::network::Envelope;
use dashmap::DashMap;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, warn};
use uuid::Uuid;

pub type PeerId = String;

pub struct PeerHandle {
    pub outbound: mpsc::Sender<Envelope>,
}

pub struct NetworkHub {
    pub self_id: PeerId,
    pub peers: DashMap<PeerId, PeerHandle>,
    pub inbound_tx: mpsc::Sender<(PeerId, Envelope)>,
    pub inbound_rx: tokio::sync::Mutex<mpsc::Receiver<(PeerId, Envelope)>>,
    pub seen: tokio::sync::Mutex<LruCache<Uuid, ()>>,
}

const INBOUND_BUFFER: usize = 128;
const SEEN_CAPACITY: usize = 4096;

impl NetworkHub {
    pub fn new(self_id: PeerId) -> Arc<Self> {
        let (inbound_tx, inbound_rx) = mpsc::channel(INBOUND_BUFFER);
        let seen_capacity = NonZeroUsize::new(SEEN_CAPACITY).expect("non-zero LRU size");
        Arc::new(Self {
            self_id,
            peers: DashMap::new(),
            inbound_tx,
            inbound_rx: Mutex::new(inbound_rx),
            seen: Mutex::new(LruCache::new(seen_capacity)),
        })
    }

    pub async fn next_inbound(&self) -> Option<(PeerId, Envelope)> {
        self.inbound_rx.lock().await.recv().await
    }

    pub async fn send_to(&self, peer_id: &str, env: Envelope) {
        if let Some(entry) = self.peers.get(peer_id) {
            if let Err(err) = entry.value().outbound.send(env).await {
                warn!("failed to send to {peer_id}: {err}");
            }
        } else {
            debug!("peer {peer_id} not found for send");
        }
    }

    pub fn peer_ids(&self) -> Vec<String> {
        self.peers.iter().map(|p| p.key().clone()).collect()
    }

    /// Returns true if the id was not seen before.
    pub async fn track_if_new(&self, id: Uuid) -> bool {
        let mut seen = self.seen.lock().await;
        if seen.contains(&id) {
            false
        } else {
            seen.put(id, ());
            true
        }
    }
}

