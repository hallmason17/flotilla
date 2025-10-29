use bincode::{Decode, Encode, config, error::DecodeError};

use crate::raft_rpc;

#[derive(Debug, Clone, Decode, Encode)]
pub enum Command {
    Set { key: String, value: String },
    Delete { key: String },
}

#[derive(Debug, Clone)]
pub enum NodeState {
    Follower,
    Candidate,
    Leader,
}
#[derive(Debug, Clone, Decode, Encode)]
pub struct LogEntry {
    pub term: u64,
    pub idx: u64,
    pub command: Command,
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
