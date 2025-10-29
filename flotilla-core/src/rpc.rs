use anyhow::Result;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tonic::{Request, Response, Status};
use tracing::info;

use crate::raft_rpc::*;

use crate::RaftMessage;
use crate::raft_rpc::{
    key_value_service_server::KeyValueService, raft_service_server::RaftService,
};

pub(crate) struct RaftServiceImpl {
    pub tx: mpsc::Sender<RaftMessage>,
}

#[tonic::async_trait]
impl RaftService for RaftServiceImpl {
    async fn request_vote(
        &self,
        request: Request<RequestVoteRequest>,
    ) -> Result<Response<RequestVoteResponse>, Status> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(RaftMessage::VoteRequest {
                message: request.into_inner(),
                response: tx,
            })
            .await
            .expect("Could not send message");
        let response = rx.await.expect("No response");
        info!("{:?}", response);
        Ok(Response::new(response))
    }
    async fn append_entries(
        &self,
        request: Request<AppendEntriesRequest>,
    ) -> Result<Response<AppendEntriesResponse>, Status> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(RaftMessage::AppendEntries {
                message: request.into_inner(),
                response: tx,
            })
            .await
            .expect("Could not send message");
        let response = rx.await.expect("No response");
        info!("{:?}", response);
        Ok(Response::new(response))
    }
}

#[tonic::async_trait]
impl KeyValueService for RaftServiceImpl {
    async fn delete(
        &self,
        request: Request<DeleteRequest>,
    ) -> Result<Response<DeleteResponse>, Status> {
        let (tx, rx) = oneshot::channel();
        let req = request.into_inner();
        self.tx
            .send(RaftMessage::ClientDeleteRequest {
                key: req.key,
                response: tx,
            })
            .await
            .expect("Failed to send message");
        let response = rx.await.expect("No response");
        Ok(Response::new(response))
    }

    async fn get(&self, request: Request<GetRequest>) -> Result<Response<GetResponse>, Status> {
        let (tx, rx) = oneshot::channel();
        let req = request.into_inner();
        self.tx
            .send(RaftMessage::ClientGetRequest {
                key: req.key,
                response: tx,
            })
            .await
            .expect("Failed to send message");
        let response = rx.await.expect("No response");
        Ok(Response::new(response))
    }

    async fn set(&self, request: Request<SetRequest>) -> Result<Response<SetResponse>, Status> {
        let (tx, rx) = oneshot::channel();
        let req = request.into_inner();
        self.tx
            .send(RaftMessage::ClientSetRequest {
                key: req.key,
                value: req.value,
                response: tx,
            })
            .await
            .expect("Failed to send message");
        let response = rx.await.expect("No response");
        Ok(Response::new(response))
    }
}
