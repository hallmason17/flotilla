//! Raft consensus algorithm implementation.
//!
//! This module implements the Raft distributed consensus protocol, which allows
//! a cluster of nodes to agree on a replicated state machine. The implementation
//! includes leader election, log replication, and a key-value store as the state machine.
//!
//! # Architecture
//!
//! - **NodeState**: Nodes can be Followers, Candidates, or Leaders
//! - **Elections**: Triggered by election timeouts when no heartbeat is received
//! - **Log Replication**: Leaders replicate log entries to followers
//! - **State Machine**: A simple key-value store backed by the replicated log

#![deny(missing_docs)]
use rand::Rng;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tonic::transport::Server;
use tracing::info;

use crate::Command;
use crate::LogEntry;
use crate::NodeState;
use crate::error::RaftError;
use crate::error::RaftResult;
use crate::raft_rpc::key_value_service_server::KeyValueServiceServer;
use crate::raft_rpc::raft_service_server::RaftServiceServer;
use crate::raft_rpc::{
    self, AppendEntriesRequest, AppendEntriesResponse, DeleteResponse, GetResponse, NotFound,
    RequestVoteRequest, RequestVoteResponse, SetResponse, delete_response, get_response,
    set_response,
};
use crate::rpc::RaftServiceImpl;
use crate::send_append_entries;
use crate::send_request_vote;

/// Messages that can be sent to the Raft node's message processing loop.
///
/// These messages represent both internal Raft protocol messages (voting, log replication)
/// and client requests (get, set, delete operations).
#[derive(Debug)]
pub enum RaftMessage {
    /// Request for a vote during leader election.
    VoteRequest {
        /// The vote request from a candidate node
        message: RequestVoteRequest,
        /// Channel to send the vote response back to the requester
        response: oneshot::Sender<RequestVoteResponse>,
    },
    /// Request to append entries to the log (used for both heartbeats and replication).
    AppendEntries {
        /// The append entries request from the leader
        message: AppendEntriesRequest,
        /// Channel to send the response back to the leader
        response: oneshot::Sender<AppendEntriesResponse>,
    },
    /// Client request to retrieve a value from the key-value store.
    ClientGetRequest {
        /// The key to retrieve
        key: String,
        /// Channel to send the response back to the client
        response: oneshot::Sender<GetResponse>,
    },
    /// Client request to set a value in the key-value store.
    ClientSetRequest {
        /// The key to set
        key: String,
        /// The value to associate with the key
        value: String,
        /// Channel to send the response back to the client
        response: oneshot::Sender<SetResponse>,
    },
    /// Client request to delete a value from the key-value store.
    ClientDeleteRequest {
        /// The key to delete
        key: String,
        /// Channel to send the response back to the client
        response: oneshot::Sender<DeleteResponse>,
    },
}

/// A node in the Raft cluster.
///
/// Each node maintains its own state, log, and understanding of the cluster.
/// Nodes can be in one of three states: Follower, Candidate, or Leader.
///
/// # Raft Protocol Overview
///
/// - **Followers**: Passive nodes that respond to requests from leaders and candidates
/// - **Candidates**: Nodes attempting to become leader through elections
/// - **Leaders**: Handle all client requests and replicate log entries to followers
#[allow(dead_code)]
#[derive(Debug)]
pub struct RaftNode {
    /// Current state of this node (Follower, Candidate, or Leader)
    state: NodeState,
    
    /// Latest term this node has seen (monotonically increasing)
    current_term: u64,
    
    /// Index of highest log entry known to be committed
    commit_index: u64,
    
    /// Index of highest log entry applied to state machine
    last_applied: u64,
    
    /// Number of votes received in current election (used when Candidate)
    votes_received: u64,
    
    /// CandidateId that received vote in current term (or None)
    voted_for: Option<String>,
    
    /// Unique identifier for this node
    id: String,
    
    /// Network address where this node listens
    addr: SocketAddr,
    
    /// Handle to the leader's heartbeat task (Some when Leader, None otherwise)
    leader_heartbeat_handle: Option<JoinHandle<()>>,
    
    /// Timestamp of last heartbeat received from leader
    last_heartbeat: Instant,
    
    /// Duration to wait before starting election (randomized to avoid split votes)
    election_timeout_ms: Duration,
    
    /// Map of peer node IDs to their network addresses
    peers: HashMap<String, SocketAddr>,
    
    /// For each peer, index of the next log entry to send (Leader only)
    next_index: HashMap<String, u64>,
    
    /// For each peer, index of highest log entry known to be replicated (Leader only)
    match_index: HashMap<String, u64>,
    
    /// Replicated log of commands
    log: Vec<LogEntry>,
    
    /// The key-value store state machine
    store: HashMap<String, String>,
}

/// Configuration for creating a new Raft node.
///
/// Contains all the information needed to initialize a node and connect
/// it to the cluster.
#[derive(Debug, Clone)]
pub struct RaftNodeConfig {
    /// Unique identifier for the node
    id: String,
    
    /// Network address where the node will listen
    pub addr: SocketAddr,
    
    /// Map of peer node IDs to their network addresses
    peers: HashMap<String, SocketAddr>,
}

impl RaftNodeConfig {
    /// Creates a new Raft node configuration.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier for this node
    /// * `addr` - Network address for this node to listen on
    /// * `peers` - Map of all peer nodes in the cluster (including this node)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = RaftNodeConfig::new(
    ///     "node1".to_string(),
    ///     "127.0.0.1:5001".parse().unwrap(),
    ///     HashMap::from([
    ///         ("node2".to_string(), "127.0.0.1:5002".parse().unwrap()),
    ///         ("node3".to_string(), "127.0.0.1:5003".parse().unwrap()),
    ///     ])
    /// );
    /// ```
    pub fn new(id: String, addr: SocketAddr, peers: HashMap<String, SocketAddr>) -> Self {
        RaftNodeConfig { id, addr, peers }
    }
}

impl RaftNode {
    /// Creates a new Raft node from the given configuration.
    ///
    /// The node starts in the Follower state with term 0 and a randomized
    /// election timeout to prevent split votes.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration containing node ID, address, and peer information
    ///
    /// # Returns
    ///
    /// A new `RaftNode` initialized as a Follower
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

    /// Runs the main event loop for the Raft node.
    ///
    /// This method starts the gRPC server and enters the main message processing loop.
    /// It handles incoming messages (vote requests, append entries, client requests) and
    /// periodically checks for election timeouts.
    ///
    /// # Returns
    ///
    /// An error if the node fails to run properly
    ///
    /// # Event Loop
    ///
    /// The loop processes:
    /// - Vote requests from candidates
    /// - Append entries requests from leaders
    /// - Client get/set/delete requests
    /// - Election timeout checks (with randomized timing)
    pub async fn run(&mut self) -> anyhow::Result<()> {
        let (tx, mut rx) = mpsc::channel(100);

        self.start_grpc_server(tx.clone());
        //self.start_heartbeat_timer(tx.clone());

        let sleep = tokio::time::sleep(Duration::ZERO);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                    Some(RaftMessage::VoteRequest { message, response }) => {
                        let resp = self.handle_vote_request(message);
                        let _ = response.send(resp);
                    }
                    Some(RaftMessage::AppendEntries { message, response }) => {
                        let resp = self.handle_append_entries(message);
                        let _ = response.send(resp);
                    }
                    Some(RaftMessage::ClientGetRequest { key, response } )=> {
                        let resp = self.handle_get_request(&key);
                        let _ = response.send(resp);
                    }
                    Some(RaftMessage::ClientSetRequest {
                        key,
                        value,
                        response,
                    }) => {
                        let resp = self.handle_set_request(&key, &value).await;
                        let _ = response.send(resp);
                    }
                    Some(RaftMessage::ClientDeleteRequest { key, response } )=> {
                        let resp = self.handle_delete_request(&key);
                        let _ = response.send(resp);
                    }
                    None => break,
                }
                    }
                _ = sleep.as_mut() => {
                    self.check_election_timeout().await?;
                    sleep.as_mut().set(tokio::time::sleep(self.get_random_election_timeout()));
                }
            }
        }
        Ok(())
    }

    /// Starts the gRPC server for this node.
    ///
    /// The server handles both Raft protocol messages (RequestVote, AppendEntries)
    /// and client key-value operations (Get, Set, Delete).
    ///
    /// # Arguments
    ///
    /// * `tx` - Channel sender for forwarding messages to the main event loop
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

    /// Starts or stops the heartbeat timer based on the node's current state.
    ///
    /// - **Leader**: Starts sending periodic heartbeats to all followers
    /// - **Follower/Candidate**: Stops any existing heartbeat task
    ///
    /// Leaders send heartbeats every 100ms to prevent followers from starting elections.
    fn start_heartbeat_timer(&mut self) {
        match self.state {
            NodeState::Follower | NodeState::Candidate => {
                if let Some(h) = self.leader_heartbeat_handle.take() {
                    h.abort();
                }
            }
            NodeState::Leader => {
                let handle = self.start_leader_heartbeat();
                self.leader_heartbeat_handle = Some(handle);
            }
        }
    }

    /// Spawns a task that sends periodic heartbeats to all peers.
    ///
    /// Heartbeats are empty AppendEntries requests sent every 100ms to maintain
    /// leadership and prevent followers from timing out and starting elections.
    ///
    /// # Returns
    ///
    /// A handle to the spawned heartbeat task
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

    /// Generates a random election timeout duration.
    ///
    /// Returns a random duration between 150ms and 350ms. The randomization
    /// helps prevent split votes by ensuring nodes don't all timeout simultaneously.
    ///
    /// # Returns
    ///
    /// A random duration between 150ms and 350ms
    fn get_random_election_timeout(&self) -> Duration {
        let (min, max) = (150, 350);
        let timeout_ms = rand::rng().random_range(min..=max);
        info!("New heartbeat timer: {}", timeout_ms);
        Duration::from_millis(timeout_ms)
    }

    /// Handles a client delete request.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to delete (currently unused)
    ///
    /// # Returns
    ///
    /// An error response indicating the operation is not yet implemented
    ///
    /// # TODO
    ///
    /// Implement delete functionality following the same pattern as set_request
    fn handle_delete_request(&self, _key: &str) -> DeleteResponse {
        DeleteResponse {
            result: Some(delete_response::Result::Error(raft_rpc::Error {
                message: "Not implemented".to_string(),
                leader_id: None,
                leader_addr: None,
            })),
        }
    }

    /// Handles a client set request to store a key-value pair.
    ///
    /// Only the leader can process set requests. The operation involves:
    /// 1. Appending the command to the local log
    /// 2. Replicating the log entry to a majority of followers
    /// 3. Committing the entry and applying it to the state machine
    ///
    /// # Arguments
    ///
    /// * `key` - The key to set
    /// * `value` - The value to associate with the key
    ///
    /// # Returns
    ///
    /// A response indicating success or failure of the operation
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - This node is not the leader
    /// - Replication to a majority of nodes fails
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

    /// Handles a client get request to retrieve a value.
    ///
    /// Only the leader processes get requests to ensure linearizable reads.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to retrieve
    ///
    /// # Returns
    ///
    /// A response containing:
    /// - The value if the key exists
    /// - A not found response if the key doesn't exist
    /// - An error if this node is not the leader
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

    /// Handles a RequestVote RPC from a candidate node.
    ///
    /// Grants vote if:
    /// - Haven't voted for another candidate in this term
    /// - The candidate's log is at least as up-to-date as ours
    ///
    /// If the request has a higher term, steps down to follower state.
    ///
    /// # Arguments
    ///
    /// * `message` - The vote request from the candidate
    ///
    /// # Returns
    ///
    /// A response indicating whether the vote was granted and the current term
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

    /// Handles an AppendEntries RPC from the leader.
    ///
    /// This handles both heartbeats (empty entries) and actual log replication.
    ///
    /// # Behavior
    ///
    /// - **Heartbeat**: Empty entries list, updates last heartbeat timestamp
    /// - **Log replication**: Appends new entries if log consistency checks pass
    /// - **Term update**: Steps down if request has higher term
    ///
    /// # Arguments
    ///
    /// * `message` - The append entries request from the leader
    ///
    /// # Returns
    ///
    /// A response indicating success/failure and the current term
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

    /// Replicates a log entry to a majority of followers.
    ///
    /// Sends the log entry at the given index to all peers concurrently and
    /// waits until a majority have successfully replicated it.
    ///
    /// # Arguments
    ///
    /// * `index` - The log index to replicate (1-based)
    ///
    /// # Returns
    ///
    /// Ok if successfully replicated to a majority, Err otherwise
    ///
    /// # Errors
    ///
    /// Returns `RaftError::ReplicationFailure` if unable to replicate to a majority
    async fn replicate_log(&mut self, index: u64) -> RaftResult<()> {
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
            let peer_addr = *peer_addr;
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

        Err(RaftError::ReplicationFailure)
    }

    /// Checks if the election timeout has elapsed and starts an election if so.
    ///
    /// Only followers check for election timeouts. If the timeout period passes
    /// without receiving a heartbeat from the leader, the node transitions to
    /// candidate state and begins an election.
    ///
    /// # Returns
    ///
    /// Ok normally, or an error if the election process fails
    async fn check_election_timeout(&mut self) -> RaftResult<()> {
        if matches!(self.state, NodeState::Follower)
            && self.last_heartbeat.elapsed() > self.election_timeout_ms
        {
            let _ = self.start_election().await;
        }
        Ok(())
    }

    /// Gets the term of the last entry in the log.
    ///
    /// # Returns
    ///
    /// The term of the last log entry, or 0 if the log is empty
    fn get_last_log_term(&self) -> u64 {
        self.log.last().map(|e| e.term).unwrap_or(0)
    }

    /// Gets the index of the last entry in the log.
    ///
    /// # Returns
    ///
    /// The index of the last log entry, or 0 if the log is empty
    fn get_last_log_index(&self) -> u64 {
        self.log.last().map(|e| e.idx).unwrap_or(0)
    }

    /// Starts a leader election for this node.
    ///
    /// The election process:
    /// 1. Transition to Candidate state
    /// 2. Increment current term
    /// 3. Vote for self
    /// 4. Send RequestVote RPCs to all peers
    /// 5. Wait for votes:
    ///    - Become leader if receive votes from majority
    ///    - Step down if discover higher term
    ///    - Return to follower if election fails
    ///
    /// # Returns
    ///
    /// Ok(()) if elected leader, Err otherwise
    ///
    /// # Errors
    ///
    /// Returns `RaftError::ElectionFailure` if unable to win the election
    async fn start_election(&mut self) -> RaftResult<()> {
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
            self.voted_for = None;
            self.votes_received = 0;
        }

        Err(RaftError::ElectionFailure)
    }

    /// Transitions this node to the Leader state.
    ///
    /// Upon becoming leader, the node:
    /// - Updates its state to Leader
    /// - Starts sending periodic heartbeats to all followers
    ///
    /// # Returns
    ///
    /// Ok(()) on successful transition
    #[tracing::instrument]
    fn become_leader(&mut self) -> RaftResult<()> {
        info!("Becoming leader");
        self.state = NodeState::Leader;
        self.start_heartbeat_timer();
        Ok(())
    }

    /// Steps down from Leader or Candidate to Follower state.
    ///
    /// This occurs when:
    /// - Discovering a higher term from another node
    /// - Receiving an AppendEntries RPC with a higher term
    /// - Receiving a vote response with a higher term
    ///
    /// # Arguments
    ///
    /// * `new_term` - The higher term that caused this node to step down
    ///
    /// # Returns
    ///
    /// Ok(()) on successful transition
    #[tracing::instrument]
    fn step_down(&mut self, new_term: u64) -> RaftResult<()> {
        info!("Stepping down");
        self.state = NodeState::Follower;
        self.voted_for = None;
        self.current_term = new_term;
        self.last_heartbeat = Instant::now();
        self.votes_received = 0;
        self.start_heartbeat_timer();
        Ok(())
    }
}
