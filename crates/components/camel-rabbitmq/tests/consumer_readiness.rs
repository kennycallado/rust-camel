//! Consumer start-readiness against a real broker (task 2.1).
//!
//! The container starts on a caller-reserved host port so the consumer points
//! at a known, initially-absent address. A gated real-lapin connect adapter
//! (`RabbitConnectionManager::new`'s public connect seam) holds each
//! established connection until the test has declared the queue, making the
//! readiness sequence deterministic without a fake connection: the broker is a
//! real `rabbitmq:3.13-alpine` container and every handshake is real AMQP.

mod common;

use std::sync::Arc;
use std::time::Duration;

use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{Consumer, ConsumerContext, NetworkRetryPolicy, StartupSignal};
use camel_component_rabbitmq::config::RabbitEndpointConfig;
use camel_component_rabbitmq::{ConnectFn, RabbitConnectionManager, RabbitConsumer};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn consumer_not_ready_until_connected() {
    if !common::gate_active() {
        return;
    }

    let port = common::reserve_free_port();
    let queue = format!("readiness-{}", common::nanos());
    let url = format!("amqp://rmq:rmq@127.0.0.1:{port}/%2f");

    // Real-lapin connect adapter that holds the established connection until
    // the test releases the gate, so the consumer cannot `basic.consume`
    // before the queue has been declared.
    let gate = Arc::new(Notify::new());
    let gate_for_fn = Arc::clone(&gate);
    let connect_fn: ConnectFn = Arc::new(move |url: &str| {
        let url = url.to_string();
        let gate = Arc::clone(&gate_for_fn);
        Box::pin(async move {
            let connection =
                lapin::Connection::connect(&url, lapin::ConnectionProperties::default()).await?;
            gate.notified().await;
            Ok(connection)
        })
    });

    let retry = NetworkRetryPolicy {
        max_attempts: 0, // unlimited, per the component reconnect default
        initial_delay: Duration::from_millis(50),
        multiplier: 1.0,
        max_delay: Duration::from_millis(50),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    };

    let manager = Arc::new(RabbitConnectionManager::new(url, retry, connect_fn));
    let config = RabbitEndpointConfig::from_uri(&format!("rabbitmq:default?queue={queue}"))
        .expect("readiness URI must parse");
    let mut consumer = RabbitConsumer::new(config, manager, Arc::new(NoopRuntimeObservability));

    let (tx, _rx) = mpsc::channel(8);
    let ctx_cancel = CancellationToken::new();
    let (signal, receiver) = StartupSignal::pair();
    let ctx = ConsumerContext::new(tx, ctx_cancel.clone(), "readiness-route".to_string())
        .with_startup(signal);

    let start = tokio::spawn(async move { consumer.start(ctx).await });

    // Observe readiness across both phases by pinning the await future: the
    // deferred route waits while the broker is absent.
    let ready = receiver.await_ready();
    tokio::pin!(ready);

    assert!(
        tokio::time::timeout(Duration::from_millis(500), &mut ready)
            .await
            .is_err(),
        "the consumer must not be ready while the broker is absent"
    );

    // Broker up, queue declared, then release the held connection. Only now
    // may the consumer reach `basic.consume`.
    let _fixture = common::RabbitFixture::start_on_port(port);
    let (_connection, _channel) = _fixture.declare_queue(&queue).await;
    gate.notify_one();

    tokio::time::timeout(Duration::from_secs(30), &mut ready)
        .await
        .expect("the consumer must become ready within 30s of the broker starting")
        .expect("consumer startup must succeed");

    ctx_cancel.cancel();
    let _ = start.await;
}
