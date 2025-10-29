use std::{collections::HashMap, time::Duration};

use anyhow::Result;
use bincode::error::DecodeError;
use bincode::{Decode, Encode, config};
use rand::Rng;
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{self, JoinSet};
use tokio::time::Instant;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tracing::{debug, info};

use raft_rpc::raft_service_server::RaftService;

use raft_rpc::{AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse};

use raft_rpc::AppendEntriesRequest;

use raft_rpc::key_value_service_server::{KeyValueService, KeyValueServiceServer};
use raft_rpc::raft_service_client::RaftServiceClient;
use raft_rpc::raft_service_server::RaftServiceServer;
use raft_rpc::{
    DeleteRequest, DeleteResponse, GetRequest, GetResponse, NotFound, SetRequest, SetResponse,
    delete_response, get_response, set_response,
};

pub mod raft_rpc {
    tonic::include_proto!("raft");
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
enum NodeState {
    Follower,
    Candidate,
    Leader,
}
#[allow(dead_code)]
#[derive(Debug, Clone, Decode, Encode)]
enum Command {
    Set { key: String, value: String },
    Delete { key: String },
}
#[allow(dead_code)]
#[derive(Debug, Clone, Decode, Encode)]
struct LogEntry {
    term: u64,
    idx: u64,
    command: Command,
}
impl From<LogEntry> for raft_rpc::LogEntry {
    fn from(entry: LogEntry) -> Self {
        let command_bytes = bincode::encode_to_vec(&entry.command, config::standard())
            .expect("Failed to serialize command");

        raft_rpc::LogEntry {
            term: entry.term,
            index: entry.idx,
            command: command_bytes,
        }
    }
}

impl TryFrom<raft_rpc::LogEntry> for LogEntry {
    type Error = DecodeError;

    fn try_from(proto: raft_rpc::LogEntry) -> Result<Self, Self::Error> {
        let (command, _): (Command, usize) =
            bincode::decode_from_slice(&proto.command, config::standard())?;

        Ok(LogEntry {
            term: proto.term,
            idx: proto.index,
            command,
        })
    }
}
#[allow(dead_code)]
#[derive(Debug)]
pub struct RaftNode {
    state: NodeState,
    current_term: u64,
    commit_index: u64,
    last_applied: u64,
    votes_received: u64,
    voted_for: Option<String>,
    id: String,
    addr: SocketAddr,
    leader_heartbeat_handle: Option<task::JoinHandle<()>>,
    last_heartbeat: Instant,
    election_timeout_ms: Duration,
    peers: HashMap<String, SocketAddr>,
    store: HashMap<String, String>,
    next_index: HashMap<String, u64>,
    match_index: HashMap<String, u64>,
    log: Vec<LogEntry>,
}

#[derive(Debug)]
enum RaftMessage {
    VoteRequest {
        message: RequestVoteRequest,
        response: oneshot::Sender<RequestVoteResponse>,
    },
    AppendEntries {
        message: AppendEntriesRequest,
        response: oneshot::Sender<AppendEntriesResponse>,
    },
    ClientGetRequest {
        key: String,
        response: oneshot::Sender<GetResponse>,
    },
    ClientSetRequest {
        key: String,
        value: String,
        response: oneshot::Sender<SetResponse>,
    },
    ClientDeleteRequest {
        key: String,
        response: oneshot::Sender<DeleteResponse>,
    },
    ElectionTimeout,
}

struct RaftServiceImpl {
    tx: mpsc::Sender<RaftMessage>,
}

#[derive(Debug, Clone)]
pub struct RaftNodeConfig {
    id: String,
    pub addr: SocketAddr,
    peers: HashMap<String, SocketAddr>,
}
impl RaftNodeConfig {
    pub fn new(id: String, addr: SocketAddr, peers: HashMap<String, SocketAddr>) -> Self {
        RaftNodeConfig { id, addr, peers }
    }
}

impl RaftNode {
    pub fn new(config: RaftNodeConfig) -> Self {
        let timeout = rand::rng().random_range(150..=300);
        RaftNode {
            state: NodeState::Follower,
            current_term: 0,
            commit_index: 0,
            last_applied: 0,
            votes_received: 0,
            voted_for: None,
            id: config.id,
            addr: config.addr,
            leader_heartbeat_handle: None,
            last_heartbeat: Instant::now(),
            election_timeout_ms: Duration::from_millis(timeout),
            peers: config.peers,
            store: HashMap::new(),
            next_index: HashMap::new(),
            match_index: HashMap::new(),
            log: vec![],
        }
    }

    fn start_grpc_server(&self, tx: mpsc::Sender<RaftMessage>) {
        info!("Cluster socket listening at {}", self.addr);
        let service = RaftServiceImpl { tx: tx.clone() };
        let service1 = RaftServiceImpl { tx: tx.clone() };
        let addr = self.addr;
        tokio::spawn(async move {
            let _ = Server::builder()
                .add_service(RaftServiceServer::new(service))
                .add_service(KeyValueServiceServer::new(service1))
                .serve(addr)
                .await;
        });
    }

    fn start_heartbeat_timer(&mut self, tx: mpsc::Sender<RaftMessage>) {
        match self.state {
            NodeState::Follower | NodeState::Candidate => {
                if let Some(h) = self.leader_heartbeat_handle.take() {
                    h.abort();
                }
                self.start_election_timeout(tx.clone());
            }
            NodeState::Leader => {
                let handle = self.start_leader_heartbeat();
                self.leader_heartbeat_handle = Some(handle);
            }
        }
    }
    fn start_election_timeout(&self, tx: mpsc::Sender<RaftMessage>) -> tokio::task::JoinHandle<()> {
        let interval = self.election_timeout_ms;
        tokio::spawn(async move {
            let mut int_timer = tokio::time::interval(interval);
            int_timer.tick().await;
            loop {
                int_timer.tick().await;
                let _ = tx.send(RaftMessage::ElectionTimeout).await;
            }
        })
    }
    fn start_leader_heartbeat(&self) -> tokio::task::JoinHandle<()> {
        let interval = Duration::from_millis(100);
        let request = AppendEntriesRequest {
            term: self.current_term,
            leader_id: self.id.clone(),
            prev_log_index: self.get_last_log_index(),
            prev_log_term: self.get_last_log_term(),
            entries: Vec::new(),
            leader_commit: self.get_last_log_index(),
        };
        let peers = self.peers.clone();
        let addr = self.addr;

        tokio::spawn(async move {
            let mut int_timer = tokio::time::interval(interval);
            int_timer.tick().await;
            loop {
                info!("Sending heartbeats");
                int_timer.tick().await;
                for &peer_addr in peers.values().filter(|add| **add != addr) {
                    let req = request.clone();
                    tokio::spawn(async move { send_append_entries(req, peer_addr).await });
                }
            }
        })
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        let (tx, mut rx) = mpsc::channel(100);

        self.start_grpc_server(tx.clone());
        self.start_heartbeat_timer(tx.clone());

        while let Some(msg) = rx.recv().await {
            match msg {
                RaftMessage::VoteRequest { message, response } => {
                    let resp = self.handle_vote_request(message);
                    let _ = response.send(resp);
                }
                RaftMessage::AppendEntries { message, response } => {
                    let resp = self.handle_append_entries(message);
                    let _ = response.send(resp);
                }
                RaftMessage::ElectionTimeout => self.check_election_timeout().await?,
                RaftMessage::ClientGetRequest { key, response } => {
                    let resp = self.handle_get_request(&key);
                    let _ = response.send(resp);
                }
                RaftMessage::ClientSetRequest {
                    key,
                    value,
                    response,
                } => {
                    let resp = self.handle_set_request(&key, &value).await;
                    let _ = response.send(resp);
                }
                RaftMessage::ClientDeleteRequest { key, response } => {
                    let resp = self.handle_delete_request(&key);
                    let _ = response.send(resp);
                }
            }
        }
        Ok(())
    }

    fn handle_delete_request(&self, _key: &str) -> DeleteResponse {
        DeleteResponse {
            result: Some(delete_response::Result::Error(raft_rpc::Error {
                message: "Not implemented".to_string(),
                leader_id: None,
                leader_addr: None,
            })),
        }
    }

    async fn handle_set_request(&mut self, key: &str, value: &str) -> SetResponse {
        if !matches!(self.state, NodeState::Leader) {
            return SetResponse {
                result: Some(set_response::Result::Error(raft_rpc::Error {
                    message: "Not leader".to_string(),
                    leader_id: None,
                    leader_addr: None,
                })),
            };
        }
        let log_entry = LogEntry {
            term: self.current_term,
            idx: self.log.len() as u64 + 1,
            command: Command::Set {
                key: key.to_string(),
                value: value.to_string(),
            },
        };
        self.log.push(log_entry.clone());
        let append_result = self.replicate_log(log_entry.idx).await;
        match append_result {
            Ok(()) => {
                self.commit_index = log_entry.idx;
                self.store.insert(key.to_string(), value.to_string());
                SetResponse {
                    result: Some(set_response::Result::Success(raft_rpc::Success {})),
                }
            }
            Err(e) => SetResponse {
                result: Some(set_response::Result::Error(raft_rpc::Error {
                    message: format!("Replication failed: {}", e),
                    leader_id: None,
                    leader_addr: None,
                })),
            },
        }
    }

    fn handle_get_request(&self, key: &str) -> GetResponse {
        if !matches!(self.state, NodeState::Leader) {
            return GetResponse {
                result: Some(get_response::Result::Error(raft_rpc::Error {
                    message: "Not leader".to_string(),
                    leader_id: None,
                    leader_addr: None,
                })),
            };
        }

        let result = if let Some(value) = self.store.get(key) {
            get_response::Result::Value(value.clone())
        } else {
            get_response::Result::NotFound(NotFound {})
        };

        GetResponse {
            result: Some(result),
        }
    }

    fn handle_vote_request(&mut self, message: RequestVoteRequest) -> RequestVoteResponse {
        info!(
            "Handling vote request from {} for term {}",
            message.candidate_id, message.term
        );

        if self.current_term == message.term && self.voted_for.is_some() {
            return RequestVoteResponse {
                term: self.current_term,
                vote_granted: false,
            };
        }

        if message.term > self.current_term {
            self.state = NodeState::Follower;
            self.current_term = message.term;
            self.voted_for = None;
        }

        self.voted_for = Some(message.candidate_id.clone());
        self.last_heartbeat = Instant::now();

        info!(
            "Granted vote to {} for term {}",
            message.candidate_id, message.term
        );

        RequestVoteResponse {
            term: self.current_term,
            vote_granted: true,
        }
    }

    fn handle_append_entries(&mut self, message: AppendEntriesRequest) -> AppendEntriesResponse {
        let mut success = false;
        if message.term > self.current_term {
            self.current_term = message.term;
            self.state = NodeState::Follower;
            self.voted_for = None;
        }

        if message.entries.is_empty() {
            info!("Received heartbeat from leader");
            self.last_heartbeat = Instant::now();
            self.current_term = message.term;
            return AppendEntriesResponse {
                term: self.current_term,
                success: true,
            };
        }

        if message.term >= self.current_term && message.prev_log_index == 0
            || (message.prev_log_index <= self.log.len() as u64
                && self.log[message.prev_log_index as usize - 1].term == message.prev_log_term)
        {
            success = true;
            let start_idx = message.prev_log_index as usize;
            for entry in message.entries {
                info!("Adding {:?} to log from leader!", entry);
                if start_idx < self.log.len() {
                    self.log[start_idx] =
                        LogEntry::try_from(entry).expect("could not convert to LogEntry");
                } else {
                    self.log
                        .push(LogEntry::try_from(entry).expect("could not convert to LogEntry"));
                }
            }
        }

        AppendEntriesResponse {
            term: self.current_term,
            success,
        }
    }

    async fn replicate_log(&mut self, index: u64) -> Result<()> {
        let entry = self
            .log
            .get(index as usize - 1)
            .expect("No log entries")
            .clone();
        let mut tasks = JoinSet::new();
        for (id, peer_addr) in &self.peers {
            if id == &self.id {
                continue;
            }
            let request = AppendEntriesRequest {
                term: self.current_term,
                leader_id: self.id.clone(),
                prev_log_index: self.get_last_log_index() - 1,
                prev_log_term: self.get_last_log_term(),
                entries: vec![entry.clone().into()],
                leader_commit: self.commit_index,
            };
            info!("Sending request: {:?}", request);
            let peer_addr = peer_addr.clone();
            tasks.spawn(async move { send_append_entries(request, peer_addr).await });
        }

        let needed = (self.peers.len() / 2) + 1;
        let mut success_count = 1;

        while let Some(result) = tasks.join_next().await {
            if let Ok(Ok(response)) = result
                && response.success
            {
                success_count += 1;
                if success_count >= needed {
                    tasks.abort_all();
                    return Ok(());
                }
            }
        }

        Err(anyhow::anyhow!("Failed to achieve majority"))
    }

    async fn check_election_timeout(&mut self) -> Result<()> {
        if matches!(self.state, NodeState::Follower)
            && self.last_heartbeat.elapsed() > self.election_timeout_ms
        {
            let _ = self.start_election().await;
        }
        Ok(())
    }

    fn get_last_log_term(&self) -> u64 {
        self.log.last().map(|e| e.term).unwrap_or(0)
    }
    fn get_last_log_index(&self) -> u64 {
        self.log.last().map(|e| e.idx).unwrap_or(0)
    }

    async fn start_election(&mut self) -> Result<()> {
        self.state = NodeState::Candidate;
        self.current_term += 1;
        self.votes_received += 1;
        self.voted_for = Some(self.id.clone());
        self.last_heartbeat = Instant::now();

        info!("Starting election for term {}.", self.current_term);

        let mut requests = JoinSet::new();

        for &peer_addr in self.peers.values().filter(|addr| **addr != self.addr) {
            let last_log_index = self.get_last_log_index();
            let last_log_term = self.get_last_log_term();
            let request = RequestVoteRequest {
                term: self.current_term,
                candidate_id: self.id.clone(),
                last_log_index,
                last_log_term,
            };
            requests.spawn(async move { send_request_vote(request, peer_addr).await });
        }

        let needed_votes = self.peers.len() / 2 + 1;

        while let Some(res) = requests.join_next().await {
            if let Ok(Ok(resp)) = res {
                if resp.term > self.current_term {
                    let _ = self.step_down(resp.term);
                    requests.abort_all();
                    return Ok(());
                }

                if resp.vote_granted {
                    self.votes_received += 1;
                    if self.votes_received as usize >= needed_votes {
                        info!("Election succeeded, becoming leader");
                        let _ = self.become_leader();
                        requests.abort_all();
                        return Ok(());
                    }
                }
            }
        }

        if !matches!(self.state, NodeState::Leader) {
            info!("Election failed, not enough votes");
            self.state = NodeState::Follower;
        }

        Ok(())
    }

    fn become_leader(&mut self) -> Result<()> {
        info!("Becoming leader");
        let (tx, _rx) = mpsc::channel(1);
        self.state = NodeState::Leader;
        self.start_heartbeat_timer(tx.clone());
        Ok(())
    }
    fn step_down(&mut self, new_term: u64) -> Result<()> {
        info!("Stepping down");
        self.state = NodeState::Follower;
        self.voted_for = None;
        self.current_term = new_term;
        self.last_heartbeat = Instant::now();
        Ok(())
    }
}

async fn send_append_entries(
    request: AppendEntriesRequest,
    peer_addr: SocketAddr,
) -> Result<AppendEntriesResponse> {
    let mut client =
        RaftServiceClient::connect(format!("http://{}:{}", peer_addr.ip(), peer_addr.port()))
            .await?;
    let res = client.append_entries(request).await?;
    Ok(res.into_inner())
}

async fn send_request_vote(
    request: RequestVoteRequest,
    peer_addr: SocketAddr,
) -> Result<RequestVoteResponse> {
    let mut client =
        RaftServiceClient::connect(format!("http://{}:{}", peer_addr.ip(), peer_addr.port()))
            .await?;
    let res = client.request_vote(request).await?;
    Ok(res.into_inner())
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
