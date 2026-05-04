pub mod wait;

#[cfg(feature = "integration-tests")]
use camel_api::Exchange;

#[cfg(feature = "integration-tests")]
use camel_component_api::CamelError;

#[cfg(feature = "integration-tests")]
use camel_test::CamelTestContext;

#[cfg(feature = "integration-tests")]
use tower::ServiceExt;

#[cfg(feature = "integration-tests")]
pub mod activemq;

#[cfg(feature = "integration-tests")]
pub mod artemis;

#[cfg(feature = "integration-tests")]
pub mod jms;

#[cfg(feature = "integration-tests")]
pub mod cxf;

#[cfg(feature = "integration-tests")]
pub mod xml_bridge;

#[cfg(feature = "integration-tests")]
#[allow(dead_code)]
pub fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,camel=info")),
        )
        .with_test_writer()
        .try_init();
}

#[cfg(feature = "integration-tests")]
#[allow(dead_code)]
pub async fn send_to_direct(
    h: &CamelTestContext,
    endpoint_uri: &str,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let producer = {
        let ctx = h.ctx().lock().await;
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered");
        let endpoint = component
            .create_endpoint(endpoint_uri, &*ctx)
            .expect("failed to create direct endpoint");
        endpoint
            .create_producer(&producer_ctx)
            .expect("failed to create direct producer")
    };

    producer.oneshot(exchange).await
}
