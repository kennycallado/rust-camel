mod support;

use std::collections::HashMap;
use std::time::Duration;

use camel_component_cxf::proto::{
    ConsumerRequest, ConsumerResponse, cxf_bridge_client::CxfBridgeClient,
};
use support::mock_bridge::{MockState, spawn_mock_bridge};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;

async fn open_consumer_stream() -> Result<
    (
        tonic::Streaming<ConsumerRequest>,
        mpsc::Sender<ConsumerResponse>,
        MockState,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let (port, state) = spawn_mock_bridge().await?;
    let endpoint = format!("http://127.0.0.1:{port}");
    let channel = Channel::from_shared(endpoint)?.connect().await?;
    let mut client = CxfBridgeClient::new(channel);

    let (response_tx, response_rx) = mpsc::channel::<ConsumerResponse>(32);
    let response_stream = ReceiverStream::new(response_rx);
    let inbound = client
        .open_consumer_stream(response_stream)
        .await?
        .into_inner();

    Ok((inbound, response_tx, state))
}

async fn wait_for_consumer_request_sender(
    state: &MockState,
) -> Result<
    mpsc::Sender<Result<ConsumerRequest, tonic::Status>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    for _ in 0..50 {
        if let Some(tx) = state.consumer_requests_tx.lock().await.clone() {
            return Ok(tx);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("consumer request sender not available".into())
}

async fn wait_for_recorded_responses(
    state: &MockState,
    n: usize,
) -> Result<Vec<ConsumerResponse>, Box<dyn std::error::Error + Send + Sync>> {
    for _ in 0..50 {
        let current = state.consumer_responses_received.lock().await.clone();
        if current.len() >= n {
            return Ok(current);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("timed out waiting for recorded responses".into())
}

#[tokio::test]
async fn test_consumer_receives_request_from_bridge()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut inbound, _response_tx, state) = open_consumer_stream().await?;
    let requests_tx = wait_for_consumer_request_sender(&state).await?;

    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-1".to_string(),
            operation: "sayHello".to_string(),
            payload: b"<hello>world</hello>".to_vec(),
            headers: HashMap::new(),
            soap_action: "urn:sayHello".to_string(),
        }))
        .await?;

    let received = inbound
        .message()
        .await?
        .ok_or("expected consumer request")?;
    assert_eq!(received.request_id, "req-1");
    assert_eq!(received.operation, "sayHello");
    assert_eq!(received.payload, b"<hello>world</hello>".to_vec());
    assert_eq!(received.soap_action, "urn:sayHello");

    Ok(())
}

#[tokio::test]
async fn test_consumer_sends_response_matched_by_request_id()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut inbound, response_tx, state) = open_consumer_stream().await?;
    let requests_tx = wait_for_consumer_request_sender(&state).await?;

    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-2".to_string(),
            operation: "op".to_string(),
            payload: b"<in/>".to_vec(),
            headers: HashMap::new(),
            soap_action: "urn:op".to_string(),
        }))
        .await?;

    let req = inbound
        .message()
        .await?
        .ok_or("expected consumer request")?;
    response_tx
        .send(ConsumerResponse {
            request_id: req.request_id.clone(),
            payload: b"<ok/>".to_vec(),
            fault: false,
            fault_code: String::new(),
            fault_string: String::new(),
        })
        .await?;

    let recorded = wait_for_recorded_responses(&state, 1).await?;
    let first = &recorded[0];
    assert_eq!(first.request_id, "req-2");
    assert_eq!(first.payload, b"<ok/>".to_vec());
    assert!(!first.fault);

    Ok(())
}

#[tokio::test]
async fn test_consumer_fault_on_route_error() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let (mut inbound, response_tx, state) = open_consumer_stream().await?;
    let requests_tx = wait_for_consumer_request_sender(&state).await?;

    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-3".to_string(),
            operation: "op".to_string(),
            payload: b"<in/>".to_vec(),
            headers: HashMap::new(),
            soap_action: "urn:op".to_string(),
        }))
        .await?;

    let req = inbound
        .message()
        .await?
        .ok_or("expected consumer request")?;
    response_tx
        .send(ConsumerResponse {
            request_id: req.request_id,
            payload: Vec::new(),
            fault: true,
            fault_code: "soap:Server".to_string(),
            fault_string: "route error".to_string(),
        })
        .await?;

    let recorded = wait_for_recorded_responses(&state, 1).await?;
    let first = &recorded[0];
    assert!(first.fault);
    assert_eq!(first.fault_code, "soap:Server");
    assert_eq!(first.fault_string, "route error");

    Ok(())
}

#[tokio::test]
async fn test_request_id_correlation_multiple_concurrent()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut inbound, response_tx, state) = open_consumer_stream().await?;
    let requests_tx = wait_for_consumer_request_sender(&state).await?;

    for i in 1..=3 {
        requests_tx
            .send(Ok(ConsumerRequest {
                request_id: format!("req-{i}"),
                operation: "bulk".to_string(),
                payload: format!("<in>{i}</in>").into_bytes(),
                headers: HashMap::new(),
                soap_action: "urn:bulk".to_string(),
            }))
            .await?;
    }

    let mut received = Vec::new();
    for _ in 0..3 {
        let req = inbound
            .message()
            .await?
            .ok_or("expected consumer request")?;
        received.push(req);
    }

    for req in received.iter().rev() {
        response_tx
            .send(ConsumerResponse {
                request_id: req.request_id.clone(),
                payload: format!("<out>{}</out>", req.request_id).into_bytes(),
                fault: false,
                fault_code: String::new(),
                fault_string: String::new(),
            })
            .await?;
    }

    let recorded = wait_for_recorded_responses(&state, 3).await?;
    let mut ids: Vec<String> = recorded.into_iter().map(|r| r.request_id).collect();
    ids.sort();
    assert_eq!(ids, vec!["req-1", "req-2", "req-3"]);

    Ok(())
}

#[tokio::test]
async fn test_consumer_receives_headers_from_request()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut inbound, _response_tx, state) = open_consumer_stream().await?;
    let requests_tx = wait_for_consumer_request_sender(&state).await?;

    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "text/xml".to_string());
    headers.insert("X-Correlation".to_string(), "abc-123".to_string());

    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-headers".to_string(),
            operation: "withHeaders".to_string(),
            payload: b"<in/>".to_vec(),
            headers: headers.clone(),
            soap_action: "urn:withHeaders".to_string(),
        }))
        .await?;

    let received = inbound
        .message()
        .await?
        .ok_or("expected consumer request")?;
    assert_eq!(
        received.headers.get("Content-Type"),
        Some(&"text/xml".to_string())
    );
    assert_eq!(
        received.headers.get("X-Correlation"),
        Some(&"abc-123".to_string())
    );

    Ok(())
}

#[tokio::test]
async fn test_consumer_request_with_empty_payload()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut inbound, _response_tx, state) = open_consumer_stream().await?;
    let requests_tx = wait_for_consumer_request_sender(&state).await?;

    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-empty".to_string(),
            operation: "emptyOp".to_string(),
            payload: Vec::new(),
            headers: HashMap::new(),
            soap_action: "urn:emptyOp".to_string(),
        }))
        .await?;

    let received = inbound
        .message()
        .await?
        .ok_or("expected consumer request")?;
    assert_eq!(received.request_id, "req-empty");
    assert_eq!(received.operation, "emptyOp");
    assert!(received.payload.is_empty());
    assert_eq!(received.soap_action, "urn:emptyOp");

    Ok(())
}

#[tokio::test]
async fn test_consumer_response_with_empty_payload()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut inbound, response_tx, state) = open_consumer_stream().await?;
    let requests_tx = wait_for_consumer_request_sender(&state).await?;

    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-resp-empty".to_string(),
            operation: "respEmpty".to_string(),
            payload: b"<in/>".to_vec(),
            headers: HashMap::new(),
            soap_action: "urn:respEmpty".to_string(),
        }))
        .await?;

    let req = inbound
        .message()
        .await?
        .ok_or("expected consumer request")?;
    response_tx
        .send(ConsumerResponse {
            request_id: req.request_id.clone(),
            payload: Vec::new(),
            fault: false,
            fault_code: String::new(),
            fault_string: String::new(),
        })
        .await?;

    let recorded = wait_for_recorded_responses(&state, 1).await?;
    let first = &recorded[0];
    assert_eq!(first.request_id, "req-resp-empty");
    assert!(first.payload.is_empty());
    assert!(!first.fault);

    Ok(())
}

#[tokio::test]
async fn test_consumer_multiple_headers_preserved()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut inbound, _response_tx, state) = open_consumer_stream().await?;
    let requests_tx = wait_for_consumer_request_sender(&state).await?;

    let mut headers = HashMap::new();
    headers.insert(
        "Content-Type".to_string(),
        "text/xml; charset=utf-8".to_string(),
    );
    headers.insert("SOAPAction".to_string(), "urn:multiHeaders".to_string());
    headers.insert("Authorization".to_string(), "Bearer token-xyz".to_string());
    headers.insert("X-Custom".to_string(), "custom-value".to_string());

    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-headers-multi".to_string(),
            operation: "multiHeaders".to_string(),
            payload: b"<in/>".to_vec(),
            headers: headers.clone(),
            soap_action: "urn:multiHeaders".to_string(),
        }))
        .await?;

    let received = inbound
        .message()
        .await?
        .ok_or("expected consumer request")?;
    assert_eq!(received.headers.len(), 4);
    assert_eq!(
        received.headers.get("Content-Type"),
        Some(&"text/xml; charset=utf-8".to_string())
    );
    assert_eq!(
        received.headers.get("SOAPAction"),
        Some(&"urn:multiHeaders".to_string())
    );
    assert_eq!(
        received.headers.get("Authorization"),
        Some(&"Bearer token-xyz".to_string())
    );
    assert_eq!(
        received.headers.get("X-Custom"),
        Some(&"custom-value".to_string())
    );

    Ok(())
}

#[tokio::test]
async fn test_consumer_stream_closes_gracefully()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut inbound, response_tx, state) = open_consumer_stream().await?;
    let requests_tx = wait_for_consumer_request_sender(&state).await?;

    // Drop the local request sender
    drop(requests_tx);
    // Drop the response sender so the mock bridge's response recording loop ends
    drop(response_tx);
    // Clear the mock bridge's internal sender so the inbound channel has no remaining senders
    *state.consumer_requests_tx.lock().await = None;

    // The inbound stream should eventually return None (channel closed)
    let result = tokio::time::timeout(Duration::from_secs(2), inbound.message()).await?;
    assert!(result?.is_none(), "expected inbound stream to close (None)");

    Ok(())
}

#[tokio::test]
async fn test_consumer_50_concurrent_requests_correlated()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut inbound, response_tx, state) = open_consumer_stream().await?;
    let requests_tx = wait_for_consumer_request_sender(&state).await?;

    // Send all 50 requests
    for i in 1..=50 {
        requests_tx
            .send(Ok(ConsumerRequest {
                request_id: format!("req-{i:03}"),
                operation: "concurrent".to_string(),
                payload: format!("<payload>{i}</payload>").into_bytes(),
                headers: HashMap::new(),
                soap_action: "urn:concurrent".to_string(),
            }))
            .await?;
    }

    // Receive all 50 from inbound
    let mut received = Vec::new();
    for _ in 0..50 {
        let req = inbound
            .message()
            .await?
            .ok_or("expected consumer request")?;
        received.push(req);
    }

    // Send back 50 responses correlated by request_id
    for req in &received {
        response_tx
            .send(ConsumerResponse {
                request_id: req.request_id.clone(),
                payload: format!("<response>{}</response>", req.request_id).into_bytes(),
                fault: false,
                fault_code: String::new(),
                fault_string: String::new(),
            })
            .await?;
    }

    // Wait for 50 recorded responses and verify
    let recorded = wait_for_recorded_responses(&state, 50).await?;
    assert_eq!(recorded.len(), 50);

    let mut ids: Vec<String> = recorded.into_iter().map(|r| r.request_id).collect();
    ids.sort();

    let expected: Vec<String> = (1..=50).map(|i| format!("req-{i:03}")).collect();
    assert_eq!(ids, expected);

    Ok(())
}
