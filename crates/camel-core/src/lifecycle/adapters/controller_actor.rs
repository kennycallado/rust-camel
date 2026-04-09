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
    use super::{RouteControllerCommand, RouteControllerHandle, spawn_controller_actor};
    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::shared::components::domain::Registry;
    use camel_api::CamelError;
    use std::sync::Arc;
    use tokio::sync::mpsc;

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
}
