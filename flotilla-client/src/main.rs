use anyhow::Result;
use clap::{Parser, Subcommand};
use flotilla_core::raft_rpc::{
    GetRequest, SetRequest, get_response, key_value_service_client::KeyValueServiceClient,
    set_response,
};
use tokio::time::Instant;

#[derive(Subcommand, Debug)]
enum Command {
    Get{ key: String },
    Set { key: String, value: String },
    #[command(name = "rm")]
    Delete { key: String },
}

#[derive(Parser, Debug)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    leader_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut client = KeyValueServiceClient::connect(cli.leader_url).await?;
    let timer = Instant::now();
    match &cli.command {
        Command::Get { key } => {
            let res = client.get(GetRequest{key: key.to_string()}).await?;
            println!("{:?}", res);
        }
        Command::Set { key, value } => {
            let res = client.set(SetRequest{key: key.to_string(), value: value.to_string()}).await?;
            println!("{:?}", res);
        }
        _ => {}
    }
    println!("{:?}", timer.elapsed());

    Ok(())
}
