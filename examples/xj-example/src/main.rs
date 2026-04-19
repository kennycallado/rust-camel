//! Demonstrates Apache Camel XJ conversions with three flows:
//! 1) XML -> JSON (`direct:xml2json`)
//! 2) JSON -> XML in Camel XJ format (`direct:json2xml`)
//! 3) Round-trip JSON -> XML -> JSON using both routes

use camel_api::{Body, CamelError, Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_core::context::CamelContext;
use camel_xj::XjComponent;
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
    ctx.register_component(XjComponent::default());
    ctx.register_component(LogComponent::new());

    let xml_to_json = RouteBuilder::from("direct:xml2json")
        .route_id("xml2json")
        .to("xj:classpath:identity?direction=xml2json")
        .to("log:result?showBody=true")
        .build()?;

    let json_to_xml = RouteBuilder::from("direct:json2xml")
        .route_id("json2xml")
        .to("xj:classpath:identity?direction=json2xml")
        .to("log:result?showBody=true")
        .build()?;

    ctx.add_route_definition(xml_to_json).await?;
    ctx.add_route_definition(json_to_xml).await?;
    ctx.start().await?;

    let xml_input = r#"<order><id>42</id><status>open</status></order>"#;
    let xml_to_json_result = send_to_direct(&ctx, "direct:xml2json", xml_input).await?;

    let json_input = r#"{"order":{"id":"42","status":"open"}}"#;
    let json_to_xml_result = send_to_direct(&ctx, "direct:json2xml", json_input).await?;

    // Flow 3: round-trip JSON -> XML -> JSON
    let round_trip_xml = body_to_string(&json_to_xml_result.input.body);
    let round_trip_result = send_to_direct(&ctx, "direct:xml2json", &round_trip_xml).await?;
    let round_trip_json = body_to_string(&round_trip_result.input.body);
    let round_trip_lossless = json_input == round_trip_json;

    println!("=== Flow 1: XML → JSON ===");
    println!("  Input:  {xml_input}");
    println!(
        "  Output: {}",
        body_to_string(&xml_to_json_result.input.body)
    );
    println!();

    println!("=== Flow 2: JSON → XML (Apache Camel XJ format) ===");
    println!("  Input:  {json_input}");
    println!("  Output: {}", body_to_string(&json_to_xml_result.input.body));
    println!();

    println!("=== Flow 3: Round-trip JSON → XML → JSON ===");
    println!("  Input:  {json_input}");
    println!("  After JSON→XML: {round_trip_xml}");
    println!("  After XML→JSON: {round_trip_json}");
    println!("  Round-trip lossless: {round_trip_lossless}");

    ctx.stop().await?;
    Ok(())
}
