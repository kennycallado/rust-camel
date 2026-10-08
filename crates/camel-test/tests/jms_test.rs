//! Integration tests for the JMS component.
//!
//! **Requires Docker to be running.**
//! **Requires `integration-tests` feature:** `cargo test -p camel-test --features integration-tests`
//! **Requires the JMS bridge binary.** Set `CAMEL_JMS_BRIDGE_RELEASE_URL` or pre-build.
//!
//! Tests share a single bridge process per broker type (via `support::jms`) so they can
//! run in parallel without exhausting system resources.

#![cfg(feature = "integration-tests")]

mod support;

use std::collections::HashMap;
use std::time::Duration;

use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_jms::proto::bridge_service_client::BridgeServiceClient;
use camel_component_jms::proto::{HealthRequest, JmsMessage, SendRequest, SubscribeRequest};
use camel_test::CamelTestContext;
use support::activemq::ActiveMqBroker;
use support::install_crypto_provider;
use support::jms::{shared_jms_activemq, shared_jms_artemis, shared_jms_artemis_auth};
use support::jms_bridge_procs::{require_jms_bridge_binary, spawn_jms_bridge};
use support::wait::wait_until;

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,camel=info")),
        )
        .with_test_writer()
        .try_init();
}

#[tokio::test(flavor = "multi_thread")]
async fn jms_producer_sends_to_activemq() {
    init_tracing();
    install_crypto_provider();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_activemq().await)
        .build()
        .await;

    // Use multiple attempts so that a transient JMS-broker connection delay on
    // the first fire does not cause the whole test to fail (shared bridge may
    // still be establishing the broker connection even though gRPC health passed).
    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=5&delay=0")
        .set_body("hello-jms".to_string())
        .to("jms:queue:test-produce")
        .to("mock:sent")
        .route_id("jms-producer-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("sent").unwrap();
    wait_until(
        "jms producer route delivery",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        !exchanges.is_empty(),
        "expected at least one message to be delivered via JMS producer"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jms_consumer_receives_from_activemq() {
    init_tracing();
    install_crypto_provider();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_activemq().await)
        .build()
        .await;

    let consumer_route = RouteBuilder::from("jms:queue:test-consume-activemq")
        .to("mock:consumed")
        .route_id("jms-consumer-activemq")
        .build()
        .unwrap();

    let inject_route =
        RouteBuilder::from("timer:inject-activemq?period=300&delay=500&repeatCount=1")
            .set_body("consume-from-activemq".to_string())
            .to("jms:queue:test-consume-activemq")
            .route_id("jms-consumer-activemq-inject")
            .build()
            .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(inject_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "jms consumer receives message from activemq",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(!exchanges.is_empty());
    assert_eq!(
        exchanges[0].input.body.as_text(),
        Some("consume-from-activemq"),
        "Body should survive the ActiveMQ round-trip intact"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jms_consumer_receives_from_artemis() {
    init_tracing();
    install_crypto_provider();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_artemis().await)
        .build()
        .await;

    let consumer_route = RouteBuilder::from("jms:queue:test-consume-artemis")
        .to("mock:consumed")
        .route_id("jms-consumer-artemis")
        .build()
        .unwrap();

    let inject_route =
        RouteBuilder::from("timer:inject-artemis?period=300&delay=500&repeatCount=1")
            .set_body("consume-from-artemis".to_string())
            .to("jms:queue:test-consume-artemis")
            .route_id("jms-consumer-artemis-inject")
            .build()
            .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(inject_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "jms consumer receives message from artemis",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(!exchanges.is_empty());
    assert_eq!(
        exchanges[0].input.body.as_text(),
        Some("consume-from-artemis"),
        "Body should survive the Artemis round-trip intact"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jms_producer_sends_to_artemis() {
    init_tracing();
    install_crypto_provider();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_artemis().await)
        .build()
        .await;

    // Use multiple attempts so that a transient JMS-broker connection delay on
    // the first fire does not cause the whole test to fail (shared bridge may
    // still be establishing the broker connection even though gRPC health passed).
    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=5&delay=0")
        .set_body("hello-artemis".to_string())
        .to("jms:queue:test-produce-artemis")
        .to("mock:sent")
        .route_id("jms-producer-artemis-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("sent").unwrap();
    wait_until(
        "jms producer route delivery (artemis)",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        !exchanges.is_empty(),
        "expected at least one message to be delivered via JMS producer (artemis)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jms_headers_propagated() {
    init_tracing();
    install_crypto_provider();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_activemq().await)
        .build()
        .await;

    let consumer_route = RouteBuilder::from("jms:queue:test-headers")
        .to("mock:headers")
        .route_id("jms-headers-consumer")
        .build()
        .unwrap();

    let producer_route =
        RouteBuilder::from("timer:headers-inject?period=300&delay=500&repeatCount=1")
            .set_body("header-check".to_string())
            .set_header("x-custom-header", Value::String("custom-value".to_string()))
            .to("jms:queue:test-headers")
            .route_id("jms-headers-producer")
            .build()
            .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(producer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("headers").unwrap();
    wait_until(
        "jms headers consumer receives message",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert!(!exchanges.is_empty(), "Expected at least one exchange");
    let received = &exchanges[0];
    assert_eq!(
        received.input.body.as_text(),
        Some("header-check"),
        "Body should survive the round-trip intact"
    );
    assert_eq!(
        received
            .input
            .header("x-custom-header")
            .and_then(|v| v.as_str()),
        Some("custom-value")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jms_producer_sends_multiple_messages() {
    init_tracing();
    install_crypto_provider();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_activemq().await)
        .build()
        .await;

    // Verifies that the JMS bridge can deliver multiple sequential messages
    // through a producer+consumer round-trip on the same broker.
    // Uses 5 repetitions with 1s intervals so the test survives transient
    // transport errors (e.g. H2 connection teardown from concurrent test
    // cancellation) without losing all fire attempts.
    let consumer_route = RouteBuilder::from("jms:queue:test-multi")
        .to("mock:consumed")
        .route_id("jms-multi-consumer")
        .build()
        .unwrap();

    let producer_route =
        RouteBuilder::from("timer:multi-inject?period=1000&delay=2000&repeatCount=5")
            .set_body("multi-msg".to_string())
            .to("jms:queue:test-multi")
            .route_id("jms-multi-producer")
            .build()
            .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(producer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "jms multi-message consumer receives two messages",
        Duration::from_secs(35),
        Duration::from_millis(500),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 2) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(exchanges.len() >= 2);
    for ex in &exchanges {
        assert_eq!(
            ex.input.body.as_text(),
            Some("multi-msg"),
            "Each message body should survive the round-trip intact"
        );
    }
}

/// Regression test for: bridge crashes ~7s after start when Artemis uses mandatory auth.
///
/// Root cause: `cached_channel_healthy` used a 1s timeout, but the Artemis health check
/// (creating a bare Netty connection in GraalVM native) takes >1s → health check fails →
/// Rust kills and attempts to restart the bridge → subscribers lose their gRPC connection.
///
/// This test runs two concurrent consumers and a producer that sends messages every second
/// for 15 seconds (well past the ~7s crash window). If the bridge restarts mid-test,
/// messages will be lost and the assertion will fail.
#[tokio::test(flavor = "multi_thread")]
async fn jms_artemis_bridge_stable_under_mandatory_auth() {
    init_tracing();
    install_crypto_provider();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_artemis_auth().await)
        .build()
        .await;

    // Two competing consumers on the same queue — if the bridge restarts, both
    // lose their gRPC streams simultaneously, making the gap obvious.
    let consumer_a = RouteBuilder::from("jms:queue:test-auth-stability")
        .to("mock:stable")
        .route_id("jms-auth-stable-consumer-a")
        .build()
        .unwrap();

    let consumer_b = RouteBuilder::from("jms:queue:test-auth-stability")
        .to("mock:stable")
        .route_id("jms-auth-stable-consumer-b")
        .build()
        .unwrap();

    // Send 1 message/s for 10 iterations starting at 2s (well past the 7s crash).
    let producer = RouteBuilder::from("timer:auth-inject?period=1000&delay=2000&repeatCount=10")
        .set_body("auth-stable-msg".to_string())
        .to("jms:queue:test-auth-stability")
        .route_id("jms-auth-stable-producer")
        .build()
        .unwrap();

    h.add_route(consumer_a).await.unwrap();
    h.add_route(consumer_b).await.unwrap();
    h.add_route(producer).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("stable").unwrap();

    // We expect all 10 messages to be consumed. Allow 30s: bridge is already
    // healthy at this point (h.start() waited), timer fires at +2s, last msg
    // at +12s, plus processing overhead.
    wait_until(
        "jms bridge stable under mandatory auth: 10 messages delivered",
        Duration::from_secs(30),
        Duration::from_millis(500),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint.get_received_exchanges().await.len() >= 10) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    let exchanges = endpoint.get_received_exchanges().await;
    assert!(
        exchanges.len() >= 10,
        "Expected at least 10 messages; bridge may have restarted under mandatory auth"
    );
    for ex in &exchanges {
        assert_eq!(
            ex.input.body.as_text(),
            Some("auth-stable-msg"),
            "Each message body should survive the Artemis auth round-trip intact"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// rc-trxge regression tests: frozen ActiveMQ Classic IdGenerator (data loss).
//
// Bug: `org.apache.activemq.util.IdGenerator` statics were frozen into the
// native image heap, so every process derived identical connection/message ID
// stems. Classic's KahaDB duplicate index then rejected post-restart sends
// ("Duplicate message add attempt rejected" — silent data loss), and two
// bridge processes from the same binary collided on the broker.
//
// Fix under test: class-level `--initialize-at-run-time` for IdGenerator plus
// random per-factory connectionIDPrefix/clientIDPrefix in JmsClientFactory.
// ─────────────────────────────────────────────────────────────────────────────

/// Waits until the bridge process reports a live broker connection, so the
/// subsequent single-shot sends do not race lazy broker setup.
async fn await_broker_connected(client: &mut BridgeServiceClient<tonic::transport::Channel>) {
    wait_until(
        "bridge broker connection",
        Duration::from_secs(30),
        Duration::from_millis(200),
        || {
            let mut probe = client.clone();
            async move {
                Ok(probe
                    .health(HealthRequest {})
                    .await
                    .map(|r| {
                        let h = r.into_inner();
                        h.healthy && h.broker_connected
                    })
                    .unwrap_or(false))
            }
        },
    )
    .await
    .expect("bridge did not report a broker connection in time");
}

/// Receives exactly one message from the subscription stream, bounded.
async fn next_message(stream: &mut tonic::Streaming<JmsMessage>, what: &str) -> JmsMessage {
    tokio::time::timeout(Duration::from_secs(35), stream.message())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        .unwrap_or_else(|e| panic!("stream error while waiting for {what}: {e}"))
        .unwrap_or_else(|| panic!("subscription stream ended before {what} arrived"))
}

/// Sends one text message through `client` (single shot — no retries, so the
/// exact-count assertions below stay sound).
async fn send_one(
    client: &mut BridgeServiceClient<tonic::transport::Channel>,
    queue: &str,
    body: &str,
) -> String {
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        client.send(SendRequest {
            destination: queue.to_string(),
            body: body.as_bytes().to_vec(),
            headers: HashMap::new(),
            content_type: "text/plain".to_string(),
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out sending body {body:?}"))
    .unwrap_or_else(|e| panic!("send of body {body:?} failed: {e}"));
    response.into_inner().message_id
}

/// Blocks until the broker reports `queue`'s `EnqueueCount` at or above
/// `expected`, then re-reads and asserts it is EXACTLY `expected`. This is
/// broker-side evidence (Classic's web console via Jolokia), not an inference
/// from client-side deliveries — a duplicate-add rejection leaves the count
/// lower even though every send call returned (rc-trxge). Callers use isolated
/// regression queues where exactly `expected` messages are sent, so an
/// overshoot is itself a defect and must fail the assertion.
async fn wait_for_enqueue_count(broker: &ActiveMqBroker, queue: &str, expected: u64) {
    let wait_queue = queue.to_string();
    wait_until(
        "broker queue EnqueueCount",
        Duration::from_secs(30),
        Duration::from_millis(200),
        move || {
            let queue = wait_queue.clone();
            async move {
                broker
                    .enqueue_count(&queue)
                    .await
                    .map(|count| count >= expected)
            }
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));

    let observed = broker
        .enqueue_count(queue)
        .await
        .expect("read broker EnqueueCount");
    assert_eq!(
        observed, expected,
        "broker EnqueueCount for {queue} is {observed}, expected exactly {expected}"
    );
}

/// Blocks until the broker registers at least `min` distinct live connection
/// client IDs, then returns the deduplicated set. Two processes that inherited
/// one frozen client ID stem collapse to a single entry, so distinct IDs are
/// the direct broker-side signal that both processes coexisted (rc-trxge).
async fn wait_for_distinct_client_ids(broker: &ActiveMqBroker, min: usize) -> Vec<String> {
    wait_until(
        "broker distinct connection client IDs",
        Duration::from_secs(30),
        Duration::from_millis(200),
        move || async move {
            let mut ids = broker.client_ids().await?;
            ids.sort();
            ids.dedup();
            Ok(ids.len() >= min)
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));

    let mut ids = broker
        .client_ids()
        .await
        .expect("read broker connection client IDs");
    ids.sort();
    ids.dedup();
    ids
}

/// Issues the subscribe RPC with a bounded await. The bridge's gRPC server can
/// hold the response headers until the first message arrives on the queue, so
/// an unbounded `await` on an empty queue risks hanging the whole test. Tests
/// therefore queue their messages before subscribing, and bound this call too.
async fn subscribe_bounded(
    consumer: &mut BridgeServiceClient<tonic::transport::Channel>,
    queue: &str,
    subscription_id: &str,
) -> tonic::Streaming<JmsMessage> {
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        consumer.subscribe(SubscribeRequest {
            destination: queue.to_string(),
            subscription_id: subscription_id.to_string(),
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out subscribing consumer to {queue}"))
    .expect("subscribe via consumer bridge process");
    response.into_inner()
}

/// Two bridge processes started from the SAME native binary must coexist on
/// one ActiveMQ Classic broker: the broker accepts both connections (distinct
/// client IDs) and neither process's messages are dropped as duplicates.
#[tokio::test(flavor = "multi_thread")]
async fn jms_two_bridge_processes_same_binary_coexist() {
    init_tracing();
    install_crypto_provider();

    let broker = ActiveMqBroker::start().await;
    let binary = require_jms_bridge_binary();
    let queue = "queue.test-two-procs-same-binary";

    // Consumer bridge process is spawned first and stays alive, but its
    // subscribe is issued only after the sends: the bridge can withhold the
    // gRPC response headers until the first message arrives, so subscribing to
    // an empty queue can hang. A JMS queue retains the messages meanwhile.
    let (consumer_proc, consumer_channel) = spawn_jms_bridge(&binary, &broker.broker_url).await;
    let mut consumer = BridgeServiceClient::new(consumer_channel);

    // Two concurrent producer bridge processes from the SAME binary.
    let (proc1, channel1) = spawn_jms_bridge(&binary, &broker.broker_url).await;
    let (proc2, channel2) = spawn_jms_bridge(&binary, &broker.broker_url).await;
    let mut client1 = BridgeServiceClient::new(channel1);
    let mut client2 = BridgeServiceClient::new(channel2);
    await_broker_connected(&mut client1).await;
    await_broker_connected(&mut client2).await;

    let id1 = send_one(&mut client1, queue, "two-procs-1").await;
    let id2 = send_one(&mut client2, queue, "two-procs-2").await;
    assert!(
        !id1.is_empty() && !id2.is_empty(),
        "both sends must return broker-assigned JMSMessageIDs"
    );
    assert_ne!(
        id1, id2,
        "JMSMessageIDs from two same-binary bridge processes must differ \
         (frozen IdGenerator regression, rc-trxge)"
    );

    // Actual broker-side evidence #1: the queue's EnqueueCount reached 2. A
    // KahaDB duplicate-add rejection would leave it at 1 even though both
    // send calls returned.
    wait_for_enqueue_count(&broker, queue, 2).await;

    // Actual broker-side evidence #2: two distinct client IDs are registered
    // for processes spawned from the SAME binary. A frozen client ID stem made
    // the second process collide with the first on the broker.
    let client_ids = wait_for_distinct_client_ids(&broker, 2).await;
    assert!(
        client_ids.len() >= 2,
        "expected two same-binary bridge processes to register distinct client \
         IDs on the broker, got {client_ids:?} (frozen IdGenerator regression, \
         rc-trxge)"
    );

    // Messages are queued; now attach the consumer and collect both.
    let mut stream = subscribe_bounded(&mut consumer, queue, "two-procs-consumer").await;
    let msg1 = next_message(&mut stream, "message from bridge process 1").await;
    let msg2 = next_message(&mut stream, "message from bridge process 2").await;
    let mut bodies: Vec<String> = [msg1.body, msg2.body]
        .iter()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect();
    bodies.sort();
    assert_eq!(
        bodies,
        vec!["two-procs-1".to_string(), "two-procs-2".to_string()],
        "both messages must be delivered; neither may be dropped as a duplicate"
    );
    let mut delivered_ids = vec![msg1.message_id.clone(), msg2.message_id.clone()];
    delivered_ids.sort();
    let mut sent_ids = vec![id1, id2];
    sent_ids.sort();
    assert_eq!(
        delivered_ids, sent_ids,
        "delivered JMSMessageIDs must match the send-acknowledged ones"
    );

    drop(client1);
    drop(client2);
    drop(consumer);
    drop(proc1);
    drop(proc2);
    drop(consumer_proc);
}

/// Message IDs must stay unique across a bridge restart, and no message may
/// be dropped: send → kill the bridge process → send again from a fresh
/// process of the same binary → both messages arrive with distinct IDs
/// (EnqueueCount grows by 2).
#[tokio::test(flavor = "multi_thread")]
async fn jms_message_ids_unique_across_bridge_restart() {
    init_tracing();
    install_crypto_provider();

    let broker = ActiveMqBroker::start().await;
    let binary = require_jms_bridge_binary();
    let queue = "queue.test-restart-unique-ids";

    // The consumer bridge process stays alive across the producer restarts.
    // It subscribes only after both sends (see `subscribe_bounded`): the queue
    // retains the messages, and subscribing to an empty one can block.
    let (consumer_proc, consumer_channel) = spawn_jms_bridge(&binary, &broker.broker_url).await;
    let mut consumer = BridgeServiceClient::new(consumer_channel);

    // Generation 1: send, then kill the bridge process (simulated restart).
    let (proc1, channel1) = spawn_jms_bridge(&binary, &broker.broker_url).await;
    let mut client1 = BridgeServiceClient::new(channel1);
    await_broker_connected(&mut client1).await;
    let id_before_restart = send_one(&mut client1, queue, "restart-1").await;
    assert!(
        !id_before_restart.is_empty(),
        "send before restart must return a JMSMessageID"
    );
    let ids_before_restart = wait_for_distinct_client_ids(&broker, 1).await;
    drop(client1);
    drop(proc1);

    // Generation 2: fresh process from the SAME binary.
    let (proc2, channel2) = spawn_jms_bridge(&binary, &broker.broker_url).await;
    let mut client2 = BridgeServiceClient::new(channel2);
    await_broker_connected(&mut client2).await;
    let id_after_restart = send_one(&mut client2, queue, "restart-2").await;
    assert!(
        !id_after_restart.is_empty(),
        "send after restart must return a JMSMessageID"
    );
    assert_ne!(
        id_before_restart, id_after_restart,
        "JMSMessageIDs must differ across a bridge restart — identical stems \
         made Classic reject the second send as a duplicate add (silent data \
         loss, rc-trxge)"
    );

    // Actual broker-side evidence: both sends reached the queue (EnqueueCount
    // grew by 2). The pre-fix broker dropped the post-restart message via its
    // duplicate index, leaving the count at 1.
    wait_for_enqueue_count(&broker, queue, 2).await;

    // The fresh process must present its own client ID, not the one the killed
    // process used (frozen client ID stem regression).
    let ids_after_restart = wait_for_distinct_client_ids(&broker, 1).await;
    assert!(
        ids_after_restart
            .iter()
            .any(|id| !ids_before_restart.contains(id)),
        "fresh bridge process must register a client ID distinct from the \
         restarted one; before={ids_before_restart:?} after={ids_after_restart:?}"
    );

    // Both messages are queued; now attach the consumer and collect them.
    let mut stream = subscribe_bounded(&mut consumer, queue, "restart-consumer").await;
    let msg1 = next_message(&mut stream, "pre-restart message").await;
    let msg2 = next_message(&mut stream, "post-restart message").await;
    let mut delivered: Vec<(String, String)> = [
        (
            String::from_utf8_lossy(&msg1.body).to_string(),
            msg1.message_id.clone(),
        ),
        (
            String::from_utf8_lossy(&msg2.body).to_string(),
            msg2.message_id.clone(),
        ),
    ]
    .into_iter()
    .collect();
    delivered.sort();
    let mut expected = vec![
        ("restart-1".to_string(), id_before_restart),
        ("restart-2".to_string(), id_after_restart),
    ];
    expected.sort();
    assert_eq!(
        delivered, expected,
        "both messages must be delivered with their send-acknowledged IDs"
    );

    drop(client2);
    drop(consumer);
    drop(proc2);
    drop(consumer_proc);
}
