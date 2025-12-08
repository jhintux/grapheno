# Grapheno - A Toy Blockchain Implementation

Grapheno is a Rust-based blockchain implementation featuring a distributed node network, mining capabilities, and a wallet interface. This project demonstrates core blockchain concepts including block validation, transaction processing, UTXO management, and proof-of-work mining.

## Project Structure

This is a Cargo workspace containing the following components:

- **`lib`** - Core blockchain library (`btclib`) containing:
  - Blockchain data structures (Block, Transaction, UTXO)
  - Cryptographic functions (ECDSA key generation, SHA-256 hashing)
  - Network message protocol
  - Utility functions for block/transaction generation and printing

- **`node`** - Blockchain node server that:
  - Maintains the blockchain state
  - Handles peer connections
  - Processes blocks and transactions
  - Serves block templates to miners
  - Manages UTXO sets

- **`miner`** - Mining client that:
  - Connects to a node
  - Fetches block templates
  - Performs proof-of-work mining
  - Submits mined blocks back to the node

- **`wallet`** - Interactive wallet application with:
  - TUI (Terminal User Interface) for viewing balance
  - Transaction creation and sending
  - UTXO tracking
  - Connection to blockchain nodes

## Prerequisites

- Rust (latest stable version)
- Cargo (comes with Rust)

## Getting Started

### Step 1: Generate Key Pair

First, you need to generate a key pair for your wallet. The `key_gen` utility creates both a private key (`.priv.cbor`) and a public key (`.pub.pem`).

From the project root directory, run:

```bash
# Generate keys (replace 'alice' with your preferred name)
cargo run --bin key_gen -- alice
```

This will create in `wallet` folder:
- `alice.priv.cbor` - Your private key (keep this secure!)
- `alice.pub.pem` - Your public key

### Step 2: Start the Node

Start the blockchain node. The node will listen on port 9000 by default and create a new blockchain database if no existing database is found.

```bash
# Start node on default port 9000 with default database directory
cargo run --bin node

# Or specify a custom port and database directory
cargo run --bin node -- --port 9001 --db-path ./node1_db
```

The node will:
- Start as a seed node if no blockchain database exists
- Listen for connections from miners and wallets
- Save the blockchain to the database directory periodically (default: `./blockchain_db`)

### Step 3: Start the Miner

In a new terminal, start the miner. The miner will connect to the node and begin mining blocks. The first block mined will be the genesis block.

```bash
# Connect miner to node at 127.0.0.1:9000
cargo run --bin miner -- --address 127.0.0.1:9000 --public-key-file wallet/alice.pub.pem

# Or using short flags:
cargo run --bin miner -- -a 127.0.0.1:9000 -p wallet/alice.pub.pem
```

The miner will:
- Connect to the specified node
- Request block templates
- Mine blocks using proof-of-work
- Submit successfully mined blocks back to the node
- Create the genesis block automatically when mining the first block

**Note:** Make sure the miner's public key file path is correct. The miner will receive the block reward (coinbase transaction) to the address associated with this public key.

### Step 4: Start the Wallet

In another terminal, navigate to the wallet directory and start the wallet application:

```bash
cd wallet
cargo run -- --node 127.0.0.1:9000
```

The wallet will:
- Load your keys from `wallet_config.toml`
- Connect to the specified node
- Display your balance in the TUI
- Allow you to send transactions

**Wallet Controls:**
- Press `Esc` to access the menu bar
- Use `Send` from the menu to create and send transactions
- Press `q` to quit

### Step 5: View Your Balance

Once the miner has successfully mined the genesis block (and any subsequent blocks), your wallet will display the balance associated with your public key. The balance updates automatically as new blocks are mined and transactions are processed.

## Running Multiple Nodes

You can run multiple nodes to create a distributed network. Each node needs:
- A unique port number
- A unique database directory
- Addresses of other nodes to connect to

### Example: Running 2 Nodes

**Terminal 1 - Node 1 (Seed Node):**
```bash
cargo run --bin node -- --port 9000 --db-path ./node1_db
```

**Terminal 2 - Node 2 (Connects to Node 1):**
```bash
cargo run --bin node -- --port 9001 --db-path ./node2_db 127.0.0.1:9000
```

### Example: Running 3 Nodes

**Terminal 1 - Node 1:**
```bash
cargo run --bin node -- --port 9000 --db-path ./node1_db
```

**Terminal 2 - Node 2:**
```bash
cargo run --bin node -- --port 9001 --db-path ./node2_db 127.0.0.1:9000
```

**Terminal 3 - Node 3:**
```bash
cargo run --bin node -- --port 9002 --db-path ./node3_db 127.0.0.1:9000 127.0.0.1:9001
```

### Key Points:

- **Unique Database Paths**: Each node must have its own database directory (`--db-path`) to avoid conflicts
- **Unique Ports**: Each node must listen on a different port (`--port`)
- **Node Discovery**: When starting a node, provide addresses of other nodes as positional arguments (e.g., `127.0.0.1:9000`)
- **Network Behavior**: 
  - Nodes automatically discover each other through the `DiscoverNodes` message
  - When a node connects to another, it receives a list of all known nodes
  - Nodes sync blockchain state from the longest chain
  - New blocks and transactions are broadcast to all connected peer nodes

### Node Command-Line Options

```bash
cargo run --bin node -- --help
```

Available options:
- `--port <PORT>` - Port number to listen on (default: 9000)
- `--db-path <PATH>` - Database directory path (default: `./blockchain_db`)
- `<nodes...>` - Addresses of initial nodes to connect to (positional arguments)

## Configuration

### Wallet Configuration

The wallet uses `wallet_config.toml` for configuration. An example configuration:

```toml
my_keys = [
    { public = "alice.pub.pem", private = "alice.priv.cbor" }
]
default_node = "127.0.0.1:9000"

[[contacts]]
name = "Alice"
key = "alice.pub.pem"

[[contacts]]
name = "Bob"
key = "bob.pub.pem"

[fee_config]
fee_type = "Percent"
value = 0.1
```

You can generate a default config file:

```bash
cd wallet
cargo run -- generate-config --output wallet_config.toml
```

## Additional Utilities

The `lib` crate includes several utility binaries:

- **`key_gen`** - Generate cryptographic key pairs
- **`block_gen`** - Generate a block file (useful for testing)
- **`block_print`** - Print block information from a file
- **`tx_gen`** - Generate a transaction file
- **`tx_print`** - Print transaction information from a file

## Network Architecture

- **Nodes** communicate via TCP connections
- **Miners** connect to nodes to fetch templates and submit blocks
- **Wallets** connect to nodes to query UTXOs and submit transactions
- Nodes broadcast new blocks and transactions to all connected peers

## Development

Build all workspace members:

```bash
cargo build
```

Run tests:

```bash
cargo test
```

## Notes

- The blockchain database (default: `./blockchain_db`) persists the blockchain state between node restarts
- Each node uses its own database directory to store blocks, UTXOs, mempool, and metadata
- The genesis block is automatically created when the first block is mined on an empty blockchain
- Multiple miners can connect to the same node and compete to mine blocks
- Multiple nodes can run simultaneously, each with its own database and port
- Nodes automatically sync with peers and maintain consensus on the longest valid chain
- The wallet TUI requires a terminal that supports ANSI escape codes
