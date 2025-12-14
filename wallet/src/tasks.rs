use crate::core::{Core, TransactionResult};
use crate::ui::run_ui;
use crate::util::big_mode_btc;
use btclib::types::Transaction;
use cursive::views::TextContent;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio::time::{self, Duration};
use tokio::sync::oneshot;
use tracing::*;

pub fn update_utxos(core: Arc<Core>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(20));
        loop {
            interval.tick().await;
            if let Err(e) = core.fetch_utxos().await {
                error!("Failed to update UTXOs: {}", e);
            }
        }
    })
}

pub fn handle_transactions(
    rx: kanal::AsyncReceiver<(Transaction, Option<oneshot::Sender<TransactionResult>>)>,
    core: Arc<Core>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok((transaction, result_tx)) = rx.recv().await {
            info!("Handling transaction: {}", transaction.hash());
            match core.send_transaction(transaction).await {
                Ok(result) => {
                    // Send result back to the caller if they provided a channel
                    if let Some(tx) = result_tx {
                        let _ = tx.send(result.clone());
                    }
                    
                    match result {
                        TransactionResult::Success => {
                            info!("Transaction successfully sent and accepted");
                        }
                        TransactionResult::Rejected(reason) => {
                            error!("Transaction rejected: {}", reason);
                        }
                        TransactionResult::Error(e) => {
                            error!("Transaction error: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to send transaction: {}", e);
                    // Send error result back if channel provided
                    if let Some(tx) = result_tx {
                        let _ = tx.send(TransactionResult::Error(format!("{}", e)));
                    }
                }
            }
        }
    })
}

pub fn ui_task(core: Arc<Core>, balance_content: TextContent) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        info!("Running UI");
        if let Err(e) = run_ui(core, balance_content) {
            error!("UI ended with error: {e}");
        };
    })
}

pub fn update_balance(core: Arc<Core>, balance_content: TextContent) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            info!("updating balance string");
            balance_content.set_content(big_mode_btc(&core));
        }
    })
}
