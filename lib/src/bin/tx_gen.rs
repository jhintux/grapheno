use btclib::types::{Transaction, TransactionOutput};
use btclib::crypto::PrivateKey;
use btclib::util::Saveable;
use std::env;
use uuid::Uuid;

fn main() {
    let path = if let Some(arg) = env::args().nth(1) {
        arg
    } else {
        eprintln!("Usage: tx_gen <path to transaction file> <path to private key>");
        std::process::exit(1);
    };

    let key_path = if let Some(key_path) = env::args().nth(2) {
        key_path
    } else {
        eprintln!("Usage: tx_gen <path to transaction file> <path to private key>");
        eprintln!("Error: Private key file path is required");
        std::process::exit(1);
    };

    let private_key = PrivateKey::load_from_file(&key_path)
        .expect("Failed to load private key from file");
    let transaction = Transaction::new(
        vec![],
        vec![TransactionOutput {
            unique_id: Uuid::new_v4(),
            value: btclib::INITIAL_REWARD * 10u64.pow(8),
            pubkey: private_key.public_key(),
        }]
    );

    transaction.save_to_file(path)
        .expect("Failed to save transaction to file");
}