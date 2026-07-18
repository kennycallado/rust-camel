// lifecycle/application/context_lifecycle.rs
// Use-cases for the CamelContext lifecycle: start, stop, abort.
//
// These were previously inherent methods on `CamelContext` (see
// `context.rs:562-752` pre-Tier-C). Extracted here as free functions so
// the context stays a thin composition root. Public method signatures
// on `CamelContext` are UNCHANGED — they are one-line delegates.
//
// Established in Tier C Task C2 (`rc-d0pu.3`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use camel_api::{CamelError, Lifecycle, RuntimeCommandBus};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::lifecycle::adapters::controller_actor::RouteControllerHandle;
use crate::lifecycle::application::ports::RouteOrderingPort;
use crate::lifecycle::application::runtime_bus::RuntimeBus;
use crate::startup_validation::{ConfigCheck, run_startup_validation};

static CONTEXT_COMMAND_SEQ: AtomicU64 = AtomicU64::new(0);

/// Generate a deterministic-ish command ID for context-issued runtime
/// commands. Lifted verbatim from `CamelContext::next_context_command_id`.
pub(crate) fn next_context_command_id(op: &str, route_id: &str) -> String {
    let seq = CONTEXT_COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("context:{op}:{route_id}:{seq}")
}

/// Start all routes and lifecycle services.
///
/// Algorithm pasted verbatim from `CamelContext::start` (context.rs:662-724)
/// in the pre-Tier-C layout: services loop with rollback, fail-closed
/// startup validation, transient-state reconciliation, and aggregate-first
/// `auto_startup_route_ids` StartRoute loop.
///
/// `cancel_token` is reset to a fresh token on every entry so a restart
/// after `stop()` gets a clean cancellation state.
pub(crate) async fn start_context(
    services: &mut [Box<dyn Lifecycle>],
    startup_checks: &mut Vec<Box<dyn ConfigCheck>>,
    runtime: &RuntimeBus,
    route_controller: &dyn RouteOrderingPort,
    cancel_token: &mut CancellationToken,
) -> Result<(), CamelError> {
    info!("Starting CamelContext");

    // Reset cancellation state so a restart after stop() gets a fresh token.
    *cancel_token = CancellationToken::new();

    // Start lifecycle services first
    for (i, service) in services.iter_mut().enumerate() {
        info!("Starting service: {}", service.name());
        if let Err(e) = service.start().await {
            // Rollback: stop already started services in reverse order
            warn!(
                "Service {} failed to start, rolling back {} services",
                service.name(),
                i
            );
            for j in (0..i).rev() {
                if let Err(rollback_err) = services[j].stop().await {
                    warn!(
                        "Failed to stop service {} during rollback: {}",
                        services[j].name(),
                        rollback_err
                    );
                }
            }
            return Err(e);
        }
    }

    // ADR-0033: fail-closed startup validation. Drain the registered
    // ConfigCheck list and run every check synchronously. If any check
    // returns Err, refuse to start the runtime — no route consumer is
    // started, no reconciliation runs. Drains the registry so a second
    // call to start() (currently not supported) would not re-run checks.
    let checks = std::mem::take(startup_checks);
    if let Err(e) = run_startup_validation(checks) {
        warn!("Startup validation failed: {e}");
        return Err(e);
    }

    // H8: boot reconciliation — fail routes stuck in transient state
    // (Starting/Stopping) from a previous run before auto_startup runs.
    runtime
        .reconcile_transient_states()
        .await
        .map_err(|e| CamelError::RouteError(format!("boot reconciliation failed: {e}")))?;

    // Then start routes via runtime command bus (aggregate-first),
    // preserving route controller startup ordering metadata.
    let route_ids = route_controller.auto_startup_route_ids().await?;
    for route_id in route_ids {
        runtime
            .execute(camel_api::RuntimeCommand::StartRoute {
                route_id: route_id.clone(),
                command_id: next_context_command_id("start", &route_id),
                causation_id: None,
            })
            .await?;
    }

    info!("CamelContext started");
    Ok(())
}

/// Graceful shutdown. The controller actor stays alive (owning route
/// registrations needed for a subsequent `start()`); only `abort()` is
/// destructive.
///
/// Algorithm pasted verbatim from `CamelContext::stop_timeout`
/// (context.rs:737-787) in the pre-Tier-C layout. The `stop()` method
/// was a one-line delegate to `stop_timeout(self.shutdown_timeout)`, so
/// the real algorithm lives here.
///
/// LIFO service stop + first-error semantics are preserved EXACTLY —
/// do not simplify away the per-service `first_error` capture.
pub(crate) async fn stop_context(
    cancel_token: &CancellationToken,
    supervision_join: &mut Option<JoinHandle<()>>,
    runtime: &RuntimeBus,
    route_controller: &dyn RouteOrderingPort,
    services: &mut [Box<dyn Lifecycle>],
) -> Result<(), CamelError> {
    info!("Stopping CamelContext");

    // Signal cancellation (for any legacy code that might use it)
    cancel_token.cancel();
    if let Some(join) = supervision_join.take() {
        join.abort();
    }

    // Stop all routes via runtime command bus (aggregate-first),
    // preserving route controller shutdown ordering metadata.
    let route_ids = route_controller.shutdown_route_ids().await?;
    for route_id in route_ids {
        if let Err(err) = runtime
            .execute(camel_api::RuntimeCommand::StopRoute {
                route_id: route_id.clone(),
                command_id: next_context_command_id("stop", &route_id),
                causation_id: None,
            })
            .await
        {
            warn!(route_id = %route_id, error = %err, "Runtime stop command failed during context shutdown");
        }
    }

    // The controller actor stays alive — it owns route registrations
    // needed for a subsequent start(). Destructive teardown (actor kill,
    // health cancel) happens only in abort().

    // Then stop lifecycle services in reverse insertion order (LIFO)
    // Continue stopping all services even if some fail
    let mut first_error = None;
    for service in services.iter_mut().rev() {
        info!("Stopping service: {}", service.name());
        if let Err(e) = service.stop().await {
            warn!("Service {} failed to stop: {}", service.name(), e);
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }

    info!("CamelContext stopped");

    if let Some(e) = first_error {
        Err(e)
    } else {
        Ok(())
    }
}

/// Destructive, non-restartable teardown.
///
/// **ACCEPTED §4 EXCEPTION (ADR-0045):** this use-case takes the
/// **concrete** `&RouteControllerHandle` for the destructive
/// `shutdown()` call. The narrow-port alternative
/// (`RouteDestructiveTeardownPort` with only `shutdown()`) was rejected
/// as YAGNI — one rare destructive call. **Precedent:** none at the
/// use-case ring — C1 (`ReloadExecutorPort`) removed the prior
/// use-case→concrete-handle inversion, making this the first such
/// exception. Closest analog is adapter-internal
/// (`lifecycle/adapters/runtime_execution.rs` holds a concrete
/// `RouteControllerHandle` within the adapters ring). Documented in
/// ADR-0045 §4.
///
/// For health cancellation, the cancel token is passed in (not the whole
/// `HealthCheckRegistry`) to keep the use-case pure of unrelated state.
///
/// Algorithm pasted verbatim from `CamelContext::abort`
/// (context.rs:807-852) in the pre-Tier-C layout, in the original order:
/// cancel + supervision abort, route stop loop, **LIFO service stop with
/// 5s timeout ladder**, controller actor `shutdown()`, health cancel,
/// actor join 5s ladder.
///
/// The LIFO loop and the first-error semantics from `stop_context` are
/// preserved here too — `abort()` does NOT collect first_error (it returns
/// `()`), but the per-service timeout ladder and Ok(Ok)/Ok(Err)/Err
/// match arms must remain intact.
pub(crate) async fn abort_context(
    cancel_token: &CancellationToken,
    supervision_join: &mut Option<JoinHandle<()>>,
    runtime: &RuntimeBus,
    route_controller: &RouteControllerHandle,
    services: &mut [Box<dyn Lifecycle>],
    health_cancel_token: CancellationToken,
    actor_join: &mut Option<JoinHandle<()>>,
) {
    cancel_token.cancel();
    if let Some(join) = supervision_join.take() {
        join.abort();
    }
    let route_ids = route_controller
        .shutdown_route_ids()
        .await
        .unwrap_or_default();
    for route_id in route_ids {
        let _ = runtime
            .execute(camel_api::RuntimeCommand::StopRoute {
                route_id: route_id.clone(),
                command_id: next_context_command_id("abort-stop", &route_id),
                causation_id: None,
            })
            .await;
    }

    for service in services.iter_mut().rev() {
        let name = service.name().to_string();
        match timeout(Duration::from_secs(5), service.stop()).await {
            Ok(Ok(())) => info!("Aborted service: {}", name),
            Ok(Err(e)) => warn!("Service {} failed to stop during abort: {}", name, e),
            Err(_) => warn!("Service {} timed out during abort (5s)", name),
        }
    }

    // Destructive teardown: kill the controller actor and cancel health
    // probes. This is what makes abort() non-restartable vs stop().
    let _ = route_controller.shutdown().await;
    health_cancel_token.cancel();
    if let Some(mut join) = actor_join.take() {
        match tokio::time::timeout(Duration::from_secs(5), &mut join).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("Controller actor task error during abort: {e}"),
            Err(_) => {
                warn!("Controller actor did not stop within 5s during abort; force-aborting");
                join.abort();
                let _ = join.await;
            }
        }
    }
}
