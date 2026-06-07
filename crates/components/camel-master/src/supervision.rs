use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_component_api::{Consumer, ConsumerContext, is_retryable_camel_error};
use tokio::time::{interval, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

use crate::consumer::{DelegateState, MasterConsumer};
use crate::leadership::{ReconcileContext, reconcile_event, stop_delegate};

const DELEGATE_RETRY_INTERVAL: Duration = Duration::from_millis(200);

#[async_trait]
impl Consumer for MasterConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        if self.leadership_task.is_some() {
            return Ok(());
        }

        let handle = self
            .platform_service
            .leadership()
            .start(&self.lock_name)
            .await
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!("failed to start leader election: {e}"))
            })?;

        let lock_name = self.lock_name.clone();
        let delegate_uri = self.delegate_uri.clone();
        let delegate_component = Arc::clone(&self.delegate_component);
        let metrics = Arc::clone(&self.metrics);
        let platform_service = Arc::clone(&self.platform_service);
        let sender = context.sender();
        let parent_cancel = context.cancel_token();
        let drain_timeout = self.drain_timeout;
        let reconnect = self.reconnect.clone();
        let runtime = Arc::clone(&self.runtime);
        let mut events = handle.events.clone();

        let stop_token = CancellationToken::new();
        let stop_token_loop = stop_token.clone();
        let leadership_handle = handle;

        let task = tokio::spawn(async move {
            let mut state = DelegateState::Inactive;
            let mut is_leading = false;
            let mut delegate_attempts = 0u32;
            let mut retry_tick = interval(DELEGATE_RETRY_INTERVAL);

            let rctx = ReconcileContext {
                lock_name: &lock_name,
                delegate_component: &delegate_component,
                delegate_uri: &delegate_uri,
                sender: &sender,
                parent_cancel: &parent_cancel,
                drain_timeout,
                metrics: &metrics,
                platform_service: &platform_service,
                runtime: Arc::clone(&runtime),
            };

            let initial_event = { events.borrow().clone() };
            if let Some(initial_event) = initial_event {
                is_leading = matches!(&initial_event, camel_api::LeadershipEvent::StartedLeading);
                if is_leading {
                    delegate_attempts = 0;
                }
                if let Err(err) = reconcile_event(initial_event, &mut state, &rctx).await {
                    // log-policy: system-broken
                    error!(lock = %lock_name, "master delegate error: {err}");
                    return Err(err);
                }
            }

            loop {
                tokio::select! {
                    _ = stop_token_loop.cancelled() => {
                        break;
                    }
                    _ = context.cancelled() => {
                        break;
                    }
                    changed = events.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let event = { events.borrow().clone() };
                        if let Some(event) = event {
                            let was_leading = is_leading;
                            is_leading = matches!(&event, camel_api::LeadershipEvent::StartedLeading);
                            if !was_leading && is_leading {
                                delegate_attempts = 0;
                            }
                            if let Err(err) = reconcile_event(event, &mut state, &rctx).await {
                                // log-policy: system-broken
                                error!(lock = %lock_name, "master delegate error: {err}");
                                return Err(err);
                            }
                        }
                    }
                    _ = retry_tick.tick() => {
                        if matches!(&state, DelegateState::Active { handle, .. } if handle.is_finished())
                            && let Err(err) = stop_delegate(&mut state, drain_timeout).await
                        {
                            // log-policy: system-broken
                            error!(lock = %lock_name, "master delegate task failed: {err}");
                            return Err(err);
                        }

                        if is_leading && matches!(state, DelegateState::Inactive) {
                            // Manual retry loop (not retry_async) because:
                            // - The retry logic is embedded inside a periodic
                            //   retry_tick.tick() handler; the outer select! runs
                            //   every DELEGATE_RETRY_INTERVAL regardless, so the
                            //   delay is applied as an additive sleep on top of
                            //   the tick interval, not as a replacement for it.
                            // - reconcile_event() requires &mut state, and the
                            //   inter-attempt logic checks handle.is_finished()
                            //   before retrying — both require state access
                            //   between iterations that retry_async cannot provide.
                            // - Classifies errors (rc-i1z): permanent → fail-fast,
                            //   transient → retry with backoff.
                            // Use NetworkRetryPolicy for bounded retries.
                            // delegate_attempts tracks the next zero-based attempt index.
                            if !reconnect.should_retry(delegate_attempts) {
                                warn!(
                                    lock = %lock_name,
                                    attempts = delegate_attempts,
                                    "delegate start exceeded max attempts, stopping consumer"
                                );
                                break;
                            }
                            // Apply backoff delay for retries (skip first attempt).
                            if delegate_attempts > 0 {
                                let delay = reconnect.delay_for(delegate_attempts - 1);
                                if delay > DELEGATE_RETRY_INTERVAL {
                                    tokio::select! {
                                        _ = stop_token_loop.cancelled() => break,
                                        _ = tokio::time::sleep(delay.saturating_sub(DELEGATE_RETRY_INTERVAL)) => {}
                                    }
                                }
                            }
                            delegate_attempts = delegate_attempts.saturating_add(1);
                            if let Err(err) = reconcile_event(
                                camel_api::LeadershipEvent::StartedLeading,
                                &mut state,
                                &rctx,
                            )
                            .await {
                                if is_retryable_camel_error(&err) {
                                    // log-policy: system-broken
                                    error!(
                                        lock = %lock_name,
                                        error = %err,
                                        attempt = delegate_attempts,
                                        "master delegate transient error, will retry"
                                    );
                                    // Don't return — let the next tick attempt retry.
                                } else {
                                    // log-policy: system-broken
                                    error!(
                                        lock = %lock_name,
                                        error = %err,
                                        "master delegate permanent error, terminating"
                                    );
                                    return Err(err);
                                }
                            }
                        }
                    }
                }
            }

            stop_delegate(&mut state, drain_timeout).await?;
            let _ = timeout(drain_timeout, leadership_handle.step_down()).await;
            Ok::<(), CamelError>(())
        });

        self.stop_token = Some(stop_token);
        self.leadership_task = Some(task);

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(token) = self.stop_token.take() {
            token.cancel();
        }

        if let Some(handle) = self.leadership_task.take() {
            if handle.is_finished() {
                match timeout(self.drain_timeout, handle).await {
                    Ok(Ok(Ok(()))) => {}
                    Ok(Ok(Err(err))) => return Err(err),
                    Ok(Err(e)) => {
                        return Err(CamelError::ProcessorError(format!(
                            "leadership task join failed: {e}"
                        )));
                    }
                    Err(_) => {
                        return Err(CamelError::ProcessorError(
                            "leadership task join timed out".to_string(),
                        ));
                    }
                }
                return Ok(());
            }

            // Abort first so the task is guaranteed to stop; then await with
            // a timeout as a safety-net in case abort takes a moment to land.
            handle.abort();
            match timeout(self.drain_timeout, handle).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(err))) => return Err(err),
                Ok(Err(e)) if e.is_panic() => {
                    // log-policy: system-broken
                    error!(lock = %self.lock_name, error = %e, "leadership task panicked");
                }
                Ok(Err(e)) => {
                    warn!(lock = %self.lock_name, error = %e, "leadership task cancelled");
                }
                Err(_) => {
                    warn!("master leadership loop shutdown timed out after abort");
                }
            }
        }

        Ok(())
    }

    fn background_task_handle(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
        self.leadership_task.take()
    }
}
