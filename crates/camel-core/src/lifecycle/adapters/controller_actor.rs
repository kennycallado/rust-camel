use std::sync::Arc;
use std::time::Instant;

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{
    BoxProcessor, CamelError, MetricsCollector, RouteController, RuntimeHandle, SupervisionConfig,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{error, info};

use super::route_controller::{CrashNotification, DefaultRouteController};
use crate::lifecycle::application::route_definition::RouteDefinition;
use crate::shared::observability::domain::TracerConfig;

pub(crate) enum RouteControllerCommand {
    StartRoute {
        route_id: String,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    StopRoute {
        route_id: String,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    RestartRoute {
        route_id: String,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    SuspendRoute {
        route_id: String,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    ResumeRoute {
        route_id: String,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    StartAllRoutes {
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    StopAllRoutes {
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    AddRoute {
        definition: RouteDefinition,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    RemoveRoute {
        route_id: String,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    SwapPipeline {
        route_id: String,
        pipeline: BoxProcessor,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    CompileRouteDefinition {
        definition: RouteDefinition,
        reply: oneshot::Sender<Result<BoxProcessor, CamelError>>,
    },
    RouteFromUri {
        route_id: String,
        reply: oneshot::Sender<Option<String>>,
    },
    SetErrorHandler {
        config: ErrorHandlerConfig,
    },
    SetTracerConfig {
        config: TracerConfig,
    },
    RouteCount {
        reply: oneshot::Sender<usize>,
    },
    InFlightCount {
        route_id: String,
        reply: oneshot::Sender<Option<u64>>,
    },
    RouteExists {
        route_id: String,
        reply: oneshot::Sender<bool>,
    },
    RouteIds {
        reply: oneshot::Sender<Vec<String>>,
    },
    AutoStartupRouteIds {
        reply: oneshot::Sender<Vec<String>>,
    },
    ShutdownRouteIds {
        reply: oneshot::Sender<Vec<String>>,
    },
    GetPipeline {
        route_id: String,
        reply: oneshot::Sender<Option<BoxProcessor>>,
    },
    StartRouteReload {
        route_id: String,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    StopRouteReload {
        route_id: String,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    SetRuntimeHandle {
        runtime: Arc<dyn RuntimeHandle>,
    },
    RouteSourceHash {
        route_id: String,
        reply: oneshot::Sender<Option<u64>>,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct RouteControllerHandle {
    tx: mpsc::Sender<RouteControllerCommand>,
}

impl RouteControllerHandle {
    pub async fn start_route(&self, route_id: impl Into<String>) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::StartRoute {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn stop_route(&self, route_id: impl Into<String>) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::StopRoute {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn restart_route(&self, route_id: impl Into<String>) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::RestartRoute {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn suspend_route(&self, route_id: impl Into<String>) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::SuspendRoute {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn resume_route(&self, route_id: impl Into<String>) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::ResumeRoute {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn start_all_routes(&self) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::StartAllRoutes { reply: reply_tx })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn stop_all_routes(&self) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::StopAllRoutes { reply: reply_tx })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn add_route(&self, definition: RouteDefinition) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::AddRoute {
                definition,
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn remove_route(&self, route_id: impl Into<String>) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::RemoveRoute {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn swap_pipeline(
        &self,
        route_id: impl Into<String>,
        pipeline: BoxProcessor,
    ) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::SwapPipeline {
                route_id: route_id.into(),
                pipeline,
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn compile_route_definition(
        &self,
        definition: RouteDefinition,
    ) -> Result<BoxProcessor, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::CompileRouteDefinition {
                definition,
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn route_from_uri(
        &self,
        route_id: impl Into<String>,
    ) -> Result<Option<String>, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::RouteFromUri {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))
    }

    pub async fn route_count(&self) -> Result<usize, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::RouteCount { reply: reply_tx })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))
    }

    pub async fn in_flight_count(
        &self,
        route_id: impl Into<String>,
    ) -> Result<Option<u64>, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::InFlightCount {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))
    }

    pub async fn route_exists(&self, route_id: impl Into<String>) -> Result<bool, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::RouteExists {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))
    }

    pub async fn route_ids(&self) -> Result<Vec<String>, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::RouteIds { reply: reply_tx })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))
    }

    pub async fn auto_startup_route_ids(&self) -> Result<Vec<String>, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::AutoStartupRouteIds { reply: reply_tx })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))
    }

    pub async fn shutdown_route_ids(&self) -> Result<Vec<String>, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::ShutdownRouteIds { reply: reply_tx })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))
    }

    pub async fn start_route_reload(&self, route_id: impl Into<String>) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::StartRouteReload {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn stop_route_reload(&self, route_id: impl Into<String>) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::StopRouteReload {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn get_pipeline(
        &self,
        route_id: impl Into<String>,
    ) -> Result<Option<BoxProcessor>, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::GetPipeline {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))
    }

    pub async fn set_error_handler(&self, config: ErrorHandlerConfig) -> Result<(), CamelError> {
        self.tx
            .send(RouteControllerCommand::SetErrorHandler { config })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))
    }

    pub async fn set_tracer_config(&self, config: TracerConfig) -> Result<(), CamelError> {
        self.tx
            .send(RouteControllerCommand::SetTracerConfig { config })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))
    }

    pub async fn set_runtime_handle(
        &self,
        runtime: Arc<dyn RuntimeHandle>,
    ) -> Result<(), CamelError> {
        self.tx
            .send(RouteControllerCommand::SetRuntimeHandle { runtime })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))
    }

    pub fn try_set_runtime_handle(
        &self,
        runtime: Arc<dyn RuntimeHandle>,
    ) -> Result<(), CamelError> {
        self.tx
            .try_send(RouteControllerCommand::SetRuntimeHandle { runtime })
            .map_err(|err| {
                CamelError::ProcessorError(format!("controller actor mailbox full: {err}"))
            })
    }

    pub async fn route_source_hash(&self, route_id: impl Into<String>) -> Option<u64> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::RouteSourceHash {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .ok()?;
        reply_rx.await.ok()?
    }

    pub async fn shutdown(&self) -> Result<(), CamelError> {
        self.tx
            .send(RouteControllerCommand::Shutdown)
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))
    }
}

pub fn spawn_controller_actor(
    controller: DefaultRouteController,
) -> (RouteControllerHandle, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<RouteControllerCommand>(256);
    let handle = tokio::spawn(async move {
        let mut controller = controller;
        while let Some(cmd) = rx.recv().await {
            match cmd {
                RouteControllerCommand::StartRoute { route_id, reply } => {
                    let _ = reply.send(controller.start_route(&route_id).await);
                }
                RouteControllerCommand::StopRoute { route_id, reply } => {
                    let _ = reply.send(controller.stop_route(&route_id).await);
                }
                RouteControllerCommand::RestartRoute { route_id, reply } => {
                    let _ = reply.send(controller.restart_route(&route_id).await);
                }
                RouteControllerCommand::SuspendRoute { route_id, reply } => {
                    let _ = reply.send(controller.suspend_route(&route_id).await);
                }
                RouteControllerCommand::ResumeRoute { route_id, reply } => {
                    let _ = reply.send(controller.resume_route(&route_id).await);
                }
                RouteControllerCommand::StartAllRoutes { reply } => {
                    let _ = reply.send(controller.start_all_routes().await);
                }
                RouteControllerCommand::StopAllRoutes { reply } => {
                    let _ = reply.send(controller.stop_all_routes().await);
                }
                RouteControllerCommand::AddRoute { definition, reply } => {
                    let _ = reply.send(controller.add_route(definition));
                }
                RouteControllerCommand::RemoveRoute { route_id, reply } => {
                    let _ = reply.send(controller.remove_route(&route_id));
                }
                RouteControllerCommand::SwapPipeline {
                    route_id,
                    pipeline,
                    reply,
                } => {
                    let _ = reply.send(controller.swap_pipeline(&route_id, pipeline));
                }
                RouteControllerCommand::CompileRouteDefinition { definition, reply } => {
                    let _ = reply.send(controller.compile_route_definition(definition));
                }
                RouteControllerCommand::RouteFromUri { route_id, reply } => {
                    let _ = reply.send(controller.route_from_uri(&route_id));
                }
                RouteControllerCommand::SetErrorHandler { config } => {
                    controller.set_error_handler(config);
                }
                RouteControllerCommand::SetTracerConfig { config } => {
                    controller.set_tracer_config(&config);
                }
                RouteControllerCommand::RouteCount { reply } => {
                    let _ = reply.send(controller.route_count());
                }
                RouteControllerCommand::InFlightCount { route_id, reply } => {
                    let _ = reply.send(controller.in_flight_count(&route_id));
                }
                RouteControllerCommand::RouteExists { route_id, reply } => {
                    let _ = reply.send(controller.route_exists(&route_id));
                }
                RouteControllerCommand::RouteIds { reply } => {
                    let _ = reply.send(controller.route_ids());
                }
                RouteControllerCommand::AutoStartupRouteIds { reply } => {
                    let _ = reply.send(controller.auto_startup_route_ids());
                }
                RouteControllerCommand::ShutdownRouteIds { reply } => {
                    let _ = reply.send(controller.shutdown_route_ids());
                }
                RouteControllerCommand::GetPipeline { route_id, reply } => {
                    let _ = reply.send(controller.get_pipeline(&route_id));
                }
                RouteControllerCommand::StartRouteReload { route_id, reply } => {
                    let _ = reply.send(controller.start_route_reload(&route_id).await);
                }
                RouteControllerCommand::StopRouteReload { route_id, reply } => {
                    let _ = reply.send(controller.stop_route_reload(&route_id).await);
                }
                RouteControllerCommand::SetRuntimeHandle { runtime } => {
                    controller.set_runtime_handle(runtime);
                }
                RouteControllerCommand::RouteSourceHash { route_id, reply } => {
                    let _ = reply.send(controller.route_source_hash(&route_id));
                }
                RouteControllerCommand::Shutdown => {
                    break;
                }
            }
        }
    });
    (RouteControllerHandle { tx }, handle)
}

pub fn spawn_supervision_task(
    controller: RouteControllerHandle,
    config: SupervisionConfig,
    _metrics: Option<Arc<dyn MetricsCollector>>,
    mut crash_rx: mpsc::Receiver<CrashNotification>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut attempts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let mut last_restart_time: std::collections::HashMap<String, Instant> =
            std::collections::HashMap::new();
        let mut currently_restarting: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        info!("Supervision loop started");

        while let Some(notification) = crash_rx.recv().await {
            let route_id = notification.route_id;
            if currently_restarting.contains(&route_id) {
                continue;
            }

            if let Some(last_time) = last_restart_time.get(&route_id)
                && last_time.elapsed() >= config.initial_delay
            {
                attempts.insert(route_id.clone(), 0);
            }

            let current_attempt = attempts.entry(route_id.clone()).or_insert(0);
            *current_attempt += 1;

            if config
                .max_attempts
                .is_some_and(|max| *current_attempt > max)
            {
                error!(
                    route_id = %route_id,
                    attempts = *current_attempt,
                    "Route exceeded max restart attempts, giving up"
                );
                continue;
            }

            let delay = config.next_delay(*current_attempt);
            currently_restarting.insert(route_id.clone());
            tokio::time::sleep(delay).await;

            match controller.restart_route(route_id.clone()).await {
                Ok(()) => {
                    info!(route_id = %route_id, "Route restarted successfully");
                    last_restart_time.insert(route_id.clone(), Instant::now());
                }
                Err(err) => {
                    error!(route_id = %route_id, error = %err, "Failed to restart route");
                }
            }

            currently_restarting.remove(&route_id);
        }

        info!("Supervision loop ended");
    })
}

#[cfg(test)]
mod tests {
    use super::{
        RouteControllerCommand, RouteControllerHandle, spawn_controller_actor,
        spawn_supervision_task,
    };
    use crate::lifecycle::adapters::route_controller::{CrashNotification, DefaultRouteController};
    use crate::lifecycle::application::route_definition::RouteDefinition;
    use crate::shared::components::domain::Registry;
    use crate::shared::observability::domain::TracerConfig;
    use camel_api::{
        CamelError, ErrorHandlerConfig, RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult,
        RuntimeQuery, RuntimeQueryBus, RuntimeQueryResult, SupervisionConfig,
    };
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio::time::sleep;

    fn build_actor_with_components() -> (RouteControllerHandle, tokio::task::JoinHandle<()>) {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        {
            let mut guard = registry.lock().expect("lock");
            guard.register(camel_component_timer::TimerComponent::new());
            guard.register(camel_component_mock::MockComponent::new());
        }
        let controller = DefaultRouteController::new(Arc::clone(&registry));
        spawn_controller_actor(controller)
    }

    fn build_empty_actor() -> (RouteControllerHandle, tokio::task::JoinHandle<()>) {
        let controller =
            DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new())));
        spawn_controller_actor(controller)
    }

    fn route_def(route_id: &str, from_uri: &str) -> RouteDefinition {
        RouteDefinition::new(from_uri, vec![]).with_route_id(route_id)
    }

    struct NoopRuntime;

    #[async_trait::async_trait]
    impl RuntimeCommandBus for NoopRuntime {
        async fn execute(&self, _cmd: RuntimeCommand) -> Result<RuntimeCommandResult, CamelError> {
            Ok(RuntimeCommandResult::Accepted)
        }
    }

    #[async_trait::async_trait]
    impl RuntimeQueryBus for NoopRuntime {
        async fn ask(&self, query: RuntimeQuery) -> Result<RuntimeQueryResult, CamelError> {
            Ok(match query {
                RuntimeQuery::GetRouteStatus { route_id }
                | RuntimeQuery::InFlightCount { route_id } => {
                    RuntimeQueryResult::RouteNotFound { route_id }
                }
                RuntimeQuery::ListRoutes => RuntimeQueryResult::Routes {
                    route_ids: Vec::new(),
                },
            })
        }
    }

    #[tokio::test]
    async fn start_route_sends_command_and_returns_reply() {
        let (tx, mut rx) = mpsc::channel(1);
        let handle = RouteControllerHandle { tx };

        let task = tokio::spawn(async move { handle.start_route("route-a").await });

        let command = rx.recv().await.expect("command should be received");
        match command {
            RouteControllerCommand::StartRoute { route_id, reply } => {
                assert_eq!(route_id, "route-a");
                let _ = reply.send(Ok(()));
            }
            _ => panic!("unexpected command variant"),
        }

        let result = task.await.expect("join should succeed");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn start_route_returns_error_when_actor_stops() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);

        let handle = RouteControllerHandle { tx };
        let result = handle.start_route("route-a").await;

        assert!(matches!(result, Err(CamelError::ProcessorError(_))));
    }

    #[tokio::test]
    async fn spawn_controller_actor_processes_commands_and_shutdown() {
        let controller =
            DefaultRouteController::new(Arc::new(std::sync::Mutex::new(Registry::new())));
        let (handle, join_handle) = spawn_controller_actor(controller);

        assert_eq!(handle.route_count().await.expect("route_count"), 0);
        assert_eq!(
            handle.route_ids().await.expect("route_ids"),
            Vec::<String>::new()
        );

        handle.shutdown().await.expect("shutdown send");
        join_handle.await.expect("actor join");
    }

    #[tokio::test]
    async fn actor_handle_introspection_and_mutation_commands() {
        let (handle, join_handle) = build_actor_with_components();
        let definition = route_def("h-1", "timer:tick?period=100");

        handle.add_route(definition).await.expect("add route");
        assert!(handle.route_exists("h-1").await.expect("route exists h-1"));
        assert!(!handle
            .route_exists("no-such")
            .await
            .expect("route exists no-such"));

        let from_uri = handle.route_from_uri("h-1").await.expect("route_from_uri");
        assert_eq!(from_uri.as_deref(), Some("timer:tick?period=100"));
        assert_eq!(handle.route_count().await.expect("route_count"), 1);

        let auto_ids = handle
            .auto_startup_route_ids()
            .await
            .expect("auto_startup_route_ids");
        assert!(auto_ids.iter().any(|id| id == "h-1"));

        let shutdown_ids = handle
            .shutdown_route_ids()
            .await
            .expect("shutdown_route_ids");
        assert!(shutdown_ids.iter().any(|id| id == "h-1"));

        let compiled = handle
            .compile_route_definition(route_def("h-1", "timer:tick?period=100"))
            .await
            .expect("compile_route_definition");

        assert!(handle
            .get_pipeline("h-1")
            .await
            .expect("get_pipeline")
            .is_some());
        handle
            .swap_pipeline("h-1", compiled)
            .await
            .expect("swap_pipeline");

        let _ = handle
            .in_flight_count("h-1")
            .await
            .expect("in_flight_count");
        let _ = handle.route_source_hash("h-1").await;

        handle
            .set_error_handler(ErrorHandlerConfig::dead_letter_channel("log:dlq"))
            .await
            .expect("set_error_handler");
        handle
            .set_tracer_config(TracerConfig::default())
            .await
            .expect("set_tracer_config");
        handle
            .set_runtime_handle(Arc::new(NoopRuntime))
            .await
            .expect("set_runtime_handle");

        handle.remove_route("h-1").await.expect("remove_route");
        assert_eq!(handle.route_count().await.expect("route_count after remove"), 0);
        handle
            .stop_all_routes()
            .await
            .expect("stop_all_routes on empty");

        handle.shutdown().await.expect("shutdown send");
        join_handle.await.expect("actor join");
    }

    #[tokio::test]
    async fn actor_handle_lifecycle_start_stop_restart_suspend_resume() {
        let (handle, join_handle) = build_actor_with_components();
        handle
            .add_route(route_def("lc-1", "timer:tick?period=50"))
            .await
            .expect("add route lc-1");

        handle.start_route("lc-1").await.expect("start_route");
        sleep(Duration::from_millis(20)).await;

        handle.restart_route("lc-1").await.expect("restart_route");
        sleep(Duration::from_millis(20)).await;

        handle.suspend_route("lc-1").await.expect("suspend_route");
        handle.resume_route("lc-1").await.expect("resume_route");
        sleep(Duration::from_millis(20)).await;

        handle.stop_route("lc-1").await.expect("stop_route");
        handle.start_all_routes().await.expect("start_all_routes");
        sleep(Duration::from_millis(20)).await;
        handle.stop_all_routes().await.expect("stop_all_routes");

        handle
            .start_route_reload("lc-1")
            .await
            .expect("start_route_reload");
        handle
            .stop_route_reload("lc-1")
            .await
            .expect("stop_route_reload");

        handle.shutdown().await.expect("shutdown send");
        join_handle.await.expect("actor join");
    }

    #[tokio::test]
    async fn spawn_supervision_restarts_route_on_crash() {
        let (handle, join_handle) = build_actor_with_components();
        handle
            .add_route(route_def("sup-1", "timer:tick?period=100"))
            .await
            .expect("add route sup-1");
        handle.start_route("sup-1").await.expect("start_route sup-1");

        let (crash_tx, crash_rx) = mpsc::channel(8);
        let supervision = spawn_supervision_task(
            handle.clone(),
            SupervisionConfig {
                initial_delay: Duration::from_millis(10),
                max_attempts: Some(2),
                ..SupervisionConfig::default()
            },
            None,
            crash_rx,
        );

        crash_tx
            .send(CrashNotification {
                route_id: "sup-1".to_string(),
                error: "simulated".to_string(),
            })
            .await
            .expect("send crash notification");

        sleep(Duration::from_millis(150)).await;
        drop(crash_tx);
        supervision.await.expect("supervision join");

        handle.shutdown().await.expect("shutdown send");
        join_handle.await.expect("actor join");
    }

    #[tokio::test]
    async fn supervision_skips_duplicate_and_gives_up_after_max_attempts() {
        let (handle, join_handle) = build_actor_with_components();
        handle
            .add_route(route_def("sup-2", "timer:tick?period=100"))
            .await
            .expect("add route sup-2");
        handle.start_route("sup-2").await.expect("start_route sup-2");

        let (crash_tx, crash_rx) = mpsc::channel(8);
        let supervision = spawn_supervision_task(
            handle.clone(),
            SupervisionConfig {
                initial_delay: Duration::from_millis(10),
                max_attempts: Some(1),
                ..SupervisionConfig::default()
            },
            None,
            crash_rx,
        );

        crash_tx
            .send(CrashNotification {
                route_id: "sup-2".to_string(),
                error: "attempt-1".to_string(),
            })
            .await
            .expect("send crash attempt-1");
        crash_tx
            .send(CrashNotification {
                route_id: "sup-2".to_string(),
                error: "attempt-2".to_string(),
            })
            .await
            .expect("send crash attempt-2");

        sleep(Duration::from_millis(200)).await;
        drop(crash_tx);
        supervision.await.expect("supervision join");

        handle.shutdown().await.expect("shutdown send");
        join_handle.await.expect("actor join");
    }

    #[tokio::test]
    async fn try_set_runtime_handle_succeeds_on_fresh_actor() {
        let (handle, join_handle) = build_empty_actor();

        handle
            .try_set_runtime_handle(Arc::new(NoopRuntime))
            .expect("try_set_runtime_handle should succeed");

        handle.shutdown().await.expect("shutdown send");
        join_handle.await.expect("actor join");
    }

    #[tokio::test]
    async fn shutdown_returns_error_when_actor_stopped() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);

        let handle = RouteControllerHandle { tx };
        let result = handle.shutdown().await;

        assert!(matches!(result, Err(CamelError::ProcessorError(_))));
    }
}
