use btclib::{
    crypto::PrivateKey,
    sha256::Hash,
    types::{Block, BlockHeader, Transaction, TransactionOutput},
    util::{MerkleRoot, Saveable},
};
use chrono::Utc;
use std::env;
use uuid::Uuid;

fn main() {
    let path = if let Some(arg) = env::args().nth(1) {
        arg
    } else {
        eprint!("Usage: block_gen <path to block file> [path to private key]");
        std::process::exit(1);
    };

    let private_key = if let Some(key_path) = env::args().nth(2) {
        PrivateKey::load_from_file(&key_path)
            .expect("Failed to load private key from file")
    } else {
        PrivateKey::new_key()
    };
    let transactions = vec![Transaction::new(
        vec![],
        vec![TransactionOutput {
            unique_id: Uuid::new_v4(),
            value: btclib::INITIAL_REWARD * 10u64.pow(8),
            pubkey: private_key.public_key(),
        }],
    )];

    let merkle_root = MerkleRoot::calculate(&transactions);
    let block = Block::new(
        BlockHeader::new(Utc::now(), 0, Hash::zero(), merkle_root, btclib::MIN_TARGET),
        transactions,
    );

    block
        .save_to_file(path)
        .expect("Failed to save block to file");
}
