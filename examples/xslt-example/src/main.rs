use std::path::PathBuf;

use camel_api::{Body, CamelError, Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_core::context::CamelContext;
use camel_xslt::XsltComponent;
use tower::ServiceExt;

fn body_to_string(body: &Body) -> String {
    match body {
        Body::Empty => String::new(),
        Body::Text(s) => s.clone(),
        Body::Xml(s) => s.clone(),
        Body::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        Body::Json(v) => v.to_string(),
        Body::Stream(_) => "<stream body>".to_string(),
    }
}

fn stylesheet_path() -> String {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    base.join("stylesheets")
        .join("transform.xslt")
        .to_string_lossy()
        .into_owned()
}

async fn send_to_direct(ctx: &CamelContext, uri: &str, payload: &str) -> Result<Exchange, CamelError> {
    let component = {
        let registry = ctx.registry();
        registry.get_or_err("direct")?
    };

    let endpoint = component.create_endpoint(uri, ctx)?;
    let producer = endpoint.create_producer(&ctx.producer_context())?;
    producer
        .oneshot(Exchange::new(Message::new(payload)))
        .await
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(DirectComponent::new());
    ctx.register_component(XsltComponent::default());
    ctx.register_component(LogComponent::new());

    let xslt_uri = format!("xslt:{}", stylesheet_path());
    let route = RouteBuilder::from("direct:in")
        .route_id("xslt-transform")
        .to(&xslt_uri)
        .to("log:out?showBody=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    let input = "<order><id>42</id></order>";
    let result = send_to_direct(&ctx, "direct:in", input).await?;
    println!("Input XML: {input}");
    println!("Transformed XML: {}", body_to_string(&result.input.body));

    ctx.stop().await?;
    Ok(())
}
