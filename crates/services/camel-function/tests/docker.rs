#![cfg(feature = "docker-tests")]

use camel_api::function::*;
use camel_api::{Body, Exchange, Message};
use camel_function::{ContainerProvider, PullPolicy, RunnerHandle};
use std::time::Duration;

async fn build_runner_image() {
    let output = std::process::Command::new("docker")
        .args([
            "build",
            "-t",
            "rustcamel/deno-runner:test",
            &format!("{}/runner", env!("CARGO_MANIFEST_DIR")),
        ])
        .output()
        .expect("docker build failed");
    if !output.status.success() {
        panic!(
            "docker build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn create_provider() -> ContainerProvider {
    ContainerProvider::builder()
        .image("rustcamel/deno-runner:test")
        .pull_policy(PullPolicy::Never)
        .boot_timeout(Duration::from_secs(15))
        .build()
        .expect("create container provider")
}

fn make_definition(id: &str, source: &str) -> FunctionDefinition {
    FunctionDefinition {
        id: FunctionId(id.to_string()),
        runtime: "deno".to_string(),
        source: source.to_string(),
        timeout_ms: 5000,
        route_id: None,
        step_index: None,
    }
}

async fn wait_for_health(
    provider: &ContainerProvider,
    handle: &RunnerHandle,
    timeout: Duration,
) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        match provider.health_runner(handle).await {
            Ok(camel_function::HealthReport::Healthy) => return Ok(()),
            _ => {
                if start.elapsed() > timeout {
                    return Err(format!("runner not healthy after {:?}", timeout));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn assert_no_runner_containers() {
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let options = bollard::query_parameters::ListContainersOptions {
        all: true,
        ..Default::default()
    };
    let containers = docker
        .list_containers(Some(options))
        .await
        .expect("list containers");
    let runner_containers: Vec<_> = containers
        .into_iter()
        .filter(|c| {
            c.labels
                .as_ref()
                .map(|l| l.get("camel.function.runner") == Some(&"true".to_string()))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        runner_containers.is_empty(),
        "no containers with label camel.function.runner=true should remain, found {}",
        runner_containers.len()
    );
}

#[tokio::test]
async fn test_spawn_and_health() {
    build_runner_image().await;
    let provider = create_provider();
    let handle = provider.spawn_runner("deno").await.expect("spawn");
    wait_for_health(&provider, &handle, Duration::from_secs(10))
        .await
        .expect("health");
    let report = provider.health_runner(&handle).await.expect("health");
    assert!(matches!(report, camel_function::HealthReport::Healthy));
    provider.shutdown_runner(handle).await.expect("shutdown");
    assert_no_runner_containers().await;
}

#[tokio::test]
async fn test_register_and_invoke() {
    build_runner_image().await;
    let provider = create_provider();
    let handle = provider.spawn_runner("deno").await.expect("spawn");
    wait_for_health(&provider, &handle, Duration::from_secs(10))
        .await
        .expect("health");

    let def = make_definition(
        "echo_fn",
        "export default (c) => { c.setBody(c.body().toString().toUpperCase()); }",
    );
    provider
        .register_function(&handle, &def)
        .await
        .expect("register");

    let exchange = Exchange::new(Message::new(Body::Text("hello".into())));
    let patch = provider
        .invoke_function(&handle, &def.id, &exchange)
        .await
        .expect("invoke");
    assert!(
        matches!(patch.body, Some(PatchBody::Text(ref s)) if s == "HELLO")
    );

    provider.shutdown_runner(handle).await.expect("shutdown");
    assert_no_runner_containers().await;
}

#[tokio::test]
async fn test_shutdown_removes_container() {
    build_runner_image().await;
    let provider = create_provider();
    let handle = provider.spawn_runner("deno").await.expect("spawn");
    wait_for_health(&provider, &handle, Duration::from_secs(10))
        .await
        .expect("health");
    provider.shutdown_runner(handle).await.expect("shutdown");
    assert_no_runner_containers().await;
}

#[tokio::test]
async fn test_cleanup_all() {
    build_runner_image().await;
    let provider = create_provider();
    let h1 = provider.spawn_runner("deno").await.expect("spawn");
    wait_for_health(&provider, &h1, Duration::from_secs(10))
        .await
        .expect("health");
    provider.cleanup_all().await;
    assert_no_runner_containers().await;
}
