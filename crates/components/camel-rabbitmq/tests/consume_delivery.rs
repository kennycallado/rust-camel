//! Task 2.2 — ack-after-route and failure disposition against a real broker.
//!
//! The route is the test itself: it receives each [`ExchangeEnvelope`] from
//! the consumer's mpsc channel and answers it `Ok` or `Err`. Holding an
//! envelope unanswered blocks the route mid-flight, which is how these tests
//! prove a delivery stays unacknowledged until the route completes (the
//! ack-only-after-completion contract).
//!
//! Broker-dependent tests are gated by `RABBITMQ_ITEST=1` (see
//! `tests/common/mod.rs`): unset prints a notice and returns early, while
//! `RABBITMQ_ITEST=1` with no docker panics `infra-unavailable` (no silent
//! skip).

mod common;

use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange};
use camel_component_api::test_support::{NoopRuntimeObservability, RecordingRuntimeObservability};
use camel_component_api::{
    Consumer, ConsumerContext, ExchangeEnvelope, NetworkRetryPolicy, RuntimeObservability,
    StartupSignal,
};
use camel_component_rabbitmq::config::{RabbitBrokerConfig, RabbitEndpointConfig};
use camel_component_rabbitmq::{RabbitConnectionManager, RabbitConsumer};
use lapin::options::{
    BasicGetOptions, BasicNackOptions, BasicPublishOptions, ExchangeDeclareOptions,
    QueueBindOptions, QueueDeclareOptions,
};
use lapin::types::{AMQPValue, FieldTable, LongString, ShortString};
use lapin::{Channel, ExchangeKind};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

/// Deadline for a delivery the test is actively waiting for.
const RECV_BOUND: Duration = Duration::from_secs(20);
/// Deadline for consumer start/stop and broker round trips.
const RPC_BOUND: Duration = Duration::from_secs(30);

/// Unique-per-process suffix so parallel tests and re-runs never collide.
fn unique(tag: &str) -> String {
    format!("{tag}-{}-{}", std::process::id(), common::nanos())
}

/// Short, local-retry policy so a fixture that is already up connects fast.
fn retry_policy() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 0, // unlimited, per the component reconnect default
        initial_delay: Duration::from_millis(50),
        multiplier: 1.0,
        max_delay: Duration::from_millis(50),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    }
}

/// Real-lapin manager pointed at the fixture URL.
fn manager_for(url: &str) -> Arc<RabbitConnectionManager> {
    let broker = RabbitBrokerConfig {
        url: url.to_string(),
        username: None,
        password: None,
        vhost: None,
    };
    Arc::new(RabbitConnectionManager::from_broker_config(
        &broker,
        retry_policy(),
    ))
}

/// A no-op observability for tests that do not assert emissions.
fn noop_rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoopRuntimeObservability)
}

/// Start a real consumer and return it with the receiver its engine feeds.
async fn start_consumer(
    config: RabbitEndpointConfig,
    manager: Arc<RabbitConnectionManager>,
    rt: Arc<dyn RuntimeObservability>,
    route_id: &str,
) -> (
    RabbitConsumer,
    mpsc::Receiver<ExchangeEnvelope>,
    CancellationToken,
) {
    let (tx, rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();
    let (signal, receiver) = StartupSignal::pair();
    let ctx = ConsumerContext::new(tx, cancel.clone(), route_id.to_string()).with_startup(signal);
    let mut consumer = RabbitConsumer::new(config, manager, rt);

    timeout(RPC_BOUND, consumer.start(ctx))
        .await
        .expect("consumer.start must return within the bound")
        .expect("consumer.start must succeed against the fixture");
    timeout(RPC_BOUND, receiver.await_ready())
        .await
        .expect("readiness must resolve within the bound")
        .expect("readiness must be Ok after a successful start");

    (consumer, rx, cancel)
}

/// Receive one in-flight envelope, bounded.
async fn recv_envelope(rx: &mut mpsc::Receiver<ExchangeEnvelope>) -> ExchangeEnvelope {
    timeout(RECV_BOUND, rx.recv())
        .await
        .expect("a delivery must arrive within the bound")
        .expect("the delivery channel must stay open")
}

/// Stop a consumer and confirm the loop joined within the bound.
async fn stop_consumer(consumer: &mut RabbitConsumer) {
    timeout(RPC_BOUND, consumer.stop())
        .await
        .expect("consumer.stop must return within the bound")
        .expect("consumer.stop must succeed");
}

/// Answer a held envelope `Ok` with its own exchange. A dropped receiver —
/// the engine already observed stop and abandoned its wait — is expected and
/// never unwrapped.
fn answer_ok(envelope: ExchangeEnvelope) {
    let ExchangeEnvelope {
        exchange, reply_tx, ..
    } = envelope;
    if let Some(reply_tx) = reply_tx {
        let _ = reply_tx.send(Ok(exchange));
    }
}

/// Answer a held envelope `Err`, failing the route.
fn answer_err(envelope: ExchangeEnvelope, error: CamelError) {
    if let Some(reply_tx) = envelope.reply_tx {
        let _ = reply_tx.send(Err(error));
    }
}

/// Assert the exchange payload is exactly `expected` bytes.
fn assert_payload(exchange: &Exchange, expected: &[u8]) {
    match &exchange.input.body {
        Body::Bytes(bytes) => assert_eq!(bytes.as_ref(), expected, "unexpected payload"),
        other => panic!("expected Body::Bytes, got {other:?}"),
    }
}

/// Publish `payload` straight to `queue` via the default exchange, awaiting
/// the broker confirm.
async fn publish(channel: &Channel, queue: &str, payload: &[u8]) {
    let confirm = channel
        .basic_publish(
            ShortString::default(),
            ShortString::from(queue),
            BasicPublishOptions::default(),
            payload,
            lapin::BasicProperties::default().with_delivery_mode(2),
        )
        .await
        .expect("fixture basic_publish must be accepted");
    timeout(RPC_BOUND, confirm)
        .await
        .expect("publish confirm must arrive within the bound")
        .expect("publish confirm must be delivered");
}

/// One-shot pull. `no_ack=false` leaves the delivery unacknowledged so the
/// caller decides its fate.
async fn basic_get(
    channel: &Channel,
    queue: &str,
    no_ack: bool,
) -> Option<lapin::message::BasicGetMessage> {
    timeout(
        RPC_BOUND,
        channel.basic_get(ShortString::from(queue), BasicGetOptions { no_ack }),
    )
    .await
    .expect("basic_get must return within the bound")
    .expect("basic_get must not error")
}

/// Wait (bounded) for a message to appear on `queue`, consuming it.
async fn wait_for_message(channel: &Channel, queue: &str) -> lapin::message::BasicGetMessage {
    timeout(RECV_BOUND, async {
        loop {
            if let Some(message) = basic_get(channel, queue, true).await {
                return message;
            }
            // Finite by contract, so the bounded poll is not a wait site.
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the message must arrive within the bound")
}

/// Passive declare: the ready-message count must be zero (the acked message
/// was not redelivered a third time).
async fn assert_queue_empty(channel: &Channel, queue: &str) {
    let declared = timeout(
        RPC_BOUND,
        channel.queue_declare(
            ShortString::from(queue),
            QueueDeclareOptions {
                passive: true,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        ),
    )
    .await
    .expect("passive declare must return within the bound")
    .expect("passive declare must succeed");
    assert_eq!(
        declared.message_count(),
        0,
        "no third delivery must remain on the queue"
    );
}

/// Bounded passive-declare poll until the queue reports exactly `consumers`
/// live consumers and `messages` ready messages. No sleeps: the loop yields
/// between bounded broker round trips, and the whole poll is itself bounded.
async fn wait_for_queue_counts(
    channel: &Channel,
    queue: &str,
    consumers: usize,
    messages: usize,
) -> lapin::Queue {
    timeout(RECV_BOUND, async {
        loop {
            let declared = timeout(
                RPC_BOUND,
                channel.queue_declare(
                    ShortString::from(queue),
                    QueueDeclareOptions {
                        passive: true,
                        ..QueueDeclareOptions::default()
                    },
                    FieldTable::default(),
                ),
            )
            .await
            .expect("passive declare must return within the bound")
            .expect("passive declare must succeed");
            if declared.consumer_count() as usize == consumers
                && declared.message_count() as usize == messages
            {
                return declared;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the queue must reach the expected consumer/message counts within the bound")
}

/// The message stays unacked while the route is blocked and is acked only
/// after the route completes.
#[tokio::test]
async fn ack_only_after_send_and_wait() {
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let queue = unique("ack-after");
    // Pre-declare: the consumer only `basic.consume`s; auto-declare is 3.4.
    let (_connection, channel) = fx.declare_queue(&queue).await;
    publish(&channel, &queue, b"ack-after-payload").await;

    let manager = manager_for(&fx.amqp_url);
    let config = RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue}"))
        .expect("ack-after URI must parse");
    let (mut consumer, mut rx, _cancel) = start_consumer(
        config.clone(),
        Arc::clone(&manager),
        noop_rt(),
        "ack-after-route",
    )
    .await;

    // Route blocked: the message is delivered but held unanswered.
    let held = recv_envelope(&mut rx).await;
    assert_payload(&held.exchange, b"ack-after-payload");

    // Stop closes the consumer channel; the still-unacked in-flight delivery
    // is requeued by the broker.
    stop_consumer(&mut consumer).await;

    // The requeue probe proves the message was still unacked mid-route. A
    // bare `basic_get` on the main queue would race ack ordering; the
    // redelivery flag is the observable.
    let probe = basic_get(&channel, &queue, false)
        .await
        .expect("the stopped consumer must requeue the in-flight message");
    assert!(
        probe.redelivered,
        "a requeued delivery must be marked redelivered"
    );
    probe
        .acker
        .nack(BasicNackOptions {
            requeue: true,
            ..BasicNackOptions::default()
        })
        .await
        .expect("probe nack with requeue must be accepted");

    // A late Ok for the abandoned delivery must not panic: the engine dropped
    // its receiver when stop won the select.
    answer_ok(held);

    // Restart: the requeued payload is delivered again and acked on Ok.
    let (mut consumer2, mut rx2, _cancel2) =
        start_consumer(config, Arc::clone(&manager), noop_rt(), "ack-after-route-2").await;
    let redelivered = recv_envelope(&mut rx2).await;
    assert_payload(&redelivered.exchange, b"ack-after-payload");
    answer_ok(redelivered);
    stop_consumer(&mut consumer2).await;

    // No third delivery: the acked message left the queue.
    assert_queue_empty(&channel, &queue).await;
}

/// A failed route with default options nacks without requeue, so the broker
/// dead-letters the message to the configured DLX.
#[tokio::test]
async fn failed_route_rejects_to_dlx() {
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let main = unique("dlx-main");
    let dlx = unique("dlx-exchange");
    let dlq = unique("dlx-queue");
    // `declare_queue` yields the raw connection/channel used for the topology
    // below and pre-declares the DLQ (durable, matching the bind).
    let (_connection, channel) = fx.declare_queue(&dlq).await;

    // Raw topology: task 3.4 owns auto-declare; 2.2 declares explicitly.
    timeout(
        RPC_BOUND,
        channel.exchange_declare(
            ShortString::from(dlx.as_str()),
            ExchangeKind::Direct,
            ExchangeDeclareOptions {
                durable: true,
                ..ExchangeDeclareOptions::default()
            },
            FieldTable::default(),
        ),
    )
    .await
    .expect("DLX declare must return within the bound")
    .expect("DLX declare must succeed");

    timeout(
        RPC_BOUND,
        channel.queue_declare(
            ShortString::from(dlq.as_str()),
            QueueDeclareOptions {
                durable: true,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        ),
    )
    .await
    .expect("DLQ declare must return within the bound")
    .expect("DLQ declare must succeed");

    timeout(
        RPC_BOUND,
        channel.queue_bind(
            ShortString::from(dlq.as_str()),
            ShortString::from(dlx.as_str()),
            ShortString::from(main.as_str()),
            QueueBindOptions::default(),
            FieldTable::default(),
        ),
    )
    .await
    .expect("DLQ bind must return within the bound")
    .expect("DLQ bind must succeed");

    let mut main_args = FieldTable::default();
    main_args.insert(
        ShortString::from("x-dead-letter-exchange"),
        AMQPValue::LongString(LongString::from(dlx.clone())),
    );
    timeout(
        RPC_BOUND,
        channel.queue_declare(
            ShortString::from(main.as_str()),
            QueueDeclareOptions {
                durable: true,
                ..QueueDeclareOptions::default()
            },
            main_args,
        ),
    )
    .await
    .expect("main declare must return within the bound")
    .expect("main declare must succeed");

    publish(&channel, &main, b"poison").await;

    let manager = manager_for(&fx.amqp_url);
    let config = RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={main}"))
        .expect("dlx URI must parse");
    let (mut consumer, mut rx, _cancel) =
        start_consumer(config, Arc::clone(&manager), noop_rt(), "dlx-route").await;

    let poisoned = recv_envelope(&mut rx).await;
    assert_payload(&poisoned.exchange, b"poison");
    answer_err(
        poisoned,
        CamelError::ProcessorError("route failed".to_string()),
    );

    let dead = wait_for_message(&channel, &dlq).await;
    assert_eq!(
        dead.data, b"poison",
        "the rejected payload must land on the dead-letter queue"
    );

    stop_consumer(&mut consumer).await;
}

/// `requeueOnFailure=true` opts into requeue: a failed route redelivers the
/// same payload. The redelivered-header assert is task 2.4.
#[tokio::test]
async fn requeue_opt_in_redelivers() {
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let queue = unique("requeue");
    let (_connection, channel) = fx.declare_queue(&queue).await;
    publish(&channel, &queue, b"requeue-payload").await;

    let manager = manager_for(&fx.amqp_url);
    let config = RabbitEndpointConfig::from_uri(&format!(
        "rabbitmq:default?queue={queue}&requeueOnFailure=true"
    ))
    .expect("requeue URI must parse");
    let (mut consumer, mut rx, _cancel) =
        start_consumer(config, Arc::clone(&manager), noop_rt(), "requeue-route").await;

    let first = recv_envelope(&mut rx).await;
    assert_payload(&first.exchange, b"requeue-payload");
    answer_err(
        first,
        CamelError::ProcessorError("route failed".to_string()),
    );

    // Opt-in requeue: the broker redelivers the same payload.
    let second = recv_envelope(&mut rx).await;
    assert_payload(&second.exchange, b"requeue-payload");
    // Task 2.4: the redelivered flag must surface as a camel header.
    assert_eq!(
        second.exchange.input.headers.get("rabbitmq.redelivered"),
        Some(&serde_json::json!(true)),
        "a requeued delivery must carry rabbitmq.redelivered=true"
    );
    // Answer Ok so the redelivered copy is acked, then stop (no hot loop).
    answer_ok(second);
    stop_consumer(&mut consumer).await;

    assert_queue_empty(&channel, &queue).await;
}

/// `concurrentConsumers=3` and `prefetch=5` register three consumers on one
/// queue; with 20 preloaded messages and every route blocked, the broker holds
/// 3 x 5 = 15 deliveries unacked and leaves 5 ready. A single engine (or a
/// missing prefetch) would leave a different ready count, so the option is
/// non-vacuous. The engines stay serial (one in-flight route each); lapin
/// buffers the remaining prefetch deliveries, so no extra futures concurrency
/// is needed in this test.
#[tokio::test]
async fn concurrent_consumers_register_on_queue() {
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let queue = unique("concurrent");
    let (_connection, channel) = fx.declare_queue(&queue).await;
    for index in 0..20 {
        publish(&channel, &queue, format!("msg-{index}").as_bytes()).await;
    }

    let manager = manager_for(&fx.amqp_url);
    let config = RabbitEndpointConfig::from_uri(&format!(
        "rabbitmq:default?queue={queue}&concurrentConsumers=3&prefetch=5"
    ))
    .expect("concurrent URI must parse");
    let (mut consumer, mut rx, _cancel) =
        start_consumer(config, Arc::clone(&manager), noop_rt(), "concurrent-route").await;

    // One held envelope per engine blocks each engine mid-route; the remaining
    // deliveries stay buffered client-side and counted unacked by the broker.
    let mut held = Vec::new();
    for _ in 0..3 {
        held.push(recv_envelope(&mut rx).await);
    }
    assert_eq!(
        held.len(),
        3,
        "each engine must dispatch exactly one delivery"
    );

    let declared = wait_for_queue_counts(&channel, &queue, 3, 5).await;
    assert_eq!(
        declared.consumer_count() as usize,
        3,
        "three engines must register as three consumers"
    );
    assert_eq!(
        declared.message_count() as usize,
        5,
        "prefetch=5 across three engines must leave 5 of 20 messages ready"
    );

    stop_consumer(&mut consumer).await;
}

/// Bounded poll (no sleeps) until `rt` has recorded at least `n` component
/// ops, returning the snapshot. Finite by contract: the consumer records an
/// op for every disposition it applies.
async fn wait_for_ops(
    rt: &RecordingRuntimeObservability,
    n: usize,
) -> Vec<(String, String, String)> {
    timeout(RECV_BOUND, async {
        loop {
            let ops = rt.ops();
            if ops.len() >= n {
                return ops;
            }
            // Finite by contract, so the bounded poll is not a wait site.
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the consumer must record its outcome ops within the bound")
}

/// The consumer counts route outcomes post-disposition: one route answered
/// `Ok` (acked) and one answered `Err` (nacked) yield exactly one
/// `("rabbitmq","consume","success")` and one
/// `("rabbitmq","consume","failure")` observation (task 2.6, gates
/// "consume outcome counted"). The observer is attached to the real consumer
/// through `RabbitConsumer::new`; no production test hook is added.
#[tokio::test]
async fn consume_outcome_counted() {
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let queue = unique("consume-outcome");
    let (_connection, channel) = fx.declare_queue(&queue).await;
    publish(&channel, &queue, b"ok-payload").await;
    publish(&channel, &queue, b"err-payload").await;

    let manager = manager_for(&fx.amqp_url);
    let config = RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue}"))
        .expect("consume-outcome URI must parse");
    let rt = RecordingRuntimeObservability::new(true);
    let (mut consumer, mut rx, _cancel) = start_consumer(
        config,
        Arc::clone(&manager),
        rt.clone(),
        "consume-outcome-route",
    )
    .await;

    // One engine drains the queue FIFO: answer the first Ok, the second Err.
    let ok = recv_envelope(&mut rx).await;
    assert_payload(&ok.exchange, b"ok-payload");
    answer_ok(ok);

    let failed = recv_envelope(&mut rx).await;
    assert_payload(&failed.exchange, b"err-payload");
    answer_err(
        failed,
        CamelError::ProcessorError("route failed".to_string()),
    );

    let ops = wait_for_ops(&rt, 2).await;
    let success = ops
        .iter()
        .filter(|(c, o, out)| c == "rabbitmq" && o == "consume" && out == "success")
        .count();
    let failure = ops
        .iter()
        .filter(|(c, o, out)| c == "rabbitmq" && o == "consume" && out == "failure")
        .count();
    assert_eq!(
        ops.len(),
        2,
        "exactly the two route outcomes must be observed, got: {ops:?}"
    );
    assert_eq!(success, 1, "one acked route must count one consume success");
    assert_eq!(
        failure, 1,
        "one nacked route must count one consume failure"
    );

    stop_consumer(&mut consumer).await;
}

/// A lost route transport (the reply oneshot is dropped unanswered) is not a
/// business failure: the delivery is abandoned without a disposition, the
/// engine closes only its own channel, and the broker requeues the message.
/// The shared connection stays healthy (no broker-wide reset).
#[tokio::test]
async fn route_transport_loss_requeues_unacked_message() {
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let queue = unique("transport-loss");
    let (_connection, channel) = fx.declare_queue(&queue).await;
    publish(&channel, &queue, b"transport-loss-payload").await;

    let manager = manager_for(&fx.amqp_url);
    let config = RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue}"))
        .expect("transport-loss URI must parse");
    let (mut consumer, mut rx, _cancel) = start_consumer(
        config,
        Arc::clone(&manager),
        noop_rt(),
        "transport-loss-route",
    )
    .await;

    let held = recv_envelope(&mut rx).await;
    assert_payload(&held.exchange, b"transport-loss-payload");
    // Drop the reply oneshot unanswered: the route transport is gone. The
    // engine must abandon the delivery (no ack/nack) and release its channel.
    drop(held.reply_tx);

    // Broker requeue is observable: a bounded poll (no sleeps) sees the same
    // payload marked redelivered.
    let redelivered = wait_for_message(&channel, &queue).await;
    assert_eq!(
        redelivered.data, b"transport-loss-payload",
        "the abandoned payload must be requeued"
    );
    assert!(
        redelivered.redelivered,
        "a requeued delivery must be marked redelivered"
    );

    // Only the consumer's channel was closed: the shared manager connection
    // must stay healthy.
    timeout(RPC_BOUND, manager.connection_within(Duration::from_secs(2)))
        .await
        .expect("the connection check must return within the bound")
        .expect("the shared connection must stay healthy after a route transport loss");

    stop_consumer(&mut consumer).await;
}
