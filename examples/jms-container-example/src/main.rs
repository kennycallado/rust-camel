//! JMS + Container example for rust-camel.
//!
//! Demonstrates realistic JMS patterns using `camel-component-container` to
//! manage the broker lifecycle. The Apache ActiveMQ Artemis broker is started
//! via a Camel route and cleaned up on shutdown.
//!
//! ## What this example shows
//!
//! - **Pinned image**: `apache/activemq-artemis:2.36.0-alpine` (no `:latest` surprises)
//! - **Credentials**: broker started with mandatory auth (`ARTEMIS_USER` /
//!   `ARTEMIS_PASSWORD`); `ANONYMOUS_LOGIN` is intentionally **not set** so the
//!   broker rejects unauthenticated connections. `BrokerConfig` passes
//!   `username`/`password` so the bridge can authenticate.
//! - **Custom JMS headers**: producer sets `x-order-id` and `x-priority` on
//!   every message so downstream consumers and filters can act on them.
//! - **Competing consumers**: two independent consumer routes (`consumer-a` and
//!   `consumer-b`) both subscribe to the same `orders` queue. Artemis
//!   distributes messages between them automatically (round-robin).
//! - **Dead Letter Channel**: `consumer-b` always rejects its messages by
//!   returning an error. Camel's DLC catches the failure and routes the exchange
//!   to `log:dlc-sink` instead of silently dropping it.
//! - **Topic fan-out**: a timer publishes to `jms:topic:events`; two topic
//!   consumers each receive every message independently.
//!
//! ## Known limitation — no JMS redelivery
//!
//! The bridge uses `Session.AUTO_ACKNOWLEDGE`: the broker marks a message as
//! consumed the moment it is delivered to the bridge, **before** the Rust
//! processor runs. If the processor fails the message is already gone from the
//! broker — it cannot be redelivered. Camel's Dead Letter Channel handles the
//! failure on the Rust side but does not put the message back in the queue.
//! True JMS redelivery would require `CLIENT_ACKNOWLEDGE` + a bidirectional
//! Ack RPC in the bridge proto — a deliberate out-of-scope decision for this
//! branch.
//!
//! ## Running
//!
//!   cargo run -p jms-container-example
//!
//! The example runs for ~30 seconds then shuts down cleanly.
//! Requires a running Docker daemon.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_container::{ContainerComponent, cleanup_tracked_containers};
use camel_component_jms::{
    BrokerConfig, BrokerType, JmsBridgePool, JmsComponent, JmsPoolConfig,
};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Pinned image — never use `:latest` in reproducible examples.
const ARTEMIS_IMAGE: &str = "apache/activemq-artemis:2.36.0-alpine";

/// Fixed host port. Change if 61616 is already in use on your machine.
const BROKER_PORT: u16 = 61616;

/// Broker credentials — must match ARTEMIS_USER / ARTEMIS_PASSWORD below.
/// ANONYMOUS_LOGIN is intentionally not set so the broker enforces auth.
const BROKER_USER: &str = "artemis";
const BROKER_PASS: &str = "artemis";

// ---------------------------------------------------------------------------
// Broker readiness probe
// ---------------------------------------------------------------------------

/// Poll TCP until the broker port accepts connections or the timeout expires.
///
/// Note: a successful TCP connect only means the port is open, not that the
/// broker is fully initialised. Artemis needs a few extra seconds after the
/// port opens to finish loading its address/queue registry — the caller sleeps
/// an additional 4 seconds after this returns.
async fn wait_for_broker(host: &str, port: u16, timeout: Duration) -> Result<(), CamelError> {
    let addr = format!("{host}:{port}");
    let start = Instant::now();
    loop {
        if std::net::TcpStream::connect(&addr).is_ok() {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(CamelError::ProcessorError(format!(
                "Broker at {addr} did not become reachable within {}s",
                timeout.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    // -----------------------------------------------------------------------
    // Step 1: Start the Artemis broker via camel-component-container
    // -----------------------------------------------------------------------
    //
    // We use a one-shot init context (timer repeatCount=1) to start the
    // container imperatively. The container is tracked globally and cleaned
    // up via `cleanup_tracked_containers()` on exit.
    //
    // Auth is configured through Artemis env-vars:
    //   ARTEMIS_USER     → broker username
    //   ARTEMIS_PASSWORD → broker password
    //
    // ANONYMOUS_LOGIN is intentionally absent — without it, Artemis rejects
    // any connection that does not supply valid credentials. The pool config
    // below passes the same credentials so the bridge authenticates correctly.

    println!("==> Starting ActiveMQ Artemis broker via camel-component-container");
    println!("    Image : {ARTEMIS_IMAGE}");
    println!("    Port  : {BROKER_PORT}");
    println!("    Auth  : {BROKER_USER} / {BROKER_PASS}  (ANONYMOUS_LOGIN not set)");

    let container_uri = format!(
        "container:run\
         ?image={ARTEMIS_IMAGE}\
         &name=artemis-jms-example\
         &ports={BROKER_PORT}:{BROKER_PORT}\
         &env=ARTEMIS_USER={BROKER_USER}\
         &env=ARTEMIS_PASSWORD={BROKER_PASS}"
    );

    let mut init_ctx = CamelContext::builder().build().await.unwrap();
    init_ctx.register_component(TimerComponent::new());
    init_ctx.register_component(ContainerComponent::new());

    let start_route = RouteBuilder::from("timer:init?repeatCount=1&delay=0")
        .route_id("broker-start")
        .to(container_uri)
        .build()?;

    init_ctx.add_route_definition(start_route).await?;
    init_ctx.start().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    init_ctx.stop().await?;

    // -----------------------------------------------------------------------
    // Step 2: Wait until the broker port is reachable
    // -----------------------------------------------------------------------

    println!("==> Waiting for broker to be ready...");
    wait_for_broker("127.0.0.1", BROKER_PORT, Duration::from_secs(30)).await?;

    // Extra wait: TCP connect succeeds before Artemis finishes loading its
    // address/queue registry. 4 seconds is enough for Artemis 2.36-alpine.
    tokio::time::sleep(Duration::from_secs(4)).await;
    println!("==> Broker ready at tcp://127.0.0.1:{BROKER_PORT}");

    // -----------------------------------------------------------------------
    // Step 3: Build the main context with JMS routes
    // -----------------------------------------------------------------------

    let broker_url = format!("tcp://127.0.0.1:{BROKER_PORT}");
    let pool_config = JmsPoolConfig {
        max_bridges: 1,
        default_broker: "default".to_string(),
        brokers: HashMap::from([(
            "default".to_string(),
            BrokerConfig {
                broker_url,
                broker_type: BrokerType::Artemis,
                username: Some(BROKER_USER.to_string()),
                password: Some(BROKER_PASS.to_string()),
            },
        )]),
        bridge_start_timeout_ms: 90_000,
        ..JmsPoolConfig::default()
    };
    let pool = Arc::new(JmsBridgePool::from_config(pool_config)?);

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(JmsComponent::with_scheme("jms", Arc::clone(&pool)));
    ctx.register_component(JmsComponent::with_scheme("activemq", Arc::clone(&pool)));
    ctx.register_component(JmsComponent::with_scheme("artemis", Arc::clone(&pool)));

    // -----------------------------------------------------------------------
    // Route 1 — Queue producer
    //
    // Fires every 2 seconds. Sets two custom JMS headers on each message:
    //   x-order-id  — monotonically increasing order identifier
    //   x-priority  — alternates between "high" and "normal"
    //
    // These headers survive the round-trip through the broker and are visible
    // in the consumer log output (showHeaders=true).
    // -----------------------------------------------------------------------

    let order_counter = Arc::new(AtomicU64::new(0));
    let order_counter_clone = Arc::clone(&order_counter);

    let queue_producer = RouteBuilder::from("timer:queue-tick?period=2000")
        .route_id("queue-producer")
        .process(move |mut exchange| {
            let c = Arc::clone(&order_counter_clone);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                let priority = if n.is_multiple_of(2) {
                    "high"
                } else {
                    "normal"
                };
                exchange.input.body = camel_api::body::Body::Text(format!(
                    r#"{{"event":"order","id":{n},"source":"rust-camel"}}"#
                ));
                exchange
                    .input
                    .set_header("x-order-id", Value::Number(serde_json::Number::from(n)));
                exchange
                    .input
                    .set_header("x-priority", Value::String(priority.to_string()));
                Ok(exchange)
            }
        })
        .to("jms:queue:orders")
        .build()?;

    // -----------------------------------------------------------------------
    // Route 2 — Competing consumer A  (happy path)
    //
    // Receives messages from the `orders` queue and logs them.
    // ActiveMQ distributes messages between consumer-a and consumer-b using
    // round-robin — each message goes to exactly one consumer.
    // -----------------------------------------------------------------------

    let consumer_a = RouteBuilder::from("jms:queue:orders")
        .route_id("consumer-a")
        .process(|exchange| async move {
            let order_id = exchange
                .input
                .header("x-order-id")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".to_string());
            let priority = exchange
                .input
                .header("x-priority")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".to_string());
            println!("[consumer-a] received order-id={order_id} priority={priority}");
            Ok(exchange)
        })
        .to("log:consumer-a?showHeaders=true&showBody=true")
        .build()?;

    // -----------------------------------------------------------------------
    // Route 3 — Competing consumer B  (always fails → Dead Letter Channel)
    //
    // Also subscribes to the same `orders` queue. Every message it receives
    // triggers an error. Camel's DLC catches the failure and forwards the
    // exchange to `log:dlc-sink` — demonstrating the Dead Letter Channel EIP
    // with JMS consumers.
    //
    // ⚠ LIMITATION: Because the bridge uses AUTO_ACKNOWLEDGE, the broker
    // considers the message delivered the moment consumer-b receives it.
    // The DLC runs on the Rust/Camel side only — the message is NOT put back
    // into the queue for redelivery. This is a known, intentional trade-off.
    // -----------------------------------------------------------------------

    let consumer_b = RouteBuilder::from("jms:queue:orders")
        .route_id("consumer-b")
        .process(|exchange| async move {
            let order_id = exchange
                .input
                .header("x-order-id")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".to_string());
            println!("[consumer-b] received order-id={order_id} — simulating processing failure");
            Err(CamelError::ProcessorError(format!(
                "consumer-b always fails (order-id={order_id})"
            )))
        })
        .error_handler(ErrorHandlerConfig::dead_letter_channel(
            "log:dlc-sink?showHeaders=true&showBody=true",
        ))
        .build()?;

    // -----------------------------------------------------------------------
    // Route 4 — Topic producer
    //
    // Broadcasts a telemetry event every 3 seconds to `jms:topic:events`.
    // Both topic consumers receive every message independently (pub/sub).
    // -----------------------------------------------------------------------

    let topic_producer = RouteBuilder::from("timer:topic-tick?period=3000")
        .route_id("topic-producer")
        .set_body(Value::String(
            r#"{"event":"telemetry","source":"rust-camel","type":"topic"}"#.to_string(),
        ))
        .to("jms:topic:events")
        .build()?;

    // -----------------------------------------------------------------------
    // Route 5 — Topic consumer 1
    // Route 6 — Topic consumer 2
    //
    // Both subscribe to the same topic. Unlike a queue (competing consumers),
    // every topic subscriber receives every published message — fan-out.
    // -----------------------------------------------------------------------

    let topic_consumer_1 = RouteBuilder::from("jms:topic:events")
        .route_id("topic-consumer-1")
        .to("log:topic-consumer-1?showHeaders=true&showBody=true")
        .build()?;

    let topic_consumer_2 = RouteBuilder::from("jms:topic:events")
        .route_id("topic-consumer-2")
        .to("log:topic-consumer-2?showHeaders=true&showBody=true")
        .build()?;

    // -----------------------------------------------------------------------
    // Register all routes and start
    // -----------------------------------------------------------------------

    ctx.add_route_definition(queue_producer).await?;
    ctx.add_route_definition(consumer_a).await?;
    ctx.add_route_definition(consumer_b).await?;
    ctx.add_route_definition(topic_producer).await?;
    ctx.add_route_definition(topic_consumer_1).await?;
    ctx.add_route_definition(topic_consumer_2).await?;

    print_banner();
    ctx.start().await?;

    // Run for 30 seconds then shut down cleanly.
    tokio::time::sleep(Duration::from_secs(30)).await;

    println!("==> Stopping routes...");
    ctx.stop().await?;

    println!("==> Cleaning up broker container...");
    cleanup_tracked_containers().await;

    println!("==> Done.");
    Ok(())
}

fn print_banner() {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║          rust-camel  —  JMS Container Example (Artemis)              ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    println!();
    println!("Routes:");
    println!("  queue-producer    timer → set custom headers → jms:queue:orders");
    println!("  consumer-a        jms:queue:orders → log (happy path)");
    println!("  consumer-b        jms:queue:orders → FAIL → Dead Letter Channel");
    println!("  topic-producer    timer → jms:topic:events");
    println!("  topic-consumer-1  jms:topic:events → log (fan-out copy 1)");
    println!("  topic-consumer-2  jms:topic:events → log (fan-out copy 2)");
    println!();
    println!("Patterns demonstrated:");
    println!("  - Pinned Docker image (apache/activemq-artemis:2.36.0-alpine)");
    println!("  - Mandatory broker auth (ARTEMIS_USER/PASSWORD, no ANONYMOUS_LOGIN)");
    println!("  - Custom JMS headers (x-order-id, x-priority)");
    println!("  - Competing consumers (consumer-a and consumer-b share orders queue)");
    println!("  - Dead Letter Channel on consumer-b failure");
    println!("  - Topic fan-out (both topic consumers receive every message)");
    println!();
    println!("Known limitation:");
    println!("  The bridge uses AUTO_ACKNOWLEDGE — DLC runs on the Rust side only.");
    println!("  Failed messages are NOT redelivered by the broker.");
    println!();
    println!("Running for 30 seconds...");
    println!();
}
