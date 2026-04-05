use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::CamelError;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_language_api::Language;
use camel_language_xpath::XPathLanguage;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    ctx.register_language("xpath", Box::new(XPathLanguage))
        .expect("xpath not yet registered");

    let lang = XPathLanguage;

    let title_expr = lang
        .create_expression("/catalog/book[1]/title")
        .expect("valid expression");

    let in_stock_pred = lang
        .create_predicate("/catalog/book[@in-stock='true']")
        .expect("valid predicate");

    let title_expr = Arc::new(title_expr);
    let in_stock_pred = Arc::new(in_stock_pred);

    let counter = Arc::new(AtomicU64::new(0));
    let counter_clone = Arc::clone(&counter);

    let books = [
        ("The Rust Book", "true"),
        ("Programming Rust", "false"),
        ("Rust in Action", "true"),
    ];

    let route = RouteBuilder::from("timer:tick?period=800&repeatCount=6")
        .route_id("language-xpath-demo")
        .process(move |mut exchange: camel_api::Exchange| {
            let c = Arc::clone(&counter_clone);
            Box::pin(async move {
                let n = c.fetch_add(1, Ordering::SeqCst) as usize;
                let (title, in_stock) = books[n % books.len()];
                let xml = format!(
                    "<catalog><book id=\"{}\" in-stock=\"{}\"><title>{}</title></book></catalog>",
                    n + 1,
                    in_stock,
                    title
                );
                exchange.input.body = Body::Xml(xml);
                Ok(exchange)
            })
        })
        .process({
            let expr = Arc::clone(&title_expr);
            move |mut exchange: camel_api::Exchange| {
                let expr = Arc::clone(&expr);
                Box::pin(async move {
                    if let Ok(value) = expr.evaluate(&exchange)
                        && let Some(name) = value.as_str()
                    {
                        exchange
                            .input
                            .set_header("book-title", camel_api::Value::String(name.to_string()));
                    }
                    Ok(exchange)
                })
            }
        })
        .to("log:all-books?showBody=true&showHeaders=true")
        .filter({
            let pred = Arc::clone(&in_stock_pred);
            move |ex: &camel_api::Exchange| pred.matches(ex).unwrap_or(false)
        })
        .to("log:in-stock?showBody=true&showHeaders=true")
        .end_filter()
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("XPath Language example running.");
    println!("Producing 6 book catalog entries cycling through 3 titles.");
    println!("- /catalog/book[1]/title expression extracts the book title");
    println!("- /catalog/book[@in-stock='true'] predicate filters to in-stock only");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    println!("\nStopped.");
    Ok(())
}
