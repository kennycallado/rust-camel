use std::future::Future;
use std::time::{Duration, Instant};

#[allow(dead_code)]
pub async fn wait_until<F, Fut>(
    label: &str,
    timeout: Duration,
    poll: Duration,
    mut cond: F,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool, String>>,
{
    let deadline = Instant::now() + timeout;
    let mut last_error: Option<String> = None;

    loop {
        // Bound cond() by the remaining time until the deadline. Without this,
        // a condition that blocks indefinitely (e.g. an HTTP request to a server
        // whose kernel has accepted the TCP connection via the listen backlog but
        // whose app never produces a response) would hang forever inside
        // `cond().await`, making the deadline check below unreachable. This is the
        // difference between a test that fails fast and one that hangs the whole
        // suite until manually cancelled.
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, cond()).await {
            Err(_elapsed) => {
                return Err(format!(
                    "timeout waiting for {label} after {:?} (condition did not complete \
                     in time — likely a hanging probe, e.g. a server that accepted the \
                     TCP connection but never responded). Last error: {}",
                    timeout,
                    last_error.unwrap_or_else(|| "<none>".to_string())
                ));
            }
            Ok(Ok(true)) => return Ok(()),
            Ok(Ok(false)) => {}
            Ok(Err(err)) => last_error = Some(err),
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "timeout waiting for {label} after {:?}. Last error: {}",
                timeout,
                last_error.unwrap_or_else(|| "<none>".to_string())
            ));
        }

        tokio::time::sleep(poll).await;
    }
}
