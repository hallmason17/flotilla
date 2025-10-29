pub mod raft_node;
pub mod rpc;
pub mod state;
pub mod raft_rpc {
    tonic::include_proto!("raft");
}
use anyhow::Result;
use std::net::SocketAddr;

pub use raft_node::{RaftMessage, RaftNode, RaftNodeConfig};
pub use state::{Command, LogEntry, NodeState};

use raft_rpc::raft_service_client::RaftServiceClient;
use raft_rpc::{
    AppendEntriesRequest, AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse,
};

pub(crate) async fn send_append_entries(
    request: AppendEntriesRequest,
    peer_addr: SocketAddr,
) -> Result<AppendEntriesResponse> {
    let mut client =
        RaftServiceClient::connect(format!("http://{}:{}", peer_addr.ip(), peer_addr.port()))
            .await?;
    let res = client.append_entries(request).await?;
    Ok(res.into_inner())
}

pub(crate) async fn send_request_vote(
    request: RequestVoteRequest,
    peer_addr: SocketAddr,
) -> Result<RequestVoteResponse> {
    let mut client =
        RaftServiceClient::connect(format!("http://{}:{}", peer_addr.ip(), peer_addr.port()))
            .await?;
    let res = client.request_vote(request).await?;
    Ok(res.into_inner())
}
