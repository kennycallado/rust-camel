//! Actor command types and handle — the control-plane channel façade.
//!
//! Extracted from [`controller_actor`](super::controller_actor) to reduce its size.
//! Contains:
//! - [`RouteControllerCommand`] — enum of all commands the actor can process
//! - [`RouteControllerHandle`] — a cloneable handle that sends commands over an mpsc channel

use std::sync::Arc;

use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::{BoxProcessor, CamelError, FunctionInvoker, RuntimeHandle, StepLifecycle};
use tokio::sync::{mpsc, oneshot};

use super::route_helpers::{CompiledPipeline, PreparedRoute};
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
    /// Raw swap — skips lifecycle/aggregate rejection check.
    /// Used by the Restart path after the route has been stopped.
    SwapPipelineRaw {
        route_id: String,
        pipeline: BoxProcessor,
        lifecycle: Vec<Arc<dyn StepLifecycle>>,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    CompileRouteDefinition {
        definition: RouteDefinition,
        reply: oneshot::Sender<Result<BoxProcessor, CamelError>>,
    },
    CompileRouteDefinitionWithGeneration {
        definition: RouteDefinition,
        generation: u64,
        reply: oneshot::Sender<Result<BoxProcessor, CamelError>>,
    },
    CompileRouteDefinitionPipeline {
        definition: RouteDefinition,
        generation: u64,
        reply: oneshot::Sender<Result<CompiledPipeline, CamelError>>,
    },
    /// Compile without function generation, returning full CompiledPipeline.
    /// Oracle Fix 1: stateless hot-reload path preserves lifecycle handles.
    CompileRouteDefinitionDryPipeline {
        definition: RouteDefinition,
        reply: oneshot::Sender<Result<CompiledPipeline, CamelError>>,
    },
    PrepareRouteDefinitionWithGeneration {
        definition: RouteDefinition,
        generation: u64,
        reply: oneshot::Sender<Result<PreparedRoute, CamelError>>,
    },
    InsertPreparedRoute {
        prepared: PreparedRoute,
        reply: oneshot::Sender<Result<(), CamelError>>,
    },
    RemoveRoutePreservingFunctions {
        route_id: String,
        reply: oneshot::Sender<Result<(), CamelError>>,
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
    SetFunctionInvoker {
        invoker: Arc<dyn FunctionInvoker>,
    },
    RouteSourceHash {
        route_id: String,
        reply: oneshot::Sender<Option<u64>>,
    },
    RouteHasLifecycle {
        route_id: String,
        reply: oneshot::Sender<bool>,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct RouteControllerHandle {
    pub(crate) tx: mpsc::Sender<RouteControllerCommand>,
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

    pub(crate) async fn swap_pipeline_raw(
        &self,
        route_id: impl Into<String>,
        pipeline: BoxProcessor,
        lifecycle: Vec<Arc<dyn StepLifecycle>>,
    ) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::SwapPipelineRaw {
                route_id: route_id.into(),
                pipeline,
                lifecycle,
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

    pub async fn compile_route_definition_with_generation(
        &self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<BoxProcessor, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(
                RouteControllerCommand::CompileRouteDefinitionWithGeneration {
                    definition,
                    generation,
                    reply: reply_tx,
                },
            )
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub(crate) async fn compile_route_definition_pipeline(
        &self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<CompiledPipeline, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::CompileRouteDefinitionPipeline {
                definition,
                generation,
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    /// Compile without function generation, returning full [`CompiledPipeline`].
    pub(crate) async fn compile_route_definition_dry_pipeline(
        &self,
        definition: RouteDefinition,
    ) -> Result<CompiledPipeline, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::CompileRouteDefinitionDryPipeline {
                definition,
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub(crate) async fn prepare_route_definition_with_generation(
        &self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<PreparedRoute, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(
                RouteControllerCommand::PrepareRouteDefinitionWithGeneration {
                    definition,
                    generation,
                    reply: reply_tx,
                },
            )
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub(crate) async fn insert_prepared_route(
        &self,
        prepared: PreparedRoute,
    ) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::InsertPreparedRoute {
                prepared,
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))?
    }

    pub async fn remove_route_preserving_functions(
        &self,
        route_id: String,
    ) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::RemoveRoutePreservingFunctions {
                route_id,
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

    pub async fn set_function_invoker(
        &self,
        invoker: Arc<dyn FunctionInvoker>,
    ) -> Result<(), CamelError> {
        self.tx
            .send(RouteControllerCommand::SetFunctionInvoker { invoker })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))
    }

    pub fn try_set_function_invoker(
        &self,
        invoker: Arc<dyn FunctionInvoker>,
    ) -> Result<(), CamelError> {
        self.tx
            .try_send(RouteControllerCommand::SetFunctionInvoker { invoker })
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

    pub(crate) async fn route_has_lifecycle(
        &self,
        route_id: impl Into<String>,
    ) -> Result<bool, CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RouteControllerCommand::RouteHasLifecycle {
                route_id: route_id.into(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor dropped reply".into()))
    }

    pub async fn shutdown(&self) -> Result<(), CamelError> {
        self.tx
            .send(RouteControllerCommand::Shutdown)
            .await
            .map_err(|_| CamelError::ProcessorError("controller actor stopped".into()))
    }
}
