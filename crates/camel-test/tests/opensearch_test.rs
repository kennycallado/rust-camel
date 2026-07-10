//! Integration tests for OpenSearch component.
//!
//! Uses testcontainers to spin up OpenSearch instances for testing.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;

use std::time::Duration;

use camel_api::Value;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_opensearch::OpenSearchComponent;
use camel_test::CamelTestContext;
use support::wait::wait_until;
use testcontainers::ContainerAsync;
use testcontainers::core::{ContainerPort, Image, WaitFor};
use testcontainers::runners::AsyncRunner;

const OPENSEARCH_HTTP_PORT: u16 = 9200;

#[derive(Debug, Clone)]
struct OpenSearchImage;

impl Image for OpenSearchImage {
    fn name(&self) -> &str {
        "opensearchproject/opensearch"
    }

    fn tag(&self) -> &str {
        "2.18.0"
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<
        Item = (
            impl Into<std::borrow::Cow<'_, str>>,
            impl Into<std::borrow::Cow<'_, str>>,
        ),
    > {
        vec![
            ("discovery.type".to_string(), "single-node".to_string()),
            (
                "OPENSEARCH_JAVA_OPTS".to_string(),
                "-Xms512m -Xmx512m".to_string(),
            ),
            ("DISABLE_SECURITY_PLUGIN".to_string(), "true".to_string()),
        ]
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(OPENSEARCH_HTTP_PORT)]
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::Nothing]
    }
}

async fn setup_opensearch_container() -> ContainerAsync<OpenSearchImage> {
    OpenSearchImage.start().await.unwrap()
}

async fn get_opensearch_url(container: &ContainerAsync<OpenSearchImage>) -> String {
    let port = container
        .get_host_port_ipv4(OPENSEARCH_HTTP_PORT)
        .await
        .unwrap();
    format!("127.0.0.1:{}", port)
}

async fn wait_for_opensearch(container: &ContainerAsync<OpenSearchImage>) {
    let port = container
        .get_host_port_ipv4(OPENSEARCH_HTTP_PORT)
        .await
        .unwrap();
    let client = reqwest::Client::new();
    wait_until(
        "OpenSearch ready",
        Duration::from_secs(60),
        Duration::from_millis(500),
        || {
            let client = client.clone();
            let url = format!("http://127.0.0.1:{}/", port);
            async move {
                let resp = client.get(&url).send().await;
                match resp {
                    Ok(r) if r.status().is_success() => Ok(true),
                    Ok(r) => Err(format!("non-200 status: {}", r.status())),
                    Err(e) => Err(format!("connection failed: {}", e)),
                }
            }
        },
    )
    .await
    .unwrap();
}

/// Index a document directly via HTTP and wait for it to be searchable.
async fn seed_document(url: &str, index: &str, id: &str, body: serde_json::Value) {
    let client = reqwest::Client::new();
    let put_url = format!("http://{}/{}/_doc/{}?refresh=true", url, index, id);
    let resp = client.put(&put_url).json(&body).send().await.unwrap();
    assert!(resp.status().is_success(), "seed failed: {}", resp.status());
}

async fn get_document_status(url: &str, index: &str, id: &str) -> reqwest::StatusCode {
    let client = reqwest::Client::new();
    let get_url = format!("http://{}/{}/_doc/{}", url, index, id);
    client.get(get_url).send().await.unwrap().status()
}

async fn count_documents(url: &str, index: &str) -> u64 {
    let client = reqwest::Client::new();
    let search_url = format!("http://{}/{}/_search", url, index);
    let resp = client
        .post(search_url)
        .json(&serde_json::json!({"query": {"match_all": {}}}))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "count search failed: {}",
        resp.status()
    );
    let body = resp.json::<serde_json::Value>().await.unwrap();
    body.get("hits")
        .and_then(|h| h.get("total"))
        .and_then(|t| t.get("value"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

// ===========================================================================
// INDEX operation
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_index_operation() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({
            "title": "Test Document",
            "content": "Integration test"
        }))
        .set_header("CamelOpenSearch.Id", Value::String("doc-1".into()))
        .to(format!("opensearch://{}/test-index?operation=INDEX", url))
        .to("mock:result")
        .route_id("opensearch-index-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "opensearch index route delivery",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    assert_no_errors(&h).await;
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// SEARCH operation
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_search_operation() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    // Seed a document directly via HTTP (with ?refresh=true for immediate visibility)
    seed_document(
        &url,
        "test-search-idx",
        "search-doc-1",
        serde_json::json!({"title": "Searchable"}),
    )
    .await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"query": {"match_all": {}}}))
        .to(format!(
            "opensearch://{}/test-search-idx?operation=SEARCH",
            url
        ))
        .to("mock:result")
        .route_id("opensearch-search-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "opensearch search route delivery",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert_no_errors(&h).await;
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// GET + DELETE operations
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_get_and_delete() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    // Seed a document directly via HTTP
    seed_document(
        &url,
        "test-getdel-idx",
        "getdel-doc",
        serde_json::json!({"title": "ToDelete"}),
    )
    .await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    // Keep GET and DELETE in same route to avoid timer race between independent routes.
    let get_delete_route = RouteBuilder::from("timer:getdel-fire?period=50&repeatCount=1")
        .set_header("CamelOpenSearch.Id", Value::String("getdel-doc".into()))
        .to(format!(
            "opensearch://{}/test-getdel-idx?operation=GET",
            url
        ))
        .to("mock:got")
        .to(format!(
            "opensearch://{}/test-getdel-idx?operation=DELETE",
            url
        ))
        .to("mock:deleted")
        .route_id("opensearch-get-delete-test")
        .build()
        .unwrap();

    h.add_route(get_delete_route).await.unwrap();
    h.start().await;

    let got_ep = h.mock().get_endpoint("got").unwrap();
    wait_until(
        "opensearch get route",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let ep = got_ep.clone();
            async move { Ok(!ep.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    let deleted_ep = h.mock().get_endpoint("deleted").unwrap();
    wait_until(
        "opensearch delete route",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let ep = deleted_ep.clone();
            async move { Ok(!ep.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert_no_errors(&h).await;
}

// ===========================================================================
// BULK operation
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_bulk_operation() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!([
            {"index": {"_id": "bulk-1"}},
            {"title": "Bulk Doc 1"},
            {"index": {"_id": "bulk-2"}},
            {"title": "Bulk Doc 2"}
        ]))
        .to(format!("opensearch://{}/test-bulk-idx?operation=BULK", url))
        .to("mock:result")
        .route_id("opensearch-bulk-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "opensearch bulk route delivery",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert_no_errors(&h).await;
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// UPDATE operation
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_update_operation() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    seed_document(
        &url,
        "test-update-idx",
        "update-doc-1",
        serde_json::json!({"title": "Old"}),
    )
    .await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelOpenSearch.Id", Value::String("update-doc-1".into()))
        .set_body(serde_json::json!({"doc": {"title": "New"}}))
        .to(format!(
            "opensearch://{}/test-update-idx?operation=UPDATE",
            url
        ))
        .to("mock:result")
        .route_id("opensearch-update-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "opensearch update route delivery",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    wait_until(
        "updated document visible",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let url = url.clone();
            async move {
                let client = reqwest::Client::new();
                let get_url = format!("http://{}/test-update-idx/_doc/update-doc-1", url);
                let resp = client
                    .get(get_url)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let body = resp
                    .json::<serde_json::Value>()
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(body
                    .get("_source")
                    .and_then(|s| s.get("title"))
                    .and_then(|t| t.as_str())
                    == Some("New"))
            }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert_no_errors(&h).await;
    endpoint.assert_exchange_count(1).await;
}

// ===========================================================================
// MULTIGET operation
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_multiget_operation() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    seed_document(
        &url,
        "test-mget-idx",
        "mget-1",
        serde_json::json!({"title": "Doc1"}),
    )
    .await;
    seed_document(
        &url,
        "test-mget-idx",
        "mget-2",
        serde_json::json!({"title": "Doc2"}),
    )
    .await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"ids": ["mget-1", "mget-2"]}))
        .to(format!(
            "opensearch://{}/test-mget-idx?operation=MULTIGET",
            url
        ))
        .to("mock:result")
        .route_id("opensearch-multiget-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "opensearch multiget route delivery",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert_no_errors(&h).await;
    endpoint.assert_exchange_count(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_header_operation_override_delete() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    seed_document(
        &url,
        "test-override-idx",
        "override-doc-1",
        serde_json::json!({"title": "Override"}),
    )
    .await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_header("CamelOpenSearch.Operation", Value::String("DELETE".into()))
        .set_header("CamelOpenSearch.Id", Value::String("override-doc-1".into()))
        .set_body(serde_json::json!({"query": {"match_all": {}}}))
        .to(format!(
            "opensearch://{}/test-override-idx?operation=SEARCH",
            url
        ))
        .to("mock:result")
        .route_id("opensearch-header-override-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "opensearch header override route delivery",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    wait_until(
        "overridden delete removed doc",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let url = url.clone();
            async move {
                Ok(
                    get_document_status(&url, "test-override-idx", "override-doc-1").await
                        == reqwest::StatusCode::NOT_FOUND,
                )
            }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert_no_errors(&h).await;
    endpoint.assert_exchange_count(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_index_without_id() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"title": "AutoId"}))
        .to(format!(
            "opensearch://{}/test-autoid-idx?operation=INDEX",
            url
        ))
        .to("mock:result")
        .route_id("opensearch-index-autoid-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "opensearch index auto-id route delivery",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    wait_until(
        "auto-id document indexed",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let url = url.clone();
            async move { Ok(count_documents(&url, "test-autoid-idx").await >= 1) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert_no_errors(&h).await;
    endpoint.assert_exchange_count(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_invalid_operation_goes_to_error() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"query": {"match_all": {}}}))
        .to(format!(
            "opensearch://{}/test-invalid-op-idx?operation=NOPE",
            url
        ))
        .to("mock:result")
        .route_id("opensearch-invalid-op-test")
        .build()
        .unwrap();

    // The component validates the operation at endpoint-creation time (startup),
    // so add_route must fail — invalid config never reaches the dead-letter channel.
    let result = h.add_route(route).await;
    assert!(
        result.is_err(),
        "expected add_route to fail for unknown operation, but it succeeded"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("unknown OpenSearch operation"),
        "expected error about unknown operation, got: {err_msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_get_missing_id_goes_to_error() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "opensearch://{}/test-missing-get-idx?operation=GET",
            url
        ))
        .to("mock:result")
        .route_id("opensearch-missing-get-id-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let error_ep = h.mock().get_endpoint("error").unwrap();
    wait_until(
        "missing GET id hits error",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let ep = error_ep.clone();
            async move { Ok(!ep.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    error_ep.assert_exchange_count(1).await;
    h.mock()
        .get_endpoint("result")
        .unwrap()
        .assert_exchange_count(0)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_delete_missing_id_goes_to_error() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .to(format!(
            "opensearch://{}/test-missing-del-idx?operation=DELETE",
            url
        ))
        .to("mock:result")
        .route_id("opensearch-missing-delete-id-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let error_ep = h.mock().get_endpoint("error").unwrap();
    wait_until(
        "missing DELETE id hits error",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let ep = error_ep.clone();
            async move { Ok(!ep.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    error_ep.assert_exchange_count(1).await;
    h.mock()
        .get_endpoint("result")
        .unwrap()
        .assert_exchange_count(0)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_invalid_host_goes_to_error() {
    install_crypto_provider();
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"query": {"match_all": {}}}))
        .to("opensearch://127.0.0.1:1/test-bad-host-idx?operation=SEARCH&retryEnabled=false")
        .to("mock:result")
        .route_id("opensearch-invalid-host-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let error_ep = h.mock().get_endpoint("error").unwrap();
    wait_until(
        "invalid host hits error",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let ep = error_ep.clone();
            async move { Ok(!ep.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    error_ep.assert_exchange_count(1).await;
    h.mock()
        .get_endpoint("result")
        .unwrap()
        .assert_exchange_count(0)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_auth_username_password_in_uri() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"title": "Auth Path"}))
        .set_header("CamelOpenSearch.Id", Value::String("auth-doc-1".into()))
        .to(format!(
            "opensearch://{}/test-auth-idx?operation=INDEX&username=admin&password=admin",
            url
        ))
        .to("mock:result")
        .route_id("opensearch-auth-uri-test")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("result").unwrap();
    wait_until(
        "opensearch auth URI route delivery",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;
    assert_no_errors(&h).await;
    endpoint.assert_exchange_count(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opensearch_tls_scheme_goes_to_error_without_tls_server() {
    install_crypto_provider();
    let container = setup_opensearch_container().await;
    wait_for_opensearch(&container).await;
    let url = get_opensearch_url(&container).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(OpenSearchComponent::new())
        .build()
        .await;
    h.ctx()
        .lock()
        .await
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:error"))
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
        .set_body(serde_json::json!({"query": {"match_all": {}}}))
        .to(format!(
            "opensearchs://{}/test-tls-idx?operation=SEARCH",
            url
        ))
        .to("mock:result")
        .route_id("opensearch-tls-scheme-test")
        .build()
        .unwrap();

    let err = h.add_route(route).await.unwrap_err().to_string();
    assert!(
        err.contains("Component not found: opensearchs"),
        "unexpected error: {err}"
    );
}

// ===========================================================================
// Helper
// ===========================================================================

async fn assert_no_errors(h: &CamelTestContext) {
    if let Some(error_ep) = h.mock().get_endpoint("error") {
        let errors = error_ep.get_received_exchanges().await;
        if !errors.is_empty() {
            panic!("Route had errors: {:?}", errors[0].error);
        }
    }
}
