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
use tokio::sync::Mutex;
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

/// Core functionality for the wallet
pub struct Core {
    pub config: Arc<RwLock<Config>>,
    config_path: PathBuf,
    utxos: UtxoStore,
    pub tx_sender: Sender<Transaction>,
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

    /// Fetch UTXOs from the node for all loaded keys
    pub async fn fetch_utxos(&self) -> Result<()> {
        for key in &self.utxos.my_keys {
            let address = key.public.to_address();
            let message = Message::FetchUTXOs(address.clone());
            message.send_async(&mut *self.stream.lock().await).await?;

            if let Message::UTXOs(utxos) =
                Message::receive_async(&mut *self.stream.lock().await).await?
            {
                self.utxos.utxos.insert(
                    address,
                    utxos
                        .into_iter()
                        .map(|(output, marked)| (marked, output))
                        .collect::<Vec<_>>(),
                );
            } else {
                return Err(anyhow!("Unexpected response from node"));
            }
        }
        Ok(())
    }

    /// Send a transaction to the node
    pub async fn send_transaction(&self, transaction: Transaction) -> Result<()> {
        let message = Message::SubmitTransaction(transaction.clone());
        message.send_async(&mut *self.stream.lock().await).await?;
        Ok(())
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

    pub fn send_transaction_async(&self, recipient: &str, amount: u64) -> Result<()> {
        info!("Preparing to send {} satoshis to {}", amount, recipient);

        let recipient_address = self.resolve_recipient_address(recipient)?;

        let transaction = self.create_transaction(&recipient_address, amount)?;
        debug!("Sending transaction asynchronously");
        self.tx_sender.send(transaction)
            .map_err(|e| anyhow!("Failed to send transaction to channel: {}", e))?;
        Ok(())
    }

    pub fn get_balance(&self) -> u64 {
        self.utxos
            .utxos
            .iter()
            .map(|entry| entry.value().iter().map(|utxo| utxo.1.value).sum::<u64>())
            .sum()
    }

    pub fn create_transaction(&self, recipient_address: &str, amount: u64) -> Result<Transaction> {
        let fee = self.calculate_fee(amount);
        let total_amount = amount + fee;
        let mut inputs = Vec::new();
        let mut input_sum = 0;

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
                    continue;
                }

                if input_sum >= total_amount {
                    break;
                }

                inputs.push(TransactionInput {
                    prev_transaction_output_hash: utxo.hash(),
                    public_key: pubkey.clone(),
                    signature: Signature::sign_output(
                        &utxo.hash(),
                        &private_key,
                    ),
                });
                input_sum += utxo.value;
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

        config.contacts.push(Recipient { name, address });
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
