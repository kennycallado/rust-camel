//! Emission-proof task 3.1: cxf consume emission legs. Each test drives
//! the cxf consumer through a full request/response leg against the mock
//! bridge and asserts what the component emits on the
//! [`RuntimeObservability`] port: the `("cxf", "consume", "success")`
//! operation on the happy path, the `("cxf", "e:cxf:consume")` error
//! family plus `("cxf", "consume", "failure")` operation on a route
//! rejection, the error family alone when the components lever is off,
//! and the D5 double-count shape (retained b-prime plus error family,
//! exactly one operation) on a response-marshalling failure.

mod support;

use std::sync::Arc;
use std::time::Duration;

use camel_api::{CamelError, Message};
use camel_component_api::consumer::{Consumer, ConsumerContext, ExchangeEnvelope};
use camel_component_api::test_support::{RecordingRuntimeObservability, acquire_deadline};
use camel_component_api::{Body, RuntimeObservability, StreamBody};
use camel_component_cxf::config::CxfPoolConfig;
use camel_component_cxf::consumer::CxfConsumer;
use camel_component_cxf::proto::{ConsumerRequest, ConsumerResponse};
use camel_component_cxf::{BridgeSlot, CxfBridgePool};
use support::mock_bridge::{MockState, spawn_mock_bridge};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;

/// What the test's stand-in route handler replies with.
// `Reply*` names prescribed by the emissionproof plan (tasks.md 3.1).
#[allow(clippy::enum_variant_names)]
enum Behavior {
    /// Success: output body `<ok/>`, reply Ok.
    ReplyOk,
    /// Route rejection: reply Err.
    ReplyErr,
    /// Success reply whose output body is a stream — a body the cxf
    /// response marshaller cannot encode.
    ReplyStreamBody,
}

async fn start_consumer(
    rt: Arc<dyn RuntimeObservability>,
    route_id: &str,
) -> Result<
    (
        MockState,
        CancellationToken,
        mpsc::Receiver<ExchangeEnvelope>,
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
        profiles: vec![],
        ..CxfPoolConfig::default()
    })?;
    pool.insert_slot_for_test(CxfBridgePool::slot_key(), slot);

    let mut consumer = CxfConsumer::new(Arc::new(pool), "emission-proof".into(), rt);

    let (tx, rx) = mpsc::channel(16);
    let token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, token.clone(), route_id.to_string());

    consumer.start(ctx).await?;

    Ok((state, token, rx, consumer))
}

/// Consumes exactly one envelope — the single request each test sends —
/// and replies according to `behavior`.
///
/// MUST be spawned before the test sends its `ConsumerRequest`, or the
/// consumer's `send_and_wait` waits forever.
async fn reply_handler(mut rx: mpsc::Receiver<ExchangeEnvelope>, behavior: Behavior) {
    let envelope = match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
        Ok(Some(envelope)) => envelope,
        Ok(None) => return,
        Err(_elapsed) => return,
    };
    match behavior {
        Behavior::ReplyOk => {
            let mut exchange = envelope.exchange;
            exchange.output = Some(Message::new(Body::Text("<ok/>".into())));
            if let Some(reply_tx) = envelope.reply_tx {
                let _ = reply_tx.send(Ok(exchange));
            }
        }
        Behavior::ReplyErr => {
            if let Some(reply_tx) = envelope.reply_tx {
                let _ = reply_tx.send(Err(CamelError::ProcessorError("route rejected".into())));
            }
        }
        Behavior::ReplyStreamBody => {
            let mut exchange = envelope.exchange;
            // Zero-item stream: the marshaller must reject the body type
            // itself, independent of any item it would yield.
            let stream = futures::stream::empty::<Result<bytes::Bytes, CamelError>>();
            exchange.output = Some(Message::new(Body::Stream(StreamBody {
                stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(stream)))),
                metadata: Default::default(),
            })));
            if let Some(reply_tx) = envelope.reply_tx {
                let _ = reply_tx.send(Ok(exchange));
            }
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
        // Per-lock deadline (lintwiden D4.1S): the loop's own 500ms
        // budget governs readiness; this only keeps a stalled MockState
        // holder from parking the helper silently.
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
        // Per-lock deadline (lintwiden D4.1S): same anti-wedge backstop
        // as wait_for_consumer_request_sender above.
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
async fn cxf_consume_success_emits_operation()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rt = RecordingRuntimeObservability::new(true);
    let (state, token, rx, mut consumer) =
        start_consumer(rt.clone(), "emission-proof-route").await?;

    let handler = tokio::spawn(reply_handler(rx, Behavior::ReplyOk));

    let requests_tx = wait_for_consumer_request_sender(&state).await?;
    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-em-1".to_string(),
            operation: "op".to_string(),
            payload: b"<in/>".to_vec(),
            headers: Default::default(),
            soap_action: "urn:op".to_string(),
            security_profile: String::new(),
        }))
        .await?;

    let recorded = wait_for_recorded_responses(&state, 1).await?;
    assert!(
        !recorded[0].fault,
        "success leg must not be recorded as a fault: {:?}",
        recorded[0]
    );
    assert_eq!(
        rt.ops(),
        vec![(
            "cxf".to_string(),
            "consume".to_string(),
            "success".to_string()
        )],
        "happy path must emit exactly the success operation: {:?}",
        rt.ops()
    );
    assert!(
        rt.errors().is_empty(),
        "happy path must emit no error family: {:?}",
        rt.errors()
    );

    let () = tokio::time::timeout(Duration::from_secs(2), handler)
        .await
        .map_err(|_| "reply handler did not exit within 2s")??;

    token.cancel();
    let handle = consumer
        .background_task_handle()
        .expect("consumer background handle");
    let outcome = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        outcome.is_ok(),
        "consumer task did not observe the context token cancel and exit within 2s"
    );

    Ok(())
}

#[tokio::test]
async fn cxf_consume_route_error_emits_error_family()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rt = RecordingRuntimeObservability::new(true);
    let (state, token, rx, mut consumer) =
        start_consumer(rt.clone(), "emission-proof-route").await?;

    let handler = tokio::spawn(reply_handler(rx, Behavior::ReplyErr));

    let requests_tx = wait_for_consumer_request_sender(&state).await?;
    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-em-1".to_string(),
            operation: "op".to_string(),
            payload: b"<in/>".to_vec(),
            headers: Default::default(),
            soap_action: "urn:op".to_string(),
            security_profile: String::new(),
        }))
        .await?;

    let recorded = wait_for_recorded_responses(&state, 1).await?;
    assert!(
        recorded[0].fault,
        "route rejection must be recorded as a fault: {:?}",
        recorded[0]
    );
    assert!(
        rt.errors()
            .contains(&("cxf".to_string(), "e:cxf:consume".to_string())),
        "error family must be emitted with the lever on: {:?}",
        rt.errors()
    );
    assert!(
        rt.ops().contains(&(
            "cxf".to_string(),
            "consume".to_string(),
            "failure".to_string()
        )),
        "failure operation must be emitted with the lever on: {:?}",
        rt.ops()
    );

    let () = tokio::time::timeout(Duration::from_secs(2), handler)
        .await
        .map_err(|_| "reply handler did not exit within 2s")??;

    token.cancel();
    let handle = consumer
        .background_task_handle()
        .expect("consumer background handle");
    let outcome = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        outcome.is_ok(),
        "consumer task did not observe the context token cancel and exit within 2s"
    );

    Ok(())
}

#[tokio::test]
async fn cxf_consume_failure_with_lever_off_still_emits_error_family()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rt = RecordingRuntimeObservability::new(false);
    let (state, token, rx, mut consumer) =
        start_consumer(rt.clone(), "emission-proof-route").await?;

    let handler = tokio::spawn(reply_handler(rx, Behavior::ReplyErr));

    let requests_tx = wait_for_consumer_request_sender(&state).await?;
    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-em-1".to_string(),
            operation: "op".to_string(),
            payload: b"<in/>".to_vec(),
            headers: Default::default(),
            soap_action: "urn:op".to_string(),
            security_profile: String::new(),
        }))
        .await?;

    let recorded = wait_for_recorded_responses(&state, 1).await?;
    assert!(
        recorded[0].fault,
        "route rejection must be recorded as a fault: {:?}",
        recorded[0]
    );
    assert!(
        rt.errors()
            .contains(&("cxf".to_string(), "e:cxf:consume".to_string())),
        "error family must be emitted even with the lever off: {:?}",
        rt.errors()
    );
    assert!(
        rt.ops().is_empty(),
        "success/failure operations must be suppressed with the lever off: {:?}",
        rt.ops()
    );

    let () = tokio::time::timeout(Duration::from_secs(2), handler)
        .await
        .map_err(|_| "reply handler did not exit within 2s")??;

    token.cancel();
    let handle = consumer
        .background_task_handle()
        .expect("consumer background handle");
    let outcome = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        outcome.is_ok(),
        "consumer task did not observe the context token cancel and exit within 2s"
    );

    Ok(())
}

#[tokio::test]
async fn cxf_marshalling_failure_retains_b_prime_and_double_counts()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rt = RecordingRuntimeObservability::new(true);
    let (state, token, rx, mut consumer) =
        start_consumer(rt.clone(), "emission-proof-route").await?;

    let handler = tokio::spawn(reply_handler(rx, Behavior::ReplyStreamBody));

    let requests_tx = wait_for_consumer_request_sender(&state).await?;
    requests_tx
        .send(Ok(ConsumerRequest {
            request_id: "req-em-1".to_string(),
            operation: "op".to_string(),
            payload: b"<in/>".to_vec(),
            headers: Default::default(),
            soap_action: "urn:op".to_string(),
            security_profile: String::new(),
        }))
        .await?;

    let recorded = wait_for_recorded_responses(&state, 1).await?;
    assert!(
        recorded[0].fault,
        "response-marshalling failure must be recorded as a fault: {:?}",
        recorded[0]
    );

    // D5: the double-count lives on the error FAMILY only — the retained
    // b-prime increment and the uniform family emission each land exactly
    // once, while the op side stays at exactly one failure observation.
    let b_prime = (
        "emission-proof-route".to_string(),
        "b-prime:cxf:response-marshalling".to_string(),
    );
    let family = ("cxf".to_string(), "e:cxf:consume".to_string());
    let b_prime_count = rt.errors().iter().filter(|e| **e == b_prime).count();
    let family_count = rt.errors().iter().filter(|e| **e == family).count();
    assert_eq!(
        b_prime_count,
        1,
        "b-prime must be retained exactly once: {:?}",
        rt.errors()
    );
    assert_eq!(
        family_count,
        1,
        "error family must be emitted exactly once: {:?}",
        rt.errors()
    );
    assert_eq!(
        rt.ops(),
        vec![(
            "cxf".to_string(),
            "consume".to_string(),
            "failure".to_string()
        )],
        "exactly one failure op — the D5 double-count lives on the error family only: {:?}",
        rt.ops()
    );

    let () = tokio::time::timeout(Duration::from_secs(2), handler)
        .await
        .map_err(|_| "reply handler did not exit within 2s")??;

    token.cancel();
    let handle = consumer
        .background_task_handle()
        .expect("consumer background handle");
    let outcome = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        outcome.is_ok(),
        "consumer task did not observe the context token cancel and exit within 2s"
    );

    Ok(())
}
