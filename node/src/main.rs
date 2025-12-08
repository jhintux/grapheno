use anyhow::Result;
use argh::FromArgs;
use btclib::types::Blockchain;
use dashmap::DashMap;
use static_init::dynamic;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

mod database;
mod handler;
mod util;

#[derive(FromArgs)]
/// A toy blockchain node
struct Args {
    #[argh(option, default = "9000")]
    /// port number
    port: u16,
    #[argh(option, default = "String::from(\"./blockchain_db\")")]
    /// blockchain database directory
    db_path: String,
    #[argh(positional)]
    /// addresses of initial nodes
    nodes: Vec<String>,
}

#[dynamic]
pub static BLOCKCHAIN: RwLock<Blockchain> = RwLock::new(Blockchain::new());
#[dynamic]
pub static NODES: DashMap<String, TcpStream> = DashMap::new();
#[dynamic]
pub static DB: RwLock<Option<database::BlockchainDB>> = RwLock::new(None);

#[tokio::main]
async fn main() -> Result<()> {
    let args: Args = argh::from_env();

    // Access the parsed arguments
    let port = args.port;
    let db_path = args.db_path;
    let nodes = args.nodes;

    // Initialize database
    println!("opening database at {}", db_path);
    let db = database::BlockchainDB::open(&db_path)?;
    {
        let mut db_guard = DB.write().await;
        *db_guard = Some(db);
    }

    // Try to load blockchain from database
    if util::load_blockchain().await.is_ok() {
        println!("blockchain loaded from database");
    } else {
        println!("no blockchain found in database, initializing...");
        util::populate_connections(&nodes).await?;
        println!("total amount of known nodes: {}", NODES.len());

        if nodes.is_empty() {
            println!("no initial nodes provided, starting as a seed node");
        } else {
            let (longest_name, longest_count) = util::find_longest_chain_node().await?;

            util::download_blockchain(&longest_name, longest_count).await?;

            println!("blockchain downloaded, from {}", longest_name);

            {
                let mut blockchain = BLOCKCHAIN.write().await;
                blockchain.rebuild_utxos();
            }

            {
                let mut blockchain = BLOCKCHAIN.write().await;
                blockchain.try_adjust_target();
            }

            // Save the downloaded blockchain to database
            util::save_blockchain().await?;
        }
    }

    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    println!("Listening on {}", addr);

    // start a task to periodically cleanup the mempool. Normally, you would want to keep and join the handle
    tokio::spawn(util::cleanup());
    // and a task to periodically save the blockchain
    tokio::spawn(util::save());

    loop {
        let (socket, _) = listener.accept().await?;
        tokio::spawn(handler::handle_connection(socket));
    }
}
