pub mod wait;

#[cfg(feature = "integration-tests")]
use camel_api::Exchange;

/// Returns a handle to a dedicated background Tokio runtime used for all
/// bridge pool initialisation. The runtime lives for the entire test-process
/// lifetime so that tonic Channel dispatch tasks and health monitors are
/// never killed when an individual test runtime is dropped between tests.
///
/// # How it works
/// Each `#[tokio::test(flavor = "multi_thread")]` creates its own runtime that
/// is dropped when the test finishes. Any `tokio::spawn` (including the tonic
/// `Channel` internal dispatch task) runs on the *current* runtime at the call
/// site. If a shared pool's slot is created on test T1's runtime, the channel
/// dispatch task dies with that runtime, and test T2 receives `DispatchGone`.
///
/// By running `get_or_create_slot` inside `bridge_bg_rt().spawn(...)`, all
/// tasks that the pool spawns (dispatch tasks, health monitors) end up on this
/// permanent runtime instead.
#[cfg(feature = "integration-tests")]
pub fn bridge_bg_rt() -> &'static tokio::runtime::Handle {
    use std::sync::OnceLock;
    static HANDLE: OnceLock<tokio::runtime::Handle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("bridge-bg-rt")
            .build()
            .expect("bridge background runtime");
        let handle = rt.handle().clone();
        // Intentionally leak the Runtime so its scheduler (and all tasks
        // spawned on it) live for the entire test-process lifetime.
        std::mem::forget(rt);
        handle
    })
}

#[cfg(feature = "integration-tests")]
use camel_component_api::CamelError;

pub fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(camel_component_api::NoOpComponentContext)
}

#[cfg(feature = "integration-tests")]
use camel_test::CamelTestContext;

#[cfg(feature = "integration-tests")]
use tower::ServiceExt;

#[cfg(feature = "integration-tests")]
pub mod activemq;

#[cfg(feature = "integration-tests")]
pub mod mosquitto;

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
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(feature = "integration-tests")]
fn is_direct_not_registered(err: &CamelError) -> bool {
    matches!(
        err,
        CamelError::EndpointCreationFailed(msg)
            if msg.starts_with("direct endpoint '") && msg.ends_with("' not registered")
    )
}

#[cfg(feature = "integration-tests")]
async fn retry_direct_not_registered<F, Fut>(
    timeout: std::time::Duration,
    poll: std::time::Duration,
    mut op: F,
) -> Result<Exchange, CamelError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Exchange, CamelError>>,
{
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        match op().await {
            Ok(exchange) => return Ok(exchange),
            Err(err)
                if is_direct_not_registered(&err) && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(poll).await;
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(all(test, feature = "integration-tests"))]
mod tests {
    use super::*;

    #[test]
    fn direct_not_registered_matcher_is_specific() {
        assert!(is_direct_not_registered(
            &CamelError::EndpointCreationFailed(
                "direct endpoint 'start_a' not registered".to_string(),
            )
        ));
        assert!(!is_direct_not_registered(
            &CamelError::EndpointCreationFailed(
                "direct endpoint 'start_a' channel closed".to_string(),
            )
        ));
        assert!(!is_direct_not_registered(&CamelError::ProcessorError(
            "direct endpoint 'start_a' not registered".to_string(),
        )));
    }

    #[tokio::test]
    async fn direct_not_registered_retry_eventually_succeeds() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let attempts = Arc::new(Mutex::new(0_u8));
        let attempts_for_op = attempts.clone();
        let exchange = Exchange::default();

        let result = retry_direct_not_registered(
            Duration::from_millis(50),
            Duration::from_millis(1),
            || {
                let attempts_for_op = attempts_for_op.clone();
                let exchange = exchange.clone();
                async move {
                    let mut guard = attempts_for_op.lock().expect("attempts lock");
                    *guard += 1;
                    if *guard < 3 {
                        Err(CamelError::EndpointCreationFailed(
                            "direct endpoint 'start_a' not registered".to_string(),
                        ))
                    } else {
                        Ok(exchange)
                    }
                }
            },
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(*attempts.lock().expect("attempts lock"), 3);
    }
}

#[cfg(feature = "integration-tests")]
#[allow(dead_code)]
pub async fn send_to_direct(
    h: &CamelTestContext,
    endpoint_uri: &str,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    retry_direct_not_registered(
        std::time::Duration::from_secs(2),
        std::time::Duration::from_millis(5),
        || {
            let exchange = exchange.clone();
            async move {
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
                        .create_producer(test_rt(), &producer_ctx)
                        .expect("failed to create direct producer")
                };

                producer.oneshot(exchange).await
            }
        },
    )
    .await
}
