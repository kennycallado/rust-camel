//! # language-simple example
//!
//! Demonstrates using `camel-language-simple` to evaluate predicates and
//! expressions inside a rust-camel route.
//!
//! The timer fires 8 times (every 800 ms). Each tick is assigned rotating
//! `type` and `priority` headers.
//!
//! Filters demonstrate:
//! - compound predicates with `&&`
//! - null checks (`${body} != null`)
//! - boolean comparisons (`${header.approved} == true`)
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

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // --- Build language predicates / expressions up-front ---

    let lang = SimpleLanguage::new();

    // Predicate: only let 'order' messages through the filter
    let order_pred = lang
        .create_predicate("${header.type} == 'order'")
        .expect("valid predicate");

    // Predicate: compound condition using &&
    let high_priority_order_pred = lang
        .create_predicate("${header.type} == 'order' && ${header.priority} == 'high'")
        .expect("valid compound predicate");

    // Predicate: null-safe body existence check
    let body_present_pred = lang
        .create_predicate("${body} != null")
        .expect("valid body presence predicate");

    // Predicate: boolean literal comparison
    let approved_pred = lang
        .create_predicate("${header.approved} == true")
        .expect("valid boolean predicate");

    // Expression: evaluate ${header.type} to enrich the body
    let type_expr = lang
        .create_expression("${header.type}")
        .expect("valid expression");

    // Wrap in Arc so they can be moved into multiple route closures
    let order_pred = Arc::new(order_pred);
    let high_priority_order_pred = Arc::new(high_priority_order_pred);
    let body_present_pred = Arc::new(body_present_pred);
    let approved_pred = Arc::new(approved_pred);
    let type_expr = Arc::new(type_expr);

    // --- Build route ---

    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    let types = ["order", "invoice", "order", "shipment"];
    let priorities = ["high", "low", "high", "low"];

    let route = RouteBuilder::from("timer:tick?period=800&repeatCount=8")
        .route_id("language-simple-demo")
        // Step 1: assign rotating headers and alternate empty/non-empty body
        .process(move |mut exchange: camel_api::Exchange| {
            let c = Arc::clone(&counter_clone);
            Box::pin(async move {
                let n = c.fetch_add(1, Ordering::SeqCst) as usize;
                let msg_type = types[n % types.len()];
                let priority = priorities[n % priorities.len()];
                let approved = n.is_multiple_of(2);
                exchange
                    .input
                    .set_header("type", camel_api::Value::String(msg_type.to_string()));
                exchange
                    .input
                    .set_header("priority", camel_api::Value::String(priority.to_string()));
                exchange
                    .input
                    .set_header("approved", camel_api::Value::Bool(approved));

                exchange.input.body = if n.is_multiple_of(3) {
                    Body::Empty
                } else {
                    Body::Text(format!("message #{}", n + 1))
                };
                Ok(exchange)
            })
        })
        // Step 2: demonstrate body null check
        .filter({
            let pred = Arc::clone(&body_present_pred);
            move |ex: &camel_api::Exchange| pred.matches(ex).unwrap_or(false)
        })
        .to("log:body-present?showBody=true&showHeaders=true")
        .end_filter()
        // Step 3: use Simple Language expression to append type info to body
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
        // Step 4: filter — only 'order' messages pass
        .filter({
            let pred = Arc::clone(&order_pred);
            move |ex: &camel_api::Exchange| pred.matches(ex).unwrap_or(false)
        })
        .to("log:orders?showBody=true&showHeaders=true")
        .end_filter()
        // Step 5: compound predicate with && for high-priority orders
        .filter({
            let pred = Arc::clone(&high_priority_order_pred);
            move |ex: &camel_api::Exchange| pred.matches(ex).unwrap_or(false)
        })
        .to("log:high-priority-orders?showBody=true&showHeaders=true")
        .end_filter()
        // Step 6: boolean comparison against true literal
        .filter({
            let pred = Arc::clone(&approved_pred);
            move |ex: &camel_api::Exchange| pred.matches(ex).unwrap_or(false)
        })
        .to("log:approved-orders?showBody=true&showHeaders=true")
        .end_filter()
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Simple Language example running.");
    println!("Producing 8 messages with rotating type and priority headers.");
    println!("Filters demonstrate: body != null, order && high priority, and approved == true.");
    println!("Press Ctrl+C to stop early...\n");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    println!("\nStopped.");
    Ok(())
}
