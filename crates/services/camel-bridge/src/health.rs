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
            Err(_) => debug!("bridge health check probe timed out after {PROBE_TIMEOUT:?}, retrying"),
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
