use btclib::crypto::PrivateKey;
use btclib::util::Saveable;
use bip39::{Mnemonic, Language};
use rand::RngCore;
use std::io::{self, Write};
use std::path::PathBuf;

fn main() {
    println!("=== Deterministic Wallet Key Generator ===\n");

    // Generate a new BIP39 mnemonic (12 words = 128 bits of entropy)
    let mut entropy = [0u8; 16]; // 128 bits for 12-word mnemonic
    rand::rng().fill_bytes(&mut entropy);
    
    let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy)
        .expect("Failed to generate mnemonic");
    let mnemonic_phrase = mnemonic.to_string();

    println!("Generated mnemonic phrase:");
    println!("{}\n", mnemonic_phrase);
    println!("⚠️  IMPORTANT: Save this mnemonic phrase in a secure location!");
    println!("   You will need it to recover your keys.\n");

    // Derive private key from mnemonic
    let private_key = PrivateKey::from_mnemonic(&mnemonic_phrase)
        .expect("Failed to derive key from mnemonic");
    let public_key = private_key.public_key();

    // Display public address (Bitcoin-style)
    println!("Public Address: {}", public_key.to_address());
    println!();

    // Prompt for directory to save keys
    print!("Enter directory path to save keys (default: ./keys): ");
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("Failed to read input");

    let wallet_dir = if input.trim().is_empty() {
        PathBuf::from("keys")
    } else {
        PathBuf::from(input.trim())
    };

    // Prompt for key name
    print!("Enter a name for this key pair (default: default): ");
    io::stdout().flush().unwrap();

    let mut name_input = String::new();
    io::stdin()
        .read_line(&mut name_input)
        .expect("Failed to read input");

    let name = if name_input.trim().is_empty() {
        "default".to_string()
    } else {
        name_input.trim().to_string()
    };

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&wallet_dir)
        .expect("Failed to create wallet directory");

    // Save keys
    let public_key_file = wallet_dir.join(format!("{}.pub.pem", name));
    let private_key_file = wallet_dir.join(format!("{}.priv.cbor", name));

    private_key
        .save_to_file(&private_key_file)
        .expect("Failed to save private key");
    public_key
        .save_to_file(&public_key_file)
        .expect("Failed to save public key");

    println!("\n✓ Keys saved successfully!");
    println!("  Private key: {:?}", private_key_file);
    println!("  Public key: {:?}", public_key_file);
    println!("\nMnemonic phrase: {}", mnemonic_phrase);
    println!("Public Address: {}", public_key.to_address());
}
