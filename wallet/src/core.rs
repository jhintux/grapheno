use anyhow::{Result, anyhow};
use btclib::crypto::{PrivateKey, PublicKey, Signature};
use btclib::network::Message;
use btclib::types::{Transaction, TransactionInput, TransactionOutput};
use btclib::util::Saveable;
use crossbeam_skiplist::SkipMap;
use kanal::Sender;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, oneshot};
use tokio::io::AsyncReadExt;
use tracing::*;
use uuid::Uuid;

/// Represent a key pair with paths to public and private keys
#[derive(Serialize, Deserialize, Clone)]
pub struct Key {
    pub public: PathBuf,
    pub private: PathBuf,
}

/// Represent a loaded key pair with actual public and private keys
#[derive(Clone)]
struct LoadedKey {
    public: PublicKey,
    private: PrivateKey,
}

/// Represent a recipient with a name and Bitcoin address
#[derive(Serialize, Deserialize, Clone)]
pub struct Recipient {
    pub name: String,
    pub address: String,
}

/// Define the type of fee calculation
#[derive(Serialize, Deserialize, Clone)]
pub enum FeeType {
    Fixed,
    Percent,
}

/// Configure the fee calculation
#[derive(Serialize, Deserialize, Clone)]
pub struct FeeConfig {
    pub fee_type: FeeType,
    pub value: f64,
}

/// Store the configuration for the Core
#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub my_keys: Vec<Key>,
    pub contacts: Vec<Recipient>,
    pub default_node: String,
    pub fee_config: FeeConfig,
}

/// Store and manage Unspent Transaction Outputs (UTXOs) for the Core
#[derive(Clone)]
struct UtxoStore {
    my_keys: Vec<LoadedKey>,
    // Map from address (String) to UTXOs
    utxos: Arc<SkipMap<String, Vec<(bool, TransactionOutput)>>>,
    // Map from address to the public key that owns it (for signing)
    address_to_key: Arc<SkipMap<String, PublicKey>>,
}

impl UtxoStore {
    fn new() -> Self {
        Self {
            my_keys: vec![],
            utxos: Arc::new(SkipMap::new()),
            address_to_key: Arc::new(SkipMap::new()),
        }
    }
    fn add_key(&mut self, key: LoadedKey) {
        let address = key.public.to_address();
        self.address_to_key.insert(address.clone(), key.public.clone());
        self.my_keys.push(key);
    }
}

/// Transaction result for reporting back to UI
#[derive(Clone)]
pub enum TransactionResult {
    Success,
    Rejected(String),
    Error(String),
}

/// Core functionality for the wallet
pub struct Core {
    pub config: Arc<RwLock<Config>>,
    config_path: PathBuf,
    utxos: UtxoStore,
    pub tx_sender: Sender<(Transaction, Option<oneshot::Sender<TransactionResult>>)>,
    pub stream: Mutex<TcpStream>,
}

impl Core {
    fn new(config: Config, config_path: PathBuf, utxos: UtxoStore, stream: TcpStream) -> Self {
        let (tx_sender, _) = kanal::bounded(10);
        Self {
            config: Arc::new(RwLock::new(config)),
            config_path,
            utxos,
            tx_sender,
            stream: Mutex::new(stream),
        }
    }

    /// Load the core from a config file
    pub async fn load(config_path: PathBuf) -> Result<Self> {
        let config: Config = toml::from_str(&fs::read_to_string(&config_path)?)?;
        let default_node = config.default_node.clone();
        let mut utxos = UtxoStore::new();
        let stream = TcpStream::connect(&default_node).await?;

        for key in &config.my_keys {
            let public = PublicKey::load_from_file(&key.public)?;
            let private = PrivateKey::load_from_file(&key.private)?;
            utxos.add_key(LoadedKey { public, private });
        }
        Ok(Core::new(config, config_path, utxos, stream))
    }
    
    /// Reconnect to the node
    async fn reconnect(&self) -> Result<()> {
        let node_address = {
            let config = self.config.read().unwrap();
            config.default_node.clone()
        };
        
        info!("Reconnecting to node: {}", node_address);
        let new_stream = tokio::net::TcpStream::connect(&node_address).await?;
        *self.stream.lock().await = new_stream;
        info!("Reconnected successfully");
        Ok(())
    }

    /// Fetch UTXOs from the node for all loaded keys
    pub async fn fetch_utxos(&self) -> Result<()> {
        info!("Starting UTXO fetch for {} keys", self.utxos.my_keys.len());
        for key in &self.utxos.my_keys {
            let address = key.public.to_address();
            info!("Fetching UTXOs for address: {}", address);
            let message = Message::FetchUTXOs(address.clone());
            message.send_async(&mut *self.stream.lock().await).await?;

            if let Message::UTXOs(utxos) =
                Message::receive_async(&mut *self.stream.lock().await).await?
            {
                info!("Received {} UTXOs for address {}", utxos.len(), address);
                let mut received_hashes = Vec::new();
                for (utxo, marked) in &utxos {
                    let utxo_hash = utxo.hash();
                    received_hashes.push(utxo_hash);
                    info!("  UTXO from node: hash={}, value={}, marked={}, address={}, unique_id={}", 
                        utxo_hash, utxo.value, marked, utxo.address, utxo.unique_id);
                    info!("    UTXO raw data: value={}, address={}, unique_id={}", 
                        utxo.value, utxo.address, utxo.unique_id);
                }
                
                // Store the UTXOs and compare with old ones
                let old_utxos = self.utxos.utxos.get(&address).map(|entry| entry.value().clone());
                let new_utxos: Vec<_> = utxos
                    .into_iter()
                    .map(|(output, marked)| (marked, output))
                    .collect();
                self.utxos.utxos.insert(
                    address.clone(),
                    new_utxos.clone(),
                );
                
                // Compare with old UTXOs if they existed
                if let Some(old_utxos_vec) = old_utxos {
                    info!("Comparing with previously cached UTXOs for address {}", address);
                    let old_hashes: Vec<_> = old_utxos_vec.iter()
                        .map(|(_, utxo)| utxo.hash())
                        .collect();
                    
                    let new_hashes_set: std::collections::HashSet<_> = received_hashes.iter().collect();
                    let old_hashes_set: std::collections::HashSet<_> = old_hashes.iter().collect();
                    
                    info!("  Old UTXO count: {}, New UTXO count: {}", old_hashes.len(), received_hashes.len());
                    
                    for old_hash in &old_hashes {
                        if !new_hashes_set.contains(old_hash) {
                            warn!("  UTXO disappeared from node: {}", old_hash);
                        }
                    }
                    
                    for new_hash in &received_hashes {
                        if !old_hashes_set.contains(new_hash) {
                            info!("  New UTXO appeared: {}", new_hash);
                        }
                    }
                }
            } else {
                return Err(anyhow!("Unexpected response from node"));
            }
        }
        info!("UTXO fetch completed");
        Ok(())
    }

    /// Send a transaction to the node and wait to detect if it was rejected
    pub async fn send_transaction(&self, transaction: Transaction) -> Result<TransactionResult> {
        info!("=== SENDING TRANSACTION TO NODE ===");
        info!("Transaction hash: {}", transaction.hash());
        info!("Transaction has {} inputs:", transaction.inputs.len());
        for (idx, input) in transaction.inputs.iter().enumerate() {
            info!("  Input {}: prev_tx_hash={}, pubkey_address={}", 
                idx, input.prev_transaction_output_hash, input.public_key.to_address());
        }
        info!("Transaction has {} outputs:", transaction.outputs.len());
        for (idx, output) in transaction.outputs.iter().enumerate() {
            info!("  Output {}: address={}, value={}, unique_id={}", 
                idx, output.address, output.value, output.unique_id);
        }
        
        let message = Message::SubmitTransaction(transaction.clone());
        let mut stream = self.stream.lock().await;
        
        // Send the transaction
        info!("Sending SubmitTransaction message to node...");
        if let Err(e) = message.send_async(&mut *stream).await {
            error!("Failed to send transaction: {}", e);
            // Try to reconnect
            drop(stream);
            if let Err(reconnect_err) = self.reconnect().await {
                return Err(anyhow!("Failed to send transaction and reconnect: {} (reconnect: {})", e, reconnect_err));
            }
            return Err(anyhow!("Failed to send transaction: {}", e));
        }
        
        // Give the node a moment to process and potentially close the connection
        drop(stream);
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        
        // Try to use the connection for a simple operation to see if it's still alive
        // If the connection was closed, this will fail
        let mut stream = self.stream.lock().await;
        
        // Try to read with a very short timeout - if connection is closed, this will fail
        let mut buf = [0u8; 1];
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(50),
            stream.read(&mut buf)
        ).await {
            Ok(Ok(0)) | Ok(Err(_)) => {
                // Connection was closed - transaction was likely rejected
                warn!("Connection closed after sending transaction - likely rejected");
                drop(stream);
                
                // Reconnect for future operations
                if let Err(e) = self.reconnect().await {
                    error!("Failed to reconnect after transaction rejection: {}", e);
                }
                
                Ok(TransactionResult::Rejected(
                    "Transaction was rejected by the node. The UTXO may not exist or may have been spent. Check node logs for details.".to_string()
                ))
            }
            Ok(Ok(_)) => {
                // We read a byte - connection is open but we consumed data
                // This shouldn't happen normally, but if it does, assume success
                warn!("Unexpected data read from stream after transaction - assuming success");
                Ok(TransactionResult::Success)
            }
            Err(_) => {
                // Timeout - connection is still open (no data to read, but not closed)
                // This is the normal case - transaction was likely accepted
                info!("Transaction appears to have been accepted (connection still open)");
                Ok(TransactionResult::Success)
            }
        }
    }

    /// Resolve recipient string to address (handles contact names or addresses)
    pub fn resolve_recipient_address(&self, recipient: &str) -> Result<String> {
        let config = self.config.read().unwrap();
        
        // First try contact name lookup
        if let Some(contact) = config.contacts.iter().find(|r| r.name == recipient) {
            return Ok(contact.address.clone());
        }

        // If not found, validate as address
        if PublicKey::validate_address(recipient)
            .map_err(|e| anyhow!("Invalid address format: {}", e))? {
            return Ok(recipient.to_string());
        }

        Err(anyhow!("Recipient '{}' is neither a contact name nor a valid Bitcoin address", recipient))
    }

    pub fn send_transaction_async(self: Arc<Self>, recipient: &str, amount: u64) -> Result<()> {
        info!("Preparing to send {} satoshis to {}", amount, recipient);

        let recipient_address = self.resolve_recipient_address(recipient)?;
        let core = Arc::clone(&self);
        let tx_sender = self.tx_sender.clone();
        
        // Create a channel to receive the result from the async task
        let (result_tx, result_rx) = oneshot::channel::<Result<()>>();
        let result_tx = Arc::new(Mutex::new(Some(result_tx)));
        
        // Spawn async task to refresh UTXOs and create transaction
        let result_tx_clone = Arc::clone(&result_tx);
        tokio::spawn(async move {
            // Refresh UTXOs to ensure we have the latest state
            info!("Refreshing UTXOs before creating transaction");
            if let Err(e) = core.fetch_utxos().await {
                let error_msg = format!("Failed to refresh UTXOs: {}", e);
                error!("{}", error_msg);
                if let Some(tx) = result_tx_clone.lock().await.take() {
                    let _ = tx.send(Err(anyhow!("{}", error_msg)));
                }
                return;
            }
            
            // Small delay to ensure blockchain state is consistent
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            
            // Create transaction with fresh UTXOs
            info!("Creating transaction for {} satoshis to {}", amount, recipient_address);
            let transaction = match core.create_transaction(&recipient_address, amount) {
                Ok(tx) => {
                    info!("Transaction created successfully with {} inputs", tx.inputs.len());
                    tx
                },
                Err(e) => {
                    let error_msg = format!("Failed to create transaction: {}", e);
                    error!("{}", error_msg);
                    if let Some(tx) = result_tx_clone.lock().await.take() {
                        let _ = tx.send(Err(anyhow!("{}", error_msg)));
                    }
                    return;
                }
            };
            
            // Log transaction details for debugging
            info!("Transaction created with {} inputs:", transaction.inputs.len());
            for (idx, input) in transaction.inputs.iter().enumerate() {
                info!("  Input {}: prev_tx_hash={}", idx, input.prev_transaction_output_hash);
            }
            info!("Transaction outputs:");
            for (idx, output) in transaction.outputs.iter().enumerate() {
                info!("  Output {}: address={}, value={}", idx, output.address, output.value);
            }
            
            info!("Sending transaction to handler");
            
            // Create a result channel to get the transaction result
            let (tx_result_tx, tx_result_rx) = oneshot::channel::<TransactionResult>();
            if let Err(e) = tx_sender.send((transaction, Some(tx_result_tx))) {
                let error_msg = format!("Failed to send transaction to channel: {}", e);
                error!("{}", error_msg);
                if let Some(tx) = result_tx_clone.lock().await.take() {
                    let _ = tx.send(Err(anyhow!("{}", error_msg)));
                }
                return;
            }
            
            // Wait for the transaction result from the handler
            match tx_result_rx.await {
                Ok(TransactionResult::Success) => {
                    info!("Transaction accepted by node");
                    if let Some(tx) = result_tx_clone.lock().await.take() {
                        let _ = tx.send(Ok(()));
                    }
                }
                Ok(TransactionResult::Rejected(reason)) => {
                    let error_msg = format!("Transaction rejected: {}", reason);
                    error!("{}", error_msg);
                    if let Some(tx) = result_tx_clone.lock().await.take() {
                        let _ = tx.send(Err(anyhow!("{}", error_msg)));
                    }
                }
                Ok(TransactionResult::Error(e)) => {
                    let error_msg = format!("Transaction error: {}", e);
                    error!("{}", error_msg);
                    if let Some(tx) = result_tx_clone.lock().await.take() {
                        let _ = tx.send(Err(anyhow!("{}", error_msg)));
                    }
                }
                Err(_) => {
                    let error_msg = "Failed to receive transaction result";
                    error!("{}", error_msg);
                    if let Some(tx) = result_tx_clone.lock().await.take() {
                        let _ = tx.send(Err(anyhow!("{}", error_msg)));
                    }
                }
            }
        });
        
        // Wait for the result using block_in_place to avoid blocking the runtime
        tokio::task::block_in_place(|| {
            let rt = match tokio::runtime::Handle::try_current() {
                Ok(handle) => handle,
                Err(_) => return Err(anyhow!("No tokio runtime available")),
            };
            
            rt.block_on(async {
                match tokio::time::timeout(tokio::time::Duration::from_secs(10), result_rx).await {
                    Ok(Ok(result)) => result,
                    Ok(Err(_)) => Err(anyhow!("Channel closed")),
                    Err(_) => Err(anyhow!("Transaction creation timed out after 10 seconds")),
                }
            })
        })
    }

    pub fn get_balance(&self) -> u64 {
        self.utxos
            .utxos
            .iter()
            .map(|entry| entry.value().iter().map(|utxo| utxo.1.value).sum::<u64>())
            .sum()
    }

    /// Get all addresses for the loaded keys
    pub fn get_addresses(&self) -> Vec<String> {
        self.utxos
            .my_keys
            .iter()
            .map(|key| key.public.to_address())
            .collect()
    }

    pub fn create_transaction(&self, recipient_address: &str, amount: u64) -> Result<Transaction> {
        let fee = self.calculate_fee(amount);
        let total_amount = amount + fee;
        let mut inputs = Vec::new();
        let mut input_sum = 0;

        // Check if we have any UTXOs at all
        let has_utxos = self.utxos.utxos.iter().any(|entry| {
            entry.value().iter().any(|(marked, _)| !marked)
        });
        
        if !has_utxos {
            return Err(anyhow!("No unspent UTXOs available. Please ensure you have received funds."));
        }

        info!("Creating transaction: amount={}, fee={}, total={}", amount, fee, total_amount);
        info!("Available UTXOs by address:");
        for entry in self.utxos.utxos.iter() {
            let address = entry.key();
            let utxos = entry.value();
            let unspent_count = utxos.iter().filter(|(marked, _)| !marked).count();
            let total_value: u64 = utxos.iter().filter(|(marked, _)| !marked).map(|(_, utxo)| utxo.value).sum();
            info!("  Address {}: {} unspent UTXOs, total value: {}", address, unspent_count, total_value);
            
            // Log all UTXOs in detail
            for (marked, utxo) in utxos.iter() {
                let utxo_hash = utxo.hash();
                info!("    UTXO: hash={}, value={}, marked={}, address={}, unique_id={}", 
                    utxo_hash, utxo.value, marked, utxo.address, utxo.unique_id);
            }
        }

        for entry in self.utxos.utxos.iter() {
            let address = entry.key();
            let utxos = entry.value();

            // Get the public key for this address (needed for signing)
            let pubkey = self.utxos.address_to_key
                .get(address)
                .ok_or_else(|| anyhow!("No public key found for address {}", address))?
                .value()
                .clone();

            // Find the corresponding private key
            let private_key = self.utxos.my_keys
                .iter()
                .find(|k| k.public == pubkey)
                .ok_or_else(|| anyhow!("No private key found for address {}", address))?
                .private.clone();

            for (marked, utxo) in utxos.iter() {
                if *marked {
                    info!("Skipping marked UTXO: {}", utxo.hash());
                    continue;
                }

                if input_sum >= total_amount {
                    info!("Sufficient funds collected: {} >= {}", input_sum, total_amount);
                    break;
                }

                let utxo_hash = utxo.hash();
                info!("Selecting UTXO: hash={}, value={}, address={}", utxo_hash, utxo.value, address);
                info!("  UTXO details for hash calculation: value={}, address={}, unique_id={}", 
                    utxo.value, utxo.address, utxo.unique_id);
                
                // Verify the hash matches what we expect
                let recalculated_hash = utxo.hash();
                if recalculated_hash != utxo_hash {
                    error!("UTXO hash mismatch! Expected {}, got {}", utxo_hash, recalculated_hash);
                }
                
                info!("  Creating transaction input with prev_tx_hash={}", utxo_hash);
                info!("  Public key address: {}", pubkey.to_address());
                info!("  UTXO address: {}", utxo.address);
                
                inputs.push(TransactionInput {
                    prev_transaction_output_hash: utxo_hash,
                    public_key: pubkey.clone(),
                    signature: Signature::sign_output(
                        &utxo_hash,
                        &private_key,
                    ),
                });
                input_sum += utxo.value;
                info!("  Input added successfully. Total input_sum: {}", input_sum);
            }

            if input_sum >= total_amount {
                break;
            }
        }

        if input_sum < total_amount {
            return Err(anyhow!("Insufficient funds"));
        }

        let mut outputs = vec![TransactionOutput {
            value: amount,
            unique_id: Uuid::new_v4(),
            address: recipient_address.to_string(),
        }];

        if input_sum > total_amount {
            // Change output goes to first address we own
            let change_address = self.utxos.my_keys[0].public.to_address();
            outputs.push(TransactionOutput {
                value: input_sum - total_amount,
                unique_id: Uuid::new_v4(),
                address: change_address,
            })
        }

        Ok(Transaction::new(inputs, outputs))
    }

    fn calculate_fee(&self, amount: u64) -> u64 {
        let config = self.config.read().unwrap();
        match config.fee_config.fee_type {
            FeeType::Fixed => config.fee_config.value as u64,
            FeeType::Percent => (amount as f64 * config.fee_config.value / 100.0) as u64,
        }
    }

    /// Find contact by name
    pub fn find_contact_by_name(&self, name: &str) -> Option<Recipient> {
        let config = self.config.read().unwrap();
        config.contacts.iter().find(|r| r.name == name).cloned()
    }

    /// Find contact by address
    pub fn find_contact_by_address(&self, address: &str) -> Option<Recipient> {
        let config = self.config.read().unwrap();
        config.contacts.iter().find(|r| r.address == address).cloned()
    }

    /// Add a new contact
    pub fn add_contact(&self, name: String, address: String) -> Result<()> {
        // Validate address format
        PublicKey::validate_address(&address)
            .map_err(|e| anyhow!("Invalid address format: {}", e))?;

        let mut config = self.config.write().unwrap();
        
        // Check if contact with this name already exists
        if config.contacts.iter().any(|r| r.name == name) {
            return Err(anyhow!("Contact with name '{}' already exists", name));
        }

        // Check if contact with this address already exists
        if config.contacts.iter().any(|r| r.address == address) {
            return Err(anyhow!("Contact with address '{}' already exists", address));
        }

        config.contacts.push(Recipient { name, address: address.to_string() });
        drop(config); // Release lock before saving
        self.save_config()?;
        Ok(())
    }

    /// Remove a contact by name
    pub fn remove_contact(&self, name: &str) -> Result<()> {
        let mut config = self.config.write().unwrap();
        let initial_len = config.contacts.len();
        config.contacts.retain(|r| r.name != name);
        
        if config.contacts.len() == initial_len {
            return Err(anyhow!("Contact '{}' not found", name));
        }

        drop(config); // Release lock before saving
        self.save_config()?;
        Ok(())
    }

    /// Save config to file
    pub fn save_config(&self) -> Result<()> {
        let config = self.config.read().unwrap();
        let config_str = toml::to_string_pretty(&*config)?;
        drop(config); // Release lock before writing
        fs::write(&self.config_path, config_str)?;
        info!("Config saved to {:?}", self.config_path);
        Ok(())
    }
}
