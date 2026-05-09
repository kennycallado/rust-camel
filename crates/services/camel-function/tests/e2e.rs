#![cfg(feature = "docker-tests")]

use camel_api::{CamelError, Exchange, Message};
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_core::context::CamelContext;
use camel_dsl;
use camel_function::{ContainerProvider, FunctionConfig, FunctionRuntimeService, PullPolicy};
use std::collections::HashMap;
use std::time::Duration;
use tower::ServiceExt;

async fn build_runner_image() {
    let output = std::process::Command::new("docker")
        .args([
            "build",
            "-t",
            "rustcamel/deno-runner:e2e",
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
        .image("rustcamel/deno-runner:e2e")
        .pull_policy(PullPolicy::Never)
        .boot_timeout(Duration::from_secs(15))
        .instance_id("e2e_yaml")
        .build()
        .expect("create container provider")
}

async fn send_to_direct(
    ctx: &CamelContext,
    uri: &str,
    payload: &str,
) -> Result<Exchange, CamelError> {
    let component = {
        let registry = ctx.registry();
        registry.get_or_err("direct")?
    };
    let endpoint = component.create_endpoint(uri, ctx)?;
    let producer = endpoint.create_producer(&ctx.producer_context())?;
    producer.oneshot(Exchange::new(Message::new(payload))).await
}

#[tokio::test]
async fn test_e2e_yaml_function_step() {
    build_runner_image().await;

    let provider = create_provider();
    let instance_id = provider.instance_id().to_string();
    let mut config = FunctionConfig::default();
    config.boot_timeout = std::time::Duration::from_secs(15);
    let service = FunctionRuntimeService::with_container_provider(config, provider);

    let mut ctx = CamelContext::builder()
        .with_lifecycle(service)
        .build()
        .await
        .expect("build context");

    ctx.register_component(DirectComponent::new());
    ctx.register_component(LogComponent::new());

    let yaml = r#"
routes:
  - id: "e2e-enrich"
    from: "direct:enrich"
    steps:
      - set_body: "hello world"
      - function:
          runtime: deno
          source: |
            export default (camel) => {
              camel.setBody(String(camel.body()).toUpperCase());
              camel.setHeader("X-Enriched", "true");
            };
          timeout_ms: 5000
      - log: "enriched"
"#;

    let defs = camel_dsl::parse_yaml(yaml).expect("parse yaml");
    assert_eq!(defs.len(), 1);

    for def in defs {
        ctx.add_route_definition(def).await.expect("add route");
    }

    ctx.start().await.expect("start context");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let result = send_to_direct(&ctx, "direct:enrich", "ignored")
        .await
        .expect("send exchange");

    assert_eq!(result.input.body.as_text(), Some("HELLO WORLD"));

    let enriched = result
        .input
        .headers
        .get("X-Enriched")
        .expect("X-Enriched header");
    assert_eq!(enriched.as_str(), Some("true"));

    ctx.stop().await.expect("stop context");

    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let options = bollard::query_parameters::ListContainersOptions {
        filters: Some(std::collections::HashMap::from([(
            "label".to_string(),
            vec![format!("camel.function.instance={}", instance_id)],
        )])),
        ..Default::default()
    };
    let containers = docker
        .list_containers(Some(options))
        .await
        .expect("list containers");
    assert!(
        containers.is_empty(),
        "no containers should remain for instance {}, found {}",
        instance_id,
        containers.len()
    );
}
