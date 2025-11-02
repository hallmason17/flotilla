# Flotilla

A distributed key-value store built on the Raft consensus algorithm in Rust.

## Overview

Flotilla is a fault-tolerant distributed key-value store that implements the [Raft consensus algorithm](https://raft.github.io/) for maintaining consistency across a cluster of nodes. The project demonstrates building reliable distributed systems through Rust's safety guarantees and is structured as a complete client-server application with a reusable Raft library at its core.

## Architecture

The project is organized as a Cargo workspace with three crates:

- **`flotilla-core`**: A reusable Raft consensus library implementing the core protocol
- **`flotilla-server`**: Server nodes that use `flotilla-core` to form a distributed key-value store cluster
- **`flotilla-client`**: Client for interacting with the cluster (GET, SET, DELETE operations)

This separation allows the Raft implementation to be used for other state machines beyond key-value storage.

## Current Status

This is an active work-in-progress implementation. Currently implemented:

-  **Leader Election**: Nodes can elect a leader through the Raft election protocol
-  **Log Replication**: In active development
-  **Key-Value Store**: Basic operations with Raft backing
-  **Persistence**: Planned
-  **Log Compaction/Snapshotting**: Planned

## Features

### flotilla-core
- Async Raft implementation using Tokio
- Leader election with randomized timeouts
- Log replication protocol

### TODO
- Comprehensive unit tests
- Type-safe state machine trait for custom applications

### flotilla-server
- Key-value storage state machine
- Cluster node implementation
- Request forwarding to leader
- Strong consistency guarantees via Raft

### flotilla-client
- Simple CLI for interacting with the cluster
- Automatic leader discovery
- GET, SET, DELETE operations

## Getting Started

### Prerequisites

- Rust 1.70 or higher
- Cargo

### Building

```bash
git clone https://github.com/hallmason17/flotilla.git
cd flotilla
cargo build --release
```

### Running Tests

```bash
# Test all workspace members
cargo test --workspace

# Test individual crates
cargo test -p flotilla-core
cargo test -p flotilla-server
cargo test -p flotilla-client
```

### Running a Cluster

```bash
# Terminal 1: Start first node
cargo run -p flotilla-server -- --id 1 --port 5001

# Terminal 2: Start second node
cargo run -p flotilla-server -- --id 2 --port 5002

# Terminal 3: Start third node
cargo run -p flotilla-server -- --id 3 --port 5003

# Terminal 4: Use the client
cargo run -p flotilla-client -- set mykey myvalue
cargo run -p flotilla-client -- get mykey
cargo run -p flotilla-client -- delete mykey
```

## Project Structure
```
flotilla/
├── flotilla-core/       # Raft consensus library
├── flotilla-server/     # KV store server implementation
├── flotilla-client/     # Client CLI
└── Cargo.toml           # Workspace configuration

```

## How It Works

1. **Consensus Layer** (`flotilla-core`): Implements Raft protocol ensuring all nodes agree on the order of operations
2. **State Machine** (`flotilla-server`): Applies committed log entries to a key-value store
3. **Client Interface** (`flotilla-client`): Sends requests to the cluster, which are replicated and executed consistently

When a client issues a SET command:
- The client contacts any server node
- Non-leader nodes forward to the current leader
- The leader appends the operation to its log
- The leader replicates the entry to follower nodes
- Once a majority confirms, the entry is committed
- All nodes apply the committed entry to their key-value stores

## Project Goals

This project is a learning exercise focused on:

- Understanding distributed consensus algorithms deeply
- Exploring Rust's type system for modeling complex state machines
- Building fault-tolerant systems with proper error handling
- Creating a practical application (KV store) on top of Raft

## Resources

- [The Raft Paper](https://raft.github.io/raft.pdf) - Original Raft consensus algorithm paper
- [Raft Visualization](https://raft.github.io/) - Interactive visualization of Raft
- [Students' Guide to Raft](https://thesquareplanet.com/blog/students-guide-to-raft/) - Practical implementation guidance

## Contributing

This is primarily a personal learning project, but suggestions and feedback are welcome! Feel free to open an issue if you spot any bugs or have ideas for improvements.

## License

[MIT](LICENSE)
