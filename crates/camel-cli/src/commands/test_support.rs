//! Test helpers shared by the `run` and scenario test modules.
//!
//! Feature-gated at the `mod` declaration in `commands/mod.rs` to the
//! union of the caller features (`security` for `run_tests.rs`,
//! `integration-http` for the scenario tests) so minimal-feature test
//! builds see no dead code.

/// Deliver an exchange to a `direct:` endpoint exactly as the scenario
/// harness `DirectStimulus` does (crates/camel-integration-test
/// adapters.rs): a fresh endpoint + producer per send through the
/// component registry, one `oneshot` per exchange, retrying the
/// consumer-startup race (non-gate `EndpointCreationFailed`; the SEDA
/// no-active-consumers gate is excluded) on a bounded deadline.
pub(crate) async fn direct_oneshot(
    ctx: &camel_core::CamelContext,
    uri: &str,
    exchange: camel_api::Exchange,
) -> Result<camel_api::Exchange, camel_api::CamelError> {
    use tower::ServiceExt;

    const RETRY_SLEEP: std::time::Duration = std::time::Duration::from_millis(20);
    const RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(1);

    let deadline = tokio::time::Instant::now() + RETRY_DEADLINE;
    loop {
        let producer_ctx = ctx.producer_context();
        let component = ctx
            .registry()
            .get("direct")
            .expect("direct component registered by the bundle cascade"); // allow-unwrap
        let endpoint = component
            .create_endpoint(uri, ctx)
            .expect("direct endpoint creation must succeed"); // allow-unwrap
        let producer = endpoint
            .create_producer(
                std::sync::Arc::new(camel_component_api::NoOpComponentContext),
                &producer_ctx,
            )
            .expect("direct producer creation must succeed"); // allow-unwrap
        match producer.oneshot(exchange.clone()).await {
            Ok(reply) => return Ok(reply),
            Err(e) => {
                // Retry only the direct startup race via the shared
                // predicate; the seda gate (rc-tgaxf) and the seda
                // terminal-config rejections — the multipleConsumers+wait
                // conflict (rc-rif19) and the endpoint config conflict
                // (rc-zovuy) — all fail fast.
                let is_startup_race = camel_component_seda::is_direct_startup_race(&e);
                if is_startup_race && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(RETRY_SLEEP).await;
                    continue;
                }
                return Err(e);
            }
        }
    }
}
