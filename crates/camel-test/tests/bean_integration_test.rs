//! End-to-end integration test for bean functionality
//!
//! This test demonstrates the complete flow:
//! 1. Bean definition with #[bean_impl]
//! 2. Bean registration with BeanRegistry
//! 3. Route definition with bean steps
//! 4. End-to-end execution (YAML → bean invocation)

use camel_api::{Body, Exchange, Message};
use camel_bean::{BeanProcessor, BeanRegistry, bean_impl, handler};
use camel_core::{DefaultRouteController, Registry};
use camel_dsl::{
    DeclarativeRoute, DeclarativeStep, ToStepDef, compile_declarative_route, model::BeanStepDef,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex as StdMutex};
use tower::ServiceExt;

// ============================================================================
// Test Data Structures
// ============================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Order {
    id: String,
    amount: u32,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct ProcessedOrder {
    order_id: String,
    status: String,
    total: u32,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct ValidatedOrder {
    order_id: String,
    is_valid: bool,
}

// ============================================================================
// Test Bean Services
// ============================================================================

/// Order processing service with multiple handlers
struct OrderService;

#[bean_impl]
impl OrderService {
    /// Process an order and return a processed order with status
    #[handler]
    pub async fn process(&self, body: Order) -> Result<ProcessedOrder, String> {
        Ok(ProcessedOrder {
            order_id: body.id,
            status: "processed".to_string(),
            total: body.amount * 100, // Convert to cents
        })
    }

    /// Validate an order
    #[handler]
    pub async fn validate(&self, body: Order) -> Result<ValidatedOrder, String> {
        Ok(ValidatedOrder {
            order_id: body.id,
            is_valid: body.amount > 0,
        })
    }

    /// Enrich order with metadata (non-handler, should be ignored)
    #[allow(dead_code)]
    pub fn helper(&self) -> String {
        "helper".to_string()
    }
}

/// Additional service for multi-bean testing
struct AuditService;

#[bean_impl]
impl AuditService {
    #[handler]
    pub async fn log(&self, body: ProcessedOrder) -> Result<ProcessedOrder, String> {
        // In a real scenario, this would log to an audit system
        // For testing, we just return the body unchanged
        println!(
            "Audit: Order {} processed with status {}",
            body.order_id, body.status
        );
        Ok(body)
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Helper to create a controller with self_ref properly set
fn create_test_controller(bean_registry: BeanRegistry) -> DefaultRouteController {
    let registry = Arc::new(StdMutex::new(Registry::new()));
    create_test_controller_with_registry(registry, bean_registry)
}

/// Helper to create a controller with custom registry and self_ref
fn create_test_controller_with_registry(
    registry: Arc<StdMutex<Registry>>,
    bean_registry: BeanRegistry,
) -> DefaultRouteController {
    let bean_registry = Arc::new(StdMutex::new(bean_registry));
    DefaultRouteController::with_beans(registry, bean_registry)
}

// ============================================================================
// Integration Tests
// ============================================================================

#[tokio::test]
async fn test_bean_registration_and_lookup() {
    // Create bean registry
    let mut bean_registry = BeanRegistry::new();

    // Register beans
    bean_registry.register("orderService", OrderService);
    bean_registry.register("auditService", AuditService);

    // Verify registration
    assert!(bean_registry.get("orderService").is_some());
    assert!(bean_registry.get("auditService").is_some());
    assert!(bean_registry.get("unknownService").is_none());
}

#[tokio::test]
async fn test_simple_bean_route() {
    // Setup registries
    let mut bean_registry = BeanRegistry::new();
    bean_registry.register("orderService", OrderService);

    let controller = create_test_controller(bean_registry);

    // Define route with bean step
    let route = DeclarativeRoute {
        from: "direct:start".to_string(),
        route_id: "test-simple-bean-route".to_string(),
        auto_startup: true,
        startup_order: 1,
        concurrency: None,
        error_handler: None,
        circuit_breaker: None,
        unit_of_work: None,
        steps: vec![DeclarativeStep::Bean(BeanStepDef::new(
            "orderService",
            "process",
        ))],
    };

    // Compile declarative route to route definition
    let route_def = compile_declarative_route(route).unwrap();

    // Compile route definition to pipeline
    let pipeline = controller.compile_route_definition(route_def).unwrap();

    // Create test order
    let order = Order {
        id: "ORDER-123".to_string(),
        amount: 5,
    };

    let message = Message {
        body: Body::Json(serde_json::to_value(&order).unwrap()),
        ..Default::default()
    };

    let exchange = Exchange::new(message);

    // Execute pipeline
    let result = pipeline.oneshot(exchange).await.unwrap();

    // Verify transformation
    match &result.input.body {
        Body::Json(value) => {
            let processed: ProcessedOrder = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(processed.order_id, "ORDER-123");
            assert_eq!(processed.status, "processed");
            assert_eq!(processed.total, 500); // 5 * 100
        }
        _ => panic!("Expected Json body"),
    }
}

#[tokio::test]
async fn test_multi_step_bean_route() {
    // Setup registries
    let mut bean_registry = BeanRegistry::new();
    bean_registry.register("orderService", OrderService);
    bean_registry.register("auditService", AuditService);

    let controller = create_test_controller(bean_registry);

    // Define route with multiple bean steps
    let route = DeclarativeRoute {
        from: "direct:start".to_string(),
        route_id: "test-multi-bean-route".to_string(),
        auto_startup: true,
        startup_order: 1,
        concurrency: None,
        error_handler: None,
        circuit_breaker: None,
        unit_of_work: None,
        steps: vec![
            DeclarativeStep::Bean(BeanStepDef::new("orderService", "process")),
            DeclarativeStep::Bean(BeanStepDef::new("auditService", "log")),
        ],
    };

    // Compile declarative route to route definition
    let route_def = compile_declarative_route(route).unwrap();

    // Compile route definition to pipeline
    let pipeline = controller.compile_route_definition(route_def).unwrap();

    // Create test order
    let order = Order {
        id: "ORDER-456".to_string(),
        amount: 10,
    };

    let message = Message {
        body: Body::Json(serde_json::to_value(&order).unwrap()),
        ..Default::default()
    };

    let exchange = Exchange::new(message);

    // Execute pipeline
    let result = pipeline.oneshot(exchange).await.unwrap();

    // Verify transformation (should be processed and logged)
    match &result.input.body {
        Body::Json(value) => {
            let processed: ProcessedOrder = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(processed.order_id, "ORDER-456");
            assert_eq!(processed.status, "processed");
            assert_eq!(processed.total, 1000); // 10 * 100
        }
        _ => panic!("Expected Json body"),
    }
}

#[tokio::test]
async fn test_bean_with_validation() {
    // Setup registries
    let mut bean_registry = BeanRegistry::new();
    bean_registry.register("orderService", OrderService);

    let controller = create_test_controller(bean_registry);

    // Define route with validation bean
    let route = DeclarativeRoute {
        from: "direct:start".to_string(),
        route_id: "test-validation-route".to_string(),
        auto_startup: true,
        startup_order: 1,
        concurrency: None,
        error_handler: None,
        circuit_breaker: None,
        unit_of_work: None,
        steps: vec![DeclarativeStep::Bean(BeanStepDef::new(
            "orderService",
            "validate",
        ))],
    };

    // Compile declarative route to route definition
    let route_def = compile_declarative_route(route).unwrap();

    // Compile route definition to pipeline
    let pipeline = controller.compile_route_definition(route_def).unwrap();

    // Test with valid order
    let valid_order = Order {
        id: "ORDER-789".to_string(),
        amount: 100,
    };

    let message = Message {
        body: Body::Json(serde_json::to_value(&valid_order).unwrap()),
        ..Default::default()
    };

    let exchange = Exchange::new(message);
    let result = pipeline.clone().oneshot(exchange).await.unwrap();

    match &result.input.body {
        Body::Json(value) => {
            let validated: ValidatedOrder = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(validated.order_id, "ORDER-789");
            assert!(validated.is_valid);
        }
        _ => panic!("Expected Json body"),
    }

    // Test with invalid order (amount = 0)
    let invalid_order = Order {
        id: "ORDER-000".to_string(),
        amount: 0,
    };

    let message = Message {
        body: Body::Json(serde_json::to_value(&invalid_order).unwrap()),
        ..Default::default()
    };

    let exchange = Exchange::new(message);
    let result = pipeline.oneshot(exchange).await.unwrap();

    match &result.input.body {
        Body::Json(value) => {
            let validated: ValidatedOrder = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(validated.order_id, "ORDER-000");
            assert!(!validated.is_valid);
        }
        _ => panic!("Expected Json body"),
    }
}

#[tokio::test]
async fn test_bean_route_with_other_steps() {
    // Setup registries
    let mut bean_registry = BeanRegistry::new();
    bean_registry.register("orderService", OrderService);

    // For this test, we need to register the mock component
    use camel_component_mock::MockComponent;
    let registry = Arc::new(StdMutex::new(Registry::new()));
    registry.lock().unwrap().register(Arc::new(MockComponent::new()));

    let controller = create_test_controller_with_registry(registry, bean_registry);

    // Define route mixing bean steps with other steps
    let route = DeclarativeRoute {
        from: "direct:start".to_string(),
        route_id: "test-mixed-route".to_string(),
        auto_startup: true,
        startup_order: 1,
        concurrency: None,
        error_handler: None,
        circuit_breaker: None,
        unit_of_work: None,
        steps: vec![
            DeclarativeStep::Bean(BeanStepDef::new("orderService", "process")),
            DeclarativeStep::To(ToStepDef::new("mock:result")),
        ],
    };

    // Compile declarative route to route definition
    let route_def = compile_declarative_route(route).unwrap();

    // Compile route definition to pipeline
    let pipeline = controller.compile_route_definition(route_def).unwrap();

    // Create test order
    let order = Order {
        id: "ORDER-MIXED".to_string(),
        amount: 7,
    };

    let message = Message {
        body: Body::Json(serde_json::to_value(&order).unwrap()),
        ..Default::default()
    };

    let exchange = Exchange::new(message);

    // Execute pipeline - should succeed with both bean processing and mock endpoint
    let result = pipeline.oneshot(exchange).await.unwrap();

    // Verify the bean step processed the order before sending to mock
    match &result.input.body {
        Body::Json(value) => {
            let processed: ProcessedOrder = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(processed.order_id, "ORDER-MIXED");
            assert_eq!(processed.status, "processed");
            assert_eq!(processed.total, 700); // 7 * 100
        }
        _ => panic!("Expected Json body"),
    }
}

#[tokio::test]
async fn test_unknown_bean_error() {
    // Setup registries (no beans registered)
    let bean_registry = BeanRegistry::new();

    let controller = create_test_controller(bean_registry);

    // Define route with unknown bean
    let route = DeclarativeRoute {
        from: "direct:start".to_string(),
        route_id: "test-unknown-bean".to_string(),
        auto_startup: true,
        startup_order: 1,
        concurrency: None,
        error_handler: None,
        circuit_breaker: None,
        unit_of_work: None,
        steps: vec![DeclarativeStep::Bean(BeanStepDef::new(
            "unknownService",
            "process",
        ))],
    };

    // Compile declarative route to route definition
    let route_def = compile_declarative_route(route).unwrap();

    // Compile should fail
    let result = controller.compile_route_definition(route_def);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Bean not found"));
}

#[tokio::test]
async fn test_bean_handler_methods_only() {
    // Verify that only methods marked with #[handler] are invokable
    let service = OrderService;
    let methods = service.methods();

    assert!(methods.contains(&"process"));
    assert!(methods.contains(&"validate"));
    assert!(!methods.contains(&"helper")); // Non-handler method
}

// ============================================================================
// YAML Integration Test (demonstrating full stack)
// ============================================================================

#[tokio::test]
async fn test_yaml_to_bean_execution() {
    // This test simulates the complete YAML → Bean flow

    // Setup registries
    let mut bean_registry = BeanRegistry::new();
    bean_registry.register("orderService", OrderService);

    let controller = create_test_controller(bean_registry);

    // In a real scenario, this YAML would come from a file
    // For this test, we manually create the DeclarativeRoute that would result from YAML parsing
    let route = DeclarativeRoute {
        from: "direct:orders".to_string(),
        route_id: "order-processing-route".to_string(),
        auto_startup: true,
        startup_order: 1,
        concurrency: None,
        error_handler: None,
        circuit_breaker: None,
        unit_of_work: None,
        steps: vec![DeclarativeStep::Bean(BeanStepDef::new(
            "orderService",
            "process",
        ))],
    };

    // Compile declarative route (this is what happens after YAML parsing)
    let route_def = compile_declarative_route(route).unwrap();

    // Compile route definition to pipeline
    let pipeline = controller.compile_route_definition(route_def).unwrap();

    // Simulate incoming order message
    let order = Order {
        id: "ORDER-YAML-TEST".to_string(),
        amount: 3,
    };

    let message = Message {
        body: Body::Json(serde_json::to_value(&order).unwrap()),
        ..Default::default()
    };

    let exchange = Exchange::new(message);

    // Execute the route
    let result = pipeline.oneshot(exchange).await.unwrap();

    // Verify the bean processed the order correctly
    match &result.input.body {
        Body::Json(value) => {
            let processed: ProcessedOrder = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(processed.order_id, "ORDER-YAML-TEST");
            assert_eq!(processed.status, "processed");
            assert_eq!(processed.total, 300);

            println!("✓ YAML → Bean integration test passed!");
            println!(
                "  Input: Order {{ id: {}, amount: {} }}",
                order.id, order.amount
            );
            println!(
                "  Output: ProcessedOrder {{ order_id: {}, status: {}, total: {} }}",
                processed.order_id, processed.status, processed.total
            );
        }
        _ => panic!("Expected Json body"),
    }
}
