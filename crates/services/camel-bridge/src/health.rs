use std::time::Duration;
use tonic::transport::Channel;
use tracing::debug;

use crate::process::BridgeError;

/// Per-probe timeout: if a single health RPC hangs (e.g. Netty blocking under
/// mandatory auth in GraalVM native), we cancel it and retry rather than
/// freezing the whole wait_for_health loop.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Poll a gRPC channel with a Health RPC until healthy=true or timeout expires.
/// The health check is generic: caller passes a closure that performs the RPC.
/// Retry interval: 100ms.
pub async fn wait_for_health<F, Fut>(
    channel: &Channel,
    timeout: Duration,
    check: F,
) -> Result<(), BridgeError>
where
    F: Fn(Channel) -> Fut,
    Fut: std::future::Future<Output = Result<bool, tonic::Status>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let probe = tokio::time::timeout(PROBE_TIMEOUT, check(channel.clone())).await;
        match probe {
            Ok(Ok(true)) => return Ok(()),
            Ok(Ok(false)) => debug!("bridge health check: not ready yet"),
            Ok(Err(e)) => debug!("bridge health check error: {e}"),
            Err(_) => {
                debug!("bridge health check probe timed out after {PROBE_TIMEOUT:?}, retrying")
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(BridgeError::Timeout(format!(
                "JMS bridge health check timed out after {}ms",
                timeout.as_millis()
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::transport::Endpoint;

    #[tokio::test]
    async fn wait_for_health_timeout_on_unreachable_channel() {
        let channel = Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
        let result = wait_for_health(&channel, Duration::from_millis(200), |_ch| async {
            Ok(false)
        })
        .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            BridgeError::Timeout(msg) => {
                assert!(msg.contains("timed out"));
            }
            other => panic!("expected Timeout error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_for_health_returns_error_when_check_always_fails() {
        let channel = Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
        let result = wait_for_health(&channel, Duration::from_millis(150), |_ch| async {
            Err(tonic::Status::unavailable("service down"))
        })
        .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            BridgeError::Timeout(_) => {}
            other => panic!("expected Timeout error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_for_health_returns_ok_when_check_succeeds_immediately() {
        let channel = Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
        let result =
            wait_for_health(&channel, Duration::from_secs(5), |_ch| async { Ok(true) }).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_for_health_retries_until_success() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let counter = std::sync::Arc::new(AtomicU32::new(0));

        let channel = Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
        let counter_clone = counter.clone();
        let result = wait_for_health(&channel, Duration::from_secs(5), |_ch| {
            let c = counter_clone.clone();
            async move {
                let prev = c.fetch_add(1, Ordering::SeqCst);
                if prev < 2 { Ok(false) } else { Ok(true) }
            }
        })
        .await;
        assert!(result.is_ok());
        assert!(counter.load(Ordering::SeqCst) >= 3);
    }

    #[test]
    fn probe_timeout_constant_is_10_seconds() {
        assert_eq!(PROBE_TIMEOUT, Duration::from_secs(10));
    }
}
