use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_component_api::test_support::NoopRuntimeObservability;
use futures::stream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use super::*;
// Task 3.5: the passive-check seam is the typed broker classification
// (`RabbitError::for_queue_error`), not a string heuristic.
use crate::error::{RabbitError, protocol_error};
// Task 4.3: the reply decision/publish seam under test.
use super::reply::{ReplyPublisher, maybe_reply, send_reply};

/// No-op observability for the unit engine tests. The consume outcome
/// emission itself is proven by the docker `consume_outcome_counted` test.
fn test_rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoopRuntimeObservability)
}

/// A manager whose connect seam always fails. The engine unit tests only
/// need `current_generation()` — the default state is generation 0 — so the
/// generation they pass matches and no live connection is required.
fn test_manager() -> Arc<RabbitConnectionManager> {
    let connect_fn: crate::connection::ConnectFn = Arc::new(|_url: &str| {
        Box::pin(async {
            Err(lapin::Error::from(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "test connect refused",
            )))
        })
    });
    Arc::new(RabbitConnectionManager::new(
        "amqp://guest:guest@127.0.0.1:5672",
        crate::config::rabbitmq_reconnect_default(),
        connect_fn,
    ))
}

/// Ack-only fake: the stop test never completes the route, so no
/// disposition is applied and the acker is only carried.
struct NoopAcker;

#[async_trait]
impl DeliveryAcker for NoopAcker {
    async fn ack(&self) -> Result<(), CamelError> {
        Ok(())
    }

    async fn nack(&self, _requeue: bool) -> Result<(), CamelError> {
        Ok(())
    }
}

/// Counting fake so a test can prove a disposition was (or was not)
/// applied. Clone shares the counters, so the test keeps one handle while
/// the delivery owns another.
#[derive(Clone, Default)]
struct RecordingAcker {
    acks: Arc<AtomicUsize>,
    nacks: Arc<AtomicUsize>,
}

#[async_trait]
impl DeliveryAcker for RecordingAcker {
    async fn ack(&self) -> Result<(), CamelError> {
        self.acks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn nack(&self, _requeue: bool) -> Result<(), CamelError> {
        self.nacks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn inbound_with_acker(payload: &[u8], acker: Box<dyn DeliveryAcker>) -> InboundDelivery {
    InboundDelivery {
        payload: payload.to_vec(),
        props: lapin::BasicProperties::default(),
        headers: FieldTable::default(),
        redelivered: false,
        acker,
    }
}

fn inbound(payload: &[u8]) -> InboundDelivery {
    inbound_with_acker(payload, Box::new(NoopAcker))
}

/// Task 3.5: the passive-check error-mapping seam classifies by the typed AMQP
/// reply code. Only a broker `404 NOT_FOUND` proves absence (`MissingQueue`);
/// a non-404 (here a stalled/timed-out RPC) is a neutral `PublishFailed` that
/// still names the queue. Neither carries the broker URL or credentials.
#[test]
fn passive_check_error_names_queue() {
    let queue = "orders-missing-probe";

    // The stable constructor used directly.
    let missing = RabbitError::MissingQueue(queue.to_string());
    assert!(
        missing.to_string().contains(queue),
        "MissingQueue must name the queue exactly: {missing}"
    );

    // A broker 404 is classified as MissingQueue naming the queue.
    let not_found = protocol_error(404, "NOT_FOUND - no queue 'orders-missing-probe'");
    let classified = RabbitError::for_queue_error(queue, &not_found);
    assert_eq!(classified, RabbitError::MissingQueue(queue.to_string()));
    let message = classified.to_string();
    assert!(message.contains(queue), "must name the queue: {message}");
    assert!(
        !message.contains("amqp://"),
        "must not carry a broker URL: {message}"
    );

    // A non-404 (timeout) is neutral, never a false missing-queue claim.
    let stalled = lapin::Error::from(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "timed out after 10s",
    ));
    let neutral = RabbitError::for_queue_error(queue, &stalled);
    match &neutral {
        RabbitError::PublishFailed(detail) => {
            assert!(
                detail.contains(queue),
                "the neutral error must still name the queue: {detail}"
            );
            assert!(
                detail.contains("timed out after 10s"),
                "the neutral error must preserve the detail: {detail}"
            );
            assert!(
                !detail.contains("amqp://"),
                "the neutral error must not carry a broker URL: {detail}"
            );
        }
        other => panic!("a non-404 must be neutral PublishFailed, got {other:?}"),
    }
}

#[test]
fn disposition_ok_acks() {
    assert_eq!(disposition(&Ok(()), false), Disposition::Ack);
}

#[test]
fn disposition_err_default_rejects_without_requeue() {
    let failed: Result<(), CamelError> =
        Err(CamelError::ProcessorError("route failed".to_string()));
    assert_eq!(
        disposition(&failed, false),
        Disposition::Nack { requeue: false }
    );
}

#[test]
fn disposition_err_opt_in_requeues() {
    let failed: Result<(), CamelError> =
        Err(CamelError::ProcessorError("route failed".to_string()));
    assert_eq!(
        disposition(&failed, true),
        Disposition::Nack { requeue: true }
    );
}

#[tokio::test]
async fn apply_disposition_stale_generation_drops() {
    let acker = RecordingAcker::default();
    let result = apply_disposition(Disposition::Nack { requeue: true }, false, &acker).await;
    assert!(
        matches!(result.as_ref(), Ok(DispositionApplied::Stale)),
        "a stale tag must be reported as not applied: {result:?}"
    );
    assert_eq!(
        acker.acks.load(Ordering::SeqCst),
        0,
        "a stale tag must not ack"
    );
    assert_eq!(
        acker.nacks.load(Ordering::SeqCst),
        0,
        "a stale tag must not nack"
    );
}

#[tokio::test]
async fn route_transport_closed_abandons_delivery_without_disposition() {
    let (tx, mut rx) = mpsc::channel(4);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "transport-loss-route".to_string());
    let acker = RecordingAcker::default();
    let stream = stream::iter(vec![inbound_with_acker(b"held", Box::new(acker.clone()))]);

    let handle = tokio::spawn(run_loop(
        stream,
        ctx,
        false,
        0,
        test_manager(),
        test_rt(),
        cancel.clone(),
    ));

    let envelope = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("the engine must dispatch the delivery")
        .expect("the dispatch channel must stay open");
    // Transport/lifecycle loss: the reply oneshot is dropped unanswered.
    drop(envelope.reply_tx);

    timeout(Duration::from_secs(2), handle)
        .await
        .expect("the engine must exit after the route transport closes")
        .expect("the engine must not panic");

    assert_eq!(
        acker.acks.load(Ordering::SeqCst),
        0,
        "a route transport loss must not ack"
    );
    assert_eq!(
        acker.nacks.load(Ordering::SeqCst),
        0,
        "a route transport loss must not nack"
    );
}

#[tokio::test]
async fn route_receiver_closed_abandons_delivery_without_disposition() {
    let (tx, rx) = mpsc::channel(4);
    // The route pipeline is gone before the engine dispatches.
    drop(rx);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "receiver-loss-route".to_string());
    let acker = RecordingAcker::default();
    let stream = stream::iter(vec![inbound_with_acker(b"held", Box::new(acker.clone()))]);

    let handle = tokio::spawn(run_loop(
        stream,
        ctx,
        false,
        0,
        test_manager(),
        test_rt(),
        cancel.clone(),
    ));

    timeout(Duration::from_secs(2), handle)
        .await
        .expect("the engine must exit after the route receiver closes")
        .expect("the engine must not panic");

    assert_eq!(
        acker.acks.load(Ordering::SeqCst),
        0,
        "a closed route receiver must not ack"
    );
    assert_eq!(
        acker.nacks.load(Ordering::SeqCst),
        0,
        "a closed route receiver must not nack"
    );
}

#[tokio::test]
async fn consumer_stop_joins_loop() {
    let (tx, mut rx) = mpsc::channel(4);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "stop-loop-route".to_string());
    let stream = stream::iter(vec![inbound(b"blocked")]);

    let handle = tokio::spawn(run_loop(
        stream,
        ctx,
        false,
        0,
        test_manager(),
        test_rt(),
        cancel.clone(),
    ));

    // The engine dispatches the delivery and then blocks awaiting the
    // route reply. Hold the envelope unanswered to model a blocked route.
    let _held_envelope = rx
        .recv()
        .await
        .expect("run_loop must dispatch the delivery");

    // stop(): cancel the engine token and join the loop task.
    cancel.cancel();
    timeout(Duration::from_secs(2), handle)
        .await
        .expect("run_loop must join within the stop deadline")
        .expect("run_loop must not panic");
}

/// Drop marker so the test can prove the stalled engine future was dropped
/// (aborted) rather than orphaned after the join timed out.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A mixed set — one engine already completed, one stalled forever — must
/// time out without re-polling the completed `JoinHandle` (polling after
/// completion panics), and must abort/drain only the outstanding engine.
#[tokio::test(start_paused = true)]
async fn join_engines_mixed_completion_timeout_does_not_panic() {
    let completed = tokio::spawn(async {});
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&dropped);
    let stalled = tokio::spawn(async move {
        let _guard = DropFlag(flag);
        std::future::pending::<()>().await;
    });

    let result = join_engines(vec![completed, stalled]).await;

    match result {
        Err(CamelError::ProcessorError(message)) => {
            assert!(
                message.contains("did not stop within"),
                "the bounded stop must report the timeout, got: {message}"
            );
        }
        other => panic!("expected a bounded-stop error, got: {other:?}"),
    }
    assert!(
        dropped.load(Ordering::SeqCst),
        "the stalled engine future must be dropped after abort"
    );
}

// ---------------------------------------------------------------------------
// Task 4.3: consumer-side reply publishing.
// ---------------------------------------------------------------------------

/// One recorded reply publish: `(reply_to, correlation_id, body, props)`.
type ReplyCall = (String, String, Vec<u8>, lapin::BasicProperties);

/// Records every reply publish so the tests assert exactly what was sent.
/// Clone shares the log (the Rc-free shape the other recorder fakes use).
#[derive(Clone, Default)]
struct RecordingReplyPublisher {
    calls: Arc<std::sync::Mutex<Vec<ReplyCall>>>,
    /// When set, `publish_reply` records the call and returns an error (the
    /// G-10 infra-failure branch).
    fail: Arc<AtomicBool>,
}

impl RecordingReplyPublisher {
    fn recorded(&self) -> Vec<ReplyCall> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl ReplyPublisher for RecordingReplyPublisher {
    async fn publish_reply(
        &self,
        reply_to: &str,
        correlation_id: &str,
        body: Vec<u8>,
        props: lapin::BasicProperties,
    ) -> Result<(), CamelError> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((
                reply_to.to_string(),
                correlation_id.to_string(),
                body,
                props,
            ));
        if self.fail.load(Ordering::SeqCst) {
            return Err(CamelError::ProcessorError(
                "reply transport down".to_string(),
            ));
        }
        Ok(())
    }
}

/// Original request headers with the direct reply-to name and correlation id.
fn request_reply_headers() -> camel_api::Headers {
    let mut headers = camel_api::Headers::new();
    headers.insert(
        "replyTo".to_string(),
        serde_json::json!("amq.rabbitmq.reply-to.g1"),
    );
    headers.insert("correlationId".to_string(), serde_json::json!("c1"));
    headers
}

#[tokio::test]
async fn reply_sent_after_route_success() {
    let publisher = RecordingReplyPublisher::default();
    let original = request_reply_headers();

    // Successful route with an OUT message whose body/headers differ from the
    // IN; the OUT carries a bogus correlationId the enforcement must overwrite.
    let mut input = Message::new(Body::Bytes(bytes::Bytes::from_static(b"in")));
    input.headers = original.clone();
    let mut exchange = Exchange::new(input);
    let mut out = Message::new(Body::Bytes(bytes::Bytes::from_static(b"out")));
    out.headers.insert(
        "correlationId".to_string(),
        serde_json::json!("route-override"),
    );
    exchange.output = Some(out);

    let result: Result<Exchange, CamelError> = Ok(exchange);
    let intent = maybe_reply(&result, &original)
        .expect("an Ok route with replyTo+correlationId must publish a reply");
    send_reply(&intent, &publisher)
        .await
        .expect("the reply publish must succeed");

    let calls = publisher.recorded();
    assert_eq!(calls.len(), 1, "exactly one reply must be published");
    let (reply_to, correlation_id, body, props) = &calls[0];
    assert_eq!(reply_to, "amq.rabbitmq.reply-to.g1");
    assert_eq!(correlation_id, "c1");
    assert_eq!(body, b"out", "the OUT body must be published");
    assert_eq!(
        props.delivery_mode(),
        &Some(1),
        "replies must be transient (delivery mode 1)"
    );
    assert_eq!(
        props.correlation_id().as_ref().map(|value| value.as_str()),
        Some("c1"),
        "the original request correlation id must override the route's"
    );

    // G-10 failure branch (same test, no new function): a reply publish failure
    // surfaces as Err from the helper, but the successful route's own
    // disposition is UNCHANGED — the production run_loop records b-prime + warn
    // and falls through to the normal Ack (checked here on the same route
    // result; the run_loop fall-through is verified by inspection). The b-prime
    // label shape is the canonical operational signal for that infra failure.
    publisher.fail.store(true, Ordering::SeqCst);
    let failure = send_reply(&intent, &publisher).await;
    assert!(
        failure.is_err(),
        "a reply publish failure must surface as Err from the helper"
    );
    assert_eq!(
        disposition(&result, false),
        Disposition::Ack,
        "G-10: a reply publish failure must not change the Ok route's Ack"
    );
}

#[tokio::test]
async fn no_reply_on_route_failure() {
    let publisher = RecordingReplyPublisher::default();
    let original = request_reply_headers();
    let failed: Result<Exchange, CamelError> =
        Err(CamelError::ProcessorError("route failed".to_string()));

    let intent = maybe_reply(&failed, &original);
    assert!(intent.is_none(), "a failed route must send no reply");
    if let Some(intent) = intent {
        let _ = send_reply(&intent, &publisher).await;
    }
    assert!(
        publisher.recorded().is_empty(),
        "zero publishes on a failed route"
    );
}

#[tokio::test]
async fn no_reply_without_headers() {
    let publisher = RecordingReplyPublisher::default();
    // Only correlationId: no replyTo, so no reply is warranted.
    let mut original = camel_api::Headers::new();
    original.insert("correlationId".to_string(), serde_json::json!("c1"));
    let result: Result<Exchange, CamelError> = Ok(Exchange::new(Message::new(Body::Empty)));

    let intent = maybe_reply(&result, &original);
    assert!(intent.is_none(), "a missing replyTo must send no reply");
    if let Some(intent) = intent {
        let _ = send_reply(&intent, &publisher).await;
    }
    assert!(
        publisher.recorded().is_empty(),
        "zero publishes without a replyTo"
    );
}
