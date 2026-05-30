use std::sync::Arc;

use async_trait::async_trait;
use camel_auth::oauth2::TokenProvider;
use camel_auth::types::AuthError;
use camel_component_api::{Body, Exchange, Message};
use camel_component_keycloak::{AdminEndpointConfig, AdminOperation, KeycloakAdminProducer};
use tower::Service;
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

fn make_config(
    server_url: &str,
    operation: AdminOperation,
    realm: &str,
    user_id: Option<&str>,
) -> AdminEndpointConfig {
    AdminEndpointConfig {
        server_url: server_url.to_string(),
        target_realm: Some(realm.to_string()),
        operation,
        user_id: user_id.map(|s| s.to_string()),
        token_provider: Arc::new(MockTokenProvider),
        http: reqwest::Client::new(),
    }
}

#[tokio::test]
async fn admin_producer_create_user_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/realms/test/users"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "id": "user-123",
            "username": "testuser"
        })))
        .mount(&server)
        .await;

    let config = make_config(&server.uri(), AdminOperation::CreateUser, "test", None);
    let mut producer = KeycloakAdminProducer::new(config);

    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({
        "username": "testuser",
        "email": "test@example.com"
    }))));

    let result = producer.call(exchange).await.unwrap();
    assert!(result.output.is_some());
    let output_body = result.output.unwrap().body;
    match output_body {
        Body::Json(val) => {
            assert_eq!(val["id"], "user-123");
            assert_eq!(val["username"], "testuser");
        }
        _ => panic!("expected JSON body"),
    }
}

#[tokio::test]
async fn admin_producer_delete_user_success() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/admin/realms/test/users/user-123"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let config = make_config(
        &server.uri(),
        AdminOperation::DeleteUser,
        "test",
        Some("user-123"),
    );
    let mut producer = KeycloakAdminProducer::new(config);

    let exchange = Exchange::new(Message::new(Body::Empty));

    let result = producer.call(exchange).await.unwrap();
    assert!(result.output.is_some());
    let output_body = result.output.unwrap().body;
    assert!(matches!(output_body, Body::Empty));
}

#[tokio::test]
async fn admin_producer_get_user_success() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/admin/realms/test/users/user-456"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "user-456",
            "username": "getuser",
            "email": "get@example.com"
        })))
        .mount(&server)
        .await;

    let config = make_config(
        &server.uri(),
        AdminOperation::GetUser,
        "test",
        Some("user-456"),
    );
    let mut producer = KeycloakAdminProducer::new(config);

    let exchange = Exchange::new(Message::new(Body::Empty));

    let result = producer.call(exchange).await.unwrap();
    assert!(result.output.is_some());
    let output_body = result.output.unwrap().body;
    match output_body {
        Body::Json(val) => {
            assert_eq!(val["id"], "user-456");
            assert_eq!(val["username"], "getuser");
        }
        _ => panic!("expected JSON body"),
    }
}

#[tokio::test]
async fn admin_producer_create_role_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/realms/test/roles"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let config = make_config(&server.uri(), AdminOperation::CreateRole, "test", None);
    let mut producer = KeycloakAdminProducer::new(config);

    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({
        "name": "admin-role",
        "description": "Admin role"
    }))));

    let result = producer.call(exchange).await.unwrap();
    assert!(result.output.is_some());
}

#[tokio::test]
async fn admin_producer_assign_role_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path(
            "/admin/realms/test/users/user-789/role-mappings/realm",
        ))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let config = make_config(
        &server.uri(),
        AdminOperation::AssignRole,
        "test",
        Some("user-789"),
    );
    let mut producer = KeycloakAdminProducer::new(config);

    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!([
        { "id": "role-1", "name": "admin" }
    ]))));

    let result = producer.call(exchange).await.unwrap();
    assert!(result.output.is_some());
}

#[tokio::test]
async fn admin_producer_create_client_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/realms/test/clients"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let config = make_config(&server.uri(), AdminOperation::CreateClient, "test", None);
    let mut producer = KeycloakAdminProducer::new(config);

    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({
        "clientId": "my-client",
        "publicClient": true
    }))));

    let result = producer.call(exchange).await.unwrap();
    assert!(result.output.is_some());
}

#[tokio::test]
async fn admin_producer_create_realm_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/admin/realms"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let config = make_config(&server.uri(), AdminOperation::CreateRealm, "ignored", None);
    let mut producer = KeycloakAdminProducer::new(config);

    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({
        "realm": "new-realm",
        "enabled": true
    }))));

    let result = producer.call(exchange).await.unwrap();
    assert!(result.output.is_some());
}

#[tokio::test]
async fn admin_producer_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/admin/realms/test/users/nonexistent"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "error": "User not found"
        })))
        .mount(&server)
        .await;

    let config = make_config(
        &server.uri(),
        AdminOperation::GetUser,
        "test",
        Some("nonexistent"),
    );
    let mut producer = KeycloakAdminProducer::new(config);

    let exchange = Exchange::new(Message::new(Body::Empty));

    let result = producer.call(exchange).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("404"));
}

#[tokio::test]
async fn admin_producer_user_id_from_header() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/admin/realms/test/users/from-property"))
        .and(header("Authorization", "Bearer test-admin-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "from-property",
            "username": "propertyuser"
        })))
        .mount(&server)
        .await;

    let config = make_config(&server.uri(), AdminOperation::GetUser, "test", None);
    let mut producer = KeycloakAdminProducer::new(config);

    let mut exchange = Exchange::new(Message::new(Body::Empty));
    exchange.set_property("camel.keycloak.userId", serde_json::json!("from-property"));

    let result = producer.call(exchange).await.unwrap();
    assert!(result.output.is_some());
    let output_body = result.output.unwrap().body;
    match output_body {
        Body::Json(val) => {
            assert_eq!(val["id"], "from-property");
            assert_eq!(val["username"], "propertyuser");
        }
        _ => panic!("expected JSON body"),
    }
}
