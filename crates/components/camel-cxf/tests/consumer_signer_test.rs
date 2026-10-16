//! Task 2.5 signer-descriptor rejection on the real consumer loop.
//!
//! Each test drives `CxfConsumer` end-to-end against the mock bridge RPC and a
//! recording route stand-in: an invalid signer descriptor must never reach the
//! route, must set no exchange property at all, and must yield a correlated
//! non-WSS `Receiver` fault (`soap:Server`) even when the profile carries a
//! WSS-only `fault_status = 403` override. HTTP status mapping is asserted on
//! the real native bridge (`cxf_test`); this target asserts the typed
//! `ConsumerResponse` contract.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_api::Message;
use camel_component_api::consumer::{Consumer, ConsumerContext, ExchangeEnvelope};
use camel_component_api::test_support::{RecordingRuntimeObservability, acquire_deadline};
use camel_component_api::{Body, RuntimeObservability};
use camel_component_cxf::config::{CxfPoolConfig, CxfProfileConfig, CxfSecurityFields};
use camel_component_cxf::consumer::CxfConsumer;
use camel_component_cxf::proto::{ConsumerRequest, ConsumerResponse, SignerIdentity};
use camel_component_cxf::{BridgeSlot, CxfBridgePool};
use support::mock_bridge::{MockState, spawn_mock_bridge};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;

const FINGERPRINT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// One route-observed exchange: the property key/value pairs the stand-in
/// route actually saw. A rejected request must produce no entry at all.
#[derive(Debug, Clone)]
struct ObservedExchange {
    properties: HashMap<String, String>,
    auth_keys: Vec<String>,
}

fn invalid_signer() -> SignerIdentity {
    // Oversized subject DN: exported by a genuine WSS verification in the
    // native case; crafted here at the IPC layer to drive the same rejection.
    SignerIdentity {
        subject: "a".repeat(4097),
        issuer: "CN=test-alice,O=RustCamel Test".to_string(),
        serial: "0a1b2c".to_string(),
        fingerprint_sha256: FINGERPRINT.to_string(),
    }
}

fn consumer_request(request_id: &str, signer: Option<SignerIdentity>) -> ConsumerRequest {
    ConsumerRequest {
        request_id: request_id.to_string(),
        operation: "sayHello".to_string(),
        payload: b"<hello/>".to_vec(),
        headers: HashMap::new(),
        soap_action: String::new(),
        security_profile: "esap".to_string(),
        signer,
    }
}

/// Starts a `CxfConsumer` against the mock bridge with a route stand-in that
/// records every dispatched exchange and replies `<ok/>`.
async fn start_consumer(
    rt: Arc<dyn RuntimeObservability>,
    profiles: Vec<CxfProfileConfig>,
    profile_name: &str,
) -> Result<
    (
        MockState,
        CancellationToken,
        mpsc::Receiver<ExchangeEnvelope>,
        Arc<Mutex<Vec<ObservedExchange>>>,
        CxfConsumer,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let (port, state) = spawn_mock_bridge().await?;
    let endpoint = format!("http://127.0.0.1:{port}");
    let channel = tokio::time::timeout(
        Duration::from_secs(5),
        Channel::from_shared(endpoint)?.connect(),
    )
    .await??;

    let slot = BridgeSlot::new_ready_for_test(channel);
    let pool = CxfBridgePool::from_config(CxfPoolConfig {
        profiles,
        listener_tls: None,
        ..CxfPoolConfig::default()
    })?;
    pool.insert_slot_for_test(CxfBridgePool::slot_key(), slot);

    let mut consumer = CxfConsumer::new(Arc::new(pool), profile_name.to_string(), rt);

    let (tx, rx) = mpsc::channel(16);
    let token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, token.clone(), "signer-route".to_string());

    consumer.start(ctx).await?;

    let observed: Arc<Mutex<Vec<ObservedExchange>>> = Arc::new(Mutex::new(Vec::new()));
    Ok((state, token, rx, observed, consumer))
}

/// Route stand-in: records property snapshots of every dispatched exchange and
/// replies success. MUST be spawned before the test sends its `ConsumerRequest`.
async fn record_route(
    mut rx: mpsc::Receiver<ExchangeEnvelope>,
    observed: Arc<Mutex<Vec<ObservedExchange>>>,
) {
    while let Ok(Some(envelope)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
        let exchange = envelope.exchange;
        let mut properties = HashMap::new();
        for (k, v) in &exchange.properties {
            if let Some(s) = v.as_str() {
                properties.insert(k.clone(), s.to_string());
            }
        }
        let auth_keys: Vec<String> = exchange
            .properties
            .keys()
            .filter(|k| k.starts_with("camel.auth."))
            .cloned()
            .collect();
        observed
            .lock()
            .expect("observed lock")
            .push(ObservedExchange {
                properties,
                auth_keys,
            });

        let mut exchange = exchange;
        exchange.output = Some(Message::new(Body::Text("<ok/>".into())));
        if let Some(reply_tx) = envelope.reply_tx {
            let _ = reply_tx.send(Ok(exchange));
        }
    }
}

async fn wait_for_consumer_request_sender(
    state: &MockState,
) -> Result<
    mpsc::Sender<Result<ConsumerRequest, tonic::Status>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    for _ in 0..50 {
        if let Some(tx) = acquire_deadline(
            &state.consumer_requests_tx,
            "consumer_requests_tx (wait_for_consumer_request_sender)",
            Duration::from_secs(5),
        )
        .await
        .clone()
        {
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
        let current = acquire_deadline(
            &state.consumer_responses_received,
            "consumer_responses_received (wait_for_recorded_responses)",
            Duration::from_secs(5),
        )
        .await
        .clone();
        if current.len() >= n {
            return Ok(current);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("timed out waiting for recorded responses".into())
}

#[tokio::test]
async fn invalid_signer_descriptor_is_not_dispatched()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rt = RecordingRuntimeObservability::new(true);
    let (state, _token, rx, observed, _consumer) =
        start_consumer(rt.clone(), vec![], "esap").await?;
    let _handler = tokio::spawn(record_route(rx, observed.clone()));

    let requests_tx = wait_for_consumer_request_sender(&state).await?;
    requests_tx
        .send(Ok(consumer_request("req-bad-1", Some(invalid_signer()))))
        .await?;

    // The correlated fault is emitted instead of a dispatch.
    let recorded = wait_for_recorded_responses(&state, 1).await?;
    assert!(recorded[0].fault, "invalid descriptor must answer a fault");
    assert_eq!(recorded[0].request_id, "req-bad-1");

    // Zero-dispatch is causally settled: any route dispatch awaits to
    // completion before the correlated ConsumerResponse is emitted.
    assert!(
        observed.lock().expect("observed lock").is_empty(),
        "the route must receive no request, got: {:?}",
        observed.lock().expect("observed lock")
    );
    Ok(())
}

#[tokio::test]
async fn invalid_signer_descriptor_sets_no_properties()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rt = RecordingRuntimeObservability::new(true);
    let (state, _token, rx, observed, _consumer) =
        start_consumer(rt.clone(), vec![], "esap").await?;
    let _handler = tokio::spawn(record_route(rx, observed.clone()));

    let requests_tx = wait_for_consumer_request_sender(&state).await?;
    requests_tx
        .send(Ok(consumer_request("req-bad-2", Some(invalid_signer()))))
        .await?;

    let recorded = wait_for_recorded_responses(&state, 1).await?;
    assert!(recorded[0].fault, "invalid descriptor must answer a fault");

    let snapshots = observed.lock().expect("observed lock").clone();
    // The actual error return happens before any metadata write: no partial
    // exchange exists, so the route observer sees no exchange — and therefore
    // no `camel.cxf.signer.*` value and no `camel.auth.*` value exists
    // anywhere in the request path.
    assert!(
        snapshots.is_empty(),
        "no exchange may be assembled for an invalid descriptor, got: {snapshots:?}"
    );
    for snapshot in &snapshots {
        assert!(
            snapshot.auth_keys.is_empty(),
            "no camel.auth.* key may exist, got: {:?}",
            snapshot.auth_keys
        );
        assert!(
            !snapshot
                .properties
                .keys()
                .any(|k| k.starts_with("camel.cxf.signer.")),
            "no camel.cxf.signer.* property may exist, got: {:?}",
            snapshot.properties
        );
    }
    Ok(())
}

#[tokio::test]
async fn invalid_signer_descriptor_is_receiver_500_even_with_override()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // The profile carries a WSS-only `fault_status = 403` override. A signer
    // descriptor failure is a non-WSS Receiver fault, so the override must not
    // rewrite it: the response keeps the `soap:Server` (Receiver/HTTP 500)
    // class. The native test asserts the HTTP status end-to-end.
    let profiles = vec![CxfProfileConfig {
        name: "esap".to_string(),
        address: None,
        wsdl_path: "unused.wsdl".to_string(),
        service_name: "unused".to_string(),
        port_name: "unused".to_string(),
        security: CxfSecurityFields {
            fault_status: Some(403),
            ..Default::default()
        },
    }];
    let rt = RecordingRuntimeObservability::new(true);
    let (state, _token, rx, observed, _consumer) =
        start_consumer(rt.clone(), profiles, "esap").await?;
    let _handler = tokio::spawn(record_route(rx, observed.clone()));

    let requests_tx = wait_for_consumer_request_sender(&state).await?;
    requests_tx
        .send(Ok(consumer_request("req-bad-3", Some(invalid_signer()))))
        .await?;

    let recorded = wait_for_recorded_responses(&state, 1).await?;
    let response = &recorded[0];
    assert_eq!(
        response.request_id, "req-bad-3",
        "the fault must be correlated to the request id"
    );
    assert!(response.fault, "invalid descriptor must answer a fault");
    assert_eq!(
        response.fault_code, "soap:Server",
        "the fault must be a Receiver (soap:Server) fault; the 403 override only \
         applies to WSS faults"
    );
    assert!(
        response.fault_string.contains("subject"),
        "the bounded error must name the offending field, got: {}",
        response.fault_string
    );
    Ok(())
}
