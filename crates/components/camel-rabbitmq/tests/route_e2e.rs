//! Runtime route e2e: a `rabbitmq:` consumer driven through `CamelContext`.
//!
//! This is the regression test for rc-2vgej: `RabbitConsumer` (Explicit
//! startup) spawns detached engine tasks and returns, without exposing a
//! `Consumer::background_task_handle()`. The runtime therefore believes the
//! consumer has no supervised background work, runs the finally `stop()`
//! immediately after `start()`, and the AMQP channels close before a delivery
//! can reach the route. On the fixed component the consumer stays alive for
//! the whole context lifetime and receives the published message.
//!
//! Broker-dependent tests are gated by `RABBITMQ_ITEST=1` (see
//! `tests/common/mod.rs`): unset prints a notice and returns early, while
//! `RABBITMQ_ITEST=1` with no docker panics `infra-unavailable`.

mod common;

use std::time::Duration;

use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_mock::MockComponent;
use camel_component_rabbitmq::RabbitMqBundle;
use camel_core::CamelContext;
use lapin::Channel;
use lapin::options::{BasicPublishOptions, QueueDeclareOptions};
use lapin::types::{FieldTable, ShortString};

/// Deadline for the route to observe the published message and for the
/// broker to report consumer arrival/departure.
const ROUTE_BOUND: Duration = Duration::from_secs(15);
/// Deadline for a single broker RPC.
const RPC_BOUND: Duration = Duration::from_secs(30);

/// Unique-per-process suffix so parallel tests and re-runs never collide.
fn unique(tag: &str) -> String {
    format!("{tag}-{}-{}", std::process::id(), common::nanos())
}

/// Publish `payload` to `queue` via the default exchange, awaiting the broker
/// confirm.
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
    tokio::time::timeout(RPC_BOUND, confirm)
        .await
        .expect("publish confirm must arrive within the bound")
        .expect("publish confirm must be delivered");
}

/// Bounded passive-declare poll for the queue's live consumer count.
///
/// Returns the observed count once `predicate` holds, or panics naming the
/// last observed count when the bound elapses.
async fn wait_for_consumer_count(
    channel: &Channel,
    queue: &str,
    predicate: impl Fn(usize) -> bool,
    expectation: &str,
) -> usize {
    tokio::time::timeout(ROUTE_BOUND, async {
        loop {
            let declared = tokio::time::timeout(
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
            let consumers = declared.consumer_count() as usize;
            if predicate(consumers) {
                return consumers;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("queue '{queue}' must report {expectation} within the bound"))
}

#[tokio::test]
async fn runtime_route_keeps_rabbitmq_consumer_alive() {
    let Some(fx) = common::require_fixture() else {
        return;
    };
    let queue = unique("route-e2e");
    // Pre-declare the durable queue the consumer binds to, and keep this
    // channel for publishing plus passive-declare observations.
    let (_connection, channel) = fx.declare_queue(&queue).await;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.expect("context");

    let toml_str = format!("[brokers.main]\nurl = \"{}\"\n", fx.amqp_url);
    let value: toml::Value = toml::from_str(&toml_str).expect("parse toml");
    let bundle = RabbitMqBundle::from_toml(value).expect("bundle");
    bundle.register_all(&mut ctx);
    ctx.register_component(mock.clone());

    // `default` selects the broker's default exchange.
    let route = RouteBuilder::from(&format!("rabbitmq:default?queue={queue}"))
        .route_id("rabbitmq-route-e2e")
        .to("mock:result")
        .build()
        .expect("route");
    ctx.add_route_definition(route)
        .await
        .expect("add route definition");
    ctx.start().await.expect("start context");

    let payload = b"route-e2e-payload";
    publish(&channel, &queue, payload).await;

    // 1. The route must receive the message. On the buggy component the
    //    runtime stops the consumer right after start, so this never arrives.
    let endpoint = mock.get_endpoint("result").expect("endpoint exists");
    tokio::time::timeout(ROUTE_BOUND, async {
        loop {
            if !endpoint.get_received_exchanges().await.is_empty() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the route must receive the published message within the bound");

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(
        exchanges.len(),
        1,
        "exactly one published message must reach the mock"
    );
    match &exchanges[0].input.body {
        Body::Bytes(bytes) => assert_eq!(
            bytes.as_ref(),
            payload,
            "the mock must receive the published payload"
        ),
        other => panic!("expected Body::Bytes, got {other:?}"),
    }

    // 2. While the context is running the broker must report a live consumer.
    let running_consumers =
        wait_for_consumer_count(&channel, &queue, |n| n >= 1, "at least one live consumer").await;
    assert!(
        running_consumers >= 1,
        "the running context must keep its consumer attached, got {running_consumers}"
    );

    ctx.stop().await.expect("stop context");

    // 3. Stopping the context must detach the consumer from the broker.
    let stopped_consumers = wait_for_consumer_count(
        &channel,
        &queue,
        |n| n == 0,
        "zero live consumers after stop",
    )
    .await;
    assert_eq!(
        stopped_consumers, 0,
        "the stopped context must leave no consumer attached to the queue"
    );
}
