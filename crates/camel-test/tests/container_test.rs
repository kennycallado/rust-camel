//! Integration tests for the Container component.
//!
//! Tests that require a running Docker daemon and network access to pull images.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

use bollard::Docker;
use camel_api::{Body, Exchange, Message};
use camel_component_api::{Component, NoOpComponentContext, ProducerContext};
use camel_component_container::{
    ContainerComponent, HEADER_ACTION, HEADER_ACTION_RESULT, HEADER_CMD, HEADER_CONTAINER_ID,
    HEADER_EXIT_CODE,
};
use serde_json::Value;
use tower::{Service, ServiceExt};

async fn connect_docker() -> Option<Docker> {
    let docker = Docker::connect_with_local_defaults().ok()?;
    if docker.ping().await.is_err() {
        eprintln!("Skipping test: Docker daemon not responding to ping");
        return None;
    }
    Some(docker)
}

#[tokio::test]
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
    let mut producer = endpoint.create_producer(&ctx).unwrap();

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
            Some(bollard::query_parameters::RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;
}

#[tokio::test]
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
    let mut producer = endpoint.create_producer(&ctx).unwrap();

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
            Some(bollard::query_parameters::RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;
}
