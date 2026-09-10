use super::*;
use crate::topology::ServerKind;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn test_rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(camel_component_api::test_support::PanicRuntimeObservability)
}

fn create_test_config(command: RedisCommand) -> RedisEndpointConfig {
    RedisEndpointConfig {
        host: Some("localhost".to_string()),
        port: Some(6379),
        command,
        channels: vec!["test".to_string()],
        key: Some("test-queue".to_string()),
        timeout: 1,
        username: None,
        password: None,
        db: 0,
        ssl: Some(false),
        tls_ca_cert: None,
        reconnect: camel_component_api::NetworkRetryPolicy::default(),
        // Short so lifecycle tests that spawn a real queue/pubsub consumer
        // (which now retries on connect failure) terminate quickly.
        connection_timeout_secs: 1,
        topology_kind: crate::sentinel_config::TopologyKind::Standalone,
    }
}

// Task 2.3: a `redis://` endpoint must construct a StandaloneTopology that
// resolves the fixed, structurally built connection. `RedisTopology` is a
// trait, so assert indirectly: resolve against the consumer's topology
// and check the client address.
// No broker needed — `Client::open` only parses the connection info.
#[tokio::test]
async fn standalone_consumer_uses_standalone_topology() {
    let config = RedisEndpointConfig::from_uri("redis://127.0.0.1:6379?command=BLPOP&key=demo")
        .expect("valid standalone uri");
    let consumer = RedisConsumer::new(config, test_rt()).expect("BLPOP should be valid");

    let client = consumer
        .topology()
        .resolve(ServerKind::Master)
        .await
        .expect("standalone topology should resolve the fixed, structurally built connection");
    assert_eq!(
        client.get_connection_info().addr().to_string(),
        "127.0.0.1:6379"
    );
}

#[test]
fn test_consumer_new_subscribe() {
    let config = create_test_config(RedisCommand::Subscribe);
    let consumer = RedisConsumer::new(config, test_rt()).expect("Subscribe should be valid");

    match consumer.mode {
        RedisConsumerMode::PubSub { channels, patterns } => {
            assert_eq!(channels, vec!["test".to_string()]);
            assert!(patterns.is_empty());
        }
        _ => panic!("Expected PubSub mode"),
    }
}

#[test]
fn test_consumer_new_psubscribe() {
    let config = create_test_config(RedisCommand::Psubscribe);
    let consumer = RedisConsumer::new(config, test_rt()).expect("Psubscribe should be valid");

    match consumer.mode {
        RedisConsumerMode::PubSub { channels, patterns } => {
            assert!(channels.is_empty());
            assert_eq!(patterns, vec!["test".to_string()]);
        }
        _ => panic!("Expected PubSub mode"),
    }
}

#[test]
fn test_consumer_new_blpop() {
    let config = create_test_config(RedisCommand::Blpop);
    let consumer = RedisConsumer::new(config, test_rt()).expect("Blpop should be valid");

    match consumer.mode {
        RedisConsumerMode::Queue {
            key,
            timeout,
            pop_command,
        } => {
            assert_eq!(key, "test-queue");
            assert_eq!(timeout, 1);
            assert_eq!(pop_command, QueuePopCommand::Blpop);
        }
        _ => panic!("Expected Queue mode"),
    }
}

#[test]
fn test_consumer_new_brpop_uses_right_pop_command() {
    let config = create_test_config(RedisCommand::Brpop);
    let consumer = RedisConsumer::new(config, test_rt()).expect("Brpop should be valid");

    match consumer.mode {
        RedisConsumerMode::Queue { pop_command, .. } => {
            assert_eq!(pop_command, QueuePopCommand::Brpop);
        }
        _ => panic!("Expected Queue mode"),
    }
}

#[test]
fn test_consumer_new_blpop_default_key() {
    let mut config = create_test_config(RedisCommand::Blpop);
    config.key = None;
    let consumer = RedisConsumer::new(config, test_rt()).expect("Blpop should be valid");

    match consumer.mode {
        RedisConsumerMode::Queue {
            key, pop_command, ..
        } => {
            assert_eq!(key, "queue");
            assert_eq!(pop_command, QueuePopCommand::Blpop);
        }
        _ => panic!("Expected Queue mode"),
    }
}

// REDIS-003: Invalid consumer command now returns error instead of silent fallback
#[test]
fn test_consumer_new_invalid_command_returns_error() {
    let config = create_test_config(RedisCommand::Set);
    let result = RedisConsumer::new(config, test_rt());
    assert!(
        result.is_err(),
        "SET should not be a valid consumer command"
    );
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("expected error for invalid consumer command"),
    };
    assert!(err.to_string().contains("Invalid consumer command"));
}

#[test]
fn test_consumer_new_get_command_returns_error() {
    let config = create_test_config(RedisCommand::Get);
    let result = RedisConsumer::new(config, test_rt());
    assert!(
        result.is_err(),
        "GET should not be a valid consumer command"
    );
}

#[test]
fn test_queue_command_name_matches_pop_side() {
    assert_eq!(queue_command_name(QueuePopCommand::Blpop), "BLPOP");
    assert_eq!(queue_command_name(QueuePopCommand::Brpop), "BRPOP");
}

#[test]
fn test_consumer_concurrency_model_is_sequential() {
    let config = create_test_config(RedisCommand::Subscribe);
    let consumer = RedisConsumer::new(config, test_rt()).expect("Subscribe should be valid");
    assert_eq!(consumer.concurrency_model(), ConcurrencyModel::Sequential);
}

#[test]
fn test_build_exchange_from_blpop() {
    let exchange = build_exchange_from_blpop("mykey".to_string(), "myvalue".to_string());

    assert_eq!(exchange.input.body.as_text(), Some("myvalue"));

    let header = exchange.input.header("CamelRedis.Key");
    assert_eq!(
        header,
        Some(&serde_json::Value::String("mykey".to_string()))
    );
}

#[test]
fn test_build_pubsub_exchange_without_pattern() {
    let exchange = build_pubsub_exchange("hello".to_string(), "news".to_string(), None);

    assert_eq!(exchange.input.body.as_text(), Some("hello"));
    assert_eq!(
        exchange.input.header("CamelRedis.Channel"),
        Some(&serde_json::json!("news"))
    );
    assert!(exchange.input.header("CamelRedis.Pattern").is_none());
}

#[test]
fn test_build_pubsub_exchange_with_pattern() {
    let exchange = build_pubsub_exchange(
        "hello".to_string(),
        "news.eu".to_string(),
        Some("news.*".to_string()),
    );

    assert_eq!(
        exchange.input.header("CamelRedis.Pattern"),
        Some(&serde_json::json!("news.*"))
    );
}

#[test]
fn test_queue_pop_command_derives() {
    let cmd = QueuePopCommand::Blpop;
    let _cmd2 = cmd; // Copy
    #[allow(clippy::clone_on_copy)]
    let _cmd3 = cmd.clone(); // Clone
    assert_eq!(format!("{:?}", cmd), "Blpop"); // Debug
    assert_eq!(QueuePopCommand::Blpop, QueuePopCommand::Blpop); // PartialEq
    assert_ne!(QueuePopCommand::Blpop, QueuePopCommand::Brpop);
}

#[test]
fn test_build_pubsub_exchange_with_empty_payload() {
    let exchange = build_pubsub_exchange("".to_string(), "ch".to_string(), None);
    assert_eq!(exchange.input.body.as_text(), Some(""));
    assert_eq!(
        exchange.input.header("CamelRedis.Channel"),
        Some(&serde_json::json!("ch"))
    );
}

#[test]
fn test_build_exchange_from_blpop_with_empty_values() {
    let exchange = build_exchange_from_blpop("".to_string(), "".to_string());
    assert_eq!(exchange.input.body.as_text(), Some(""));
    assert_eq!(
        exchange.input.header("CamelRedis.Key"),
        Some(&serde_json::Value::String("".to_string()))
    );
}

#[tokio::test]
async fn test_consumer_stop_without_start() {
    let config = create_test_config(RedisCommand::Subscribe);
    let mut consumer = RedisConsumer::new(config, test_rt()).expect("Subscribe should be valid");

    // Stop without start should succeed gracefully
    let result = consumer.stop().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_consumer_start_sets_task_handle() {
    let config = create_test_config(RedisCommand::Blpop);
    let mut consumer = RedisConsumer::new(config, test_rt()).expect("Blpop should be valid");

    let (tx, _rx) = mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone(), "redis-test-route".to_string());

    assert!(consumer.task_handle.is_none());
    let result = consumer.start(ctx).await;
    assert!(result.is_ok());
    assert!(consumer.task_handle.is_some());

    // Clean up
    consumer.stop().await.ok();
}

#[tokio::test]
async fn consumer_task_exits_on_context_token_cancel() {
    let config = create_test_config(RedisCommand::Blpop);
    let mut consumer = RedisConsumer::new(config, test_rt()).expect("Blpop should be valid");

    let (tx, _rx) = mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone(), "redis-test-route".to_string());

    assert!(consumer.start(ctx).await.is_ok());
    cancel_token.cancel();
    let handle = consumer.background_task_handle().expect("handle");
    let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        joined.is_ok(),
        "consumer task did not exit after context token cancel"
    );
    assert!(matches!(joined.unwrap(), Ok(Ok(()))));
}

#[tokio::test]
async fn local_stop_does_not_cancel_runtime_token() {
    let config = create_test_config(RedisCommand::Blpop);
    let mut consumer = RedisConsumer::new(config, test_rt()).expect("Blpop should be valid");

    let (tx, _rx) = mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone(), "redis-test-route".to_string());

    assert!(consumer.start(ctx).await.is_ok());
    let stopped = tokio::time::timeout(Duration::from_secs(2), consumer.stop()).await;
    assert!(stopped.is_ok(), "stop() did not return in time");
    assert!(stopped.unwrap().is_ok());
    assert!(
        !cancel_token.is_cancelled(),
        "local stop must not cancel the runtime context token"
    );
}

#[tokio::test]
async fn test_consumer_start_pubsub_mode() {
    let config = create_test_config(RedisCommand::Subscribe);
    let mut consumer = RedisConsumer::new(config, test_rt()).expect("Subscribe should be valid");

    let (tx, _rx) = mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone(), "redis-test-route".to_string());

    let result = consumer.start(ctx).await;
    assert!(result.is_ok());
    assert!(consumer.cancel_token.is_some());
    assert!(consumer.task_handle.is_some());

    consumer.stop().await.ok();
}

#[test]
fn test_consumer_new_blpop_with_default_key_when_none() {
    let mut config = create_test_config(RedisCommand::Blpop);
    config.key = None;
    let consumer = RedisConsumer::new(config, test_rt()).expect("Blpop should be valid");

    match &consumer.mode {
        RedisConsumerMode::Queue { key, .. } => {
            assert_eq!(key, "queue");
        }
        _ => panic!("Expected Queue mode"),
    }
}

#[test]
fn test_consumer_new_brpop_with_default_key_when_none() {
    let mut config = create_test_config(RedisCommand::Brpop);
    config.key = None;
    let consumer = RedisConsumer::new(config, test_rt()).expect("Brpop should be valid");

    match &consumer.mode {
        RedisConsumerMode::Queue {
            key, pop_command, ..
        } => {
            assert_eq!(key, "queue");
            assert_eq!(*pop_command, QueuePopCommand::Brpop);
        }
        _ => panic!("Expected Queue mode"),
    }
}

#[test]
fn test_consumer_mode_debug() {
    let pubsub_mode = RedisConsumerMode::PubSub {
        channels: vec!["test".to_string()],
        patterns: vec!["pattern:*".to_string()],
    };
    let debug_str = format!("{:?}", pubsub_mode);
    assert!(debug_str.contains("PubSub"));

    let queue_mode = RedisConsumerMode::Queue {
        key: "mykey".to_string(),
        timeout: 5,
        pop_command: QueuePopCommand::Brpop,
    };
    let debug_str = format!("{:?}", queue_mode);
    assert!(debug_str.contains("Queue"));
}

#[tokio::test]
async fn test_consumer_stops_gracefully() {
    let config = create_test_config(RedisCommand::Blpop);
    let mut consumer = RedisConsumer::new(config, test_rt()).expect("Blpop should be valid");

    // Create a mock context (won't actually be used in this test)
    let (tx, _rx) = mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone(), "redis-test-route".to_string());

    // Start should succeed
    let start_result = consumer.start(ctx).await;
    assert!(start_result.is_ok());

    // Give task a moment to start
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Stop should succeed
    let stop_result = consumer.stop().await;
    assert!(stop_result.is_ok());
}

// REDIS-016: start/stop/start lifecycle test
#[tokio::test]
async fn test_consumer_start_stop_start_lifecycle() {
    let config = create_test_config(RedisCommand::Blpop);
    let mut consumer = RedisConsumer::new(config, test_rt()).expect("Blpop should be valid");

    let (tx, _rx) = mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone(), "redis-test-route".to_string());

    // First start
    assert!(consumer.start(ctx).await.is_ok());
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Stop
    assert!(consumer.stop().await.is_ok());

    // Start again after stop — should succeed (clean restart)
    let (tx2, _rx2) = mpsc::channel(16);
    let cancel_token2 = CancellationToken::new();
    let ctx2 = ConsumerContext::new(tx2, cancel_token2.clone(), "redis-test-route-2".to_string());
    assert!(consumer.start(ctx2).await.is_ok());
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Final cleanup
    assert!(consumer.stop().await.is_ok());
}

// REDIS-016: stop() must fully reset internal state so start() creates fresh handles
#[tokio::test]
async fn test_redis_restart_after_stop() {
    let config = create_test_config(RedisCommand::Blpop);
    let mut consumer = RedisConsumer::new(config, test_rt()).expect("Blpop should be valid");

    let (tx, _rx) = mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone(), "redis-test-route".to_string());

    // Start the consumer
    assert!(consumer.start(ctx).await.is_ok());
    assert!(
        consumer.task_handle.is_some(),
        "task_handle should be Some after start"
    );
    assert!(
        consumer.cancel_token.is_some(),
        "cancel_token should be Some after start"
    );

    tokio::time::sleep(Duration::from_millis(10)).await;

    // Stop the consumer
    assert!(consumer.stop().await.is_ok());

    // After stop, ALL internal state must be cleared
    assert!(
        consumer.task_handle.is_none(),
        "task_handle must be None after stop — stale JoinHandle would leak"
    );
    assert!(
        consumer.cancel_token.is_none(),
        "cancel_token must be None after stop — stale token would cause issues on restart"
    );

    // Start again — must create fresh state without panic or error
    let (tx2, _rx2) = mpsc::channel(16);
    let cancel_token2 = CancellationToken::new();
    let ctx2 = ConsumerContext::new(tx2, cancel_token2.clone(), "redis-test-route-2".to_string());
    assert!(consumer.start(ctx2).await.is_ok());
    assert!(
        consumer.task_handle.is_some(),
        "task_handle should be Some after restart"
    );
    assert!(
        consumer.cancel_token.is_some(),
        "cancel_token should be Some after restart"
    );

    // Final cleanup
    assert!(consumer.stop().await.is_ok());
}

// REDIS-016: double-stop is safe
#[tokio::test]
async fn test_consumer_double_stop_is_safe() {
    let config = create_test_config(RedisCommand::Blpop);
    let mut consumer = RedisConsumer::new(config, test_rt()).expect("Blpop should be valid");

    let (tx, _rx) = mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone(), "redis-test-route".to_string());

    assert!(consumer.start(ctx).await.is_ok());
    tokio::time::sleep(Duration::from_millis(10)).await;

    // First stop
    assert!(consumer.stop().await.is_ok());
    // Second stop — should be safe (no panic, no error)
    assert!(consumer.stop().await.is_ok());
}

// rc-kxtkq: a pubsub session Err BEFORE readiness (retry budget exhausted,
// unreachable broker) must surface the real Redis cause through the startup
// handshake (ctx.mark_failed), not a dropped-startup-signal panic in the
// harness. Error surfacing only — ADR-0007 supervision semantics unchanged.
#[tokio::test]
async fn pubsub_pre_ready_session_err_marks_startup_failed() {
    use camel_component_api::StartupSignal;

    // Unreachable broker (port 1: connection refused immediately) and a
    // 1-attempt reconnect budget so the session returns Err pre-ready fast.
    let mut config = RedisEndpointConfig::from_uri("redis://127.0.0.1:1?command=SUBSCRIBE")
        .expect("valid standalone uri");
    config.reconnect = camel_component_api::NetworkRetryPolicy {
        max_attempts: 1,
        initial_delay: Duration::from_millis(1),
        ..camel_component_api::NetworkRetryPolicy::default()
    };

    let (tx, _rx) = mpsc::channel(1);
    let (startup, startup_rx) = StartupSignal::pair();
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel_token.clone(), "redis-pre-ready-err".to_string())
        .with_startup(startup);

    let topology = Arc::new(crate::topology::StandaloneTopology::new(&config));
    let consumer_task = super::run_pubsub_consumer(
        config,
        vec!["never-ready".to_string()],
        vec![],
        ctx,
        cancel_token,
        // The Err arm records an increment_errors metric before returning;
        // Noop tolerates it (Panic would abort before mark_failed lands).
        Arc::new(camel_component_api::test_support::NoopRuntimeObservability),
        topology,
    );

    let (session_result, startup_result) = tokio::join!(
        consumer_task,
        tokio::time::timeout(Duration::from_secs(5), startup_rx.await_ready()),
    );

    // The session itself still fails (supervision unchanged, ADR-0007).
    assert!(session_result.is_err(), "session must return Err");

    // The startup handshake must resolve Failed carrying the Redis cause —
    // before the fix it stayed Pending and start() panicked on the dropped
    // startup signal instead of surfacing the error.
    let startup_err = startup_result
        .expect("startup handshake resolves within 5s")
        .expect_err("startup must resolve Failed, not Ready");
    let msg = startup_err.to_string();
    assert!(
        msg.contains("refused") || msg.contains("connection"),
        "startup failure must carry the Redis cause, got: {msg}"
    );
}
