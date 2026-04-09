use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::CamelError;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_language_api::Language;
use camel_language_jsonpath::JsonPathLanguage;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    ctx.register_language("jsonpath", Box::new(JsonPathLanguage))
        .expect("jsonpath not yet registered");

    let lang = JsonPathLanguage;

    let customer_expr = lang
        .create_expression("$.customer")
        .expect("valid expression");

    let active_pred = lang.create_predicate("$.active").expect("valid predicate");

    let customer_expr = Arc::new(customer_expr);
    let active_pred = Arc::new(active_pred);

    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    let customers = ["Alice", "Bob", "Charlie"];

    let route = RouteBuilder::from("timer:tick?period=800&repeatCount=6")
        .route_id("language-jsonpath-demo")
        .process(move |mut exchange: camel_api::Exchange| {
            let c = Arc::clone(&counter_clone);
            Box::pin(async move {
                let n = c.fetch_add(1, Ordering::SeqCst) as usize;
                let active = (n as u64).is_multiple_of(2);
                let customer = customers[n % customers.len()];
                let body = json!({
                    "active": active,
                    "customer": customer,
                    "order": { "id": n + 1 }
                });
                exchange.input.body = Body::Json(body);
                Ok(exchange)
            })
        })
        .process({
            let expr = Arc::clone(&customer_expr);
            move |mut exchange: camel_api::Exchange| {
                let expr = Arc::clone(&expr);
                Box::pin(async move {
                    if let Ok(value) = expr.evaluate(&exchange)
                        && let Some(name) = value.as_str()
                    {
                        exchange
                            .input
                            .set_header("customer", camel_api::Value::String(name.to_string()));
                    }
                    Ok(exchange)
                })
            }
        })
        .to("log:all-orders?showBody=true&showHeaders=true")
        .filter({
            let pred = Arc::clone(&active_pred);
            move |ex: &camel_api::Exchange| pred.matches(ex).unwrap_or(false)
        })
        .to("log:active-orders?showBody=true&showHeaders=true")
        .end_filter()
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("JSONPath Language example running.");
    println!("Producing 6 orders with alternating active status.");
    println!("- $.customer expression sets the 'customer' header");
    println!("- $.active predicate filters to active-only log");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    println!("\nStopped.");
    Ok(())
}
