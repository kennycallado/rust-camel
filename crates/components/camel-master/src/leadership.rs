use std::sync::Arc;
use std::time::Duration;

use camel_api::{CamelError, MetricsCollector, PlatformService};
use camel_component_api::{Component, ConsumerContext, ExchangeEnvelope, is_retryable_camel_error};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::consumer::DelegateState;
use crate::endpoint::MasterDelegateContext;

pub(crate) async fn stop_delegate(
    state: &mut DelegateState,
    drain_timeout: Duration,
) -> Result<(), CamelError> {
    if let DelegateState::Active {
        run_token,
        mut handle,
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
    }
    Ok(())
}

pub(crate) struct ReconcileContext<'a> {
    pub(crate) lock_name: &'a str,
    pub(crate) delegate_component: &'a Arc<dyn Component>,
    pub(crate) delegate_uri: &'a str,
    pub(crate) sender: &'a tokio::sync::mpsc::Sender<ExchangeEnvelope>,
    pub(crate) parent_cancel: &'a CancellationToken,
    pub(crate) drain_timeout: Duration,
    pub(crate) metrics: &'a Arc<dyn MetricsCollector>,
    pub(crate) platform_service: &'a Arc<dyn PlatformService>,
    pub(crate) runtime: Arc<dyn camel_component_api::RuntimeObservability>,
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

            let run_token = ctx.parent_cancel.child_token();
            let delegate_ctx = ConsumerContext::new(ctx.sender.clone(), run_token.clone());
            let handle = tokio::spawn(async move {
                consumer.start(delegate_ctx).await?;
                consumer.stop().await?;
                Ok::<(), CamelError>(())
            });

            *state = DelegateState::Active { run_token, handle };
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
