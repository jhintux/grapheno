use super::{Block, Transaction, TransactionOutput};
use crate::{
    U256,
    error::{BtcError, Result},
    sha256::Hash,
    util::MerkleRoot,
};
use crate::util::Saveable;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use bigdecimal::BigDecimal;
use std::io::{Read, Write, Result as IoResult, Error as IoError, ErrorKind as IoErrorKind};
use tracing::{instrument, warn, error};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Blockchain {
    utxos: HashMap<Hash, (bool, TransactionOutput)>,
    target: U256,
    blocks: Vec<Block>,
    #[serde(default, skip_deserializing)]
    pub mempool: Vec<(DateTime<Utc>, Transaction)>,
}

impl Blockchain {
    pub fn new() -> Self {
        Self {
            utxos: HashMap::new(),
            target: crate::MIN_TARGET,
            blocks: vec![],
            mempool: vec![],
        }
    }

    // utxos
    pub fn utxos(&self) -> &HashMap<Hash, (bool, TransactionOutput)> {
        &self.utxos
    }
    // target
    pub fn target(&self) -> U256 {
        self.target
    }
    // blocks
    pub fn blocks(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter()
    }

    pub fn block_height(&self) -> u64 {
        self.blocks.len() as u64
    }

    pub fn mempool(&self) -> &[(DateTime<Utc>, Transaction)] {
        &self.mempool
    }

    #[instrument(skip(self, block))]
    pub fn add_block(&mut self, block: Block) -> Result<()> {
        if self.blocks.is_empty() {
            // Genesis block validation
            if block.header.prev_block_hash != Hash::zero() {
                warn!("Genesis block must have a hash of 0");
                return Err(BtcError::InvalidBlock);
            }
        } else {
            let last_block = self.blocks.last().unwrap();
            if block.header.prev_block_hash != last_block.hash() {
                warn!("Previous block hash does not match the last block hash");
                return Err(BtcError::InvalidBlock);
            }

            if !block.header.hash().matches_target(block.header.target) {
                warn!("Block hash does not match the target");
                return Err(BtcError::InvalidBlock);
            }

            let calculated_merkle_root = MerkleRoot::calculate(&block.transactions);
            if calculated_merkle_root != block.header.merkle_root {
                warn!("Calculated merkle root does not match the block header merkle root");
                return Err(BtcError::InvalidMerkleRoot);
            }

            if block.header.timestamp <= last_block.header.timestamp {
                warn!("Timestamp is not greater than the last block timestamp");
                return Err(BtcError::InvalidBlock);
            }

            block
                .verify_transactions(self.block_height(), &self.utxos)
                .map_err(|e| {
                    error!("Transaction verification failed: {:?}", e);
                    e
                })?;
        }

        let block_transactions: HashSet<_> =
            block.transactions.iter().map(|tx| tx.hash()).collect();

        self.mempool
            .retain(|(_, tx)| !block_transactions.contains(&tx.hash()));
        self.blocks.push(block);
        self.try_adjust_target();

        Ok(())
    }

    #[instrument(skip(self))]
    pub fn rebuild_utxos(&mut self) {
        for block in &self.blocks {
            for transaction in &block.transactions {
                for input in &transaction.inputs {
                    self.utxos.remove(&input.prev_transaction_output_hash);
                }

                for output in &transaction.outputs {
                    self.utxos
                        .insert(transaction.hash(), (false, output.clone()));
                }
            }
        }
    }

    #[instrument(skip(self))]
    pub fn try_adjust_target(&mut self) {
        if self.blocks.is_empty() {
            return;
        }

        if self.block_height() % crate::DIFFICULTY_UPDATE_INTERVAL != 0 {
            return;
        }

        // measure the time it took to mine the last crate::DIFFICULTY_UPDATE_INTERVAL blocks with chrono
        let start_time = self.blocks
            [(self.block_height() - crate::DIFFICULTY_UPDATE_INTERVAL) as usize]
            .header
            .timestamp;
        let end_time = self.blocks.last().unwrap().header.timestamp;
        let time_diff = end_time - start_time;
        // convert time_diff to seconds
        let time_diff_seconds = time_diff.num_seconds();
        // calculate the ideal number of seconds
        let target_seconds = crate::IDEAL_BLOCK_TIME * crate::DIFFICULTY_UPDATE_INTERVAL;
        // multiply the current target by actual time divided by ideal time
        let new_target = BigDecimal::parse_bytes(&self.target.to_string().as_bytes(), 10)
            .expect("BUG: impossible")
            * (BigDecimal::from(time_diff_seconds) / BigDecimal::from(target_seconds));
        let new_target_str = new_target
            .to_string()
            .split('.')
            .next()
            .expect("BUG: Expected a decimal point")
            .to_owned();
        let new_target: U256 = U256::from_str_radix(&new_target_str, 10).expect("BUG: impossible");
        // clamp new_target to be within the range of 4 * self.target and self.target / 4
        let new_target = if new_target < self.target / 4 {
            self.target / 4
        } else if new_target > self.target * 4 {
            self.target * 4
        } else {
            new_target
        };

        // if the new target is more than the minimum target, set it to the minimum target
        self.target = new_target.min(crate::MIN_TARGET);
    }

    #[instrument(skip(self, transaction))]
    pub fn add_to_mempool(&mut self, transaction: Transaction) -> Result<()> {
        let mut known_inputs = HashSet::new();

        for input in &transaction.inputs {
            if !self.utxos.contains_key(&input.prev_transaction_output_hash) {
                return Err(BtcError::InvalidTransaction);
            }
            if known_inputs.contains(&input.prev_transaction_output_hash) {
                return Err(BtcError::InvalidTransaction);
            }
            known_inputs.insert(input.prev_transaction_output_hash);
        }

        // TODO check if we can merge into one for loop
        // TODO Note that we are making a bit of a simplification. In two conflicting transactions, real Bitcoin would remove the one with the smaller fee (which is great for ergonomics - you can’t get your transaction through fast enough -> you increase the fee).
        for input in &transaction.inputs {
            if let Some((true, _)) = self.utxos.get(&input.prev_transaction_output_hash) {
                // find the transaction that references the utxo we are trying to reference
                let referencing_transaction =
                    self.mempool
                        .iter()
                        .enumerate()
                        .find(|(_, (_, transaction))| {
                            transaction
                                .outputs
                                .iter()
                                .any(|output| output.hash() == input.prev_transaction_output_hash)
                        });

                // if we have found on, unmark all of its utxos
                if let Some((idx, (_, referencing_transaction))) = referencing_transaction {
                    for input in &referencing_transaction.inputs {
                        // set all utxos from this transaction to false
                        self.utxos
                            .entry(input.prev_transaction_output_hash)
                            .and_modify(|(marked, _)| {
                                *marked = false;
                            });
                        // remove the transaction from the mempool
                    }
                    self.mempool.remove(idx);
                } else {
                    // if, somehow, there's no matching tx. set this utxo to false
                    self.utxos
                        .entry(input.prev_transaction_output_hash)
                        .and_modify(|(marked, _)| {
                            *marked = false;
                        });
                }
            }
        }

        // all inputs must be lower than all outputs
        let all_inputs = transaction
            .inputs
            .iter()
            .map(|input| {
                self.utxos
                    .get(&input.prev_transaction_output_hash)
                    .expect("BUG: impossible")
                    .1
                    .value
            })
            .sum::<u64>();
        let all_outputs = transaction.outputs.iter().map(|output| output.value).sum();
        if all_inputs < all_outputs {
            return Err(BtcError::InvalidTransaction);
        }

        // mark the utxos as used
        for input in &transaction.inputs {
            self.utxos
                .entry(input.prev_transaction_output_hash)
                .and_modify(|(marked, _)| *marked = true);
        }

        self.mempool.push((Utc::now(), transaction));
        // sort by miner fee
        self.mempool.sort_by_key(|(_, transaction)| {
            let all_inputs = transaction
                .inputs
                .iter()
                .map(|input| {
                    self.utxos
                        .get(&input.prev_transaction_output_hash)
                        .expect("BUG: impossible")
                        .1
                        .value
                })
                .sum::<u64>();
            let all_outputs: u64 = transaction.outputs.iter().map(|output| output.value).sum();
            let miner_fee = all_inputs - all_outputs;
            miner_fee
        });

        Ok(())
    }

    // Cleanup mempool - remove transactions older than
    // MAX_MEMPOOL_TRANSACTION_AGE
    #[instrument(skip(self))]
    pub fn cleanup_mempool(&mut self) {
        let now = Utc::now();
        let mut utxo_hashes_to_unmark: Vec<Hash> = vec![];
        self.mempool.retain(|(timestamp, transaction)| {
            if now - *timestamp
                > chrono::Duration::seconds(crate::MAX_MEMPOOL_TRANSACTION_AGE as i64)
            {
                // push all utxos to unmark to the vector
                // so we can unmark them later
                utxo_hashes_to_unmark.extend(
                    transaction
                        .inputs
                        .iter()
                        .map(|input| input.prev_transaction_output_hash),
                );
                false
            } else {
                true
            }
        });
        // unmark all of the UTXOs
        for hash in utxo_hashes_to_unmark {
            self.utxos.entry(hash).and_modify(|(marked, _)| {
                *marked = false;
            });
        }
    }

    #[instrument(skip(self))]
    pub fn calculate_block_reward(&self) -> u64 {
        let block_height = self.block_height();
        let halvings = block_height / crate::HALVING_INTERVAL;

        (crate::INITIAL_REWARD * 10u64.pow(8)) >> halvings
    }
}

impl Saveable for Blockchain {
    fn load<I: Read>(reader: I) -> IoResult<Self> {
        ciborium::de::from_reader(reader).map_err(|_| {
            IoError::new(IoErrorKind::InvalidData, "Failed to deserialize blockchain")
        })
    }

    fn save<O: Write>(&self, writer: O) -> IoResult<()> {
        ciborium::ser::into_writer(self, writer).map_err(|_| {
            IoError::new(IoErrorKind::InvalidData, "Failed to serialize blockchain")
        })
    }
}