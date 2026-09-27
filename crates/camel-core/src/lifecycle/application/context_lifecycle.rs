// lifecycle/application/context_lifecycle.rs
// Use-cases for the CamelContext lifecycle: start, stop, abort.
//
// These were previously inherent methods on `CamelContext` (see
// `context.rs:562-752` pre-Tier-C). Extracted here as free functions so
// the context stays a thin composition root. Public method signatures
// on `CamelContext` are UNCHANGED — they are one-line delegates.
//
// Established in Tier C Task C2 (`rc-d0pu.3`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use camel_api::{CamelError, Lifecycle, RuntimeCommandBus, RuntimeCommandResult, RuntimeQueryBus};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::lifecycle::application::ports::{RouteDestructiveTeardownPort, RouteOrderingPort};
use crate::lifecycle::application::runtime_bus::RuntimeBus;
use crate::startup_validation::{ConfigCheck, run_startup_validation};

static CONTEXT_COMMAND_SEQ: AtomicU64 = AtomicU64::new(0);

/// Generate a context-issued runtime command ID with the five-segment shape
/// `context:{op}:{route_id}:{boot_nonce}:{seq}`.
///
/// `boot_nonce` is the journal-derived boot nonce (`RuntimeBus::boot_nonce`)
/// and scopes IDs per boot against the durable dedup store, so a command ID
/// re-issued by a later boot is never suppressed as a duplicate of an
/// earlier boot's recorded ID. `seq` still comes from `CONTEXT_COMMAND_SEQ`
/// and provides uniqueness within a boot. Deterministic — no wall clock.
pub(crate) fn next_context_command_id(boot_nonce: u64, op: &str, route_id: &str) -> String {
    let seq = CONTEXT_COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("context:{op}:{route_id}:{boot_nonce}:{seq}")
}

/// No-silence guard, suppression half (gh#52): emit a WARN for every
/// StartRoute whose `RuntimeCommandResult` was classified as a duplicate
/// by command dedup — the exact gh#52 bug signature, where the route
/// stayed dark with no signal at any log level. WARN only: the boot
/// still proceeds; the boot-unique command IDs are the primary fix.
pub(crate) fn warn_suppressed_starts(results: &[(String, String, RuntimeCommandResult)]) {
    for (route_id, command_id, result) in results {
        if matches!(result, RuntimeCommandResult::Duplicate { .. }) {
            warn!(
                route_id = %route_id,
                command_id = %command_id,
                "StartRoute suppressed as duplicate command"
            );
        }
    }
}

/// No-silence guard, sweep half (gh#52): emit a WARN for every
/// auto-startup route that is not in the `Started` state after the start
/// sequence. `auto_startup = false` routes never appear here — the caller
/// sweeps only `auto_startup_route_ids()` — and genuine start failures
/// already fail the boot with an error before this runs.
pub(crate) fn warn_non_started_routes(statuses: &[(String, String)]) {
    for (route_id, status) in statuses {
        if status != "Started" {
            warn!(
                route_id = %route_id,
                status = %status,
                "auto-startup route not Started after start sequence"
            );
        }
    }
}

/// Start all routes and lifecycle services.
///
/// Algorithm pasted verbatim from `CamelContext::start` (context.rs:662-724)
/// in the pre-Tier-C layout: services loop with rollback, fail-closed
/// startup validation, transient-state reconciliation, and aggregate-first
/// `auto_startup_route_ids` StartRoute loop.
///
/// `cancel_token` is reset to a fresh token on every entry so a restart
/// after `stop()` gets a clean cancellation state. The cohort activation
/// gate is likewise re-armed at entry and activated on every exit (see
/// rc-jxkj).
///
/// `shutdown_token_slot` mirrors `cancel_token`: the fresh token is
/// written into the slot immediately after the reset (one lock round-trip
/// apart, so the mirror cannot drift from the Runtime token). Adapters
/// bound to the slot (`RegistryComponentContext::with_shutdown_slot`)
/// resolve the CURRENT boot's shutdown token per call.
pub(crate) async fn start_context(
    services: &mut [Box<dyn Lifecycle>],
    startup_checks: &mut Vec<Box<dyn ConfigCheck>>,
    runtime: &RuntimeBus,
    route_controller: &dyn RouteOrderingPort,
    cancel_token: &mut CancellationToken,
    shutdown_token_slot: &Arc<std::sync::Mutex<CancellationToken>>,
) -> Result<(), CamelError> {
    info!("Starting CamelContext");

    // Reset cancellation state so a restart after stop() gets a fresh token.
    *cancel_token = CancellationToken::new();
    // Mirror the fresh token into the shared slot (wasm wiring): the write
    // sits adjacent to the reset above so the two cannot drift.
    // Poison-tolerant by design — a poisoned slot must not fail the boot;
    // leaving a stale token is fail-safe (resolvers keep a lineage that
    // this boot never cancels).
    if let Ok(mut slot) = shutdown_token_slot.lock() {
        *slot = cancel_token.clone();
    }

    // Re-arm the cohort activation gate on every boot so this cohort's
    // first consumer dispatches park until startup completes. Per-boot
    // re-arm matters because every start() re-issues StartRoute for all
    // routes whose auto_startup flag is set (route_registry.rs:95 filters
    // on that flag) — a second boot after stop() would otherwise dispatch
    // against a gate left open by the previous boot.
    route_controller.reset_cohort().await;

    // Everything below funnels through `result` — no early `?` returns
    // after the reset — so activation always runs. A failing path that
    // skipped activation would strand parked first dispatches forever.
    let result: Result<(), CamelError> = async {
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
        // preserving route controller startup ordering metadata. Each
        // result is collected so the gh#52 no-silence guard below can
        // name any route whose start was suppressed as a duplicate.
        let route_ids = route_controller.auto_startup_route_ids().await?;
        let mut start_results: Vec<(String, String, RuntimeCommandResult)> = Vec::new();
        for route_id in &route_ids {
            let command_id = next_context_command_id(runtime.boot_nonce(), "start", route_id);
            let result = runtime
                .execute(camel_api::RuntimeCommand::StartRoute {
                    route_id: route_id.clone(),
                    command_id: command_id.clone(),
                    causation_id: None,
                })
                .await?;
            start_results.push((route_id.clone(), command_id, result));
        }

        // gh#52 no-silence guard: a `Duplicate` classification means the
        // route's StartRoute was suppressed by command dedup — surface it
        // at WARN instead of letting the route fail silently.
        warn_suppressed_starts(&start_results);

        // Belt-and-braces sweep: after the start loop, every auto-startup
        // route must be in the `Started` state. The sweep never fails the
        // boot — a query error or unexpected response shape is warned and
        // skipped; only the not-Started condition is reported onward.
        let mut statuses: Vec<(String, String)> = Vec::new();
        for route_id in &route_ids {
            match runtime
                .ask(camel_api::RuntimeQuery::GetRouteStatus {
                    route_id: route_id.clone(),
                })
                .await
            {
                Ok(camel_api::RuntimeQueryResult::RouteStatus { status, .. }) => {
                    statuses.push((route_id.clone(), status));
                }
                Ok(unexpected) => warn!(
                    route_id = %route_id,
                    error = ?unexpected,
                    "auto-startup route status unavailable after start sequence"
                ),
                Err(e) => warn!(
                    route_id = %route_id,
                    error = %e,
                    "auto-startup route status unavailable after start sequence"
                ),
            }
        }
        warn_non_started_routes(&statuses);

        info!("CamelContext started");
        Ok(())
    }
    .await;

    // Unconditional activation — releases parked first dispatches whether
    // the cohort started cleanly or failed. The port method is infallible
    // (watch-channel level set), so no activation error can shadow
    // `result`.
    route_controller.activate_cohort().await;

    result
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
                command_id: next_context_command_id(runtime.boot_nonce(), "stop", &route_id),
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
/// Routes through `RouteOrderingPort` (for `shutdown_route_ids`) and
/// `RouteDestructiveTeardownPort` (for the destructive `shutdown()`).
/// Both ports are impl'd on the controller adapter handle in
/// `lifecycle/adapters/route_ordering_impl.rs` — the use-case ring is pure
/// of concrete adapter types.
///
/// Algorithm pasted verbatim from `CamelContext::abort` (context.rs:807-852)
/// in the pre-Tier-C layout: cancel + supervision abort, route stop loop,
/// LIFO service stop with 5s timeout ladder, controller actor `shutdown()`,
/// health cancel, actor join 5s ladder.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn abort_context(
    cancel_token: &CancellationToken,
    supervision_join: &mut Option<JoinHandle<()>>,
    runtime: &RuntimeBus,
    route_ordering: &dyn RouteOrderingPort,
    route_teardown: &dyn RouteDestructiveTeardownPort,
    services: &mut [Box<dyn Lifecycle>],
    health_cancel_token: CancellationToken,
    actor_join: &mut Option<JoinHandle<()>>,
) {
    cancel_token.cancel();
    if let Some(join) = supervision_join.take() {
        join.abort();
    }
    let route_ids = route_ordering
        .shutdown_route_ids()
        .await
        .unwrap_or_default();
    for route_id in route_ids {
        let _ = runtime
            .execute(camel_api::RuntimeCommand::StopRoute {
                route_id: route_id.clone(),
                command_id: next_context_command_id(runtime.boot_nonce(), "abort-stop", &route_id),
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
    let _ = route_teardown.shutdown().await;
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

#[cfg(test)]
mod start_context_gate {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::lifecycle::CohortActivationGate;
    use crate::{CamelContext, RouteDefinition};

    /// Lifecycle service that records the cohort gate level each time
    /// `start()` runs — that moment sits after `reset_cohort` and before
    /// the StartRoute loop — and optionally fails its own start.
    struct GateProbeService {
        gate: Arc<CohortActivationGate>,
        levels: Arc<Mutex<Vec<bool>>>,
        fail_start: bool,
    }

    impl GateProbeService {
        fn new(
            gate: Arc<CohortActivationGate>,
            levels: Arc<Mutex<Vec<bool>>>,
            fail_start: bool,
        ) -> Self {
            Self {
                gate,
                levels,
                fail_start,
            }
        }
    }

    #[async_trait]
    impl Lifecycle for GateProbeService {
        fn name(&self) -> &str {
            "gate-probe"
        }

        async fn start(&mut self) -> Result<(), CamelError> {
            self.levels
                .lock()
                .expect("levels lock")
                .push(self.gate.is_open());
            if self.fail_start {
                return Err(CamelError::Config("gate-probe start failure".into()));
            }
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    fn gate_of(ctx: &CamelContext) -> Arc<CohortActivationGate> {
        ctx.runtime_execution_handle().controller.cohort_gate()
    }

    #[tokio::test]
    async fn start_context_gate_boot_failure_still_activates() {
        let mut ctx = CamelContext::builder()
            .build()
            .await
            .expect("build context");
        let gate = gate_of(&ctx);
        assert!(!gate.is_open(), "fresh context gate must start closed");

        let levels = Arc::new(Mutex::new(Vec::new()));
        ctx = ctx.with_lifecycle(GateProbeService::new(
            Arc::clone(&gate),
            Arc::clone(&levels),
            true,
        ));

        let result = ctx.start().await;
        assert!(
            result.is_err(),
            "boot with a failing service must return Err"
        );
        // Activation runs on every exit — a failed boot still releases
        // anything that parked behind the gate.
        assert!(gate.is_open(), "gate must open even when the boot fails");
        assert_eq!(*levels.lock().expect("levels lock"), vec![false]);
    }

    #[tokio::test]
    async fn start_context_gate_boot_success_activates() {
        let mut ctx = CamelContext::builder()
            .build()
            .await
            .expect("build context");
        ctx.register_component(camel_component_timer::TimerComponent::new());
        ctx.add_route_definition(
            RouteDefinition::new("timer:gate-r1?period=3600000", vec![]).with_route_id("gate-r1"),
        )
        .await
        .expect("add route 1");
        ctx.add_route_definition(
            RouteDefinition::new("timer:gate-r2?period=3600000", vec![]).with_route_id("gate-r2"),
        )
        .await
        .expect("add route 2");

        let gate = gate_of(&ctx);
        assert!(!gate.is_open());

        ctx.start().await.expect("context start");
        assert!(gate.is_open(), "successful boot must open the gate");

        ctx.stop().await.expect("context stop");
    }

    #[tokio::test]
    async fn start_context_gate_second_boot_rearms() {
        let mut ctx = CamelContext::builder()
            .build()
            .await
            .expect("build context");
        let gate = gate_of(&ctx);
        let levels = Arc::new(Mutex::new(Vec::new()));
        ctx = ctx.with_lifecycle(GateProbeService::new(
            Arc::clone(&gate),
            Arc::clone(&levels),
            false,
        ));

        ctx.start().await.expect("first boot");
        assert!(gate.is_open());

        ctx.stop().await.expect("stop");
        // stop() leaves the gate open; only the next boot re-arms it.
        assert!(gate.is_open());

        ctx.start().await.expect("second boot");
        // Both boots probed the gate closed at service-start time — i.e.
        // after reset_cohort, before the StartRoute loop.
        assert_eq!(*levels.lock().expect("levels lock"), vec![false, false]);
        assert!(gate.is_open(), "gate must be open after the second boot");
    }

    /// S4: Context path unchanged.
    ///
    /// Spec: openspec/specs/consumer-activation/spec.md
    /// Requirement: "Bare-controller activation of the cohort barrier"
    /// Scenario: "Context path unchanged"
    ///
    /// GIVEN a CamelContext boot with its startup cohort completing normally;
    /// WHEN the context lifecycle activates the barrier through the actor handle;
    /// THEN the barrier opens, parked dispatch proceeds, and any additional
    /// activation call has no effect and requires no ordering relative to the
    /// context's act.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn s4_context_path_activation_unchanged() {
        use crate::route::BuilderStep;
        use camel_api::{Exchange, Message};
        use camel_component_direct::DirectComponent;
        use camel_component_mock::MockComponent;
        use tower::ServiceExt;

        let mock = MockComponent::new();
        let mut ctx = CamelContext::builder()
            .build()
            .await
            .expect("build context");
        ctx.register_component(mock.clone());
        ctx.register_component(DirectComponent::new());

        let gate = gate_of(&ctx);
        assert!(!gate.is_open(), "fresh context gate must start closed");

        ctx.add_route_definition(
            RouteDefinition::new(
                "direct:s4-in",
                vec![BuilderStep::To("mock:s4-arrival".into())],
            )
            .with_route_id("s4-route"),
        )
        .await
        .expect("add route");

        ctx.start().await.expect("context start must succeed");

        // The boot must have opened the gate.
        assert!(gate.is_open(), "successful boot must open the gate");

        // Send an exchange through the route — the gate is open so the
        // dispatch proceeds normally and the exchange arrives at the mock.
        let direct = ctx
            .registry()
            .get("direct")
            .expect("direct component registered");
        let endpoint = direct
            .create_endpoint("direct:s4-in", &camel_component_api::NoOpComponentContext)
            .expect("create direct endpoint");
        let producer = endpoint
            .create_producer(
                Arc::new(camel_component_api::NoOpComponentContext),
                &ctx.producer_context(),
            )
            .expect("create direct producer");
        producer
            .oneshot(Exchange::new(Message::new("s4-exchange")))
            .await
            .expect("direct send must succeed after boot");

        let arrival = mock
            .get_endpoint("s4-arrival")
            .expect("mock endpoint 's4-arrival' must exist");
        arrival.assert_exchange_count(1).await;

        // Additional activation call after the context has already opened the
        // gate must be a no-op: no re-arm, no reset, no crash.
        let exec = ctx.runtime_execution_handle();
        let pre_level = gate.is_open();
        exec.controller.cohort.open();
        assert!(
            gate.is_open(),
            "gate must remain open after the redundant activation call"
        );
        assert_eq!(
            gate.is_open(),
            pre_level,
            "redundant activation must not change the gate level"
        );

        // A second exchange after the redundant activation must also succeed
        // — proving the gate was not re-closed or re-armed.
        let producer2 = endpoint
            .create_producer(
                Arc::new(camel_component_api::NoOpComponentContext),
                &ctx.producer_context(),
            )
            .expect("create direct producer");
        producer2
            .oneshot(Exchange::new(Message::new("s4-exchange-2")))
            .await
            .expect("second direct send must also succeed");
        arrival.assert_exchange_count(2).await;

        ctx.stop().await.expect("context stop");
    }
}

#[cfg(test)]
mod warn_guard_tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{CamelContext, RouteDefinition};

    /// `MakeWriter` that appends formatted events to a shared `Vec<u8>`
    /// sink so tests can assert on captured WARN output.
    #[derive(Clone)]
    struct CapturingWriter {
        sink: Arc<Mutex<Vec<u8>>>,
    }

    impl std::io::Write for CapturingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.sink.lock().expect("sink lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingWriter {
        type Writer = CapturingWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Install a bare registry as the process-global tracing default,
    /// once per test binary. Guards these tests against callsite-interest
    /// poisoning: `tracing` caches each callsite's `Interest` process-wide
    /// from its FIRST macro execution. A `warn!` callsite evaluated under
    /// no subscriber caches `Interest::never`, and a later thread-local
    /// `set_default` capture then silently drops its events. The global
    /// registry heals prior poison and floors future rebuilds at
    /// `sometimes` (fix pattern: c3853198; bd rc-img5).
    fn ensure_global_tracing_default() {
        static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if INIT.set(()).is_ok() {
            let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
        }
    }

    /// Install a thread-local WARN-capturing subscriber for the calling
    /// test; returns the sink and the guard (which must stay alive for
    /// the test's scope). `set_default` is thread-local, so concurrent
    /// tests neither pollute each other nor need a global subscriber.
    fn capture_warns() -> (Arc<Mutex<Vec<u8>>>, tracing::subscriber::DefaultGuard) {
        ensure_global_tracing_default();
        let sink = Arc::new(Mutex::new(Vec::<u8>::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(CapturingWriter {
                sink: Arc::clone(&sink),
            })
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (sink, guard)
    }

    fn captured(sink: &Mutex<Vec<u8>>) -> String {
        String::from_utf8(sink.lock().expect("sink lock").clone()).expect("utf8 capture")
    }

    #[test]
    fn warn_suppressed_starts_names_route_and_command() {
        let (sink, _guard) = capture_warns();
        warn_suppressed_starts(&[(
            "r1".to_string(),
            "context:start:r1:0:0".to_string(),
            RuntimeCommandResult::Duplicate {
                command_id: "context:start:r1:0:0".to_string(),
            },
        )]);
        let out = captured(&sink);
        assert!(out.contains("r1"), "route id must be named, got: {out}");
        assert!(
            out.contains("context:start:r1:0:0"),
            "suppressed command id must be named, got: {out}"
        );
    }

    #[test]
    fn warn_suppressed_starts_silent_on_accepted() {
        let (sink, _guard) = capture_warns();
        warn_suppressed_starts(&[(
            "r1".to_string(),
            "context:start:r1:0:0".to_string(),
            RuntimeCommandResult::Accepted,
        )]);
        let out = captured(&sink);
        assert!(
            !out.contains("StartRoute suppressed as duplicate command"),
            "Accepted start must not warn, got: {out}"
        );
    }

    #[test]
    fn warn_non_started_routes_names_route() {
        let (sink, _guard) = capture_warns();
        warn_non_started_routes(&[("r2".to_string(), "Registered".to_string())]);
        let out = captured(&sink);
        assert!(out.contains("r2"), "route id must be named, got: {out}");
    }

    #[test]
    fn warn_non_started_routes_silent_on_started() {
        let (sink, _guard) = capture_warns();
        warn_non_started_routes(&[("r3".to_string(), "Started".to_string())]);
        let out = captured(&sink);
        assert!(
            !out.contains("auto-startup route not Started"),
            "Started route must not warn, got: {out}"
        );
    }

    #[test]
    fn warn_guards_silent_when_no_entries() {
        let (sink, _guard) = capture_warns();
        warn_suppressed_starts(&[]);
        warn_non_started_routes(&[]);
        let out = captured(&sink);
        assert!(
            out.trim().is_empty(),
            "empty input must produce no WARN, got: {out}"
        );
    }

    #[tokio::test]
    async fn warn_silent_for_auto_startup_disabled_route() {
        let (sink, _guard) = capture_warns();
        let mut ctx = CamelContext::builder()
            .build()
            .await
            .expect("build context");
        ctx.register_component(camel_component_timer::TimerComponent::new());
        ctx.add_route_definition(
            RouteDefinition::new("timer:warn-lazy?period=3600000", vec![])
                .with_route_id("warn-lazy")
                .with_auto_startup(false),
        )
        .await
        .expect("add auto_startup=false route");

        ctx.start().await.expect("context start");
        let out = captured(&sink);
        assert!(
            !out.contains("warn-lazy"),
            "auto_startup=false route must stay silent through start, got: {out}"
        );
        ctx.stop().await.expect("context stop");
    }
}
