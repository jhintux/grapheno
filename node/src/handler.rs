use crate::context::NodeContext;
use btclib::network::Message;
use btclib::sha256::Hash;
use btclib::types::{Block, BlockHeader, Blockchain, Transaction, TransactionOutput};
use btclib::util::MerkleRoot;
use chrono::Utc;
use tokio::net::TcpStream;
use tracing::{info, debug, warn, error};
use uuid::Uuid;

fn get_last_block_hash(blockchain: &Blockchain) -> Hash {
    blockchain
        .blocks()
        .last()
        .map(|last_block| last_block.hash())
        .unwrap_or(Hash::zero())
}

async fn broadcast_to_nodes<F>(ctx: &NodeContext, message_factory: F)
where
    F: Fn() -> Message,
{
    let nodes: Vec<_> = ctx.nodes.iter().map(|x| x.key().clone()).collect();
    for node in nodes {
        if let Some(mut stream) = ctx.nodes.get_mut(&node) {
            let message = message_factory();
            if message.send_async(&mut *stream).await.is_err() {
                println!("failed to send message to {node}")
            }
        }
    }
}

pub async fn handle_connection(ctx: NodeContext, mut socket: TcpStream) {
    loop {
        // read a message from the socket
        let message = match Message::receive_async(&mut socket).await {
            Ok(message) => message,
            Err(e) => {
                warn!("invalid message from peer: {e}, closing that connection");
                return;
            }
        };
        use btclib::network::Message::*;
        match message {
            UTXOs(_) | Template(_) | Difference(_) | TemplateValidity(_) | NodeList(_)
            | AllBlocks(_) => {
                println!("I am neither a miner nor a wallet! Goodbye");
                return;
            }
            FetchBlock(height) => {
                let blockchain = ctx.blockchain.read().await;
                let Some(block) = blockchain.blocks().nth(height as usize).cloned() else {
                    return;
                };

                let message = NewBlock(block);
                message.send_async(&mut socket).await.unwrap();
            }
            FetchAllBlocks => {
                let blockchain = ctx.blockchain.read().await;
                let blocks: Vec<Block> = blockchain.blocks().cloned().collect();
                let message = AllBlocks(blocks);
                message.send_async(&mut socket).await.unwrap();
            }
            DiscoverNodes => {
                let nodes = ctx
                    .nodes
                    .iter()
                    .map(|x| x.key().clone())
                    .collect::<Vec<_>>();
                let message = NodeList(nodes);
                message.send_async(&mut socket).await.unwrap();
            }
            AskDifference(height) => {
                let blockchain = ctx.blockchain.read().await;
                let count = blockchain.block_height() as i32 - height as i32;
                let message = Difference(count);
                message.send_async(&mut socket).await.unwrap();
            }
            FetchUTXOs(key) => {
                debug!("received request to fetch UTXOs");
                let blockchain = ctx.blockchain.read().await;
                let utxos = blockchain
                    .utxos()
                    .iter()
                    .filter(|(_, (_, txout))| txout.pubkey == key)
                    .map(|(_, (marked, txout))| (txout.clone(), *marked))
                    .collect::<Vec<_>>();
                let message = UTXOs(utxos);
                message.send_async(&mut socket).await.unwrap();
            }
            // TODO send back new blocks and txs to other nodes preventing the network from creating notifications loops
            NewBlock(block) => {
                let mut blockchain = ctx.blockchain.write().await;
                info!("received new block: {}", block.hash());
                if blockchain.add_block(block.clone()).is_err() {
                    warn!("block rejected: {}", block.hash());
                    return;
                }
            }
            NewTransaction(tx) => {
                let mut blockchain = ctx.blockchain.write().await;
                info!("received new transaction: {}", tx.hash());
                if blockchain.add_to_mempool(tx.clone()).is_err() {
                    warn!("transaction rejected: {}", tx.hash());
                    return;
                }
            }
            ValidateTemplate(block_template) => {
                let blockchain = ctx.blockchain.read().await;
                let status = block_template.header.prev_block_hash == get_last_block_hash(&blockchain);

                let message = TemplateValidity(status);
                message.send_async(&mut socket).await.unwrap();
            }
            SubmitTemplate(block) => {
                info!("received allegedly mined template");
                let mut blockchain = ctx.blockchain.write().await;
                if let Err(e) = blockchain.add_block(block.clone()) {
                    warn!("block rejected: {e}, closing connection");
                    return;
                }
                blockchain.rebuild_utxos();
                info!("block looks good, broadcasting");

                let block_clone = block.clone();
                broadcast_to_nodes(&ctx, || Message::NewBlock(block_clone.clone())).await;
            }
            SubmitTransaction(tx) => {
                debug!("submit tx");
                let mut blockchain = ctx.blockchain.write().await;
                if let Err(e) = blockchain.add_to_mempool(tx.clone()) {
                    warn!("transaction rejected: {e}, closing connection");
                    return;
                }
                println!("added transaction to mempool");
                let tx_clone = tx.clone();
                broadcast_to_nodes(&ctx, || Message::NewTransaction(tx_clone.clone())).await;
                println!("transaction sent to all nodes");
            }
            FetchTemplate(pubkey) => {
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
                        pubkey,
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
                        return;
                    }
                };

                let reward = blockchain.calculate_block_reward();
                block.transactions[0].outputs[0].value = reward + miner_fees;

                // Calculate merkle root once after coinbase value is finalized
                block.header.merkle_root = MerkleRoot::calculate(&block.transactions);

                let message = Template(block);
                message.send_async(&mut socket).await.unwrap();
            }
        }
    }
}
