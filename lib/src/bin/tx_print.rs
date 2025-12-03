use std::{env, fs::File};
use btclib::{types::Transaction, util::Saveable};

fn main() {
    let path = if let Some(arg) = env::args().nth(1) {
        arg
    } else {
        eprint!("Usage: tx_print <path to transaction file>");
        std::process::exit(1);
    };

    if let Ok(file) = File::open(path) {
        let transaction = Transaction::load(file).expect("Failed to load transaction");
        println!("{:#?}", transaction);
    }
}