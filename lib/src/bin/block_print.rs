use std::{env, fs::File};
use btclib::{types::Block, util::Saveable};

fn main() {
    let path = if let Some(arg) = env::args().nth(1) {
        arg
    } else {
        eprint!("Usage: block_print <path to block file>");
        std::process::exit(1);
    };

    if let Ok(file) = File::open(path) {
        let block = Block::load(file).expect("Failed to load block");
        println!("{:#?}", block);
    }
}