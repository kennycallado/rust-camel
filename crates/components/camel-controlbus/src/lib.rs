//! ControlBus component for managing route lifecycle.
//!
//! This component allows routes to control other routes in the Camel context
//! using the ControlBus EIP pattern. It provides operations like start, stop,
//! suspend, resume, and restart for routes.
//!
//! # URI Format
//!
//! `controlbus:route?routeId=my-route&action=start`
//!
//! # Parameters
//!
//! - `routeId`: The ID of the route to operate on (optional, can come from exchange header)
//! - `action`: The action to perform: `start`, `stop`, `suspend`, `resume`, `restart`, `status`
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
use tokio::sync::Mutex;
use tower::Service;
use tracing::debug;

use camel_api::{Body, BoxProcessor, CamelError, Exchange, RouteAction, RouteStatus};
use camel_component::{Component, Consumer, Endpoint, ProducerContext};
use camel_endpoint::parse_uri;

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

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
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

        Ok(Box::new(ControlBusEndpoint {
            uri: uri.to_string(),
            route_id,
            action,
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
}

impl Endpoint for ControlBusEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "controlbus endpoint does not support consumers".to_string(),
        ))
    }

    fn create_producer(&self, ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        let action = self.action.clone().ok_or_else(|| {
            CamelError::EndpointCreationFailed(
                "controlbus: action is required to create producer".to_string(),
            )
        })?;

        Ok(BoxProcessor::new(ControlBusProducer {
            route_id: self.route_id.clone(),
            action,
            controller: ctx.route_controller().clone(),
        }))
    }
}

// ---------------------------------------------------------------------------
// ControlBusProducer
// ---------------------------------------------------------------------------

/// Producer that executes control bus actions on routes.
#[derive(Clone)]
struct ControlBusProducer {
    /// Route ID from URI params (may be None, in which case header is used).
    route_id: Option<String>,
    /// Action to perform on the route.
    action: RouteAction,
    /// Route controller for executing actions.
    controller: Arc<Mutex<dyn camel_api::RouteController>>,
}

impl Service<Exchange> for ControlBusProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        // Get route_id: prefer field, fallback to header "CamelRouteId"
        let route_id = self.route_id.clone().or_else(|| {
            exchange
                .input
                .header("CamelRouteId")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        });

        // If no route_id → error
        let route_id = match route_id {
            Some(id) => id,
            None => {
                return Box::pin(async {
                    Err(CamelError::ProcessorError(
                        "controlbus: routeId required (set via URI param or CamelRouteId header)"
                            .into(),
                    ))
                });
            }
        };

        let action = self.action.clone();
        let controller = self.controller.clone();

        Box::pin(async move {
            let mut ctrl = controller.lock().await;

            debug!(
                route_id = %route_id,
                action = ?action,
                "ControlBus executing action"
            );

            match action {
                RouteAction::Start => {
                    ctrl.start_route(&route_id).await?;
                }
                RouteAction::Stop => {
                    ctrl.stop_route(&route_id).await?;
                }
                RouteAction::Suspend => {
                    ctrl.suspend_route(&route_id).await?;
                }
                RouteAction::Resume => {
                    ctrl.resume_route(&route_id).await?;
                }
                RouteAction::Restart => {
                    ctrl.restart_route(&route_id).await?;
                }
                RouteAction::Status => {
                    let status = ctrl.route_status(&route_id).ok_or_else(|| {
                        CamelError::ProcessorError(format!(
                            "controlbus: route '{}' not found",
                            route_id
                        ))
                    })?;
                    exchange.input.body = Body::Text(format_status(&status));
                    return Ok(exchange);
                }
            }

            // For all actions except Status, set body to Empty
            exchange.input.body = Body::Empty;
            Ok(exchange)
        })
    }
}

/// Format a RouteStatus for display in the exchange body.
fn format_status(status: &RouteStatus) -> String {
    match status {
        RouteStatus::Stopped => "Stopped".to_string(),
        RouteStatus::Starting => "Starting".to_string(),
        RouteStatus::Started => "Started".to_string(),
        RouteStatus::Stopping => "Stopping".to_string(),
        RouteStatus::Suspended => "Suspended".to_string(),
        RouteStatus::Failed(msg) => format!("Failed: {}", msg),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;
    use tower::ServiceExt;

    /// A mock route controller for testing.
    struct MockRouteController {
        routes: std::collections::HashMap<String, RouteStatus>,
    }

    impl MockRouteController {
        fn new() -> Self {
            Self {
                routes: std::collections::HashMap::new(),
            }
        }

        fn with_route(mut self, id: &str, status: RouteStatus) -> Self {
            self.routes.insert(id.to_string(), status);
            self
        }
    }

    #[async_trait]
    impl camel_api::RouteController for MockRouteController {
        async fn start_route(&mut self, route_id: &str) -> Result<(), CamelError> {
            if self.routes.contains_key(route_id) {
                self.routes
                    .insert(route_id.to_string(), RouteStatus::Started);
                Ok(())
            } else {
                Err(CamelError::ProcessorError(format!(
                    "route '{}' not found",
                    route_id
                )))
            }
        }

        async fn stop_route(&mut self, route_id: &str) -> Result<(), CamelError> {
            if self.routes.contains_key(route_id) {
                self.routes
                    .insert(route_id.to_string(), RouteStatus::Stopped);
                Ok(())
            } else {
                Err(CamelError::ProcessorError(format!(
                    "route '{}' not found",
                    route_id
                )))
            }
        }

        async fn restart_route(&mut self, route_id: &str) -> Result<(), CamelError> {
            if self.routes.contains_key(route_id) {
                self.routes
                    .insert(route_id.to_string(), RouteStatus::Started);
                Ok(())
            } else {
                Err(CamelError::ProcessorError(format!(
                    "route '{}' not found",
                    route_id
                )))
            }
        }

        async fn suspend_route(&mut self, route_id: &str) -> Result<(), CamelError> {
            if self.routes.contains_key(route_id) {
                self.routes
                    .insert(route_id.to_string(), RouteStatus::Suspended);
                Ok(())
            } else {
                Err(CamelError::ProcessorError(format!(
                    "route '{}' not found",
                    route_id
                )))
            }
        }

        async fn resume_route(&mut self, route_id: &str) -> Result<(), CamelError> {
            if self.routes.contains_key(route_id) {
                self.routes
                    .insert(route_id.to_string(), RouteStatus::Started);
                Ok(())
            } else {
                Err(CamelError::ProcessorError(format!(
                    "route '{}' not found",
                    route_id
                )))
            }
        }

        fn route_status(&self, route_id: &str) -> Option<RouteStatus> {
            self.routes.get(route_id).cloned()
        }

        async fn start_all_routes(&mut self) -> Result<(), CamelError> {
            for status in self.routes.values_mut() {
                *status = RouteStatus::Started;
            }
            Ok(())
        }

        async fn stop_all_routes(&mut self) -> Result<(), CamelError> {
            for status in self.routes.values_mut() {
                *status = RouteStatus::Stopped;
            }
            Ok(())
        }
    }

    fn test_producer_ctx() -> ProducerContext {
        ProducerContext::new(Arc::new(Mutex::new(MockRouteController::new())))
    }

    fn test_producer_ctx_with_route(id: &str, status: RouteStatus) -> ProducerContext {
        ProducerContext::new(Arc::new(Mutex::new(
            MockRouteController::new().with_route(id, status),
        )))
    }

    #[test]
    fn test_endpoint_requires_action_for_route_command() {
        let comp = ControlBusComponent::new();
        let result = comp.create_endpoint("controlbus:route?routeId=foo");
        assert!(result.is_err(), "Should error when action is missing");
    }

    #[test]
    fn test_endpoint_rejects_unknown_action() {
        let comp = ControlBusComponent::new();
        let result = comp.create_endpoint("controlbus:route?routeId=foo&action=banana");
        assert!(result.is_err(), "Should error for unknown action");
    }

    #[test]
    fn test_endpoint_parses_valid_uri() {
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint("controlbus:route?routeId=foo&action=start")
            .unwrap();
        assert_eq!(endpoint.uri(), "controlbus:route?routeId=foo&action=start");
    }

    #[test]
    fn test_endpoint_returns_no_consumer() {
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint("controlbus:route?routeId=foo&action=stop")
            .unwrap();
        assert!(endpoint.create_consumer().is_err());
    }

    #[test]
    fn test_endpoint_creates_producer() {
        let ctx = test_producer_ctx();
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint("controlbus:route?routeId=foo&action=start")
            .unwrap();
        assert!(endpoint.create_producer(&ctx).is_ok());
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
            .create_endpoint("controlbus:route?routeId=my-route&action=start")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Empty));
    }

    #[tokio::test]
    async fn test_producer_stop_route() {
        let ctx = test_producer_ctx_with_route("my-route", RouteStatus::Started);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint("controlbus:route?routeId=my-route&action=stop")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Empty));
    }

    #[tokio::test]
    async fn test_producer_status_route() {
        let ctx = test_producer_ctx_with_route("my-route", RouteStatus::Started);
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint("controlbus:route?routeId=my-route&action=status")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
            .create_endpoint("controlbus:route?routeId=my-route&action=status")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Text(_)));
        if let Body::Text(status) = &result.input.body {
            assert_eq!(status, "Failed: error msg");
        }
    }

    #[tokio::test]
    async fn test_producer_uses_header_route_id() {
        let ctx = test_producer_ctx_with_route("from-header", RouteStatus::Started);
        let comp = ControlBusComponent::new();
        // No routeId in URI
        let endpoint = comp
            .create_endpoint("controlbus:route?action=status")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header(
            "CamelRouteId",
            serde_json::Value::String("from-header".to_string()),
        );

        let result = producer.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Text(_)));
        if let Body::Text(status) = &result.input.body {
            assert_eq!(status, "Started");
        }
    }

    #[tokio::test]
    async fn test_producer_uri_route_id_overrides_header() {
        let ctx = test_producer_ctx_with_route("from-uri", RouteStatus::Started);
        // Use ctx which has "from-uri" route
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint("controlbus:route?routeId=from-uri&action=status")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::default());
        // Header has different route ID, but URI param should take precedence
        exchange.input.set_header(
            "CamelRouteId",
            serde_json::Value::String("from-header".to_string()),
        );

        let result = producer.oneshot(exchange).await.unwrap();
        if let Body::Text(status) = &result.input.body {
            assert_eq!(status, "Started", "Should use URI routeId, not header");
        }
    }

    #[tokio::test]
    async fn test_producer_error_no_route_id() {
        let ctx = test_producer_ctx();
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint("controlbus:route?action=status")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.oneshot(exchange).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("routeId required"),
            "Error should mention routeId: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_producer_error_route_not_found() {
        let ctx = test_producer_ctx(); // No routes
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint("controlbus:route?routeId=nonexistent&action=status")
            .unwrap();
        let producer = endpoint.create_producer(&ctx).unwrap();

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
            .create_endpoint("controlbus:route?routeId=foo&action=suspend")
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
            .create_endpoint("controlbus:route?routeId=foo&action=resume")
            .unwrap();
        assert_eq!(endpoint.uri(), "controlbus:route?routeId=foo&action=resume");
    }

    #[test]
    fn test_endpoint_parses_restart_action() {
        let comp = ControlBusComponent::new();
        let endpoint = comp
            .create_endpoint("controlbus:route?routeId=foo&action=restart")
            .unwrap();
        assert_eq!(
            endpoint.uri(),
            "controlbus:route?routeId=foo&action=restart"
        );
    }

    #[test]
    fn test_endpoint_rejects_unknown_command() {
        let comp = ControlBusComponent::new();
        let result = comp.create_endpoint("controlbus:unknown?action=start");
        assert!(result.is_err(), "Should error for unknown command");
    }
}
