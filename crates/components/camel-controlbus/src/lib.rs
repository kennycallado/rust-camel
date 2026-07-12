//! ControlBus component for managing route lifecycle.
//!
//! This component allows routes to control other routes in the Camel context
//! using the ControlBus EIP pattern. It provides operations like start, stop,
//! suspend, resume, and restart for routes.
//!
//! TODO(CTRL-001): `SuspendRoute` and `ResumeRoute` commands are dispatched to
//!   the runtime but route-level suspension/resumption semantics (pausing message
//!   consumption without stopping the route) depend on runtime support that may
//!   not be fully implemented in all components.
//!
//! TODO(CTRL-002): The `Status` action returns only the route lifecycle status
//!   string. Full route statistics (total exchange count, failure count, mean
//!   processing time, last exchange timestamp) are not yet populated. A future
//!   `Stats` command should return a structured JSON body with these metrics.
//!
//! # URI Format
//!
//! `controlbus:route?routeId=my-route&action=start&authorizedRoutes=my-route,other-route`
//!
//! # Parameters
//!
//! - `routeId`: The ID of the route to operate on. **Required** — declared statically in the
//!   URI. The `CamelRouteId` exchange header override is denied (exchange data is untrusted;
//!   see ADR-0032 and ADR-0034).
//! - `action`: The action to perform: `start`, `stop`, `suspend`, `resume`, `restart`, `status`
//! - `authorizedRoutes`: Comma-separated allowlist of route IDs this endpoint may target.
//!   **Required** — when absent the endpoint fails closed (every command rejected).
//!
//! # Example
//!
//! ```ignore
//! // Start a route
//! from("timer:start?period=60000")
//!     .to("controlbus:route?routeId=my-route&action=start");
//!
//! // Get route status
//! from("direct:getStatus")
//!     .to("controlbus:route?routeId=my-route&action=status")
//!     .log("Status: ${body}");
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

#[cfg(test)]
use async_trait::async_trait;
#[cfg(test)]
use tokio::sync::Mutex;
use tower::Service;
use tracing::debug;

use camel_component_api::{
    Body, BoxProcessor, CamelError, Exchange, RouteAction, RuntimeCommand, RuntimeHandle,
    RuntimeQuery, RuntimeQueryResult, parse_uri,
};
use camel_component_api::{Component, Consumer, Endpoint, ProducerContext, RuntimeObservability};
#[cfg(test)]
use camel_component_api::{RouteStatus, RuntimeCommandBus, RuntimeCommandResult, RuntimeQueryBus};

// ---------------------------------------------------------------------------
// ControlBusComponent
// ---------------------------------------------------------------------------

/// The ControlBus component for managing route lifecycle.
pub struct ControlBusComponent;

impl ControlBusComponent {
    /// Create a new ControlBus component.
    pub fn new() -> Self {
        Self
    }
}

impl Default for ControlBusComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for ControlBusComponent {
    fn scheme(&self) -> &str {
        "controlbus"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let parts = parse_uri(uri)?;

        if parts.scheme != "controlbus" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'controlbus', got '{}'",
                parts.scheme
            )));
        }

        // Parse command (path portion after controlbus:)
        let command = parts.path.clone();

        // Validate command - only "route" is supported
        if command != "route" {
            return Err(CamelError::EndpointCreationFailed(format!(
                "controlbus: unknown command '{}', only 'route' is supported",
                command
            )));
        }

        // Parse routeId parameter
        let route_id = parts.params.get("routeId").cloned();

        // Parse action parameter
        let action = if let Some(action_str) = parts.params.get("action") {
            Some(parse_action(action_str)?)
        } else {
            None
        };

        // Validate: for "route" command, action is required
        if command == "route" && action.is_none() {
            return Err(CamelError::EndpointCreationFailed(
                "controlbus: 'action' parameter is required for route command".to_string(),
            ));
        }

        // Parse authorizedRoutes (R4-H1: capability authz allowlist).
        // The CamelRouteId header override is denied — target routeId must
        // be declared statically in the URI. Without authorizedRoutes the
        // endpoint fails closed (no route may be controlled).
        let authorized_routes: Option<Vec<String>> = parts
            .params
            .get("authorizedRoutes")
            .map(|s| s.split(',').map(|r| r.trim().to_string()).collect());

        Ok(Box::new(ControlBusEndpoint {
            uri: uri.to_string(),
            route_id,
            action,
            authorized_routes,
        }))
    }
}

/// Parse an action string into a RouteAction.
fn parse_action(s: &str) -> Result<RouteAction, CamelError> {
    match s.to_lowercase().as_str() {
        "start" => Ok(RouteAction::Start),
        "stop" => Ok(RouteAction::Stop),
        "suspend" => Ok(RouteAction::Suspend),
        "resume" => Ok(RouteAction::Resume),
        "restart" => Ok(RouteAction::Restart),
        "status" => Ok(RouteAction::Status),
        _ => Err(CamelError::EndpointCreationFailed(format!(
            "controlbus: unknown action '{}'",
            s
        ))),
    }
}

// ---------------------------------------------------------------------------
// ControlBusEndpoint
// ---------------------------------------------------------------------------

/// Endpoint for the ControlBus component.
struct ControlBusEndpoint {
    uri: String,
    route_id: Option<String>,
    action: Option<RouteAction>,
    /// Allowlist of route IDs this endpoint may target. `None` ⇒ fail-closed.
    authorized_routes: Option<Vec<String>>,
}

impl Endpoint for ControlBusEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "controlbus endpoint does not support consumers".to_string(),
        ))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let action = self.action.clone().ok_or_else(|| {
            CamelError::EndpointCreationFailed(
                "controlbus: action is required to create producer".to_string(),
            )
        })?;
        let runtime = ctx.runtime().cloned().ok_or_else(|| {
            CamelError::EndpointCreationFailed(
                "controlbus: runtime handle is required in ProducerContext".to_string(),
            )
        })?;

        // Capture the calling route ID for self-restart detection (R4-H1).
        let calling_route_id = ctx.route_id().map(|s| s.to_string());

        Ok(BoxProcessor::new(ControlBusProducer {
            route_id: self.route_id.clone(),
            action,
            runtime,
            authorized_routes: self.authorized_routes.clone(),
            calling_route_id,
        }))
    }
}

// ---------------------------------------------------------------------------
// ControlBusProducer
// ---------------------------------------------------------------------------

/// Producer that executes control bus actions on routes.
#[derive(Clone)]
struct ControlBusProducer {
    /// Route ID from URI params only. Header override is denied (R4-H1).
    route_id: Option<String>,
    /// Action to perform on the route.
    action: RouteAction,
    /// Runtime command/query handle.
    runtime: Arc<dyn RuntimeHandle>,
    /// Allowlist of route IDs that may be targeted. `None` ⇒ fail-closed.
    authorized_routes: Option<Vec<String>>,
    /// ID of the route that owns this producer (for self-restart deny).
    calling_route_id: Option<String>,
}

impl Service<Exchange> for ControlBusProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        // R4-H1: routeId MUST come from the URI only. The CamelRouteId
        // exchange header is untrusted data (ADR-0032) and may not override
        // the statically-declared target.
        let route_id = match self.route_id.clone() {
            Some(id) => id,
            None => {
                return Box::pin(async move {
                    Err(CamelError::Unauthorized(
                        "controlbus: routeId must be declared in URI (header override denied)"
                            .into(),
                    ))
                });
            }
        };

        // Capability authz gate. Fail-closed if no allowlist; deny self-restart.
        if let Err(e) = Self::authorize(
            &route_id,
            &self.authorized_routes,
            self.calling_route_id.as_deref(),
        ) {
            return Box::pin(async move { Err(e) });
        }

        let action = self.action.clone();
        let runtime = self.runtime.clone();
        let command_scope = format!("controlbus:{route_id}:{}", exchange.correlation_id());

        Box::pin(async move {
            debug!(
                route_id = %route_id,
                action = ?action,
                "ControlBus executing action"
            );

            match execute_runtime_action(runtime.as_ref(), &route_id, &action, &command_scope)
                .await?
            {
                Some(status) => {
                    exchange.input.body = Body::Text(status);
                    Ok(exchange)
                }
                None => {
                    exchange.input.body = Body::Empty;
                    Ok(exchange)
                }
            }
        })
    }
}

impl ControlBusProducer {
    /// Capability authz check. Three gates, all must pass:
    /// 1. `authorized_routes` is configured (fail-closed by default).
    /// 2. `route_id` is in the allowlist.
    /// 3. `route_id` is not the calling route (self-restart denied).
    fn authorize(
        route_id: &str,
        authorized_routes: &Option<Vec<String>>,
        calling_route_id: Option<&str>,
    ) -> Result<(), CamelError> {
        match authorized_routes {
            Some(list) if list.iter().any(|r| r == route_id) => {}
            Some(_) => {
                return Err(CamelError::Unauthorized(format!(
                    "controlbus: route '{route_id}' not in authorizedRoutes"
                )));
            }
            None => {
                return Err(CamelError::Unauthorized(
                    "controlbus: authorizedRoutes not configured — fail-closed".into(),
                ));
            }
        }

        if let Some(caller) = calling_route_id
            && caller == route_id
        {
            return Err(CamelError::Unauthorized(
                "controlbus: cannot control own route (self-restart denied)".into(),
            ));
        }

        Ok(())
    }
}

async fn execute_runtime_action(
    runtime: &dyn RuntimeHandle,
    route_id: &str,
    action: &RouteAction,
    command_scope: &str,
) -> Result<Option<String>, CamelError> {
    match action {
        RouteAction::Start => {
            runtime
                .execute(RuntimeCommand::StartRoute {
                    route_id: route_id.to_string(),
                    command_id: command_id(command_scope, "start"),
                    causation_id: None,
                })
                .await?;
            Ok(None)
        }
        RouteAction::Stop => {
            runtime
                .execute(RuntimeCommand::StopRoute {
                    route_id: route_id.to_string(),
                    command_id: command_id(command_scope, "stop"),
                    causation_id: None,
                })
                .await?;
            Ok(None)
        }
        RouteAction::Suspend => {
            runtime
                .execute(RuntimeCommand::SuspendRoute {
                    route_id: route_id.to_string(),
                    command_id: command_id(command_scope, "suspend"),
                    causation_id: None,
                })
                .await?;
            Ok(None)
        }
        RouteAction::Resume => {
            runtime
                .execute(RuntimeCommand::ResumeRoute {
                    route_id: route_id.to_string(),
                    command_id: command_id(command_scope, "resume"),
                    causation_id: None,
                })
                .await?;
            Ok(None)
        }
        RouteAction::Restart => {
            runtime
                .execute(RuntimeCommand::ReloadRoute {
                    route_id: route_id.to_string(),
                    command_id: command_id(command_scope, "restart"),
                    causation_id: None,
                })
                .await?;
            Ok(None)
        }
        RouteAction::Status => match runtime
            .ask(RuntimeQuery::GetRouteStatus {
                route_id: route_id.to_string(),
            })
            .await?
        {
            RuntimeQueryResult::RouteStatus { status, .. } => Ok(Some(status)),
            _ => Err(CamelError::ProcessorError(
                "controlbus: runtime returned unexpected response for route status".to_string(),
            )),
        },
    }
}

fn command_id(route_id: &str, operation: &str) -> String {
    format!("controlbus:{route_id}:{operation}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
    use super::*;
    use camel_component_api::Message;
    use camel_component_api::NoOpComponentContext;
    use tower::ServiceExt;

    struct MockRuntime {
        statuses: std::collections::HashMap<String, String>,
        commands: Arc<Mutex<Vec<String>>>,
    }

    impl MockRuntime {
        fn new() -> Self {
            Self {
                statuses: std::collections::HashMap::new(),
                commands: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_status(mut self, route_id: &str, status: &str) -> Self {
            self.statuses
                .insert(route_id.to_string(), status.to_string());
            self
        }

        fn commands(&self) -> Arc<Mutex<Vec<String>>> {
            Arc::clone(&self.commands)
        }
    }

    #[async_trait]
    impl RuntimeCommandBus for MockRuntime {
        async fn execute(&self, cmd: RuntimeCommand) -> Result<RuntimeCommandResult, CamelError> {
            let marker = match cmd {
                RuntimeCommand::RegisterRoute { .. } => "register".to_string(),
                RuntimeCommand::StartRoute { route_id, .. } => {
                    format!("start:{route_id}")
                }
                RuntimeCommand::StopRoute { route_id, .. } => {
                    format!("stop:{route_id}")
                }
                RuntimeCommand::SuspendRoute { route_id, .. } => {
                    format!("suspend:{route_id}")
                }
                RuntimeCommand::ResumeRoute { route_id, .. } => {
                    format!("resume:{route_id}")
                }
                RuntimeCommand::ReloadRoute { route_id, .. } => {
                    format!("reload:{route_id}")
                }
                RuntimeCommand::FailRoute { route_id, .. } => format!("fail:{route_id}"),
                RuntimeCommand::RemoveRoute { route_id, .. } => {
                    format!("remove:{route_id}")
                }
                RuntimeCommand::ReloadTlsCerts { .. } => "reload_tls_certs".to_string(),
            };
            self.commands.lock().await.push(marker);
            Ok(RuntimeCommandResult::Accepted)
        }
    }

    #[async_trait]
    impl RuntimeQueryBus for MockRuntime {
        async fn ask(&self, query: RuntimeQuery) -> Result<RuntimeQueryResult, CamelError> {
            match query {
                RuntimeQuery::GetRouteStatus { route_id } => {
                    let status = self.statuses.get(&route_id).ok_or_else(|| {
                        CamelError::ProcessorError(format!(
                            "runtime: route '{}' not found",
                            route_id
                        ))
                    })?;
                    Ok(RuntimeQueryResult::RouteStatus {
                        route_id,
                        status: status.clone(),
                    })
                }
                _ => Err(CamelError::ProcessorError(
                    "runtime: unsupported query in test".to_string(),
                )),
            }
        }
    }

    fn test_producer_ctx() -> ProducerContext {
        ProducerContext::new().with_runtime(Arc::new(MockRuntime::new()))
    }

    fn test_producer_ctx_with_route(id: &str, status: RouteStatus) -> ProducerContext {
        let runtime_status = runtime_status_for(&status);
        ProducerContext::new().with_runtime(Arc::new(
            MockRuntime::new().with_status(id, &runtime_status),
        ))
    }

    fn test_producer_ctx_with_runtime_status(route_id: &str, status: &str) -> ProducerContext {
        ProducerContext::new()
            .with_runtime(Arc::new(MockRuntime::new().with_status(route_id, status)))
    }

    fn test_producer_ctx_with_empty_runtime() -> ProducerContext {
        ProducerContext::new().with_runtime(Arc::new(MockRuntime::new()))
    }

    fn runtime_status_for(status: &RouteStatus) -> String {
        match status {
            RouteStatus::Stopped => "Stopped".to_string(),
            RouteStatus::Starting => "Starting".to_string(),
            RouteStatus::Started => "Started".to_string(),
            RouteStatus::Stopping => "Stopping".to_string(),
            RouteStatus::Suspended => "Suspended".to_string(),
            RouteStatus::Failed(msg) => format!("Failed: {msg}"),
        }
    }

    #[test]
    fn test_endpoint_requires_action_for_route_command() {
        let comp = ControlBusComponent::new();
        let result = comp.create_endpoint("controlbus:route?routeId=foo", &NoOpComponentContext);
        assert!(result.is_err(), "Should error when action is missing");
    }

    #[test]
    fn test_endpoint_rejects_unknown_action() {
        let comp = ControlBusComponent::new();
        let result = comp.create_endpoint(
            "controlbus:route?routeId=foo&action=banana",
            &NoOpComponentContext,
        );
        assert!(result.is_err(), "Should error for unknown action");
    }

    #[test]
    fn test_endpoint_parses_valid_uri() {
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=foo&action=start",
                &NoOpComponentContext,
            )
            .unwrap();
        assert_eq!(endpoint.uri(), "controlbus:route?routeId=foo&action=start");
    }

    #[test]
    fn test_endpoint_returns_no_consumer() {
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=foo&action=stop",
                &NoOpComponentContext,
            )
            .unwrap();
        assert!(endpoint.create_consumer(rt()).is_err());
    }

    #[test]
    fn test_endpoint_creates_producer() {
        let ctx = test_producer_ctx();
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=foo&action=start",
                &NoOpComponentContext,
            )
            .unwrap();
        assert!(endpoint.create_producer(rt(), &ctx).is_ok());
    }

    #[test]
    fn test_component_scheme() {
        let comp = ControlBusComponent::new();
        assert_eq!(comp.scheme(), "controlbus");
    }

    #[tokio::test]
    async fn test_producer_start_route() {
        let ctx = test_producer_ctx_with_route("my-route", RouteStatus::Stopped);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=my-route&action=start&authorizedRoutes=my-route",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Empty));
    }

    #[tokio::test]
    async fn test_producer_stop_route() {
        let ctx = test_producer_ctx_with_route("my-route", RouteStatus::Started);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=my-route&action=stop&authorizedRoutes=my-route",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Empty));
    }

    #[tokio::test]
    async fn test_producer_restart_maps_to_runtime_reload_command() {
        let runtime = Arc::new(MockRuntime::new().with_status("my-route", "Started"));
        let commands = runtime.commands();
        let ctx = ProducerContext::new().with_runtime(runtime);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=my-route&action=restart&authorizedRoutes=my-route",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Empty));

        let recorded = commands.lock().await.clone();
        assert_eq!(recorded, vec!["reload:my-route".to_string()]);
    }

    #[tokio::test]
    async fn test_producer_status_route() {
        let ctx = test_producer_ctx_with_route("my-route", RouteStatus::Started);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=my-route&action=status&authorizedRoutes=my-route",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Text(_)));
        if let Body::Text(status) = &result.input.body {
            assert_eq!(status, "Started");
        }
    }

    #[tokio::test]
    async fn test_producer_status_failed_route() {
        let ctx =
            test_producer_ctx_with_route("my-route", RouteStatus::Failed("error msg".to_string()));
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=my-route&action=status&authorizedRoutes=my-route",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Text(_)));
        if let Body::Text(status) = &result.input.body {
            assert_eq!(status, "Failed: error msg");
        }
    }

    #[tokio::test]
    async fn test_producer_status_uses_runtime_when_available() {
        let ctx = test_producer_ctx_with_runtime_status("runtime-route", "Started");
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=runtime-route&action=status&authorizedRoutes=runtime-route",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Text(_)));
        if let Body::Text(status) = &result.input.body {
            assert_eq!(status, "Started");
        }
    }

    #[tokio::test]
    async fn test_producer_status_errors_when_runtime_route_is_missing() {
        let ctx = test_producer_ctx_with_empty_runtime();
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=my-route&action=status&authorizedRoutes=my-route",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let err = producer.oneshot(exchange).await.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "runtime miss should not fallback to controller: {err}"
        );
    }

    #[tokio::test]
    async fn test_producer_denies_camel_route_id_header_override() {
        // R4-H1: the CamelRouteId exchange header is untrusted. Even when
        // a routeId is declared in the URI, the header must NOT be
        // consulted as a source of truth. The URI value is the only one
        // that counts for dispatch.
        let ctx = test_producer_ctx_with_route("from-uri", RouteStatus::Started);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=from-uri&action=status&authorizedRoutes=from-uri",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let mut exchange = Exchange::new(Message::default());
        // Adversary-controlled header would target a different route if
        // the override were honored. URI is the source of truth.
        exchange.input.set_header(
            "CamelRouteId",
            serde_json::Value::String("from-header".to_string()),
        );

        let result = producer.oneshot(exchange).await.unwrap();
        if let Body::Text(status) = &result.input.body {
            assert_eq!(status, "Started", "URI routeId is source of truth");
        } else {
            panic!("expected Body::Text, got: {:?}", result.input.body);
        }
    }

    #[tokio::test]
    async fn test_producer_denies_header_only_route_id() {
        // R4-H1: when routeId is missing from the URI, the header MUST
        // NOT be consulted. The endpoint must be denied.
        let ctx = test_producer_ctx_with_route("from-header", RouteStatus::Started);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?action=status&authorizedRoutes=from-header",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header(
            "CamelRouteId",
            serde_json::Value::String("from-header".to_string()),
        );

        let err = producer
            .oneshot(exchange)
            .await
            .expect_err("header must not satisfy routeId requirement");
        assert!(
            matches!(err, CamelError::Unauthorized(_)),
            "expected Unauthorized, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_producer_error_no_route_id() {
        let ctx = test_producer_ctx();
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?action=status&authorizedRoutes=foo",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("routeId must be declared in URI"),
            "Error should explain header override denied: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_producer_error_route_not_found() {
        let ctx = test_producer_ctx(); // No routes
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=nonexistent&action=status&authorizedRoutes=nonexistent",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "Error should mention not found: {}",
            err
        );
    }

    #[test]
    fn test_endpoint_parses_suspend_action() {
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=foo&action=suspend",
                &NoOpComponentContext,
            )
            .unwrap();
        assert_eq!(
            endpoint.uri(),
            "controlbus:route?routeId=foo&action=suspend"
        );
    }

    #[test]
    fn test_endpoint_parses_resume_action() {
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=foo&action=resume",
                &NoOpComponentContext,
            )
            .unwrap();
        assert_eq!(endpoint.uri(), "controlbus:route?routeId=foo&action=resume");
    }

    #[test]
    fn test_endpoint_parses_restart_action() {
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=foo&action=restart",
                &NoOpComponentContext,
            )
            .unwrap();
        assert_eq!(
            endpoint.uri(),
            "controlbus:route?routeId=foo&action=restart"
        );
    }

    #[test]
    fn test_endpoint_rejects_unknown_command() {
        let comp = ControlBusComponent::new();
        let result = comp.create_endpoint("controlbus:unknown?action=start", &NoOpComponentContext);
        assert!(result.is_err(), "Should error for unknown command");
    }

    // -----------------------------------------------------------------------
    // R4-H1: ControlBus capability authz
    //
    // The `controlbus:` endpoint MUST declare the authorized target routes
    // explicitly via the `authorizedRoutes` URI param. Without it, all
    // commands fail-closed. The `CamelRouteId` header override is denied
    // (target MUST be declared statically in URI). Self-restart
    // (routeId == calling route) is also rejected.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_authz_unauthorized_when_no_authorized_routes_param() {
        // authorizedRoutes not set → fail-closed
        let ctx = test_producer_ctx_with_route("target-route", RouteStatus::Started);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=target-route&action=status",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;
        let err = result.expect_err("must be denied when authorizedRoutes missing");
        assert!(
            matches!(err, CamelError::Unauthorized(_)),
            "expected Unauthorized, got: {err}"
        );
        assert!(
            err.to_string().contains("authorizedRoutes not configured"),
            "error should mention fail-closed default: {err}"
        );
    }

    #[tokio::test]
    async fn test_authz_unauthorized_when_route_id_not_in_allowlist() {
        // authorizedRoutes=route-a, but target is route-b → denied
        let ctx = test_producer_ctx_with_route("route-b", RouteStatus::Started);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=route-b&action=status&authorizedRoutes=route-a",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;
        let err = result.expect_err("must be denied when target not in allowlist");
        assert!(
            matches!(err, CamelError::Unauthorized(_)),
            "expected Unauthorized, got: {err}"
        );
        assert!(
            err.to_string().contains("not in authorizedRoutes"),
            "error should mention allowlist miss: {err}"
        );
    }

    #[tokio::test]
    async fn test_authz_authorized_when_route_id_in_allowlist() {
        // authorizedRoutes=target-route, routeId=target-route, caller=other
        let ctx = test_producer_ctx_with_route("target-route", RouteStatus::Started);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=target-route&action=status&authorizedRoutes=target-route",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.expect("must succeed");
        if let Body::Text(status) = &result.input.body {
            assert_eq!(status, "Started");
        } else {
            panic!("expected Body::Text(Started), got: {:?}", result.input.body);
        }
    }

    #[tokio::test]
    async fn test_authz_self_restart_rejected() {
        // target=calling route → self-restart denied
        let runtime_status = "Started".to_string();
        let runtime = Arc::new(MockRuntime::new().with_status("self-route", &runtime_status));
        let ctx = ProducerContext::new()
            .with_runtime(runtime)
            .with_route_id("self-route");
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint(
                "controlbus:route?routeId=self-route&action=restart&authorizedRoutes=self-route",
                &NoOpComponentContext,
            )
            .unwrap();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;
        let err = result.expect_err("self-restart must be denied");
        assert!(
            matches!(err, CamelError::Unauthorized(_)),
            "expected Unauthorized, got: {err}"
        );
        assert!(
            err.to_string().contains("self-restart"),
            "error should mention self-restart: {err}"
        );
    }
}
