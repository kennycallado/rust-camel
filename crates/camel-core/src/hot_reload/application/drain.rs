use std::time::{Duration, Instant};

use camel_api::CamelError;
use tracing::{debug, info, warn};

use crate::context::RuntimeExecutionHandle;

const DRAIN_POLL_INTERVAL_MS: u64 = 50;

pub(crate) enum DrainResult {
    Drained { waited_ms: u64 },
    Timeout {
        waited_ms: u64,
        timeout_ms: u64,
        remaining: u64,
    },
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
            return DrainResult::Drained { waited_ms: 0 };
        }
        Ok(n) => n,
        Err(CamelError::RouteError(_)) => {
            debug!(
                route_id = %route_id,
                action = %action,
                "hot-reload: route not found during drain, skipping"
            );
            return DrainResult::Drained { waited_ms: 0 };
        }
        Err(e) => {
            debug!(
                route_id = %route_id,
                action = %action,
                error = %e,
                "hot-reload: error querying in-flight count, skipping drain"
            );
            return DrainResult::Drained { waited_ms: 0 };
        }
    };

    info!(
        route_id = %route_id,
        action = %action,
        in_flight = initial,
        "hot-reload: consumer stopped, draining pipeline"
    );

    let start = Instant::now();
    let mut last_count = initial;

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
                return DrainResult::Drained { waited_ms: waited };
            }
            Ok(n) => {
                last_count = n;
            }
            Err(_) => {
                let waited = start.elapsed().as_millis() as u64;
                debug!(
                    route_id = %route_id,
                    action = %action,
                    "hot-reload: route not found during drain, skipping"
                );
                return DrainResult::Drained { waited_ms: waited };
            }
        }

        if start.elapsed() >= timeout {
            let waited = start.elapsed().as_millis() as u64;
            warn!(
                route_id = %route_id,
                action = %action,
                waited_ms = waited,
                timeout_ms = timeout_ms,
                remaining = last_count,
                "hot-reload: drain timeout expired, forcing stop"
            );
            return DrainResult::Timeout {
                waited_ms: waited,
                timeout_ms,
                remaining: last_count,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_result_variants_exist() {
        let _ = DrainResult::Drained { waited_ms: 0 };
        let _ = DrainResult::Timeout {
            waited_ms: 100,
            timeout_ms: 200,
            remaining: 5,
        };
    }

    #[test]
    fn poll_interval_is_50ms() {
        assert_eq!(DRAIN_POLL_INTERVAL_MS, 50);
    }

    #[tokio::test]
    #[ignore = "RuntimeExecutionHandle::new_for_test is not available; covered by integration tests in Task 6"]
    async fn drain_returns_immediately_when_zero_in_flight() {
        panic!("requires RuntimeExecutionHandle::new_for_test");
    }

    #[tokio::test]
    #[ignore = "RuntimeExecutionHandle::new_for_test is not available; covered by integration tests in Task 6"]
    async fn drain_returns_drained_for_nonexistent_route_regardless_of_timeout() {
        panic!("requires RuntimeExecutionHandle::new_for_test");
    }
}
