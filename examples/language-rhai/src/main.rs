//! # language-rhai example
//!
//! Demonstrates using `camel-language-rhai` to evaluate Rhai scripts inside
//! a rust-camel route.
//!
//! The timer fires 6 times (every 900 ms). Each message is assigned a
//! `priority` header (`high` or `normal`) and an `amount` header (numeric).
//!
//! Three Rhai capabilities are shown:
//!
//! 1. **Expression** — a Rhai script reads `body` and headers to produce
//!    a transformed string: `"[URGENT] order #1"` vs `"order #1"`.
//!
//! 2. **Predicate** — a Rhai script used as a filter: only high-value orders
//!    (`header("amount") > 100`) proceed to the alert log step.
//!
//! 3. **In-script mutation** — `set_header` / `header` within a single
//!    evaluation shows that values written with `set_header` are visible
//!    later in the same script (but not propagated back to the Exchange).
//!
//! 4. **Script step** — the `.script()` builder step uses a mutating Rhai
//!    expression: assignment syntax (`headers["k"] = v`) propagates changes
//!    back to the Exchange. Headers set this way are visible in subsequent steps.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::CamelError;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_language_api::Language;
use camel_language_rhai::RhaiLanguage;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Register RhaiLanguage manually — it is not included by default
    // to keep the Rhai dependency optional.
    ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
        .expect("rhai not yet registered");

    // --- Build language predicates / expressions up-front ---

    let lang = RhaiLanguage::new();

    // Expression 1: prepend "[URGENT] " when priority header is "high"
    let enrich_expr = lang
        .create_expression(
            r#"
            if header("priority") == "high" {
                "[URGENT] " + body
            } else {
                body
            }
            "#,
        )
        .expect("valid expression");

    // Expression 2: in-script mutation — compute a tax string using
    // set_header to store an intermediate value, then read it back.
    let tax_expr = lang
        .create_expression(
            r#"
            let tax = header("amount") * 0.1;
            set_header("tax", tax);
            "tax=" + header("tax")
            "#,
        )
        .expect("valid expression");

    // Predicate: only let through messages where amount > 100
    let high_value_pred = lang
        .create_predicate(r#"header("amount") > 100"#)
        .expect("valid predicate");

    let enrich_expr = Arc::new(enrich_expr);
    let tax_expr = Arc::new(tax_expr);
    let high_value_pred = Arc::new(high_value_pred);

    // --- Build route ---

    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    let priorities = ["high", "normal", "high", "normal", "high", "normal"];
    let amounts = [200_i64, 50, 150, 30, 300, 80];

    let route = RouteBuilder::from("timer:tick?period=900&repeatCount=6")
        .route_id("language-rhai-demo")
        // Step 1: assign headers and body
        .process(move |mut exchange: camel_api::Exchange| {
            let c = Arc::clone(&counter_clone);
            Box::pin(async move {
                let n = c.fetch_add(1, Ordering::SeqCst) as usize;
                let priority = priorities[n % priorities.len()];
                let amount = amounts[n % amounts.len()];
                exchange
                    .input
                    .set_header("priority", camel_api::Value::String(priority.to_string()));
                exchange
                    .input
                    .set_header("amount", camel_api::Value::from(amount));
                exchange.input.body = Body::Text(format!("order #{}", n + 1));
                Ok(exchange)
            })
        })
        // Step 2: Rhai expression enriches the body based on priority
        .process({
            let expr = Arc::clone(&enrich_expr);
            move |mut exchange: camel_api::Exchange| {
                let expr = Arc::clone(&expr);
                Box::pin(async move {
                    if let Ok(camel_api::Value::String(enriched)) = expr.evaluate(&exchange) {
                        exchange.input.body = Body::Text(enriched);
                    }
                    Ok(exchange)
                })
            }
        })
        // Step 3: Rhai in-script mutation — compute tax and append to body
        .process({
            let expr = Arc::clone(&tax_expr);
            move |mut exchange: camel_api::Exchange| {
                let expr = Arc::clone(&expr);
                Box::pin(async move {
                    if let Ok(camel_api::Value::String(tax_info)) = expr.evaluate(&exchange) {
                        let body = exchange.input.body.as_text().unwrap_or("").to_string();
                        exchange.input.body = Body::Text(format!("{body} ({tax_info})"));
                    }
                    Ok(exchange)
                })
            }
        })
        // Step 4: .script() — mutating Rhai expression tags the order and
        //         appends a status suffix. Changes propagate back to the Exchange.
        .script(
            "rhai",
            r#"
            headers["processed"] = true;
            let status = if headers["priority"] == "high" { "PRIORITY" } else { "STANDARD" };
            body = body + " [" + status + "]";
            "#,
        )
        // Step 5: log every message (after enrichment)
        .to("log:all-orders?showBody=true&showHeaders=true")
        // Step 6: filter — only high-value orders (amount > 100) to alert log
        .filter({
            let pred = Arc::clone(&high_value_pred);
            move |ex: &camel_api::Exchange| pred.matches(ex).unwrap_or(false)
        })
        .to("log:high-value-alert?showBody=true")
        .end_filter()
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Rhai Language example running.");
    println!("Producing 6 orders with alternating priority/amount headers.");
    println!("- .script() step tags each order with 'processed' header and priority suffix");
    println!("- All messages logged to 'all-orders'");
    println!("- Only amount > 100 logged to 'high-value-alert'");
    println!("Press Ctrl+C to stop early...\n");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    println!("\nStopped.");
    Ok(())
}
