//! Integration tests for the Container component.
//!
//! Tests that require a running Docker daemon and network access to pull images.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

use bollard::Docker;
use bollard::query_parameters::{CreateImageOptions, ListImagesOptions, RemoveContainerOptions};
use camel_api::{Body, CamelError, Exchange, Message};
use camel_component_api::test_support::PanicRuntimeObservability;
use camel_component_api::{
    Component, ConsumerContext, NoOpComponentContext, ProducerContext, RuntimeObservability,
};
use camel_component_container::{
    ContainerComponent, HEADER_ACTION, HEADER_ACTION_RESULT, HEADER_CMD, HEADER_CONTAINER_ID,
    HEADER_CONTAINER_NAME, HEADER_EXIT_CODE, HEADER_IMAGE, HEADER_NETWORK,
};
use futures::StreamExt;
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tower::{Service, ServiceExt};

fn rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(PanicRuntimeObservability)
}

async fn connect_docker() -> Option<Docker> {
    let docker = Docker::connect_with_local_defaults().ok()?;
    if docker.ping().await.is_err() {
        eprintln!("Skipping test: Docker daemon not responding to ping");
        return None;
    }
    Some(docker)
}

// =========================================================================
// Existing tests (run + exec happy paths)
// =========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_container_producer_run_with_volumes() {
    let docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let cargo_toml = std::fs::canonicalize("./Cargo.toml").expect("Cargo.toml should exist");
    let volume_path = cargo_toml.to_str().unwrap();

    let component = ContainerComponent::new();
    let ctx = NoOpComponentContext;
    let uri = format!(
        "container:run?image=alpine&cmd=cat /mnt/test.txt&volumes={}:/mnt/test.txt:ro&autoRemove=true",
        volume_path
    );
    let endpoint = component.create_endpoint(&uri, &ctx).unwrap();
    let ctx = ProducerContext::new();
    let mut producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ctx)
        .unwrap();

    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_ACTION, Value::String("run".into()));

    let result = producer
        .ready()
        .await
        .unwrap()
        .call(exchange)
        .await
        .expect("Run with volumes should succeed");

    let container_id = result
        .input
        .header(HEADER_CONTAINER_ID)
        .and_then(|v: &Value| v.as_str().map(|s| s.to_string()))
        .expect("Should have container ID");

    let _ = docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_container_producer_exec() {
    let docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let component = ContainerComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component
        .create_endpoint(
            "container:run?image=alpine&cmd=sleep 30&autoRemove=true",
            &ctx,
        )
        .unwrap();
    let ctx = ProducerContext::new();
    let mut producer = endpoint
        .create_producer(Arc::new(NoOpComponentContext), &ctx)
        .unwrap();

    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_ACTION, Value::String("run".into()));

    let result = producer
        .ready()
        .await
        .unwrap()
        .call(exchange)
        .await
        .expect("Run should succeed");

    let container_id = result
        .input
        .header(HEADER_CONTAINER_ID)
        .and_then(|v: &Value| v.as_str().map(|s| s.to_string()))
        .expect("Should have container ID");

    let mut exec_exchange = Exchange::new(Message::new(""));
    exec_exchange
        .input
        .set_header(HEADER_ACTION, Value::String("exec".into()));
    exec_exchange
        .input
        .set_header(HEADER_CONTAINER_ID, Value::String(container_id.clone()));
    exec_exchange
        .input
        .set_header(HEADER_CMD, Value::String("echo hello".into()));

    let exec_result = producer
        .ready()
        .await
        .unwrap()
        .call(exec_exchange)
        .await
        .expect("Exec should succeed");

    match &exec_result.input.body {
        Body::Text(text) => {
            assert!(
                text.contains("hello"),
                "Exec output should contain 'hello', got: {}",
                text
            );
        }
        other => panic!("Expected Body::Text, got: {:?}", other),
    }

    let exit_code: Option<i64> = exec_result
        .input
        .header(HEADER_EXIT_CODE)
        .and_then(|v: &Value| v.as_i64());
    assert_eq!(exit_code, Some(0));

    let action_result: Option<&str> = exec_result
        .input
        .header(HEADER_ACTION_RESULT)
        .and_then(|v: &Value| v.as_str());
    assert_eq!(action_result, Some("success"));

    let _ = docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;
}

// =========================================================================
// Migrated from camel-container inline tests
// =========================================================================

/// Verify that the action header overrides the endpoint operation.
#[tokio::test(flavor = "multi_thread")]
async fn test_container_producer_resolves_operation_from_header() {
    let _docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let component = ContainerComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component.create_endpoint("container:run", &ctx).unwrap();

    let ctx = ProducerContext::new();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_ACTION, Value::String("list".into()));

    let result = producer
        .ready()
        .await
        .unwrap()
        .call(exchange)
        .await
        .unwrap();

    assert_eq!(
        result
            .input
            .header(HEADER_ACTION_RESULT)
            .map(|v: &Value| v.as_str().unwrap()),
        Some("success")
    );
}

/// start, stop, remove return an error when CamelContainerId header is missing.
#[tokio::test(flavor = "multi_thread")]
async fn test_container_producer_lifecycle_operations_missing_id() {
    let _docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let component = ContainerComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component.create_endpoint("container:start", &ctx).unwrap();
    let ctx = ProducerContext::new();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    for operation in ["start", "stop", "remove"] {
        let mut exchange = Exchange::new(Message::new(""));
        exchange
            .input
            .set_header(HEADER_ACTION, Value::String(operation.to_string()));

        let result = producer.ready().await.unwrap().call(exchange).await;

        assert!(
            result.is_err(),
            "Expected error for {} operation without CamelContainerId",
            operation
        );
        let err = result.unwrap_err();
        match &err {
            CamelError::ProcessorError(msg) => {
                assert!(
                    msg.contains(HEADER_CONTAINER_ID),
                    "Error message should mention {}, got: {}",
                    HEADER_CONTAINER_ID,
                    msg
                );
            }
            _ => panic!("Expected ProcessorError for {}, got: {:?}", operation, err),
        }
    }
}

/// stop operation returns an error for a nonexistent container.
#[tokio::test(flavor = "multi_thread")]
async fn test_container_producer_stop_nonexistent() {
    let _docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let component = ContainerComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component.create_endpoint("container:stop", &ctx).unwrap();
    let ctx = ProducerContext::new();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_ACTION, Value::String("stop".into()));
    exchange.input.set_header(
        HEADER_CONTAINER_ID,
        Value::String("nonexistent-container-123".into()),
    );

    let result = producer.ready().await.unwrap().call(exchange).await;

    assert!(
        result.is_err(),
        "Expected error when stopping nonexistent container"
    );
    let err = result.unwrap_err();
    match &err {
        CamelError::ProcessorError(msg) => {
            assert!(
                msg.to_lowercase().contains("no such container")
                    || msg.to_lowercase().contains("not found")
                    || msg.contains("404"),
                "Error message should indicate container not found, got: {}",
                msg
            );
        }
        _ => panic!("Expected ProcessorError, got: {:?}", err),
    }
}

/// run operation returns an error when no image is provided.
#[tokio::test(flavor = "multi_thread")]
async fn test_container_producer_run_missing_image() {
    let _docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let component = ContainerComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component.create_endpoint("container:run", &ctx).unwrap();
    let ctx = ProducerContext::new();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_ACTION, Value::String("run".into()));

    let result = producer.ready().await.unwrap().call(exchange).await;

    assert!(
        result.is_err(),
        "Expected error for run operation without image"
    );
    let err = result.unwrap_err();
    match &err {
        CamelError::ProcessorError(msg) => {
            assert!(
                msg.to_lowercase().contains("image"),
                "Error message should mention 'image', got: {}",
                msg
            );
        }
        _ => panic!("Expected ProcessorError, got: {:?}", err),
    }
}

/// run operation uses image from header (nonexistent image should fail).
#[tokio::test(flavor = "multi_thread")]
async fn test_container_producer_run_image_from_header() {
    let _docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let component = ContainerComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component.create_endpoint("container:run", &ctx).unwrap();
    let ctx = ProducerContext::new();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_ACTION, Value::String("run".into()));
    exchange.input.set_header(
        HEADER_IMAGE,
        Value::String("nonexistent-image-xyz-12345:latest".into()),
    );

    let result = producer.ready().await.unwrap().call(exchange).await;

    assert!(
        result.is_err(),
        "Expected error when running container with nonexistent image"
    );
    let err = result.unwrap_err();
    match &err {
        CamelError::ProcessorError(msg) => {
            assert!(
                msg.to_lowercase().contains("no such image")
                    || msg.to_lowercase().contains("not found")
                    || msg.to_lowercase().contains("image")
                    || msg.to_lowercase().contains("pull")
                    || msg.contains("404"),
                "Error message should indicate image issue, got: {}",
                msg
            );
        }
        _ => panic!("Expected ProcessorError, got: {:?}", err),
    }
}

/// Full happy path: run alpine:latest, verify container ID + action result + inspect.
#[tokio::test(flavor = "multi_thread")]
async fn test_container_producer_run_alpine_container() {
    let docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let images = docker.list_images(None::<ListImagesOptions>).await.unwrap();
    let has_alpine = images
        .iter()
        .any(|img| img.repo_tags.iter().any(|t| t.starts_with("alpine")));

    if !has_alpine {
        eprintln!("Pulling alpine:latest image...");
        let mut stream = docker.create_image(
            Some(CreateImageOptions {
                from_image: Some("alpine:latest".to_string()),
                ..Default::default()
            }),
            None,
            None,
        );
        while let Some(_item) = stream.next().await {
            // Wait for pull to complete
        }
        eprintln!("Image pulled successfully");
    }

    let component = ContainerComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component.create_endpoint("container:run", &ctx).unwrap();
    let ctx = ProducerContext::new();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let container_name = format!("test-rust-camel-{}", timestamp);
    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_IMAGE, Value::String("alpine:latest".into()));
    exchange
        .input
        .set_header(HEADER_CONTAINER_NAME, Value::String(container_name.clone()));

    let result = producer
        .ready()
        .await
        .unwrap()
        .call(exchange)
        .await
        .expect("Container run should succeed");

    let container_id = result
        .input
        .header(HEADER_CONTAINER_ID)
        .and_then(|v: &Value| v.as_str().map(|s| s.to_string()))
        .expect("Expected container ID header");
    assert!(!container_id.is_empty(), "Container ID should not be empty");

    assert_eq!(
        result
            .input
            .header(HEADER_ACTION_RESULT)
            .and_then(|v: &Value| v.as_str()),
        Some("success")
    );

    let inspect = docker
        .inspect_container(&container_id, None)
        .await
        .expect("Container should exist");
    assert_eq!(inspect.id.as_deref(), Some(container_id.as_str()));

    docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .ok();

    eprintln!("Container {} created and cleaned up", container_id);
}

/// Events consumer gracefully shuts down when cancellation is requested.
#[tokio::test(flavor = "multi_thread")]
async fn test_container_consumer_cancellation() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::mpsc;

    let _docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let component = ContainerComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component.create_endpoint("container:events", &ctx).unwrap();
    let mut consumer = endpoint.create_consumer(rt()).unwrap();

    let (tx, _rx) = mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let context =
        ConsumerContext::new(tx, cancel_token.clone(), "container-test-route".to_string());

    let completed = Arc::new(AtomicBool::new(false));
    let completed_clone = completed.clone();

    let handle = tokio::spawn(async move {
        let result = consumer.start(context).await;
        completed_clone.store(true, Ordering::SeqCst);
        result
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    assert!(
        !completed.load(Ordering::SeqCst),
        "Consumer should still be running before cancellation"
    );

    cancel_token.cancel();

    let result = tokio::time::timeout(tokio::time::Duration::from_millis(500), handle).await;

    assert!(
        result.is_ok(),
        "Consumer should gracefully shut down after cancellation"
    );
    assert!(
        completed.load(Ordering::SeqCst),
        "Consumer should have completed after cancellation"
    );
}

/// List containers returns a JSON array body.
#[tokio::test(flavor = "multi_thread")]
async fn test_container_producer_list_containers() {
    let _docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let component = ContainerComponent::new();
    let ctx = NoOpComponentContext;
    let endpoint = component.create_endpoint("container:list", &ctx).unwrap();

    let ctx = ProducerContext::new();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_ACTION, Value::String("list".into()));

    let result = producer
        .ready()
        .await
        .unwrap()
        .call(exchange)
        .await
        .expect("Producer should succeed when Docker is available");

    match &result.input.body {
        Body::Json(json_value) => {
            assert!(
                json_value.is_array(),
                "Expected input body to be a JSON array, got: {:?}",
                json_value
            );
        }
        other => panic!("Expected Body::Json with array, got: {:?}", other),
    }
}

/// Network create → list → remove lifecycle.
#[tokio::test(flavor = "multi_thread")]
async fn test_container_producer_network_lifecycle() {
    let _docker = match connect_docker().await {
        Some(d) => d,
        None => return,
    };

    let network_name = format!("camel-test-{}", std::process::id());

    let component = ContainerComponent::new();
    let component_ctx = NoOpComponentContext;

    let endpoint = component
        .create_endpoint(
            &format!("container:network-create?name={}", network_name),
            &component_ctx,
        )
        .unwrap();
    let ctx = ProducerContext::new();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_ACTION, Value::String("network-create".into()));

    let result = producer
        .ready()
        .await
        .unwrap()
        .call(exchange)
        .await
        .expect("Network create should succeed");

    let network_id = result
        .input
        .header(HEADER_NETWORK)
        .and_then(|v: &Value| v.as_str().map(|s| s.to_string()))
        .expect("Should have network ID");

    assert!(!network_id.is_empty());

    let endpoint = component
        .create_endpoint("container:network-list", &component_ctx)
        .unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_ACTION, Value::String("network-list".into()));

    let list_result = producer
        .ready()
        .await
        .unwrap()
        .call(exchange)
        .await
        .expect("Network list should succeed");

    match &list_result.input.body {
        Body::Json(json_value) => {
            assert!(json_value.is_array(), "Expected JSON array");
        }
        other => panic!("Expected Body::Json, got: {:?}", other),
    }

    let endpoint = component
        .create_endpoint(
            &format!("container:network-remove?network={}", network_name),
            &component_ctx,
        )
        .unwrap();
    let mut producer = endpoint.create_producer(rt(), &ctx).unwrap();

    let mut exchange = Exchange::new(Message::new(""));
    exchange
        .input
        .set_header(HEADER_ACTION, Value::String("network-remove".into()));

    let remove_result = producer
        .ready()
        .await
        .unwrap()
        .call(exchange)
        .await
        .expect("Network remove should succeed");

    let action_result = remove_result
        .input
        .header(HEADER_ACTION_RESULT)
        .and_then(|v: &Value| v.as_str());
    assert_eq!(action_result, Some("success"));
}
