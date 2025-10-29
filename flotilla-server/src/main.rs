use anyhow::Result;
use clap::Parser;
use std::{collections::HashMap, fs, net::SocketAddr};
use tracing_subscriber::EnvFilter;

use serde::Deserialize;

use flotilla_core::raft_node::{RaftNode, RaftNodeConfig};

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
    node_id: Option<i32>,
    config_file: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_logger()?;

    let node_config = parse_config()?;

    let mut node = RaftNode::new(node_config.clone());

    println!("{:?}", node);

    node.run().await?;

    Ok(())
}

fn parse_config() -> Result<RaftNodeConfig> {
    let args = NodeConfig::parse();
    let config_path = match args.config_file {
        Some(file) => file,
        None => String::from("resources/config.toml"),
    };
    let config_file = fs::read_to_string(config_path)?;
    let mut cfg: Config = toml::from_str(&config_file)?;
    if let Some(id) = args.node_id {
        cfg.node.node_id = Some(id);
    }

    let config = RaftNodeConfig::new(
        cfg.node.node_id.expect("no id").to_string(),
        *cfg.cluster
            .nodes
            .get(&cfg.node.node_id.expect("no id").to_string())
            .expect("could not get addr from node config."),
        cfg.cluster.nodes,
    );
    Ok(config)
}

fn setup_logger() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    Ok(())
}
