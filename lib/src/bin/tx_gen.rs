use btclib::types::{Transaction, TransactionOutput};
use btclib::crypto::PrivateKey;
use btclib::util::Saveable;
use std::env;
use uuid::Uuid;

fn main() {
    let path = if let Some(arg) = env::args().nth(1) {
        arg
    } else {
        eprint!("Usage: tx_gen <path to transaction file>");
        std::process::exit(1);
    };

    let private_key = PrivateKey::new_key();
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