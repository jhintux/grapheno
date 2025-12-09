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
use std::sync::Arc;
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

/// Represent a recipient with a name and a path to their public key
#[derive(Serialize, Deserialize, Clone)]
pub struct Recipient {
    pub name: String,
    pub key: PathBuf,
}

/// Represent a loaded recipient with actual public key
#[derive(Clone)]
pub struct LoadedRecipient {
    pub key: PublicKey,
}

impl Recipient {
    /// Load the recipient key from the file
    pub fn load(&self) -> Result<LoadedRecipient> {
        debug!("Loading recipient key from: {:?}", self.key);
        let key = PublicKey::load_from_file(&self.key)?;
        Ok(LoadedRecipient { key })
    }
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
    utxos: Arc<SkipMap<PublicKey, Vec<(bool, TransactionOutput)>>>,
}

impl UtxoStore {
    fn new() -> Self {
        Self {
            my_keys: vec![],
            utxos: Arc::new(SkipMap::new()),
        }
    }
    fn add_key(&mut self, key: LoadedKey) {
        self.my_keys.push(key);
    }
}

/// Core functionality for the wallet
pub struct Core {
    pub config: Config,
    utxos: UtxoStore,
    pub tx_sender: Sender<Transaction>,
    pub stream: Mutex<TcpStream>,
}

impl Core {
    fn new(config: Config, utxos: UtxoStore, stream: TcpStream) -> Self {
        let (tx_sender, _) = kanal::bounded(10);
        Self {
            config,
            utxos,
            tx_sender,
            stream: Mutex::new(stream),
        }
    }

    /// Load the core from a config file
    pub async fn load(config_path: PathBuf) -> Result<Self> {
        let config: Config = toml::from_str(&fs::read_to_string(&config_path)?)?;
        let mut utxos = UtxoStore::new();
        let stream = TcpStream::connect(&config.default_node).await?;

        for key in &config.my_keys {
            let public = PublicKey::load_from_file(&key.public)?;
            let private = PrivateKey::load_from_file(&key.private)?;
            utxos.add_key(LoadedKey { public, private });
        }
        Ok(Core::new(config, utxos, stream))
    }

    /// Fetch UTXOs from the node for all loaded keys
    pub async fn fetch_utxos(&self) -> Result<()> {
        for key in &self.utxos.my_keys {
            let message = Message::FetchUTXOs(key.public.clone());
            message.send_async(&mut *self.stream.lock().await).await?;

            if let Message::UTXOs(utxos) =
                Message::receive_async(&mut *self.stream.lock().await).await?
            {
                self.utxos.utxos.insert(
                    key.public.clone(),
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

    pub fn send_transaction_async(&self, recipient: &str, amount: u64) -> Result<()> {
        info!("Preparing to send {} satoshis to {}", amount, recipient);

        let recipient_key = self
            .config
            .contacts
            .iter()
            .find(|r| r.name == recipient)
            .ok_or_else(|| anyhow!("Recipient not found"))?
            .load()?
            .key;

        let transaction = self.create_transaction(&recipient_key, amount)?;
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

    pub fn create_transaction(&self, recipient: &PublicKey, amount: u64) -> Result<Transaction> {
        let fee = self.calculate_fee(amount);
        let total_amount = amount + fee;
        let mut inputs = Vec::new();
        let mut input_sum = 0;

        for entry in self.utxos.utxos.iter() {
            let pubkey = entry.key();
            let utxos = entry.value();

            for (marked, utxo) in utxos.iter() {
                if *marked {
                    continue;
                }

                if input_sum >= total_amount {
                    break;
                }

                inputs.push(TransactionInput {
                    prev_transaction_output_hash: utxo.hash(),
                    signature: Signature::sign_output(
                        &utxo.hash(),
                        &self
                            .utxos
                            .my_keys
                            .iter()
                            .find(|k| k.public == *pubkey)
                            .unwrap()
                            .private,
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
            pubkey: recipient.clone(),
        }];

        if input_sum > total_amount {
            outputs.push(TransactionOutput {
                value: input_sum - total_amount,
                unique_id: Uuid::new_v4(),
                pubkey: self.utxos.my_keys[0].public.clone(),
            })
        }

        Ok(Transaction::new(inputs, outputs))
    }

    fn calculate_fee(&self, amount: u64) -> u64 {
        match self.config.fee_config.fee_type {
            FeeType::Fixed => self.config.fee_config.value as u64,
            FeeType::Percent => (amount as f64 * self.config.fee_config.value / 100.0) as u64,
        }
    }
}
