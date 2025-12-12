use crate::core::{Config, Core, FeeConfig, FeeType, Recipient};
use anyhow::Result;
use std::panic;
use std::path::PathBuf;
use tracing::*;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Initialize tracing with compact format and environment-based filtering
pub fn init_tracing() -> Result<()> {
    // Create a formatting layer for tracing output with a compact format
    let fmt_layer = fmt::layer().compact();

    // Create a filter layer to control the verbosity of logs
    // Try to get the filter configuration from the environment variables
    // If it fails, default to the "info" log level
    let filter_layer = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info"))?;

    // Build the tracing subscriber registry with the formatting layer,
    // the filter layer, and the error layer for enhanced error reporting
    tracing_subscriber::registry()
        .with(filter_layer) // Add the filter layer to control log verbosity
        .with(fmt_layer) // Add the formatting layer for compact log output
        .init(); // Initialize the tracing subscriber

    Ok(())
}

/// Initialize tracing to save logs into the logs/ folder (legacy function for compatibility)
#[allow(dead_code)]
pub fn setup_tracing() -> Result<()> {
    init_tracing()
}

/// Make sure tracing is able to log panics occurring in the wallet
pub fn setup_panic_hook() {
    panic::set_hook(Box::new(|panic_info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        error!("Application pancicked!");
        error!("Panic info: {:?}", panic_info);
        error!("Backtrace: {:?}", backtrace);
    }));
}

pub fn generate_dummy_config(path: &PathBuf) -> Result<()> {
    let dummy_config = Config {
        my_keys: vec![],
        contacts: vec![
            Recipient {
                name: "Alice".to_string(),
                key: PathBuf::from("alice.pub.pem"),
            },
            Recipient {
                name: "Bob".to_string(),
                key: PathBuf::from("bob.pub.pem"),
            },
        ],
        default_node: "127.0.0.1:9000".to_string(),
        fee_config: FeeConfig {
            fee_type: FeeType::Percent,
            value: 0.1,
        },
    };
    let config_str = toml::to_string_pretty(&dummy_config)?;
    std::fs::write(path, config_str)?;
    info!("Dummy config generated at: {}", path.display());
    Ok(())
}

/// Convert satoshis to a BTC string
pub fn sats_to_btc(sats: u64) -> String {
    let btc = sats as f64 / 100_000_000.0;
    format!("{} BTC", btc)
}

/// Make it big lmao
pub fn big_mode_btc(core: &Core) -> String {
    text_to_ascii_art::to_art(sats_to_btc(core.get_balance()), "big", 0, 1, 0).unwrap()
}
