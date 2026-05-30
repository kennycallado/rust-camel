use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_auth::oauth2::TokenProvider;
use camel_auth::types::AuthError;
use camel_component_api::{Body, Consumer, ConsumerContext, Exchange};
use camel_component_keycloak::EventsEndpointConfig;
use camel_component_keycloak::KeycloakEventConsumer;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Debug, Clone)]
struct MockTokenProvider;

#[async_trait]
impl TokenProvider for MockTokenProvider {
    async fn get_token(&self) -> Result<String, AuthError> {
        Ok("test-admin-token".to_string())
    }
}

fn make_events_config(server_url: &str, realm: &str, event_type: &str) -> EventsEndpointConfig {
    let mut params = std::collections::HashMap::new();
    params.insert("realm".to_string(), realm.to_string());
    params.insert("eventType".to_string(), event_type.to_string());
    params.insert("pollDelay".to_string(), "50".to_string());
    params.insert("lookbackWindow".to_string(), "60000".to_string());
    EventsEndpointConfig::from_params(
        &params,
        server_url,
        Arc::new(MockTokenProvider),
        reqwest::Client::new(),
    )
    .unwrap()
}

#[tokio::test]
async fn event_consumer_polls_user_events() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/admin/realms/test/events"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": "evt-1",
                "time": 1700000001000_u64,
                "type": "LOGIN",
                "realmId": "test",
                "clientId": "my-client",
                "userId": "user-1",
                "ipAddress": "10.0.0.1"
            }
        ])))
        .mount(&server)
        .await;

    let config = make_events_config(&server.uri(), "test", "events");
    let mut consumer = KeycloakEventConsumer::new(config);

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    let handle = tokio::spawn(async move {
        consumer.start(ctx).await.unwrap();
    });

    let envelope = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("should receive exchange")
        .expect("envelope should exist");

    let exchange: Exchange = envelope.exchange;
    assert!(matches!(exchange.input.body, Body::Json(_)));
    assert_eq!(
        exchange.input.header("CamelKeycloakEventId").unwrap(),
        &serde_json::json!("evt-1")
    );
    assert_eq!(
        exchange.input.header("CamelKeycloakEventType").unwrap(),
        &serde_json::json!("LOGIN")
    );
    assert_eq!(
        exchange.input.header("CamelKeycloakUserId").unwrap(),
        &serde_json::json!("user-1")
    );
    assert_eq!(
        exchange.input.header("CamelKeycloakRealmId").unwrap(),
        &serde_json::json!("test")
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
}

#[tokio::test]
async fn event_consumer_polls_admin_events() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/admin/realms/master/admin-events"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": "admin-evt-1",
                "time": 1700000002000_u64,
                "realmId": "master",
                "authDetails": {
                    "userId": "admin-user",
                    "clientId": "admin-cli"
                },
                "operationType": "CREATE",
                "resourceType": "USER",
                "resourcePath": "users/user-new"
            }
        ])))
        .mount(&server)
        .await;

    let config = make_events_config(&server.uri(), "master", "admin-events");
    let mut consumer = KeycloakEventConsumer::new(config);

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    let handle = tokio::spawn(async move {
        consumer.start(ctx).await.unwrap();
    });

    let envelope = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("should receive exchange")
        .expect("envelope should exist");

    let exchange = envelope.exchange;
    assert_eq!(
        exchange.input.header("CamelKeycloakEventType").unwrap(),
        &serde_json::json!("CREATE")
    );
    assert_eq!(
        exchange.input.header("CamelKeycloakResourceType").unwrap(),
        &serde_json::json!("USER")
    );
    assert_eq!(
        exchange.input.header("CamelKeycloakAuthUserId").unwrap(),
        &serde_json::json!("admin-user")
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
}

#[tokio::test]
async fn event_consumer_dedup() {
    let server = MockServer::start().await;

    let events = serde_json::json!([
        { "id": "dup-1", "time": 1700000001000_u64, "type": "LOGIN", "realmId": "test" }
    ]);

    Mock::given(method("GET"))
        .and(path("/admin/realms/test/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(events.clone()))
        .mount(&server)
        .await;

    let config = make_events_config(&server.uri(), "test", "events");
    let mut consumer = KeycloakEventConsumer::new(config);

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    let handle = tokio::spawn(async move {
        consumer.start(ctx).await.unwrap();
    });

    let _ = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("should receive first exchange");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut count = 0;
    while rx.try_recv().is_ok() {
        count += 1;
    }
    assert_eq!(count, 0, "duplicate event should not be delivered");

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
}

#[tokio::test]
async fn event_consumer_auth_failure_continues() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/admin/realms/test/events"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let mut params = std::collections::HashMap::new();
    params.insert("realm".to_string(), "test".to_string());
    params.insert("eventType".to_string(), "events".to_string());
    params.insert("pollDelay".to_string(), "50".to_string());
    params.insert("lookbackWindow".to_string(), "60000".to_string());
    params.insert("maxAuthErrors".to_string(), "10".to_string());
    let config = EventsEndpointConfig::from_params(
        &params,
        &server.uri(),
        Arc::new(MockTokenProvider),
        reqwest::Client::new(),
    )
    .unwrap();
    let mut consumer = KeycloakEventConsumer::new(config);

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    let handle = tokio::spawn(async move {
        let _ = consumer.start(ctx).await;
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !handle.is_finished(),
        "consumer should still be running after auth error"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
}

#[tokio::test]
async fn event_consumer_persistent_auth_failure_stops() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/admin/realms/test/events"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let mut params = std::collections::HashMap::new();
    params.insert("realm".to_string(), "test".to_string());
    params.insert("eventType".to_string(), "events".to_string());
    params.insert("pollDelay".to_string(), "50".to_string());
    params.insert("lookbackWindow".to_string(), "60000".to_string());
    params.insert("maxAuthErrors".to_string(), "2".to_string());
    let config = EventsEndpointConfig::from_params(
        &params,
        &server.uri(),
        Arc::new(MockTokenProvider),
        reqwest::Client::new(),
    )
    .unwrap();
    let mut consumer = KeycloakEventConsumer::new(config);

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    let result = consumer.start(ctx).await;
    assert!(
        result.is_err(),
        "consumer should return error after max auth failures"
    );
    assert!(result.unwrap_err().to_string().contains("auth failures"));
}

#[tokio::test]
async fn event_consumer_cancellation() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;

    let config = make_events_config(&server.uri(), "test", "events");
    let mut consumer = KeycloakEventConsumer::new(config);

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    let handle = tokio::spawn(async move {
        consumer.start(ctx).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(3), handle).await;
    assert!(result.is_ok(), "consumer should stop within timeout");
}

#[tokio::test]
async fn event_consumer_empty_results() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;

    let config = make_events_config(&server.uri(), "test", "events");
    let mut consumer = KeycloakEventConsumer::new(config);

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    let handle = tokio::spawn(async move {
        consumer.start(ctx).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !handle.is_finished(),
        "consumer should keep running on empty results"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
}

#[tokio::test]
async fn event_consumer_skips_events_without_id() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/admin/realms/test/events"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "time": 1700000001000_u64, "type": "LOGIN", "realmId": "test" },
            { "id": "evt-with-id", "time": 1700000001001_u64, "type": "LOGIN", "realmId": "test" },
            { "time": 1700000001002_u64, "type": "LOGOUT", "realmId": "test" }
        ])))
        .mount(&server)
        .await;

    let config = make_events_config(&server.uri(), "test", "events");
    let mut consumer = KeycloakEventConsumer::new(config);

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    let handle = tokio::spawn(async move {
        consumer.start(ctx).await.unwrap();
    });

    let envelope = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("should receive exchange")
        .expect("envelope should exist");

    let exchange = envelope.exchange;
    assert_eq!(
        exchange.input.header("CamelKeycloakEventId").unwrap(),
        &serde_json::json!("evt-with-id"),
        "only the event with an id should be delivered"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut extra = 0;
    while rx.try_recv().is_ok() {
        extra += 1;
    }
    assert_eq!(
        extra, 0,
        "events without id should be skipped, not delivered"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
}
