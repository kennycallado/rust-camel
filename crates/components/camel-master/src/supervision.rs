use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_component_api::{Consumer, ConsumerContext, is_retryable_camel_error};
use tokio::time::{interval, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use crate::consumer::{DelegateState, MasterConsumer};
use crate::leadership::{
    ReconcileContext, emit_leadership_transition, reconcile_event, stop_delegate,
};

const DELEGATE_RETRY_INTERVAL: Duration = Duration::from_millis(200);

/// Log a failed `reconcile_event` dispatch and classify it: returns
/// `true` when the error is permanent and the caller must fail fast
/// (`return Err`), `false` when it is transient and the caller should
/// continue — the next retry tick re-attempts. Shared by the two
/// synthetic `StartedLeading` dispatch sites in the retry-tick arm.
fn log_reconcile_failure(lock_name: &str, err: &CamelError) -> bool {
    if is_retryable_camel_error(err) {
        // log-policy: system-broken
        error!(
            lock = %lock_name,
            error = %err,
            "master delegate reconcile transient error, will retry"
        );
        // Don't return — let the next tick attempt retry.
        false
    } else {
        // log-policy: system-broken
        error!(
            lock = %lock_name,
            error = %err,
            "master delegate permanent error, terminating"
        );
        true
    }
}

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
        let route_id = context.route_id().to_string();
        let drain_timeout = self.drain_timeout;
        let reconnect = self.reconnect.clone();
        let runtime = Arc::clone(&self.runtime);
        let mut events = handle.events.clone();

        let stop_token = CancellationToken::new();
        let stop_token_loop = stop_token.clone();
        let leadership_handle = handle;
        let leader_epoch = leadership_handle.leader_epoch_arc();

        let task = tokio::spawn(async move {
            let mut state = DelegateState::Inactive;
            let mut is_leading = false;
            let mut retry_tick = interval(DELEGATE_RETRY_INTERVAL);

            let rctx = ReconcileContext {
                lock_name: &lock_name,
                delegate_component: &delegate_component,
                delegate_uri: &delegate_uri,
                route_id,
                sender: &sender,
                parent_cancel: &parent_cancel,
                drain_timeout,
                metrics: &metrics,
                platform_service: &platform_service,
                runtime: Arc::clone(&runtime),
                leader_epoch: Arc::clone(&leader_epoch),
                attempts: AtomicU32::new(0),
                reconnect,
            };

            let initial_event = { events.borrow().clone() };
            if let Some(initial_event) = initial_event {
                is_leading = matches!(&initial_event, camel_api::LeadershipEvent::StartedLeading);
                if is_leading {
                    rctx.attempts.swap(0, Ordering::Relaxed);
                    emit_leadership_transition(
                        rctx.metrics,
                        rctx.lock_name,
                        rctx.route_id.as_str(),
                        "acquired",
                    );
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
                                rctx.attempts.swap(0, Ordering::Relaxed);
                                emit_leadership_transition(
                                    rctx.metrics,
                                    rctx.lock_name,
                                    rctx.route_id.as_str(),
                                    "acquired",
                                );
                            } else if was_leading && !is_leading {
                                emit_leadership_transition(
                                    rctx.metrics,
                                    rctx.lock_name,
                                    rctx.route_id.as_str(),
                                    "lost",
                                );
                            }
                            if let Err(err) = reconcile_event(event, &mut state, &rctx).await {
                                // log-policy: system-broken
                                error!(lock = %lock_name, "master delegate error: {err}");
                                return Err(err);
                            }
                        }
                    }
                    _ = retry_tick.tick() => {
                        // Set when the stale-stamp branch dispatches below:
                        // that dispatch already drained and attempted a
                        // create this tick, so the acquisition branch must
                        // not dispatch a second, delay-free create in the
                        // same tick — the next tick retries with backoff.
                        let mut dispatched = false;
                        // Tick-driven stale-stamp detection (design §1):
                        // the renewal path clamp-adopts higher out-of-band
                        // lease terms into the published epoch WITHOUT
                        // emitting a watch event, so an Active delegate's
                        // stamp can go stale with no delivery to correct
                        // it. Dispatch a synthetic StartedLeading when the
                        // stamp differs from the published epoch; the
                        // guard's term-bump path resets the budget, drains,
                        // recreates, and restamps — subsuming the
                        // finished-handle teardown below for this tick.
                        // Ordering matters: a dead Active delegate at an
                        // exhausted budget must hit THIS dispatch first,
                        // otherwise the teardown erases the stale stamp and
                        // the Inactive acquisition consult stops the
                        // consumer on the stale budget instead of resetting
                        // it. The dispatch condition is stamp ≠ published
                        // only — never "epoch changed recently".
                        // One Acquire load per tick: the same snapshot is
                        // tested and logged, so the log cannot disagree
                        // with the condition on an advance between loads.
                        let published = rctx.leader_epoch.load(Ordering::Acquire);
                        if is_leading
                            && let DelegateState::Active { epoch, .. } = &state
                            && *epoch != published
                        {
                            dispatched = true;
                            debug!(
                                lock = %lock_name,
                                stamp = *epoch,
                                published,
                                "retry tick detected stale delegate stamp, redispatching StartedLeading"
                            );
                            if let Err(err) = reconcile_event(
                                camel_api::LeadershipEvent::StartedLeading,
                                &mut state,
                                &rctx,
                            )
                            .await
                                && log_reconcile_failure(&lock_name, &err)
                            {
                                return Err(err);
                            }
                        }

                        if matches!(&state, DelegateState::Active { handle, .. } if handle.is_finished())
                            && let Err(err) = stop_delegate(
                                &mut state,
                                drain_timeout,
                                rctx.lock_name,
                                rctx.route_id.as_str(),
                                rctx.metrics,
                            )
                            .await
                        {
                            // log-policy: system-broken
                            error!(lock = %lock_name, "master delegate task failed: {err}");
                            return Err(err);
                        }

                        // `dispatched` is set when the stale-stamp branch
                        // already dispatched this tick: re-dispatching the
                        // acquisition here would perform a second create in
                        // the same tick, delay-free (the acquisition branch
                        // applies its backoff only on subsequent ticks).
                        if !dispatched && is_leading && matches!(state, DelegateState::Inactive) {
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
                            // rctx.attempts counts create deliveries within
                            // the acquisition epoch; reconcile_event itself
                            // consults and increments it (design §2).
                            let attempts = rctx.attempts.load(Ordering::Relaxed);
                            if !rctx.reconnect.should_retry(attempts) {
                                warn!(
                                    lock = %lock_name,
                                    attempts,
                                    "delegate start exceeded max attempts or reconnect disabled, stopping consumer"
                                );
                                break;
                            }
                            // Apply backoff delay for retries (skip first
                            // attempt). The gate reads the same post-snapshot
                            // count the old local counter carried, so the
                            // schedule is unchanged: first retry delay-free.
                            if attempts > 1 {
                                let delay = rctx.reconnect.delay_for(attempts - 2);
                                if delay > DELEGATE_RETRY_INTERVAL {
                                    tokio::select! {
                                        _ = stop_token_loop.cancelled() => break,
                                        _ = tokio::time::sleep(delay.saturating_sub(DELEGATE_RETRY_INTERVAL)) => {}
                                    }
                                }
                            }
                            if let Err(err) = reconcile_event(
                                camel_api::LeadershipEvent::StartedLeading,
                                &mut state,
                                &rctx,
                            )
                            .await
                                && log_reconcile_failure(&lock_name, &err)
                            {
                                return Err(err);
                            }
                        }
                    }
                }
            }

            stop_delegate(
                &mut state,
                drain_timeout,
                rctx.lock_name,
                rctx.route_id.as_str(),
                rctx.metrics,
            )
            .await?;
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
