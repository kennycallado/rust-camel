//! # doTry / doCatch / doFinally EIP — Complete Tour
//!
//! Demonstrates three patterns:
//! 1. Catch by variant name with Handled disposition
//! 2. Catch by predicate with Handled disposition
//! 3. doFinally cleanup with Propagate disposition

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, FilterPredicate};
use camel_builder::RouteBuilder;
use camel_component_direct::DirectComponent;
use camel_core::context::CamelContext;

/// A processor that always fails with a ProcessorError.
fn always_fail(reason: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |_ex| {
        Box::pin(async move { Err(CamelError::ProcessorError(reason.into())) })
    })
}

/// A pass-through processor that logs a message via tracing.
fn log_marker(label: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |ex| {
        Box::pin(async move {
            tracing::info!(label, "doTry example step reached");
            Ok(ex)
        })
    })
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap

    ctx.register_component(DirectComponent::new());

    let cleanup_counter = Arc::new(AtomicU32::new(0));

    // -----------------------------------------------------------------------
    // Route 1: Catch by variant name, Handled disposition.
    //
    // The do_try block runs a processor that always fails. The do_catch clause
    // matches the error by its variant name ("ProcessorError") and marks the
    // error as handled — the exchange is not re-thrown.
    // -----------------------------------------------------------------------
    let route1 = RouteBuilder::from("direct:catch-by-variant")
        .route_id("catch-by-variant")
        .do_try()
        .process(always_fail("boom-from-route-1"))
        .do_catch_exception(&["ProcessorError"])
        .handled()
        .process(log_marker("log:caught-by-variant"))
        .end_do_catch()
        .end_do_try()
        .build()?;
    ctx.add_route_definition(route1).await?;

    // -----------------------------------------------------------------------
    // Route 2: Catch by predicate, Handled disposition.
    //
    // Uses a user-supplied predicate that inspects the exchange's current
    // error state and returns true when the variant name matches. This
    // demonstrates the more flexible predicate-based matching.
    // -----------------------------------------------------------------------
    let route2 = RouteBuilder::from("direct:catch-by-predicate")
        .route_id("catch-by-predicate")
        .do_try()
        .process(always_fail("boom-from-route-2"))
        .do_catch_when({
            FilterPredicate::new(|ex: &Exchange| {
                ex.error
                    .as_ref()
                    .is_some_and(|e| e.variant_name() == "ProcessorError")
            })
        })
        .process(log_marker("log:caught-by-predicate"))
        .end_do_catch()
        .end_do_try()
        .build()?;
    ctx.add_route_definition(route2).await?;

    // -----------------------------------------------------------------------
    // Route 3: doFinally cleanup with Propagate disposition.
    //
    // The catch clause uses Propagate — it logs for side-effects but the
    // original error is re-thrown after the catch block. The doFinally
    // block always runs (incrementing a counter), even when the error
    // propagates.
    // -----------------------------------------------------------------------
    let cleanup_clone = cleanup_counter.clone();
    let route3 = RouteBuilder::from("direct:finally")
        .route_id("finally-route")
        .do_try()
        .process(always_fail("boom-from-route-3"))
        .do_catch_exception(&["ProcessorError"])
        .propagate()
        .process(log_marker("log:caught-with-finally"))
        .end_do_catch()
        .do_finally()
        .process(BoxProcessor::from_fn(move |ex| {
            let c = cleanup_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ex)
            })
        }))
        .end_do_finally()
        .end_do_try()
        .build()?;
    ctx.add_route_definition(route3).await?;

    ctx.start().await?;

    tracing::info!(
        cleanup_count = cleanup_counter.load(Ordering::SeqCst),
        "compile-time demo: no producer sends, so cleanup_count is 0; routes are registered but not triggered"
    );

    ctx.stop().await?;
    Ok(())
}
