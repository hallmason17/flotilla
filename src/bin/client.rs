use anyhow::Result;
use flotilla::raft::raft_rpc::{
    GetRequest, SetRequest, get_response, key_value_service_client::KeyValueServiceClient,
    set_response,
};
use tokio::time::Instant;

#[tokio::main]
async fn main() -> Result<()> {
    let mut client = KeyValueServiceClient::connect("http://localhost:5001").await?;
    let timer = Instant::now();
    let response = client
        .set(SetRequest {
            key: "hello".to_string(),
            value: "world".to_string(),
        })
        .await?;
    let elapsed = timer.elapsed();
    match response.into_inner().result {
        Some(set_response::Result::Success(_)) => {
            println!("Set success!");
        }
        Some(set_response::Result::Error(e)) => {
            println!("Error {}", e.message);
            if let Some(leader) = e.leader_addr {
                println!("Try leader: {}", leader);
            }
        }
        _ => println!("No re"),
    }
    println!("{:?}", elapsed);

    let timer = Instant::now();
    let response = client
        .get(GetRequest {
            key: "hello".to_string(),
        })
        .await?;
    let elapsed = timer.elapsed();
    if let Some(get_response::Result::Value(v)) = response.into_inner().result {
        println!("{:?}", v);
        println!("{:?}", elapsed);
    }
    Ok(())
}
