use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use camel_api::{CamelError, MetricsCollector, PlatformService};
use camel_component_api::{Component, ConsumerContext, ExchangeEnvelope, is_retryable_camel_error};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::consumer::DelegateState;
use crate::endpoint::MasterDelegateContext;

/// Property key for the leader-epoch fencing token stamped on every
/// ExchangeEnvelope emitted by a `master:` route's delegate. Downstream sinks
/// SHOULD compare against their current leader's epoch and reject older
/// envelopes (split-brain safety). See ADR-0035.
pub(crate) const LEADER_EPOCH_PROPERTY: &str = "x-camel-leader-epoch";

/// Spawn an epoch-stamping bridge between a delegate consumer and the pipeline.
///
/// Returns `(stamp_tx, JoinHandle)`. The delegate sends `ExchangeEnvelope`s
/// to `stamp_tx`; the bridge stamps each with `x-camel-leader-epoch` =
/// `my_epoch` (a snapshot taken at spawn time, NOT a live read) and forwards
/// to `real_sender`.
///
/// On sender-drop (delegate stopped), the bridge drains remaining buffered
/// envelopes — all stamped with the same constant epoch — and exits.
/// On `parent_cancel` (route shutdown), the bridge aborts immediately.
///
/// The `JoinHandle` MUST be awaited by the caller (typically `stop_delegate`)
/// within `drain_timeout` — it is NOT detached. A detached bridge could
/// continue stamping and forwarding envelopes after the leader has yielded.
pub(crate) fn spawn_epoch_bridge(
    real_sender: tokio::sync::mpsc::Sender<ExchangeEnvelope>,
    my_epoch: u64,
    parent_cancel: CancellationToken,
) -> (
    tokio::sync::mpsc::Sender<ExchangeEnvelope>,
    tokio::task::JoinHandle<()>,
) {
    let (stamp_tx, mut stamp_rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(128);

    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = parent_cancel.cancelled() => break,
                env = stamp_rx.recv() => {
                    match env {
                        Some(mut env) => {
                            stamp_epoch(&mut env, my_epoch);
                            if real_sender.send(env).await.is_err() {
                                break;
                            }
                        }
                        None => {
                            // Sender dropped (delegate stopped) — drain remaining
                            // buffered envelopes with the same constant epoch.
                            while let Ok(mut env) = stamp_rx.try_recv() {
                                stamp_epoch(&mut env, my_epoch);
                                if real_sender.send(env).await.is_err() {
                                    break;
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
    });

    (stamp_tx, handle)
}

/// Stamp the leader-epoch fencing token on an envelope's properties.
fn stamp_epoch(env: &mut ExchangeEnvelope, epoch: u64) {
    env.exchange.properties.insert(
        LEADER_EPOCH_PROPERTY.to_string(),
        serde_json::Value::String(epoch.to_string()),
    );
}

pub(crate) async fn stop_delegate(
    state: &mut DelegateState,
    drain_timeout: Duration,
) -> Result<(), CamelError> {
    if let DelegateState::Active {
        run_token,
        mut handle,
        mut bridge_handle,
    } = std::mem::replace(state, DelegateState::Inactive)
    {
        run_token.cancel();
        match timeout(drain_timeout, &mut handle).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(err))) => {
                return Err(err);
            }
            Ok(Err(e)) if e.is_panic() => {
                // log-policy: system-broken
                error!(error = %e, "master delegate task panicked");
                return Err(CamelError::ProcessorError(format!(
                    "master delegate task panicked: {e}"
                )));
            }
            Ok(Err(e)) => {
                warn!(error = %e, "master delegate task cancelled");
                return Err(CamelError::ProcessorError(format!(
                    "master delegate task cancelled: {e}"
                )));
            }
            Err(_) => {
                warn!("master delegate shutdown timed out, aborting");
                handle.abort();
            }
        }

        // Drain the epoch-stamping bridge. It forwards any envelopes still
        // buffered after the delegate stopped, then exits. On timeout, abort
        // to prevent a detached task from sending stale exchanges.
        if let Some(mut bridge) = bridge_handle.take() {
            match timeout(drain_timeout, &mut bridge).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    warn!(error = %e, "epoch bridge task failed");
                }
                Err(_) => {
                    warn!("epoch bridge drain timed out, aborting");
                    bridge.abort();
                    let _ = bridge.await;
                }
            }
        }
    }
    Ok(())
}

pub(crate) struct ReconcileContext<'a> {
    pub(crate) lock_name: &'a str,
    pub(crate) delegate_component: &'a Arc<dyn Component>,
    pub(crate) delegate_uri: &'a str,
    pub(crate) route_id: String,
    pub(crate) sender: &'a tokio::sync::mpsc::Sender<ExchangeEnvelope>,
    pub(crate) parent_cancel: &'a CancellationToken,
    pub(crate) drain_timeout: Duration,
    pub(crate) metrics: &'a Arc<dyn MetricsCollector>,
    pub(crate) platform_service: &'a Arc<dyn PlatformService>,
    pub(crate) runtime: Arc<dyn camel_component_api::RuntimeObservability>,
    /// Monotonic fencing token, snapshotted by `reconcile_event` on the
    /// `StartedLeading` transition. See ADR-0035.
    pub(crate) leader_epoch: Arc<AtomicU64>,
}

pub(crate) async fn reconcile_event(
    event: camel_api::LeadershipEvent,
    state: &mut DelegateState,
    ctx: &ReconcileContext<'_>,
) -> Result<(), CamelError> {
    match event {
        camel_api::LeadershipEvent::StartedLeading => {
            info!(lock = %ctx.lock_name, "master leadership acquired");
            // TODO(MST-001): emit metrics here — MetricsCollector is wired but never called
            tracing::info!(lock = %ctx.lock_name, "metrics emission placeholder: leadership acquired");
            stop_delegate(state, ctx.drain_timeout).await?;

            let delegate_ctx = MasterDelegateContext {
                delegate_component: Arc::clone(ctx.delegate_component),
                metrics: Arc::clone(ctx.metrics),
                platform_service: Arc::clone(ctx.platform_service),
            };

            let endpoint = match ctx
                .delegate_component
                .create_endpoint(ctx.delegate_uri, &delegate_ctx)
            {
                Ok(endpoint) => endpoint,
                Err(err) => {
                    if is_retryable_camel_error(&err) {
                        warn!(lock = %ctx.lock_name, error = %err, "transient delegate endpoint error (will retry)");
                        return Ok(()); // swallow transient — retry via next tick
                    }
                    return Err(err); // permanent — propagate to retry loop for fail-fast
                }
            };

            let mut consumer = match endpoint.create_consumer(Arc::clone(&ctx.runtime)) {
                Ok(consumer) => consumer,
                Err(err) => {
                    if is_retryable_camel_error(&err) {
                        warn!(lock = %ctx.lock_name, error = %err, "transient delegate consumer error (will retry)");
                        return Ok(()); // swallow transient — retry via next tick
                    }
                    return Err(err); // permanent — propagate to retry loop for fail-fast
                }
            };

            // Snapshot the epoch ONCE — stale bridges carry stale terms
            // (downstream sinks reject envelopes with an older epoch).
            let my_epoch = ctx.leader_epoch.load(Ordering::Acquire);

            // Spawn the epoch-stamping bridge. The delegate sends to `stamp_tx`;
            // the bridge forwards stamped envelopes to the pipeline via
            // `ctx.sender`. Lifetime is tied to the delegate: dropping
            // `stamp_tx` (when the delegate stops) causes the bridge to
            // drain and exit.
            let (stamp_tx, bridge_handle) = spawn_epoch_bridge(
                ctx.sender.clone(),
                my_epoch,
                ctx.parent_cancel.child_token(),
            );

            let run_token = ctx.parent_cancel.child_token();
            let delegate_ctx =
                ConsumerContext::new(stamp_tx, run_token.clone(), ctx.route_id.clone());
            let handle = tokio::spawn(async move {
                consumer.start(delegate_ctx).await?;
                consumer.stop().await?;
                Ok::<(), CamelError>(())
            });

            *state = DelegateState::Active {
                run_token,
                handle,
                bridge_handle: Some(bridge_handle),
            };
        }
        camel_api::LeadershipEvent::StoppedLeading => {
            info!(lock = %ctx.lock_name, "master leadership lost");
            // TODO(MST-001): emit metrics here — MetricsCollector is wired but never called
            tracing::info!(lock = %ctx.lock_name, "metrics emission placeholder: leadership lost");
            stop_delegate(state, ctx.drain_timeout).await?;
        }
    }
    Ok(())
}

// Uses camel_component_api::is_retryable_camel_error for transient/permanent
// classification (rc-7ct consolidation, was master-local in rc-i1z).

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    use camel_api::{Body, Exchange, Message};
    use camel_component_api::ExchangeEnvelope;
    use serde_json::Value;
    use tokio::time::timeout;
    use tokio_util::sync::CancellationToken;

    use super::spawn_epoch_bridge;

    fn make_envelope(text: &str) -> ExchangeEnvelope {
        ExchangeEnvelope {
            exchange: Exchange::new(Message::new(Body::Text(text.to_string()))),
            reply_tx: None,
        }
    }

    /// Verifies that a basic envelope sent through the bridge is stamped
    /// with the spawn-time epoch on its properties, then forwarded.
    #[tokio::test]
    async fn fence_token_stamped_on_exchange() {
        let (pipeline_tx, mut pipeline_rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(10);
        let cancel = CancellationToken::new();

        let (stamp_tx, _bridge) = spawn_epoch_bridge(pipeline_tx, 42, cancel);

        stamp_tx.send(make_envelope("hello")).await.unwrap();
        drop(stamp_tx);

        let received = timeout(Duration::from_secs(1), pipeline_rx.recv())
            .await
            .expect("bridge should forward within 1s")
            .expect("channel should yield an envelope");
        let epoch_val = received
            .exchange
            .properties
            .get(super::LEADER_EPOCH_PROPERTY)
            .expect("x-camel-leader-epoch should be stamped");
        assert_eq!(epoch_val, &Value::String("42".to_string()));
    }

    /// Verifies the drain-on-sender-drop contract: buffered envelopes are
    /// forwarded (all stamped with the same constant epoch) before the
    /// bridge exits.
    #[tokio::test]
    async fn bridge_drains_buffer_on_delegate_stop() {
        let (pipeline_tx, mut pipeline_rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(10);
        let cancel = CancellationToken::new();

        let (stamp_tx, bridge_handle) = spawn_epoch_bridge(pipeline_tx, 5, cancel);

        for i in 0..3 {
            stamp_tx
                .send(make_envelope(&format!("msg-{i}")))
                .await
                .unwrap();
        }
        drop(stamp_tx);

        // Bridge should drain all 3, then exit.
        timeout(Duration::from_secs(1), bridge_handle)
            .await
            .expect("bridge should finish within 1s")
            .expect("bridge task should not panic");

        for i in 0..3 {
            let env = pipeline_rx
                .recv()
                .await
                .expect("envelope should arrive after drain");
            let epoch = env
                .exchange
                .properties
                .get(super::LEADER_EPOCH_PROPERTY)
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok());
            assert_eq!(epoch, Some(5), "envelope {i} should carry constant epoch 5");
        }
        assert!(
            pipeline_rx.try_recv().is_err(),
            "bridge should have exited — no more envelopes"
        );
    }

    /// Verifies the bridge stamps with its SPAWN-TIME snapshot, not a live
    /// read. If the live epoch changes mid-stream, envelopes still carry
    /// the snapshot value.
    #[tokio::test]
    async fn bridge_snapshot_constant_across_epoch_change() {
        let epoch = Arc::new(AtomicU64::new(7));
        let (pipeline_tx, mut pipeline_rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(10);
        let cancel = CancellationToken::new();

        // Snapshot epoch=7 at bridge spawn
        let my_epoch = epoch.load(std::sync::atomic::Ordering::Acquire);
        let (stamp_tx, bridge_handle) = spawn_epoch_bridge(pipeline_tx, my_epoch, cancel);

        // Change the live epoch — bridge should NOT pick this up
        epoch.store(99, std::sync::atomic::Ordering::Release);

        stamp_tx.send(make_envelope("stale")).await.unwrap();
        drop(stamp_tx);
        timeout(Duration::from_secs(1), bridge_handle)
            .await
            .unwrap()
            .unwrap();

        let env = pipeline_rx.recv().await.unwrap();
        let stamped = env
            .exchange
            .properties
            .get(super::LEADER_EPOCH_PROPERTY)
            .and_then(|v| v.as_str())
            .expect("stamp should be present");
        assert_eq!(
            stamped, "7",
            "bridge must stamp with snapshot (7), not live (99)"
        );
    }

    /// Verifies that a bridge spawned with an old epoch stamps envelopes with
    /// its spawn-time snapshot even if the live epoch changes. This simulates
    /// a stale bridge from a previous leader term — downstream sinks should
    /// reject envelopes with an epoch older than the current leader's.
    #[tokio::test]
    async fn stale_bridge_carries_old_epoch_not_current() {
        let (pipeline_tx, mut pipeline_rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(10);
        let cancel = CancellationToken::new();

        // Bridge spawned with epoch=3 (e.g., from a previous leader term).
        let (stamp_tx, bridge_handle) = spawn_epoch_bridge(pipeline_tx, 3, cancel);

        stamp_tx
            .send(make_envelope("from-old-leader"))
            .await
            .unwrap();
        drop(stamp_tx);
        timeout(Duration::from_secs(1), bridge_handle)
            .await
            .expect("bridge should finish within 1s")
            .expect("bridge task should not panic");

        let env = pipeline_rx
            .recv()
            .await
            .expect("stamped envelope should arrive");
        let stamped = env
            .exchange
            .properties
            .get(super::LEADER_EPOCH_PROPERTY)
            .and_then(|v| v.as_str())
            .expect("x-camel-leader-epoch should be stamped");
        assert_eq!(
            stamped, "3",
            "old bridge stamps with its snapshot epoch (3), not the current epoch"
        );
    }

    /// Verifies the bridge aborts promptly when the parent cancel token fires.
    #[tokio::test]
    async fn bridge_aborts_on_parent_cancel() {
        let (pipeline_tx, mut _pipeline_rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(10);
        let cancel = CancellationToken::new();

        let (_stamp_tx, bridge_handle) = spawn_epoch_bridge(pipeline_tx, 1, cancel.clone());

        cancel.cancel();

        // Bridge should exit promptly on parent cancel.
        timeout(Duration::from_millis(100), bridge_handle)
            .await
            .expect("bridge should abort within 100ms")
            .expect("bridge task should not panic");
    }

    #[tokio::test]
    async fn bridge_aborted_on_drain_timeout_when_pipeline_full() {
        // Backpressure scenario: pipeline channel capacity=1, we fill it,
        // then the bridge tries to send → blocks → drain timeout → abort.
        let (pipeline_tx, _pipeline_rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(1);
        let cancel = CancellationToken::new();

        // Fill the pipeline channel so bridge.send() will block
        pipeline_tx.send(make_envelope("blocker")).await.unwrap();

        let (stamp_tx, mut bridge_handle) = spawn_epoch_bridge(pipeline_tx, 42, cancel);

        // Send an envelope to the bridge — it will try to forward to the
        // full pipeline channel and block.
        stamp_tx.send(make_envelope("will-block")).await.unwrap();
        drop(stamp_tx);

        // Drain with a very short timeout — bridge is stuck in send().await
        let drain_timeout = Duration::from_millis(50);
        match timeout(drain_timeout, &mut bridge_handle).await {
            Err(_) => {
                // Timeout hit — abort the bridge (mirrors stop_delegate behavior)
                bridge_handle.abort();
                let _ = bridge_handle.await;
            }
            Ok(_) => panic!("bridge should not complete while pipeline is full"),
        }

        // If we reach here, abort worked — bridge is no longer running.
    }
}
