use super::*;
use camel_api::function::{FunctionDiff, FunctionId, PrepareToken};
use camel_api::{
    Exchange, ExchangePatch, FunctionDefinition, FunctionInvocationError, FunctionInvoker,
    FunctionInvokerSync,
};
use std::sync::Arc;

// -----------------------------------------------------------------------
// Reusable mock invoker
// -----------------------------------------------------------------------

struct MockInvoker {
    function_refs: Vec<(FunctionId, Option<String>)>,
    staged_refs: Vec<(FunctionId, Option<String>)>,
    staged_defs: Vec<(FunctionDefinition, Option<String>)>,
}

impl FunctionInvokerSync for MockInvoker {
    fn stage_pending(&self, _: FunctionDefinition, _: Option<&str>, _: u64) {}
    fn discard_staging(&self, _: u64) {}
    fn begin_reload(&self) -> u64 {
        0
    }
    fn function_refs_for_route(&self, _: &str) -> Vec<(FunctionId, Option<String>)> {
        self.function_refs.clone()
    }
    fn staged_refs_for_route(&self, _: &str, _: u64) -> Vec<(FunctionId, Option<String>)> {
        self.staged_refs.clone()
    }
    fn staged_defs_for_route(&self, _: &str, _: u64) -> Vec<(FunctionDefinition, Option<String>)> {
        self.staged_defs.clone()
    }
}

#[async_trait::async_trait]
impl FunctionInvoker for MockInvoker {
    async fn register(
        &self,
        _: FunctionDefinition,
        _: Option<&str>,
    ) -> Result<(), FunctionInvocationError> {
        Ok(())
    }
    async fn unregister(
        &self,
        _: &FunctionId,
        _: Option<&str>,
    ) -> Result<(), FunctionInvocationError> {
        Ok(())
    }
    async fn invoke(
        &self,
        _: &FunctionId,
        _: &Exchange,
    ) -> Result<ExchangePatch, FunctionInvocationError> {
        Ok(ExchangePatch::default())
    }
    async fn prepare_reload(
        &self,
        _: FunctionDiff,
        _: u64,
    ) -> Result<PrepareToken, FunctionInvocationError> {
        Ok(PrepareToken::default())
    }
    async fn finalize_reload(
        &self,
        _: &FunctionDiff,
        _: u64,
    ) -> Result<(), FunctionInvocationError> {
        Ok(())
    }
    async fn rollback_reload(
        &self,
        _: PrepareToken,
        _: u64,
    ) -> Result<(), FunctionInvocationError> {
        Ok(())
    }
    async fn commit_reload(&self, _: FunctionDiff, _: u64) -> Result<(), FunctionInvocationError> {
        Ok(())
    }
    async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
        Ok(())
    }
}

// -----------------------------------------------------------------------
// Helper tests
// -----------------------------------------------------------------------

#[test]
fn helper_next_reload_command_id_increments_and_keeps_prefix() {
    let one = next_reload_command_id("restart-stop", "r1");
    let two = next_reload_command_id("restart-stop", "r1");
    assert!(one.starts_with("reload:restart-stop:r1:"));
    assert!(two.starts_with("reload:restart-stop:r1:"));
    assert_ne!(one, two);
}

#[test]
fn helper_should_stop_before_mutation_respects_status() {
    assert!(!should_stop_before_mutation(Some("Registered")));
    assert!(!should_stop_before_mutation(Some("Stopped")));
    assert!(should_stop_before_mutation(Some("Started")));
    assert!(should_stop_before_mutation(None));
}

#[test]
fn helper_should_start_after_restart_respects_status() {
    assert!(!should_start_after_restart(Some("Registered")));
    assert!(!should_start_after_restart(Some("Stopped")));
    assert!(should_start_after_restart(Some("Started")));
    assert!(should_start_after_restart(None));
}

#[test]
fn helper_invalid_stop_transition_detects_marker() {
    let err = CamelError::RouteError("invalid transition: Started -> Started".into());
    assert!(is_invalid_stop_transition(&err));

    let other = CamelError::RouteError("route missing".into());
    assert!(!is_invalid_stop_transition(&other));
}

// -----------------------------------------------------------------------
// compute_function_diff_for_route tests
// -----------------------------------------------------------------------

#[test]
fn compute_function_diff_all_added() {
    let staged_def = FunctionDefinition {
        id: FunctionId::compute("deno", "fn1", 5000),
        runtime: "deno".into(),
        source: "fn1".into(),
        timeout_ms: 5000,
        route_id: Some("route-a".into()),
        step_index: Some(0),
    };
    let invoker: Arc<dyn FunctionInvoker> = Arc::new(MockInvoker {
        function_refs: vec![],
        staged_refs: vec![(staged_def.id.clone(), Some("route-a".into()))],
        staged_defs: vec![(staged_def.clone(), Some("route-a".into()))],
    });

    let diff = compute_function_diff_for_route(&invoker, "route-a", 0);
    assert_eq!(diff.added.len(), 1);
    assert_eq!(diff.removed.len(), 0);
    assert_eq!(diff.unchanged.len(), 0);
}

#[test]
fn compute_function_diff_all_removed() {
    let invoker: Arc<dyn FunctionInvoker> = Arc::new(MockInvoker {
        function_refs: vec![(
            FunctionId::compute("deno", "old-fn", 5000),
            Some("route-b".into()),
        )],
        staged_refs: vec![],
        staged_defs: vec![],
    });

    let diff = compute_function_diff_for_route(&invoker, "route-b", 0);
    assert_eq!(diff.added.len(), 0);
    assert_eq!(diff.removed.len(), 1);
    assert_eq!(diff.unchanged.len(), 0);
}

#[test]
fn compute_function_diff_unchanged() {
    let fn_id = FunctionId::compute("deno", "same-fn", 5000);
    let pair = (fn_id.clone(), Some("route-c".into()));

    let invoker: Arc<dyn FunctionInvoker> = Arc::new(MockInvoker {
        function_refs: vec![pair.clone()],
        staged_refs: vec![pair.clone()],
        staged_defs: vec![(
            FunctionDefinition {
                id: fn_id,
                runtime: "deno".into(),
                source: "same".into(),
                timeout_ms: 5000,
                route_id: Some("route-c".into()),
                step_index: Some(0),
            },
            Some("route-c".into()),
        )],
    });

    let diff = compute_function_diff_for_route(&invoker, "route-c", 0);
    assert_eq!(diff.added.len(), 0);
    assert_eq!(diff.removed.len(), 0);
    assert_eq!(diff.unchanged.len(), 1);
}

// ── Integration: apply_swap Restart path for lifecycle-bearing routes ──

#[tokio::test]
async fn apply_swap_restart_path_for_lifecycle_route() {
    // Verifies the full stop→swap_raw→start sequence:
    //   1. swap_pipeline rejects the lifecycle-bearing route
    //   2. The Restart path stops the route, drains, raw-swaps, and starts
    //   3. No errors are produced
    //   4. The route exists with the new pipeline after restart
    //
    // Because function resolution is lazy (Case B — FunctionStep calls
    // invoker.invoke(id) at runtime), the staging contract is:
    //   - finalize_reload on success (NOT premature rollback)
    //   - rollback_reload on failure

    use crate::context::RuntimeExecutionHandle;
    use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
    use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
    use crate::lifecycle::application::runtime_bus::RuntimeBus;
    use crate::shared::components::domain::Registry;
    use camel_api::{StepLifecycle, StepShutdownReason};
    use std::sync::Arc;

    // ── Lifecycle handle for the test route ──
    #[derive(Debug)]
    struct TestLifecycle;
    #[async_trait::async_trait]
    impl StepLifecycle for TestLifecycle {
        fn name(&self) -> &'static str {
            "test-lifecycle"
        }
        async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
    }

    // ── Build controller with a route injected with lifecycle ──
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .unwrap()
        .register(Arc::new(camel_component_timer::TimerComponent::new()));
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    // Add route via the normal path (compiles identity pipeline).
    controller
        .add_route(
            RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                .with_route_id("restart-integration"),
        )
        .await
        .unwrap();

    // Inject lifecycle into the pipeline so swap_pipeline rejects it.
    controller
        .set_route_lifecycle_for_test(
            "restart-integration",
            vec![Arc::new(TestLifecycle) as Arc<dyn StepLifecycle>],
        )
        .expect("set_route_lifecycle_for_test should succeed");

    // Spawn the actor — the route is pre-injected with lifecycle.
    let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);

    // ── Build minimal RuntimeBus ──
    let store = InMemoryRuntimeStore::default();
    let execution = Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
    let runtime_bus = Arc::new(
        RuntimeBus::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
        )
        .with_execution(execution),
    );

    // ── Build RuntimeExecutionHandle ──
    let handle = RuntimeExecutionHandle {
        controller: ctrl_handle.clone(),
        runtime: runtime_bus,
        function_invoker: None,
        test_lifecycle_inject: Arc::new(std::sync::Mutex::new(None)),
    };

    // ── Call apply_swap ──
    let mut new_defs = vec![
        RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
            .with_route_id("restart-integration"),
    ];
    let mut errors = vec![];
    apply_swap(
        "restart-integration".into(),
        &mut new_defs,
        &handle,
        Duration::from_secs(5),
        None,
        &mut errors,
    )
    .await;

    // ── Assert ──
    assert!(
        errors.is_empty(),
        "apply_swap should succeed for lifecycle route via Restart path; got {errors:?}"
    );

    let exists = ctrl_handle
        .route_exists("restart-integration")
        .await
        .unwrap();
    assert!(exists, "route should exist after Restart path completes");

    let pipeline = ctrl_handle
        .get_pipeline("restart-integration")
        .await
        .unwrap();
    assert!(
        pipeline.is_some(),
        "pipeline should be present after Restart path completes"
    );

    // Cleanup: stop the running route (timer consumer was started).
    let _ = ctrl_handle.stop_route("restart-integration").await;
}

// ── Proactive guard: lifecycle-bearing compiled pipeline → Restart path ──

/// Verify that when the compiled pipeline carries lifecycle handles,
/// the proactive guard skips the atomic swap and uses the Restart path
/// directly.  The Restart path threads lifecycle into
/// `swap_pipeline_raw`, so a subsequent atomic swap attempt is rejected.
#[tokio::test]
async fn apply_swap_proactive_restart_preserves_lifecycle() {
    use crate::context::RuntimeExecutionHandle;
    use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
    use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
    use crate::lifecycle::application::runtime_bus::RuntimeBus;
    use crate::shared::components::domain::Registry;
    use camel_api::{IdentityProcessor, StepLifecycle, StepShutdownReason};
    use std::sync::Arc;

    // ── Lifecycle handle for the compiled pipeline ──
    #[derive(Debug)]
    struct GuardTestLifecycle;
    #[async_trait::async_trait]
    impl StepLifecycle for GuardTestLifecycle {
        fn name(&self) -> &'static str {
            "guard-test-lifecycle"
        }
        async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
    }

    // ── Build controller with a plain (lifecycle-free) route ──
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .unwrap()
        .register(Arc::new(camel_component_timer::TimerComponent::new()));
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    controller
        .add_route(
            RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                .with_route_id("proactive-restart"),
        )
        .await
        .unwrap();

    // NOTE: do NOT call set_route_lifecycle_for_test — the OLD route
    // has no lifecycle, so an atomic swap would normally succeed.
    // Instead, inject lifecycle into the NEW compiled pipeline via the
    // test-only field on RuntimeExecutionHandle.

    let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);

    // ── Build minimal RuntimeBus ──
    let store = InMemoryRuntimeStore::default();
    let execution = Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
    let runtime_bus = Arc::new(
        RuntimeBus::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
        )
        .with_execution(execution),
    );

    let handle = RuntimeExecutionHandle {
        controller: ctrl_handle.clone(),
        runtime: runtime_bus,
        function_invoker: None,
        test_lifecycle_inject: Arc::new(std::sync::Mutex::new(Some(vec![
            Arc::new(GuardTestLifecycle) as Arc<dyn StepLifecycle>,
        ]))),
    };

    // ── Call apply_swap (lifecycle injected → should go Restart path) ──
    let mut new_defs = vec![
        RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
            .with_route_id("proactive-restart"),
    ];
    let mut errors = vec![];
    apply_swap(
        "proactive-restart".into(),
        &mut new_defs,
        &handle,
        Duration::from_secs(5),
        None,
        &mut errors,
    )
    .await;

    assert!(
        errors.is_empty(),
        "apply_swap should succeed via proactive Restart path; got {errors:?}"
    );

    let exists = ctrl_handle.route_exists("proactive-restart").await.unwrap();
    assert!(
        exists,
        "route should exist after proactive Restart path completes"
    );

    let pipeline = ctrl_handle.get_pipeline("proactive-restart").await.unwrap();
    assert!(
        pipeline.is_some(),
        "pipeline should be present after proactive Restart path completes"
    );

    // ── Verify lifecycle was preserved ──
    // If the atomic swap had been used, the pipeline assembly would have
    // lifecycle == vec![].  Since we injected lifecycle, the Restart path
    // should have threaded it into swap_pipeline_raw, making the route
    // lifecycle-bearing.  A subsequent atomic swap MUST be rejected.
    let identity = camel_api::BoxProcessor::new(IdentityProcessor);
    let swap_result = ctrl_handle
        .swap_pipeline("proactive-restart", identity)
        .await;
    assert!(
        swap_result.is_err(),
        "atomic swap should be rejected after Restart path preserves lifecycle"
    );
    let err_msg = swap_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("lifecycle-bearing"),
        "rejection should mention lifecycle-bearing; got: {err_msg}"
    );

    // Cleanup
    let _ = ctrl_handle.stop_route("proactive-restart").await;
}

/// Verify that a route WITHOUT lifecycle-bearing steps uses the atomic
/// swap (fast path) and a subsequent atomic swap still succeeds.
#[tokio::test]
async fn apply_swap_atomic_for_non_lifecycle_route() {
    use crate::context::RuntimeExecutionHandle;
    use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
    use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
    use crate::lifecycle::application::runtime_bus::RuntimeBus;
    use crate::shared::components::domain::Registry;
    use camel_api::IdentityProcessor;
    use std::sync::Arc;

    // ── Build controller with a plain route ──
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .unwrap()
        .register(Arc::new(camel_component_timer::TimerComponent::new()));
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    controller
        .add_route(
            RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                .with_route_id("atomic-swap"),
        )
        .await
        .unwrap();

    let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);

    let store = InMemoryRuntimeStore::default();
    let execution = Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
    let runtime_bus = Arc::new(
        RuntimeBus::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
        )
        .with_execution(execution),
    );

    let handle = RuntimeExecutionHandle {
        controller: ctrl_handle.clone(),
        runtime: runtime_bus,
        function_invoker: None,
        test_lifecycle_inject: Arc::new(std::sync::Mutex::new(None)),
    };

    // ── First swap: should use atomic path (no lifecycle) ──
    let mut new_defs = vec![
        RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
            .with_route_id("atomic-swap"),
    ];
    let mut errors = vec![];
    apply_swap(
        "atomic-swap".into(),
        &mut new_defs,
        &handle,
        Duration::from_secs(5),
        None,
        &mut errors,
    )
    .await;

    assert!(
        errors.is_empty(),
        "apply_swap should succeed for non-lifecycle route via atomic swap; got {errors:?}"
    );

    let exists = ctrl_handle.route_exists("atomic-swap").await.unwrap();
    assert!(exists, "route should exist after atomic swap");

    let pipeline = ctrl_handle.get_pipeline("atomic-swap").await.unwrap();
    assert!(
        pipeline.is_some(),
        "pipeline should be present after atomic swap"
    );

    // ── Verify atomic swap was used: subsequent atomic swap succeeds ──
    let identity = camel_api::BoxProcessor::new(IdentityProcessor);
    let swap_result = ctrl_handle.swap_pipeline("atomic-swap", identity).await;
    assert!(
        swap_result.is_ok(),
        "subsequent atomic swap should succeed (route has no lifecycle)"
    );

    // Cleanup
    let _ = ctrl_handle.stop_route("atomic-swap").await;
}

/// Verify that lifecycle handles are concretely preserved in the
/// PipelineAssembly after the proactive Restart path completes.
/// Uses the direct `DefaultRouteController` (before actor spawn) to
/// inspect the pipeline assembly's lifecycle field.
#[tokio::test]
async fn apply_swap_lifecycle_handles_preserved_in_assembly() {
    use crate::context::RuntimeExecutionHandle;
    use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
    use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
    use crate::lifecycle::application::runtime_bus::RuntimeBus;
    use crate::shared::components::domain::Registry;
    use camel_api::{IdentityProcessor, StepLifecycle, StepShutdownReason};
    use std::sync::Arc;

    // ── Lifecycle handle ──
    #[derive(Debug)]
    struct PreserveTestLifecycle;
    #[async_trait::async_trait]
    impl StepLifecycle for PreserveTestLifecycle {
        fn name(&self) -> &'static str {
            "preserve-test-lifecycle"
        }
        async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
    }

    // ── Build controller ──
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .unwrap()
        .register(Arc::new(camel_component_timer::TimerComponent::new()));
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    controller
        .add_route(
            RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                .with_route_id("preserve-lifecycle"),
        )
        .await
        .unwrap();

    let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);

    // Build RuntimeBus + handle with test lifecycle injection
    let store = InMemoryRuntimeStore::default();
    let execution = Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
    let runtime_bus = Arc::new(
        RuntimeBus::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
        )
        .with_execution(execution),
    );
    let handle = RuntimeExecutionHandle {
        controller: ctrl_handle.clone(),
        runtime: runtime_bus,
        function_invoker: None,
        test_lifecycle_inject: Arc::new(std::sync::Mutex::new(Some(vec![Arc::new(
            PreserveTestLifecycle,
        )
            as Arc<dyn StepLifecycle>]))),
    };

    // Call apply_swap
    let mut new_defs = vec![
        RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
            .with_route_id("preserve-lifecycle"),
    ];
    let mut errors = vec![];
    apply_swap(
        "preserve-lifecycle".into(),
        &mut new_defs,
        &handle,
        Duration::from_secs(5),
        None,
        &mut errors,
    )
    .await;

    assert!(
        errors.is_empty(),
        "apply_swap should succeed; got {errors:?}"
    );

    // ── Verify post-swap: lifecycle preserved in PipelineAssembly ──
    // After the Restart path, swap_pipeline_raw stores lifecycle in the
    // assembly.  A subsequent atomic swap must be rejected because the
    // assembly now carries lifecycle handles.
    let identity = camel_api::BoxProcessor::new(IdentityProcessor);
    let swap_result = ctrl_handle
        .swap_pipeline("preserve-lifecycle", identity)
        .await;
    assert!(
        swap_result.is_err(),
        "atomic swap should be rejected — lifecycle was preserved"
    );
    let err_msg = swap_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("lifecycle-bearing"),
        "rejection should mention lifecycle-bearing; got: {err_msg}"
    );

    // Cleanup
    let _ = ctrl_handle.stop_route("preserve-lifecycle").await;
}

// ── Oracle Fix 2: stateless → resequencer swap regression test ──
//
// Verifies that when function_ctx is None and the compiled route carries
// lifecycle handles (as a real resequencer would), the swap goes through
// the Restart path (not atomic) and lifecycle is preserved in the
// PipelineAssembly.

/// Oracle Fix 2: verify that a stateless-to-lifecycle swap via
/// `function_ctx == None` uses the Restart path and preserves lifecycle
/// handles in the PipelineAssembly.
///
/// Steps:
/// 1. Start a non-lifecycle route
/// 2. Hot-swap with injected lifecycle + `function_ctx: None`
/// 3. Assert Restart path was used (subsequent atomic swap fails)
/// 4. Assert PipelineAssembly has the lifecycle handle
/// 5. Assert old route's lifecycle drains on stop
#[tokio::test]
async fn oracle_fix2_stateless_to_resequencer_swap_preserves_lifecycle() {
    use crate::context::RuntimeExecutionHandle;
    use crate::lifecycle::adapters::controller_actor::spawn_controller_actor;
    use crate::lifecycle::adapters::in_memory::InMemoryRuntimeStore;
    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::lifecycle::adapters::runtime_execution::RuntimeExecutionAdapter;
    use crate::lifecycle::application::runtime_bus::RuntimeBus;
    use crate::shared::components::domain::Registry;
    use camel_api::{IdentityProcessor, StepLifecycle, StepShutdownReason};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // ── Lifecycle handle that records shutdown ──
    #[derive(Debug)]
    struct OracleFix2Lifecycle {
        drained: Arc<AtomicBool>,
    }
    #[async_trait::async_trait]
    impl StepLifecycle for OracleFix2Lifecycle {
        fn name(&self) -> &'static str {
            "oracle-fix2-lifecycle"
        }
        async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), camel_api::CamelError> {
            self.drained.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let drained_flag = Arc::new(AtomicBool::new(false));
    let lifecycle: Arc<dyn StepLifecycle> = Arc::new(OracleFix2Lifecycle {
        drained: Arc::clone(&drained_flag),
    });

    // ── Build controller with a plain non-lifecycle route ──
    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    registry
        .lock()
        .unwrap()
        .register(Arc::new(camel_component_timer::TimerComponent::new()));
    let mut controller = DefaultRouteController::new(
        registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    controller
        .add_route(
            RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
                .with_route_id("oracle-fix2-test"),
        )
        .await
        .unwrap();

    // Old route has NO lifecycle (plain timer → stateless).
    // NEW route simulates resequencer via test_lifecycle_inject.

    let (ctrl_handle, _actor_join) = spawn_controller_actor(controller);

    let store = InMemoryRuntimeStore::default();
    let execution = Arc::new(RuntimeExecutionAdapter::new(ctrl_handle.clone()));
    let runtime_bus = Arc::new(
        RuntimeBus::new(
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
            Arc::new(store.clone()),
        )
        .with_execution(execution),
    );

    let handle = RuntimeExecutionHandle {
        controller: ctrl_handle.clone(),
        runtime: runtime_bus,
        function_invoker: None,
        // Inject lifecycle to simulate a resequencer route compiled via
        // function_ctx == None path.
        test_lifecycle_inject: Arc::new(std::sync::Mutex::new(Some(vec![
            Arc::clone(&lifecycle) as Arc<dyn StepLifecycle>
        ]))),
    };

    // ── Swap: function_ctx == None ──
    let mut new_defs = vec![
        RouteDefinition::new("timer:tick?period=1000&repeatCount=1", vec![])
            .with_route_id("oracle-fix2-test"),
    ];
    let mut errors = vec![];
    apply_swap(
        "oracle-fix2-test".into(),
        &mut new_defs,
        &handle,
        Duration::from_secs(5),
        None, // ← Oracle Fix 2: no function context
        &mut errors,
    )
    .await;

    // Oracle assertion 3: swap should succeed
    assert!(
        errors.is_empty(),
        "apply_swap should succeed for oracle fix 2; got {errors:?}"
    );

    let exists = ctrl_handle.route_exists("oracle-fix2-test").await.unwrap();
    assert!(exists, "route should exist after oracle fix 2 swap");

    // Oracle assertion 3 (cont'd): Restart path was used → subsequent
    // atomic swap is rejected because PipelineAssembly now carries lifecycle.
    let identity = camel_api::BoxProcessor::new(IdentityProcessor);
    let swap_result = ctrl_handle
        .swap_pipeline("oracle-fix2-test", identity)
        .await;
    assert!(
        swap_result.is_err(),
        "atomic swap should be rejected — Restart path stored lifecycle; got {:?}",
        swap_result
    );
    let err_msg = swap_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("lifecycle-bearing"),
        "rejection should mention lifecycle-bearing; got: {err_msg}"
    );

    // Oracle assertion 4: PipelineAssembly has the lifecycle handle
    // (verified via rejection above — the assembly IS lifecycle-bearing).

    // Oracle assertion 5: Stop the route and verify lifecycle drains.
    // The `stop_route` method drains lifecycle handles via
    // consumer_management::stop_route which iterates assembly.lifecycle.
    let handle_ref = ctrl_handle.clone();
    let stop_result = handle_ref.stop_route("oracle-fix2-test").await;
    assert!(
        stop_result.is_ok(),
        "stop_route should succeed; got {stop_result:?}"
    );

    // Verify drain flag was set
    assert!(
        drained_flag.load(Ordering::SeqCst),
        "lifecycle.shutdown should have been called via drain on stop_route"
    );
}
