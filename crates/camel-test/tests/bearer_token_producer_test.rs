use std::sync::Arc;

use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, Message};
use camel_auth::bearer_token_layer::BearerTokenLayer;
use camel_auth::oauth2::ClientCredentialsProvider;
use tower::{Layer, Service, ServiceExt};
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_exchange() -> Exchange {
    Exchange::new(Message::new("hello"))
}

#[tokio::test]
async fn test_e2e_client_credentials_injects_bearer() {
    let token_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "e2e-access-token",
            "token_type": "Bearer",
            "expires_in": 300,
        })))
        .mount(&token_server)
        .await;

    let api_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("Authorization", "Bearer e2e-access-token"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&api_server)
        .await;

    let provider = Arc::new(ClientCredentialsProvider::new_unchecked_for_test(
        format!("{}/protocol/openid-connect/token", token_server.uri()),
        "test-client".into(),
        "test-secret".into(),
        None,
        None,
        reqwest::Client::new(),
    ));

    let target_url = api_server.uri();
    let processor = BoxProcessor::from_fn(move |ex| {
        let url = target_url.clone();
        Box::pin(async move {
            let auth = ex
                .input
                .header("Authorization")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let resp = reqwest::Client::new()
                .get(&url)
                .header("Authorization", auth)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            assert_eq!(status, 200);
            Ok(ex)
        })
    });

    let layer = BearerTokenLayer::new(provider);
    let mut svc = layer.layer(processor);
    let result = svc.ready().await.unwrap().call(make_exchange()).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_e2e_cached_token_reused() {
    let token_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "cached-token",
            "token_type": "Bearer",
            "expires_in": 300,
        })))
        .expect(1)
        .mount(&token_server)
        .await;

    let api_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("Authorization", "Bearer cached-token"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&api_server)
        .await;

    let provider = Arc::new(ClientCredentialsProvider::new_unchecked_for_test(
        format!("{}/protocol/openid-connect/token", token_server.uri()),
        "c".into(),
        "s".into(),
        None,
        None,
        reqwest::Client::new(),
    ));

    let shared_provider = Arc::clone(&provider);

    let layer = BearerTokenLayer::new(provider);
    let url1 = api_server.uri();
    let p1 = BoxProcessor::from_fn(move |ex| {
        let url = url1.clone();
        Box::pin(async move {
            let auth = ex
                .input
                .header("Authorization")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            reqwest::Client::new()
                .get(&url)
                .header("Authorization", auth)
                .send()
                .await
                .unwrap();
            Ok(ex)
        })
    });
    let mut svc = layer.layer(p1);
    let r1 = svc.ready().await.unwrap().call(make_exchange()).await;
    assert!(r1.is_ok());

    let layer2 = BearerTokenLayer::new(shared_provider);
    let url2 = api_server.uri();
    let p2 = BoxProcessor::from_fn(move |ex| {
        let url = url2.clone();
        Box::pin(async move {
            let auth = ex
                .input
                .header("Authorization")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            reqwest::Client::new()
                .get(&url)
                .header("Authorization", auth)
                .send()
                .await
                .unwrap();
            Ok(ex)
        })
    });
    let mut svc2 = layer2.layer(p2);
    let r2 = svc2.ready().await.unwrap().call(make_exchange()).await;
    assert!(r2.is_ok());
}
