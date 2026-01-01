use crate::context::NodeContext;
use crate::network::{PeerHandle, PeerId};
use anyhow::Result;
use btclib::network::{Envelope, Message};
use btclib::sha256::Hash;
use btclib::types::{Block, BlockHeader, Blockchain, Transaction, TransactionOutput};
use btclib::util::MerkleRoot;
use chrono::Utc;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use std::net::SocketAddr;

const DEFAULT_TTL: u8 = 8;
const OUTBOUND_BUFFER: usize = 256;

fn get_last_block_hash(blockchain: &Blockchain) -> Hash {
    blockchain
        .blocks()
        .last()
        .map(|last_block| last_block.hash())
        .unwrap_or(Hash::zero())
}

pub async fn accept_peer(
    ctx: NodeContext,
    socket: TcpStream,
    peer_addr: SocketAddr,
) -> Result<()> {
    let peer_id = peer_addr.to_string();
    let (mut rd, mut wr) = socket.into_split();

    let (out_tx, mut out_rx) = mpsc::channel::<Envelope>(OUTBOUND_BUFFER);
    ctx.network
        .peers
        .insert(peer_id.clone(), PeerHandle { outbound: out_tx });

    let writer = tokio::spawn(async move {
        while let Some(env) = out_rx.recv().await {
            if env.send_async(&mut wr).await.is_err() {
                break;
            }
        }
    });

    let network = ctx.network.clone();
    let reader = tokio::spawn(async move {
        loop {
            match Envelope::receive_async(&mut rd).await {
                Ok(env) => {
                    // // if inbound is full, this will await: backpressure by design
                    if network.inbound_tx.send((peer_id.clone(), env)).await.is_err() {
                        break;
                    }
                },
                Err(_) => break,
            }
        }
    });

    // detach; cleanup could be improved later
    let _ = (writer, reader);
    Ok(())
}

pub async fn dispatcher_loop(ctx: NodeContext) -> Result<()> {
    loop {
        let (from_peer, mut env) = match ctx.network.next_inbound().await {
            Some(x) => x,
            None => return Ok(()),
        };

        if env.origin == ctx.network.self_id {
            continue;
        }

        if !ctx.network.track_if_new(env.id).await {
            continue;
        }

        let mut should_gossip = false;

        match &env.msg {
            Message::UTXOs(_)
            | Message::Template(_)
            | Message::Difference(_)
            | Message::TemplateValidity(_)
            | Message::NodeList(_)
            | Message::AllBlocks(_) => {
                info!("unexpected inbound response for node role, ignoring");
            }
            Message::FetchBlock(height) => {
                let blockchain = ctx.blockchain.read().await;
                if let Some(block) = blockchain.blocks().nth(*height as usize).cloned() {
                    let reply = Envelope::new(
                        ctx.network.self_id.clone(),
                        DEFAULT_TTL,
                        Message::NewBlock(block),
                    );
                    ctx.network.send_to(&from_peer, reply).await;
                }
            }
            Message::FetchAllBlocks => {
                let blockchain = ctx.blockchain.read().await;
                let blocks: Vec<Block> = blockchain.blocks().cloned().collect();
                let reply = Envelope::new(
                    ctx.network.self_id.clone(),
                    DEFAULT_TTL,
                    Message::AllBlocks(blocks),
                );
                ctx.network.send_to(&from_peer, reply).await;
            }
            Message::DiscoverNodes => {
                let nodes = ctx.network.peer_ids();
                let reply = Envelope::new(
                    ctx.network.self_id.clone(),
                    DEFAULT_TTL,
                    Message::NodeList(nodes),
                );
                ctx.network.send_to(&from_peer, reply).await;
            }
            Message::AskDifference(height) => {
                let blockchain = ctx.blockchain.read().await;
                let count = blockchain.block_height() as i32 - *height as i32;
                let reply = Envelope::new(
                    ctx.network.self_id.clone(),
                    DEFAULT_TTL,
                    Message::Difference(count),
                );
                ctx.network.send_to(&from_peer, reply).await;
            }
            Message::FetchUTXOs(key) => {
                debug!("received request to fetch UTXOs");
                let blockchain = ctx.blockchain.read().await;
                let utxos = blockchain
                    .utxos()
                    .iter()
                    .filter(|(_, (_, txout))| txout.pubkey == *key)
                    .map(|(_, (marked, txout))| (txout.clone(), *marked))
                    .collect::<Vec<_>>();
                let reply = Envelope::new(
                    ctx.network.self_id.clone(),
                    DEFAULT_TTL,
                    Message::UTXOs(utxos),
                );
                ctx.network.send_to(&from_peer, reply).await;
            }
            Message::NewBlock(block) => {
                let hash = block.hash();
                let mut blockchain = ctx.blockchain.write().await;
                info!("received new block: {}", hash);
                if blockchain.add_block(block.clone()).is_err() {
                    warn!("block rejected: {} (nodes may be out of sync)", hash);
                } else {
                    should_gossip = true;
                }
            }
            Message::NewTransaction(tx) => {
                let hash = tx.hash();
                let mut blockchain = ctx.blockchain.write().await;
                info!("received new transaction: {}", hash);
                if blockchain.add_to_mempool(tx.clone()).is_err() {
                    warn!("transaction rejected: {} (nodes may be out of sync)", hash);
                } else {
                    should_gossip = true;
                }
            }
            Message::ValidateTemplate(block_template) => {
                let blockchain = ctx.blockchain.read().await;
                let status =
                    block_template.header.prev_block_hash == get_last_block_hash(&blockchain);
                let reply = Envelope::new(
                    ctx.network.self_id.clone(),
                    DEFAULT_TTL,
                    Message::TemplateValidity(status),
                );
                ctx.network.send_to(&from_peer, reply).await;
            }
            Message::SubmitTemplate(block) => {
                info!("received allegedly mined template");
                let mut blockchain = ctx.blockchain.write().await;
                if let Err(e) = blockchain.add_block(block.clone()) {
                    warn!("block rejected: {e}, closing connection");
                    continue;
                }
                blockchain.rebuild_utxos();
                info!("block looks good, broadcasting");
                let gossip = Envelope::new(
                    ctx.network.self_id.clone(),
                    DEFAULT_TTL,
                    Message::NewBlock(block.clone()),
                );
                broadcast_except(&ctx, Some(&from_peer), gossip).await;
            }
            Message::SubmitTransaction(tx) => {
                debug!("submit tx");
                let mut blockchain = ctx.blockchain.write().await;
                if let Err(e) = blockchain.add_to_mempool(tx.clone()) {
                    warn!("transaction rejected: {e}, closing connection");
                    continue;
                }
                info!("added transaction to mempool");
                let gossip = Envelope::new(
                    ctx.network.self_id.clone(),
                    DEFAULT_TTL,
                    Message::NewTransaction(tx.clone()),
                );
                broadcast_except(&ctx, Some(&from_peer), gossip).await;
                info!("transaction sent to all nodes");
            }
            Message::FetchTemplate(pubkey) => {
                let blockchain = ctx.blockchain.read().await;

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
                        address: *pubkey,
                        value: 0,
                        unique_id: Uuid::new_v4(),
                    }],
                };
                transactions.insert(0, coinbase);

                // Create block with placeholder merkle root (will be calculated after coinbase value is set)
                let prev_block_hash = get_last_block_hash(&blockchain);
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
                        error!("error calculating miner fees: {e}, closing connection");
                        continue;
                    }
                };

                let reward = blockchain.calculate_block_reward();
                block.transactions[0].outputs[0].value = reward + miner_fees;

                // Calculate merkle root once after coinbase value is finalized
                block.header.merkle_root = MerkleRoot::calculate(&block.transactions);

                let reply = Envelope::new(
                    ctx.network.self_id.clone(),
                    DEFAULT_TTL,
                    Message::Template(block),
                );
                ctx.network.send_to(&from_peer, reply).await;
            }
        }

        if should_gossip && env.ttl > 0 {
            env.ttl -= 1;
            broadcast_except(&ctx, Some(&from_peer), env).await;
        }
    }
}

async fn broadcast_except(ctx: &NodeContext, except: Option<&PeerId>, env: Envelope) {
    for item in ctx.network.peers.iter() {
        let peer_id = item.key();
        if except.is_some_and(|e| e == peer_id) {
            continue;
        }
        let _ = item.value().outbound.try_send(env.clone());
    }
}