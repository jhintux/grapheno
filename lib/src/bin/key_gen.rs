use btclib::crypto::PrivateKey;
use btclib::util::Saveable;
use std::env;
use std::path::PathBuf;
fn main() {
    let name = env::args().nth(1).expect("Please provide a name");
    let private_key = PrivateKey::new_key();
    let public_key = private_key.public_key();
    
    // Get the wallet directory (go up from bin/src/lib to workspace root)
    let wallet_dir = PathBuf::from("wallet");
    std::fs::create_dir_all(&wallet_dir).expect("Failed to create wallet directory");
    
    let public_key_file = wallet_dir.join(format!("{}.pub.pem", name));
    let private_key_file = wallet_dir.join(format!("{}.priv.cbor", name));
    
    private_key.save_to_file(&private_key_file).unwrap();
    public_key.save_to_file(&public_key_file).unwrap();
}
