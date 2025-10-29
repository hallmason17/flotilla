use anyhow::Result;
use flotilla_core::raft_rpc::{
    RequestVoteRequest, RequestVoteResponse, raft_service_client::RaftServiceClient,
};

#[tokio::test]
async fn my_test() {
    assert!(true);
}

#[tokio::test]
async fn test_request_vote() -> Result<()> {
    let expected = RequestVoteResponse {
        term: 0,
        vote_granted: false,
    };

    let mut client = RaftServiceClient::connect("http://127.0.0.1:5001").await?;
    let request = tonic::Request::new(RequestVoteRequest {
        term: 1,
        candidate_id: 2.to_string(),
        last_log_index: 1,
        last_log_term: 0,
    });
    let res = client.request_vote(request).await?;
    let msg = res.into_inner();
    assert_eq!(msg, expected);
    Ok(())
}
