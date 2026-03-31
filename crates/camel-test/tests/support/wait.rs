use std::future::Future;
use std::time::{Duration, Instant};

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
        match cond().await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(err) => last_error = Some(err),
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
