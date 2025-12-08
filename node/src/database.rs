use anyhow::{Context, Result};
use btclib::{
    sha256::Hash,
    types::{Block, Transaction, TransactionOutput},
    U256,
};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::sync::Arc;
use std::collections::HashMap;
use ciborium::{ser::into_writer, de::from_reader};
use hex;
use btclib::types::Blockchain;

/// Database keys for different data types
mod keys {
    pub const BLOCK_PREFIX: &str = "block:";
    pub const UTXO_PREFIX: &str = "utxo:";
    pub const MEMPOOL_PREFIX: &str = "mempool:";
    pub const META_TARGET: &str = "meta:target";
    pub const META_BLOCK_COUNT: &str = "meta:block_count";
    pub const META_UTXO_KEYS: &str = "meta:utxo_keys";
    pub const META_MEMPOOL_KEYS: &str = "meta:mempool_keys";
}

/// Wrapper around Sled (LevelDB-like) for blockchain storage
pub struct BlockchainDB {
    db: Arc<sled::Db>,
}

impl BlockchainDB {
    /// Open or create a new database at the given path
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = sled::open(path)
            .context("Failed to open/create database")?;
        Ok(Self {
            db: Arc::new(db),
        })
    }

    /// Store a block at the given index
    pub fn put_block(&self, index: u64, block: &Block) -> Result<()> {
        let key = format!("{}{}", keys::BLOCK_PREFIX, index);
        
        let mut value = Vec::new();
        into_writer(block, &mut value)
            .context("Failed to serialize block")?;
        
        self.db
            .insert(key.as_bytes(), value)
            .context("Failed to write block to database")?;
        Ok(())
    }

    /// Retrieve a block at the given index
    pub fn get_block(&self, index: u64) -> Result<Option<Block>> {
        let key = format!("{}{}", keys::BLOCK_PREFIX, index);
        
        match self.db.get(key.as_bytes()).context("Failed to read block from database")? {
            Some(value) => {
                let block: Block = from_reader(value.as_ref())
                    .context("Failed to deserialize block")?;
                Ok(Some(block))
            }
            None => Ok(None),
        }
    }

    /// Get all blocks in order
    pub fn get_all_blocks(&self) -> Result<Vec<Block>> {
        let mut blocks = Vec::new();
        let mut index = 0u64;
        loop {
            match self.get_block(index)? {
                Some(block) => {
                    blocks.push(block);
                    index += 1;
                }
                None => break,
            }
        }
        Ok(blocks)
    }

    /// Store a UTXO
    pub fn put_utxo(&self, hash: &Hash, marked: bool, output: &TransactionOutput) -> Result<()> {
        let hash_bytes = hash.as_bytes();
        let hash_hex = hex::encode(hash_bytes);
        let key = format!("{}{}", keys::UTXO_PREFIX, hash_hex);
        
        let mut value = Vec::new();
        into_writer(&(marked, output), &mut value)
            .context("Failed to serialize UTXO")?;
        
        self.db
            .insert(key.as_bytes(), value)
            .context("Failed to write UTXO to database")?;
        
        // Update UTXO keys list
        let mut utxo_keys = self.get_utxo_keys()?.unwrap_or_default();
        if !utxo_keys.contains(hash) {
            utxo_keys.push(*hash);
            self.put_utxo_keys(&utxo_keys)?;
        }
        
        Ok(())
    }

    /// Retrieve a UTXO
    pub fn get_utxo(&self, hash: &Hash) -> Result<Option<(bool, TransactionOutput)>> {
        let hash_bytes = hash.as_bytes();
        let hash_hex = hex::encode(hash_bytes);
        let key = format!("{}{}", keys::UTXO_PREFIX, hash_hex);
        
        match self.db.get(key.as_bytes()).context("Failed to read UTXO from database")? {
            Some(value) => {
                let utxo: (bool, TransactionOutput) = from_reader(value.as_ref())
                    .context("Failed to deserialize UTXO")?;
                Ok(Some(utxo))
            }
            None => Ok(None),
        }
    }

    /// Delete a UTXO
    pub fn delete_utxo(&self, hash: &Hash) -> Result<()> {
        let hash_bytes = hash.as_bytes();
        let hash_hex = hex::encode(hash_bytes);
        let key = format!("{}{}", keys::UTXO_PREFIX, hash_hex);
        
        self.db
            .remove(key.as_bytes())
            .context("Failed to delete UTXO from database")?;
        
        // Update UTXO keys list
        if let Some(mut utxo_keys) = self.get_utxo_keys()? {
            utxo_keys.retain(|h| h != hash);
            self.put_utxo_keys(&utxo_keys)?;
        }
        
        Ok(())
    }

    /// Get all UTXOs
    pub fn get_all_utxos(&self) -> Result<HashMap<Hash, (bool, TransactionOutput)>> {
        let mut utxos = HashMap::new();
        
        let utxo_keys = self.get_utxo_keys()?.unwrap_or_default();
        for hash in utxo_keys {
            if let Some(utxo) = self.get_utxo(&hash)? {
                utxos.insert(hash, utxo);
            }
        }
        
        Ok(utxos)
    }

    /// Store a mempool transaction
    pub fn put_mempool_tx(&self, tx_hash: &Hash, timestamp: DateTime<Utc>, tx: &Transaction) -> Result<()> {
        let hash_bytes = tx_hash.as_bytes();
        let hash_hex = hex::encode(hash_bytes);
        let key = format!("{}{}", keys::MEMPOOL_PREFIX, hash_hex);
        
        let mut value = Vec::new();
        into_writer(&(timestamp, tx), &mut value)
            .context("Failed to serialize mempool transaction")?;
        
        self.db
            .insert(key.as_bytes(), value)
            .context("Failed to write mempool transaction to database")?;
        
        // Update mempool keys list
        let mut mempool_keys = self.get_mempool_keys()?.unwrap_or_default();
        if !mempool_keys.contains(tx_hash) {
            mempool_keys.push(*tx_hash);
            self.put_mempool_keys(&mempool_keys)?;
        }
        
        Ok(())
    }

    /// Retrieve a mempool transaction
    pub fn get_mempool_tx(&self, tx_hash: &Hash) -> Result<Option<(DateTime<Utc>, Transaction)>> {
        let hash_bytes = tx_hash.as_bytes();
        let hash_hex = hex::encode(hash_bytes);
        let key = format!("{}{}", keys::MEMPOOL_PREFIX, hash_hex);
        
        match self.db.get(key.as_bytes()).context("Failed to read mempool transaction from database")? {
            Some(value) => {
                let mempool_tx: (DateTime<Utc>, Transaction) = ciborium::de::from_reader(value.as_ref())
                    .context("Failed to deserialize mempool transaction")?;
                Ok(Some(mempool_tx))
            }
            None => Ok(None),
        }
    }

    /// Delete a mempool transaction
    pub fn delete_mempool_tx(&self, tx_hash: &Hash) -> Result<()> {
        let hash_bytes = tx_hash.as_bytes();
        let hash_hex = hex::encode(hash_bytes);
        let key = format!("{}{}", keys::MEMPOOL_PREFIX, hash_hex);
        
        self.db
            .remove(key.as_bytes())
            .context("Failed to delete mempool transaction from database")?;
        
        // Update mempool keys list
        if let Some(mut mempool_keys) = self.get_mempool_keys()? {
            mempool_keys.retain(|h| h != tx_hash);
            self.put_mempool_keys(&mempool_keys)?;
        }
        
        Ok(())
    }

    /// Get all mempool transactions
    pub fn get_all_mempool_txs(&self) -> Result<Vec<(DateTime<Utc>, Transaction)>> {
        let mut mempool = Vec::new();
        
        let mempool_keys = self.get_mempool_keys()?.unwrap_or_default();
        for tx_hash in mempool_keys {
            if let Some(tx) = self.get_mempool_tx(&tx_hash)? {
                mempool.push(tx);
            }
        }
        
        Ok(mempool)
    }

    /// Store the target value
    pub fn put_target(&self, target: U256) -> Result<()> {
        let mut value = Vec::new();
        into_writer(&target, &mut value)
            .context("Failed to serialize target")?;
        
        self.db
            .insert(keys::META_TARGET.as_bytes(), value)
            .context("Failed to write target to database")?;
        Ok(())
    }

    /// Retrieve the target value
    pub fn get_target(&self) -> Result<Option<U256>> {
        match self.db.get(keys::META_TARGET.as_bytes()).context("Failed to read target from database")? {
            Some(value) => {
                let target: U256 = from_reader(value.as_ref())
                    .context("Failed to deserialize target")?;
                Ok(Some(target))
            }
            None => Ok(None),
        }
    }

    /// Store the block count
    pub fn put_block_count(&self, count: u64) -> Result<()> {
        let value = count.to_be_bytes().to_vec();
        
        self.db
            .insert(keys::META_BLOCK_COUNT.as_bytes(), value)
            .context("Failed to write block count to database")?;
        Ok(())
    }

    /// Retrieve the block count
    pub fn get_block_count(&self) -> Result<Option<u64>> {
        match self.db.get(keys::META_BLOCK_COUNT.as_bytes()).context("Failed to read block count from database")? {
            Some(value) => {
                let mut bytes = [0u8; 8];
                if value.len() >= 8 {
                    bytes.copy_from_slice(&value[..8]);
                    Ok(Some(u64::from_be_bytes(bytes)))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Store UTXO keys list
    fn put_utxo_keys(&self, keys: &[Hash]) -> Result<()> {
        let mut value = Vec::new();
        into_writer(keys, &mut value)
            .context("Failed to serialize UTXO keys")?;
        
        self.db
            .insert(keys::META_UTXO_KEYS.as_bytes(), value)
            .context("Failed to write UTXO keys to database")?;
        Ok(())
    }

    /// Get UTXO keys list
    fn get_utxo_keys(&self) -> Result<Option<Vec<Hash>>> {
        match self.db.get(keys::META_UTXO_KEYS.as_bytes()).context("Failed to read UTXO keys from database")? {
            Some(value) => {
                let keys: Vec<Hash> = ciborium::de::from_reader(value.as_ref())
                    .context("Failed to deserialize UTXO keys")?;
                Ok(Some(keys))
            }
            None => Ok(None),
        }
    }

    /// Store mempool keys list
    fn put_mempool_keys(&self, keys: &[Hash]) -> Result<()> {
        let mut value = Vec::new();
        into_writer(keys, &mut value)
            .context("Failed to serialize mempool keys")?;
        
        self.db
            .insert(keys::META_MEMPOOL_KEYS.as_bytes(), value)
            .context("Failed to write mempool keys to database")?;
        Ok(())
    }

    /// Get mempool keys list
    fn get_mempool_keys(&self) -> Result<Option<Vec<Hash>>> {
        match self.db.get(keys::META_MEMPOOL_KEYS.as_bytes()).context("Failed to read mempool keys from database")? {
            Some(value) => {
                let keys: Vec<Hash> = from_reader(value.as_ref())
                    .context("Failed to deserialize mempool keys")?;
                Ok(Some(keys))
            }
            None => Ok(None),
        }
    }

    /// Clear all mempool transactions (for cleanup)
    pub fn clear_mempool(&self) -> Result<()> {
        let mempool_keys = self.get_mempool_keys()?.unwrap_or_default();
        for tx_hash in mempool_keys {
            self.delete_mempool_tx(&tx_hash)?;
        }
        Ok(())
    }

    /// Load the entire blockchain from the database
    pub fn load_blockchain(&self) -> Result<Blockchain> {
        
        let blocks = self.get_all_blocks()?;
        let mempool = self.get_all_mempool_txs()?;
        
        // Create a new blockchain
        let mut blockchain = Blockchain::new();
        
        // Add all blocks one by one - this will rebuild UTXOs and adjust target
        for block in blocks {
            blockchain.add_block(block)
                .context("Failed to add block when loading from database")?;
        }
        
        // Restore mempool transactions
        // Note: We need to add them in order to maintain the same order as when saved
        for (_, tx) in mempool {
            // Use add_to_mempool which will validate and add the transaction
            // If it fails (e.g., UTXO no longer exists), we'll skip it
            blockchain.add_to_mempool(tx).ok();
        }
        
        Ok(blockchain)
    }

    /// Save the entire blockchain to the database
    pub fn save_blockchain(&self, blockchain: &Blockchain) -> Result<()> {
        // Save all blocks
        for (index, block) in blockchain.blocks().enumerate() {
            self.put_block(index as u64, block)?;
        }
        
        // Save block count
        self.put_block_count(blockchain.block_height())?;
        
        // Save target
        self.put_target(blockchain.target())?;
        
        // Save all UTXOs
        // First, clear existing UTXO keys to rebuild from scratch
        if let Some(old_keys) = self.get_utxo_keys()? {
            for hash in old_keys {
                self.delete_utxo(&hash)?;
            }
        }
        
        // Store new UTXO keys list
        let utxo_hashes: Vec<Hash> = blockchain.utxos().keys().copied().collect();
        self.put_utxo_keys(&utxo_hashes)?;
        
        // Save each UTXO (skip key list update since we just set it)
        for (hash, (marked, output)) in blockchain.utxos() {
            let hash_bytes = hash.as_bytes();
            let hash_hex = hex::encode(hash_bytes);
            let key = format!("{}{}", keys::UTXO_PREFIX, hash_hex);
            let mut value = Vec::new();
            into_writer(&(marked, output), &mut value)
                .context("Failed to serialize UTXO")?;
            self.db.insert(key.as_bytes(), value)
                .context("Failed to write UTXO to database")?;
        }
        
        // Save all mempool transactions
        // First, clear existing mempool
        if let Some(old_keys) = self.get_mempool_keys()? {
            for tx_hash in old_keys {
                self.delete_mempool_tx(&tx_hash)?;
            }
        }
        
        // Store new mempool keys list
        let mempool_hashes: Vec<Hash> = blockchain.mempool().iter()
            .map(|(_, tx)| tx.hash())
            .collect();
        self.put_mempool_keys(&mempool_hashes)?;
        
        // Save each mempool transaction
        for (timestamp, tx) in blockchain.mempool() {
            let tx_hash = tx.hash();
            self.put_mempool_tx(&tx_hash, *timestamp, tx)?;
        }
        
        Ok(())
    }
}
