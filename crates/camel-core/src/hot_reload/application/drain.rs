use std::time::{Duration, Instant};

use camel_api::CamelError;
use tracing::{debug, info, warn};

use crate::context::RuntimeExecutionHandle;

const DRAIN_POLL_INTERVAL_MS: u64 = 50;

pub(crate) enum DrainResult {
    Drained,
    Timeout,
}

pub(crate) async fn drain_route(
    route_id: &str,
    action: &str,
    controller: &RuntimeExecutionHandle,
    timeout: Duration,
) -> DrainResult {
    let timeout_ms = timeout.as_millis() as u64;

    let initial = match controller.in_flight_count(route_id).await {
        Ok(0) => {
            debug!(
                route_id = %route_id,
                action = %action,
                "hot-reload: no in-flight exchanges, proceeding"
            );
            return DrainResult::Drained;
        }
        Ok(n) => n,
        Err(CamelError::RouteError(_)) => {
            debug!(
                route_id = %route_id,
                action = %action,
                "hot-reload: route not found during drain, skipping"
            );
            return DrainResult::Drained;
        }
        Err(e) => {
            debug!(
                route_id = %route_id,
                action = %action,
                error = %e,
                "hot-reload: error querying in-flight count, skipping drain"
            );
            return DrainResult::Drained;
        }
    };

    info!(
        route_id = %route_id,
        action = %action,
        in_flight = initial,
        "hot-reload: consumer stopped, draining pipeline"
    );

    let start = Instant::now();

    loop {
        tokio::time::sleep(Duration::from_millis(DRAIN_POLL_INTERVAL_MS)).await;

        match controller.in_flight_count(route_id).await {
            Ok(0) => {
                let waited = start.elapsed().as_millis() as u64;
                info!(
                    route_id = %route_id,
                    action = %action,
                    waited_ms = waited,
                    in_flight = 0,
                    "hot-reload: route drained"
                );
                return DrainResult::Drained;
            }
            Ok(n) => {
                if start.elapsed() >= timeout {
                    let waited = start.elapsed().as_millis() as u64;
                    warn!(
                        route_id = %route_id,
                        action = %action,
                        waited_ms = waited,
                        timeout_ms = timeout_ms,
                        remaining = n,
                        "hot-reload: drain timeout expired, forcing stop"
                    );
                    return DrainResult::Timeout;
                }
            }
            Err(_) => {
                debug!(
                    route_id = %route_id,
                    action = %action,
                    "hot-reload: route not found during drain, skipping"
                );
                return DrainResult::Drained;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_result_variants_exist() {
        let _ = DrainResult::Drained;
        let _ = DrainResult::Timeout;
    }

    #[test]
    fn poll_interval_is_50ms() {
        assert_eq!(DRAIN_POLL_INTERVAL_MS, 50);
    }

    #[tokio::test]
    async fn drain_returns_immediately_when_zero_in_flight() {
        use crate::CamelContext;

        let ctx = CamelContext::builder().build().await.unwrap();
        let result = drain_route(
            "missing-route",
            "remove",
            &ctx.runtime_execution_handle(),
            Duration::from_millis(1),
        )
        .await;
        assert!(matches!(result, DrainResult::Drained));
    }

    #[tokio::test]
    async fn drain_returns_drained_for_nonexistent_route_regardless_of_timeout() {
        use crate::CamelContext;

        let ctx = CamelContext::builder().build().await.unwrap();
        let result = drain_route(
            "also-missing-route",
            "restart",
            &ctx.runtime_execution_handle(),
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(result, DrainResult::Drained));
    }
}
