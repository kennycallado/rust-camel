//! rc-plxar: a route must be able to branch on the HTTP status of a failed
//! `http:` call from an expression inside `on_exceptions`.

use std::sync::Arc;
use std::time::Duration;

use camel_api::{Exchange, Message};
use camel_component_api::test_support::acquire_deadline;
use camel_component_http::HttpComponent;
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use tower::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn on_exception_expression_reads_http_status_of_failed_call() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let yaml = format!(
        r#"
routes:
  - id: "http-status-branch"
    from: "direct:http-status-branch"
    error_handler:
      on_exceptions:
        - kind: "*"
          handled: true
          handled_by: "direct:http-status-shaper"
    steps:
      - to: "{}/missing?allowInternal=true"
  - id: "http-status-shaper"
    from: "direct:http-status-shaper"
    steps:
      - set_body: "status=${{header.CamelHttpResponseCode}} text=${{header.CamelHttpResponseText}}"
"#,
        server.uri()
    );
    let routes = parse_yaml(&yaml).expect("route must parse");

    let h = CamelTestContext::builder()
        .with_direct()
        .with_component(HttpComponent::new())
        .build()
        .await;
    for route in routes {
        h.add_route(route).await.expect("add route");
    }
    h.start().await;

    let producer = {
        let ctx = acquire_deadline(h.ctx(), "camel context", Duration::from_secs(10)).await;
        let producer_ctx = ctx.producer_context();
        let component = ctx
            .registry()
            .get("direct")
            .expect("direct component registered");
        let endpoint = component
            .create_endpoint("direct:http-status-branch", &*ctx)
            .expect("direct endpoint");
        endpoint
            .create_producer(
                Arc::new(camel_component_api::NoOpComponentContext),
                &producer_ctx,
            )
            .expect("direct producer")
    };
    let reply = producer
        .oneshot(Exchange::new(Message::default()))
        .await
        .expect("handled exception must complete the exchange");

    assert_eq!(
        reply.input.body.as_text(),
        Some("status=404 text=Not Found")
    );
    h.shutdown().await;
}
