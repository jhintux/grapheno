use btclib::network::Message;
use btclib::sha256::Hash;
use btclib::types::{Block, BlockHeader, Transaction, TransactionOutput};
use btclib::util::MerkleRoot;
use chrono::Utc;
use tokio::net::TcpStream;
use uuid::Uuid;
pub async fn handle_connection(mut socket: TcpStream) {
    loop {
        // read a message from the socket
        let message = match Message::receive_async(&mut socket).await {
            Ok(message) => message,
            Err(e) => {
                println!("invalid message from peer: {e}, closing that connection");
                return;
            }
        };
        use btclib::network::Message::*;
        match message {
            UTXOs(_) | Template(_) | Difference(_) | TemplateValidity(_) | NodeList(_) => {
                println!("I am neither a miner nor a wallet! Goodbye");
                return;
            }
            FetchBlock(height) => {
                let blockchain = crate::BLOCKCHAIN.read().await;
                let Some(block) = blockchain.blocks().nth(height as usize).cloned() else {
                    return;
                };

                let message = NewBlock(block);
                message.send_async(&mut socket).await.unwrap();
            }
            DiscoverNodes => {
                let nodes = crate::NODES
                    .iter()
                    .map(|x| x.key().clone())
                    .collect::<Vec<_>>();
                let message = NodeList(nodes);
                message.send_async(&mut socket).await.unwrap();
            }
            AskDifference(height) => {
                let blockchain = crate::BLOCKCHAIN.read().await;
                let count = blockchain.block_height() as i32 - height as i32;
                let message = Difference(count);
                message.send_async(&mut socket).await.unwrap();
            }
            FetchUTXOs(key) => {
                println!("received request to fetch UTXOs");
                let blockchain = crate::BLOCKCHAIN.read().await;
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
                let mut blockchain = crate::BLOCKCHAIN.write().await;
                println!("received new block: {}", block.hash());
                if blockchain.add_block(block.clone()).is_err() {
                    println!("block rejected: {}", block.hash());
                    return;
                }
            }
            NewTransaction(tx) => {
                let mut blockchain = crate::BLOCKCHAIN.write().await;
                println!("received new transaction: {}", tx.hash());
                if blockchain.add_to_mempool(tx.clone()).is_err() {
                    println!("transaction rejected: {}", tx.hash());
                    return;
                }
            }
            ValidateTemplate(block_template) => {
                let blockchain = crate::BLOCKCHAIN.read().await;
                let status = block_template.header.prev_block_hash
                    == blockchain
                        .blocks()
                        .last()
                        .map(|last_block| last_block.hash())
                        .unwrap_or(Hash::zero());

                let message = TemplateValidity(status);
                message.send_async(&mut socket).await.unwrap();
            }
            SubmitTemplate(block) => {
                println!("received allegedly mined template");
                let mut blockchain = crate::BLOCKCHAIN.write().await;
                if let Err(e) = blockchain.add_block(block.clone()) {
                    println!("block rejected: {e}, closing connection");
                    return;
                }
                blockchain.rebuild_utxos();
                println!("block looks good, broadcasting");

                //send block to all nodes
                let nodes = crate::NODES
                    .iter()
                    .map(|x| x.key().clone())
                    .collect::<Vec<_>>();
                for node in nodes {
                    if let Some(mut stream) = crate::NODES.get_mut(&node) {
                        let message = Message::NewBlock(block.clone());
                        if message.send_async(&mut *stream).await.is_err() {
                            println!("failed to send block to {node}")
                        };
                    }
                }
            }
            SubmitTransaction(tx) => {
                println!("submit tx");
                let mut blockchain = crate::BLOCKCHAIN.write().await;
                if let Err(e) = blockchain.add_to_mempool(tx.clone()) {
                    println!("transaction rejected: {e}, closing connection");
                    return;
                }
                println!("added transaction to mempool");
                // send transaction to all nodes
                let nodes = crate::NODES
                    .iter()
                    .map(|x| x.key().clone())
                    .collect::<Vec<_>>();
                for node in nodes {
                    if let Some(mut stream) = crate::NODES.get_mut(&node) {
                        let message = Message::NewTransaction(tx.clone());
                        if message.send_async(&mut *stream).await.is_err() {
                            println!("failed to send transaction to {node}")
                        };
                    }
                }
                println!("transaction sent to all nodes");
            }
            FetchTemplate(pubkey) => {
                let blockchain = crate::BLOCKCHAIN.read().await;
                let mut transactions = vec![];

                // insert txs from mempool
                transactions.extend(
                    blockchain
                        .mempool()
                        .iter()
                        .take(btclib::BLOCK_TRANSACTION_CAP)
                        .map(|(_, tx)| tx)
                        .cloned()
                        .collect::<Vec<_>>(),
                );

                // insert coinbase tx with pubkey
                transactions.insert(
                    0,
                    Transaction {
                        inputs: vec![],
                        outputs: vec![TransactionOutput {
                            pubkey,
                            value: 0,
                            unique_id: Uuid::new_v4(),
                        }],
                    },
                );

                let merkle_root = MerkleRoot::calculate(&transactions);
                let mut block = Block::new(
                    BlockHeader {
                        timestamp: Utc::now(),
                        nonce: 0,
                        prev_block_hash: blockchain
                            .blocks()
                            .last()
                            .map(|last_block| last_block.hash())
                            .unwrap_or(Hash::zero()),
                        merkle_root,
                        target: blockchain.target(),
                    },
                    transactions,
                );

                let miner_fees = match block.calculate_miner_fees(blockchain.utxos()) {
                    Ok(fees) => fees,
                    Err(e) => {
                        println!("error calculating miner fees: {e}, closing connection");
                        return;
                    }
                };

                // TODO check btclib to prevent calculating merkle tree twice
                let reward = blockchain.calculate_block_reward();
                block.transactions[0].outputs[0].value = reward + miner_fees;

                block.header.merkle_root = MerkleRoot::calculate(&block.transactions);

                let message = Template(block);
                message.send_async(&mut socket).await.unwrap();
            }
        }
    }
}
