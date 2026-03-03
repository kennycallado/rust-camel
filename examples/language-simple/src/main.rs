//! # language-simple example
//!
//! Demonstrates using `camel-language-simple` to evaluate predicates and
//! expressions inside a rust-camel route.
//!
//! The timer fires 8 times (every 800 ms). Each tick is assigned a `type`
//! header cycling through: `order`, `invoice`, `order`, `shipment`, ...
//!
//! A Simple Language predicate (`${header.type} == 'order'`) is used as the
//! filter condition — only exchanges tagged as `order` proceed to the log step.
//!
//! A Simple Language expression (`${header.type}`) enriches the body,
//! showing how expressions integrate with arbitrary processing logic.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::CamelError;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_language_api::Language;
use camel_language_simple::SimpleLanguage;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // --- Build language predicates / expressions up-front ---

    let lang = SimpleLanguage;

    // Predicate: only let 'order' messages through the filter
    let order_pred = lang
        .create_predicate("${header.type} == 'order'")
        .expect("valid predicate");

    // Expression: evaluate ${header.type} to enrich the body
    let type_expr = lang
        .create_expression("${header.type}")
        .expect("valid expression");

    // Wrap in Arc so they can be moved into multiple route closures
    let order_pred = Arc::new(order_pred);
    let type_expr = Arc::new(type_expr);

    // --- Build route ---

    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    let types = ["order", "invoice", "order", "shipment"];

    let route = RouteBuilder::from("timer:tick?period=800&repeatCount=8")
        .route_id("language-simple-demo")
        // Step 1: assign a rotating type header and an initial body
        .process(move |mut exchange: camel_api::Exchange| {
            let c = Arc::clone(&counter_clone);
            Box::pin(async move {
                let n = c.fetch_add(1, Ordering::SeqCst) as usize;
                let msg_type = types[n % types.len()];
                exchange
                    .input
                    .set_header("type", camel_api::Value::String(msg_type.to_string()));
                exchange.input.body = Body::Text(format!("message #{}", n + 1));
                Ok(exchange)
            })
        })
        // Step 2: use Simple Language expression to append type info to body
        .process({
            let type_expr = Arc::clone(&type_expr);
            move |mut exchange: camel_api::Exchange| {
                let expr = Arc::clone(&type_expr);
                Box::pin(async move {
                    if let Ok(camel_api::Value::String(t)) = expr.evaluate(&exchange) {
                        let current = exchange.input.body.as_text().unwrap_or("").to_string();
                        exchange.input.body = Body::Text(format!("{current} [type={t}]"));
                    }
                    Ok(exchange)
                })
            }
        })
        // Step 3: filter — only 'order' messages pass
        .filter({
            let pred = Arc::clone(&order_pred);
            move |ex: &camel_api::Exchange| pred.matches(ex).unwrap_or(false)
        })
        .to("log:orders?showBody=true&showHeaders=true")
        .end_filter()
        .build()?;

    ctx.add_route_definition(route)?;
    ctx.start().await?;

    println!("Simple Language example running.");
    println!("Producing 8 messages with rotating types: order / invoice / order / shipment");
    println!("Only 'order' messages will appear in the 'orders' log.");
    println!("Press Ctrl+C to stop early...\n");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    println!("\nStopped.");
    Ok(())
}
