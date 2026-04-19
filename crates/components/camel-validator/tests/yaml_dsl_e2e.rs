#[tokio::test]
async fn yaml_route_validate_step_works_end_to_end() {
    use camel_component_api::{Body, Exchange, Message};
    use camel_component_direct::DirectComponent;
    use camel_component_validator::ValidatorComponent;
    use camel_core::CamelContext;
    use camel_dsl::yaml::parse_yaml;
    use std::io::Write;
    use tower::ServiceExt;

    let json_schema = r#"{"type":"object","required":["id"]}"#;

    let mut f = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
    f.write_all(json_schema.as_bytes()).unwrap();

    let yaml = format!(
        r#"
routes:
  - id: test-validate
    from: "direct:in"
    steps:
      - validate: "{}"
"#,
        f.path().display()
    );

    let defs = parse_yaml(&yaml).unwrap();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(DirectComponent::new());
    ctx.register_component(ValidatorComponent::new());
    for def in defs {
        ctx.add_route_definition(def).await.unwrap();
    }
    ctx.start().await.unwrap();

    let producer = {
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered");
        let endpoint = component.create_endpoint("direct:in", &ctx).unwrap();
        endpoint.create_producer(&producer_ctx).unwrap()
    };

    let valid = Exchange::new(Message::new(Body::Json(serde_json::json!({"id": "42"}))));
    assert!(producer.oneshot(valid).await.is_ok());

    let producer = {
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered");
        let endpoint = component.create_endpoint("direct:in", &ctx).unwrap();
        endpoint.create_producer(&producer_ctx).unwrap()
    };

    let invalid = Exchange::new(Message::new(Body::Json(serde_json::json!({"name": "x"}))));
    let err = producer.oneshot(invalid).await.unwrap_err();
    assert!(err.to_string().contains("validation failed"), "got: {err}");

    ctx.stop().await.unwrap();
}
