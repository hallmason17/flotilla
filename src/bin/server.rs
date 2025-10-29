use anyhow::Result;
use clap::Parser;
use std::{collections::HashMap, fs, net::SocketAddr};
use tracing_subscriber::EnvFilter;

use serde::Deserialize;

use flotilla::raft::{RaftNode, RaftNodeConfig};

#[derive(Debug, Deserialize)]
struct Config {
    node: NodeConfig,
    cluster: ClusterConfig,
}
#[derive(Debug, Deserialize)]
struct ClusterConfig {
    nodes: HashMap<String, SocketAddr>,
}
#[derive(Debug, Deserialize, Parser)]
struct NodeConfig {
    id: Option<i32>,
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_logger();
    let args = NodeConfig::parse();
    let config_file = fs::read_to_string("resources/config.toml")?;
    let mut cfg: Config = toml::from_str(&config_file)?;
    if let Some(id) = args.id {
        cfg.node.id = Some(id);
    }

    let node_cfg = RaftNodeConfig::new(
        cfg.node.id.expect("no id").to_string(),
        *cfg.cluster
            .nodes
            .get(&cfg.node.id.expect("no id").to_string())
            .expect("could not get addr from node config."),
        cfg.cluster.nodes,
    );

    let mut node = RaftNode::new(node_cfg.clone());
    println!("{:?}", node);
    node.run().await?;

    Ok(())
}

fn setup_logger() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}
