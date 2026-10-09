use super::*;
use crate::config::{RabbitBrokerConfig, RabbitEndpointConfig, rabbitmq_reconnect_default};
use crate::connection::ConnectFn;
use crate::connection::docker_fixture;
use camel_component_api::test_support::{PanicRuntimeObservability, RecordingRuntimeObservability};
use lapin::ExchangeKind;
use lapin::options::{BasicGetOptions, ExchangeDeclareOptions, QueueBindOptions};
use lapin::types::FieldTable;

fn pending_manager() -> Arc<RabbitConnectionManager> {
    let connect_fn: ConnectFn = Arc::new(|_url: &str| {
        Box::pin(async { std::future::pending::<Result<lapin::Connection, lapin::Error>>().await })
    });
    Arc::new(RabbitConnectionManager::new(
        "amqp://localhost:5672",
        rabbitmq_reconnect_default(),
        connect_fn,
    ))
}

fn recorded_rt() -> Arc<RecordingRuntimeObservability> {
    RecordingRuntimeObservability::new(true)
}

#[test]
fn build_properties_persistent_default_and_override() {
    let empty = camel_api::Headers::new();
    assert_eq!(
        *build_properties(true, None, &empty).delivery_mode(),
        Some(2),
        "persistent default must set delivery mode 2"
    );
    assert_eq!(
        *build_properties(false, None, &empty).delivery_mode(),
        Some(1),
        "persistent=false must set delivery mode 1"
    );

    let mut headers = camel_api::Headers::new();
    headers.insert("contentType".to_string(), serde_json::json!("text/plain"));

    // URI option wins over the header.
    let from_uri = build_properties(true, Some("application/json"), &headers);
    assert_eq!(
        from_uri.content_type().as_ref().map(|ct| ct.as_str()),
        Some("application/json"),
        "URI contentType must win over the header"
    );

    // Header fallback when the URI option is absent.
    let from_header = build_properties(true, None, &headers);
    assert_eq!(
        from_header.content_type().as_ref().map(|ct| ct.as_str()),
        Some("text/plain"),
        "header contentType is the fallback"
    );
}

#[test]
fn build_properties_maps_free_form_headers() {
    let mut headers = camel_api::Headers::new();
    headers.insert("x-custom".to_string(), serde_json::json!("abc"));
    headers.insert("contentType".to_string(), serde_json::json!("text/plain"));
    headers.insert("messageId".to_string(), serde_json::json!("m-1"));

    let props = build_properties(true, None, &headers);
    let table = props
        .headers()
        .as_ref()
        .expect("build_properties must always attach a header table");

    match table.inner().get("x-custom") {
        Some(lapin::types::AMQPValue::LongString(value)) => {
            assert_eq!(value.as_bytes(), b"abc", "x-custom must be a LongString");
        }
        other => panic!("expected LongString for x-custom, got {other:?}"),
    }

    assert!(
        !table.contains_key("contentType"),
        "reserved contentType must not land in the free-form FieldTable"
    );
    assert!(
        !table.contains_key("messageId"),
        "reserved messageId must not land in the free-form FieldTable"
    );
}

#[test]
fn producer_uses_config_target() {
    let config = RabbitEndpointConfig::from_uri("rabbitmq:ex?queue=q&routingKey=rk")
        .expect("URI must parse");
    let rt: Arc<dyn RuntimeObservability> = Arc::new(PanicRuntimeObservability);
    let producer = RabbitProducer::new(config.clone(), pending_manager(), rt);

    assert_eq!(
        producer.target(),
        config.target(),
        "producer target must re-assert the parsed config target"
    );
    assert_eq!(
        producer.target(),
        ("ex".to_string(), "rk".to_string()),
        "explicit routingKey must win over queue"
    );
}

#[tokio::test(start_paused = true)]
async fn publish_disconnected_fails_bounded() {
    let config =
        RabbitEndpointConfig::from_uri("rabbitmq:default?queue=orders").expect("URI parses");
    let rt: Arc<dyn RuntimeObservability> = RecordingRuntimeObservability::new(true);
    let mut producer = RabbitProducer::new(config, pending_manager(), rt);

    let start = tokio::time::Instant::now();
    let result = Service::call(&mut producer, Exchange::default()).await;

    assert!(
        result.is_err(),
        "a publish while disconnected must fail, not hang"
    );
    let bound = crate::connection::PUBLISH_DISCONNECTED_BOUND + Duration::from_millis(500);
    assert!(
        start.elapsed() <= bound,
        "disconnected publish must fail within {bound:?}, took {:?}",
        start.elapsed()
    );
}

#[test]
fn confirm_error_names_target() {
    let error: CamelError = RabbitError::ConfirmNacked {
        exchange: "orders.exchange".to_string(),
        routing_key: "rk-42".to_string(),
    }
    .into();
    let message = error.to_string();
    assert!(
        message.contains("orders.exchange"),
        "the confirm error must name the exchange, got: {message}"
    );
    assert!(
        message.contains("rk-42"),
        "the confirm error must name the routing key, got: {message}"
    );
}

#[test]
fn publish_outcome_mapping() {
    let rt = recorded_rt();

    record_publish(&*rt, PublishOutcome::Success);
    record_publish(&*rt, PublishOutcome::Failure);

    assert_eq!(
        rt.ops(),
        vec![
            (
                "rabbitmq".to_string(),
                "publish".to_string(),
                "success".to_string()
            ),
            (
                "rabbitmq".to_string(),
                "publish".to_string(),
                "failure".to_string()
            ),
        ],
        "publish outcomes must map to success/failure component ops"
    );
    assert_eq!(
        rt.errors(),
        vec![("rabbitmq".to_string(), "e:rabbitmq:publish".to_string())],
        "the failure outcome must also forward to the error family"
    );
}

/// Review regression (r_gpt): lapin pops a mandatory `basic.return` FIFO
/// once per confirmation with no per-publish correlation, so two mandatory
/// publishes sharing one cached confirm channel can mispair a return. This
/// proves the fix NON-vacuously: each mandatory call must own a DISTINCT
/// dedicated channel on the shared connection. Two leases are opened at
/// once, then a routable publish (channel A) and an unroutable publish
/// (channel B) are confirmed through the production `map_confirmation`.
/// Because the channels are distinct, even a broker-grouped ack cannot move
/// a return across them: A maps to `Ok`, B to `Err(unroutable)`, and only
/// A's payload is routed.
///
/// Docker-gated: without `RABBITMQ_ITEST=1` the fixture prints its skip
/// notice and this returns early — the lease invariant is never silently
/// asserted without a broker.
#[tokio::test]
async fn concurrent_mandatory_returns_are_isolated() {
    let Some(fx) = docker_fixture::require_fixture() else {
        return;
    };
    let suffix = format!("{}-{}", std::process::id(), docker_fixture::nanos());
    let exchange = format!("mand-iso-ex-{suffix}");
    let queue = format!("mand-iso-q-{suffix}");
    let key_a = "rk-routable";
    let key_b = "rk-unroutable";

    // Raw fixture connection: declare the exchange (no bindings) and the
    // queue, binding ONLY key A so A routes and B is returned.
    let (_raw_conn, raw_channel) = fx.declare_queue(&queue).await;
    raw_channel
        .exchange_declare(
            ShortString::from(exchange.as_str()),
            ExchangeKind::Direct,
            ExchangeDeclareOptions {
                durable: false,
                auto_delete: true,
                ..ExchangeDeclareOptions::default()
            },
            FieldTable::default(),
        )
        .await
        .expect("fixture exchange declare");
    raw_channel
        .queue_bind(
            ShortString::from(queue.as_str()),
            ShortString::from(exchange.as_str()),
            ShortString::from(key_a),
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await
        .expect("fixture queue bind for the routable key");

    let broker = RabbitBrokerConfig {
        url: fx.amqp_url.clone(),
        username: None,
        password: None,
        vhost: None,
    };
    let manager = Arc::new(RabbitConnectionManager::from_broker_config(
        &broker,
        rabbitmq_reconnect_default(),
    ));
    let cache = ChannelCache::new(None);

    // Capture the shared connection identity, then open BOTH mandatory
    // leases without releasing the first.
    let (conn_before, gen_before) = manager
        .connection_within(PUBLISH_DISCONNECTED_BOUND)
        .await
        .expect("initial connect to the fixture must succeed");
    let lease_a = acquire_channel(true, &manager, &cache)
        .await
        .expect("first mandatory lease");
    let lease_b = acquire_channel(true, &manager, &cache)
        .await
        .expect("second mandatory lease");
    let (conn_after, gen_after) = manager
        .connection_within(PUBLISH_DISCONNECTED_BOUND)
        .await
        .expect("the shared connection must remain live");

    assert_ne!(
        lease_a.channel.id(),
        lease_b.channel.id(),
        "each mandatory call must own a DISTINCT channel (pre-fix both share one cached id)"
    );
    assert!(
        lease_a.dedicated && lease_b.dedicated,
        "mandatory leases must be dedicated channels"
    );
    assert_eq!(
        lease_a.generation, gen_before,
        "lease A must ride the shared connection generation"
    );
    assert_eq!(
        lease_b.generation, gen_before,
        "lease B must ride the shared connection generation"
    );
    assert!(
        Arc::ptr_eq(&conn_before, &conn_after),
        "acquiring mandatory leases must not reset the shared connection"
    );
    assert_eq!(
        gen_before, gen_after,
        "acquiring mandatory leases must not trigger a reconnect"
    );

    // Publish A (routable) on lease A and B (unroutable) on lease B, then
    // confirm both through the production mapper.
    let props = BasicProperties::default();
    let confirm_a = lease_a
        .channel
        .basic_publish(
            ShortString::from(exchange.as_str()),
            ShortString::from(key_a),
            BasicPublishOptions {
                mandatory: true,
                ..BasicPublishOptions::default()
            },
            b"A-payload",
            props.clone(),
        )
        .await
        .expect("A basic_publish must be accepted");
    let confirm_b = lease_b
        .channel
        .basic_publish(
            ShortString::from(exchange.as_str()),
            ShortString::from(key_b),
            BasicPublishOptions {
                mandatory: true,
                ..BasicPublishOptions::default()
            },
            b"B-payload",
            props,
        )
        .await
        .expect("B basic_publish must be accepted");

    let bound = Duration::from_secs(5);
    let conf_a = tokio::time::timeout(bound, confirm_a)
        .await
        .expect("A's confirm must arrive within the bound")
        .expect("A's confirm must not surface a channel error");
    let conf_b = tokio::time::timeout(bound, confirm_b)
        .await
        .expect("B's confirm must arrive within the bound")
        .expect("B's confirm must not surface a channel error");

    let target_a = (exchange.clone(), key_a.to_string());
    let target_b = (exchange.clone(), key_b.to_string());
    assert!(
        map_confirmation(&target_a, conf_a).is_ok(),
        "the routable publish on channel A must map to Ok"
    );
    let error_b = map_confirmation(&target_b, conf_b)
        .expect_err("the unroutable publish on channel B must map to Err");
    assert!(
        error_b.to_string().contains("unroutable"),
        "B's error must name the broker return, got: {error_b}"
    );

    // Broker evidence: only A was routed; B (unroutable) never landed.
    let got = raw_channel
        .basic_get(
            ShortString::from(queue.as_str()),
            BasicGetOptions { no_ack: true },
        )
        .await
        .expect("basic_get must not error")
        .expect("A's payload must be routed to the bound queue");
    assert_eq!(got.data, b"A-payload", "A's payload must round-trip");
    assert!(
        raw_channel
            .basic_get(
                ShortString::from(queue.as_str()),
                BasicGetOptions { no_ack: true },
            )
            .await
            .expect("second basic_get must not error")
            .is_none(),
        "only A may be routed; B must not land"
    );
}
