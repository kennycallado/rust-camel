# AutoStart + RouteController Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement route lifecycle management with autoStartup, startupOrder, and RouteController for dynamic control

**Architecture:** RouteController trait with DefaultRouteController impl manages route state and lifecycle. ProducerContext pattern injects RouteController into components. ControlBus component enables dynamic route management from within routes.

**Tech Stack:** Rust, Tokio, Tower, async-trait

---

## Phase 1: Core Infrastructure

### Task 1: Add RouteDefinition Fields

**Files:**
- Modify: `crates/camel-core/src/route.rs:113-174`

**Step 1: Add new fields to RouteDefinition**

```rust
pub struct RouteDefinition {
    pub(crate) from_uri: String,
    pub(crate) steps: Vec<BuilderStep>,
    pub(crate) error_handler: Option<ErrorHandlerConfig>,
    pub(crate) circuit_breaker: Option<CircuitBreakerConfig>,
    pub(crate) concurrency: Option<ConcurrencyModel>,
    
    // NEW:
    pub(crate) route_id: Option<String>,
    pub(crate) auto_startup: bool,
    pub(crate) startup_order: i32,
}
```

**Step 2: Update constructor with defaults**

```rust
impl RouteDefinition {
    pub fn new(from_uri: impl Into<String>, steps: Vec<BuilderStep>) -> Self {
        Self {
            from_uri: from_uri.into(),
            steps,
            error_handler: None,
            circuit_breaker: None,
            concurrency: None,
            route_id: None,
            auto_startup: true,
            startup_order: 1000,
        }
    }
}
```

**Step 3: Add getter methods**

```rust
impl RouteDefinition {
    pub fn route_id(&self) -> Option<&str> {
        self.route_id.as_deref()
    }
    
    pub fn auto_startup(&self) -> bool {
        self.auto_startup
    }
    
    pub fn startup_order(&self) -> i32 {
        self.startup_order
    }
}
```

**Step 4: Add setter methods (builder pattern)**

```rust
impl RouteDefinition {
    pub fn with_route_id(mut self, id: impl Into<String>) -> Self {
        self.route_id = Some(id.into());
        self
    }
    
    pub fn with_auto_startup(mut self, auto: bool) -> Self {
        self.auto_startup = auto;
        self
    }
    
    pub fn with_startup_order(mut self, order: i32) -> Self {
        self.startup_order = order;
        self
    }
}
```

**Step 5: Commit**

```bash
git add crates/camel-core/src/route.rs
git commit -m "feat(camel-core): add route_id, auto_startup, startup_order to RouteDefinition"
```

---

### Task 2: Update RouteBuilder

**Files:**
- Modify: `crates/camel-builder/src/lib.rs:130-148`

**Step 1: Add fields to RouteBuilder**

```rust
pub struct RouteBuilder {
    from_uri: String,
    steps: Vec<BuilderStep>,
    error_handler: Option<ErrorHandlerConfig>,
    circuit_breaker_config: Option<CircuitBreakerConfig>,
    concurrency: Option<ConcurrencyModel>,
    
    // NEW:
    route_id: Option<String>,
    auto_startup: Option<bool>,
    startup_order: Option<i32>,
}
```

**Step 2: Update from() constructor**

```rust
impl RouteBuilder {
    pub fn from(endpoint: &str) -> Self {
        Self {
            from_uri: endpoint.to_string(),
            steps: Vec::new(),
            error_handler: None,
            circuit_breaker_config: None,
            concurrency: None,
            route_id: None,
            auto_startup: None,
            startup_order: None,
        }
    }
}
```

**Step 3: Add builder methods**

```rust
impl RouteBuilder {
    pub fn route_id(mut self, id: impl Into<String>) -> Self {
        self.route_id = Some(id.into());
        self
    }
    
    pub fn auto_startup(mut self, auto: bool) -> Self {
        self.auto_startup = Some(auto);
        self
    }
    
    pub fn startup_order(mut self, order: i32) -> Self {
        self.startup_order = Some(order);
        self
    }
}
```

**Step 4: Update build() method**

```rust
impl RouteBuilder {
    pub fn build(self) -> Result<RouteDefinition, CamelError> {
        if self.from_uri.is_empty() {
            return Err(CamelError::RouteError("route must have a 'from' URI".into()));
        }
        
        let mut definition = RouteDefinition::new(self.from_uri, self.steps);
        
        // Existing fields
        if let Some(eh) = self.error_handler {
            definition = definition.with_error_handler(eh);
        }
        if let Some(cb) = self.circuit_breaker_config {
            definition = definition.with_circuit_breaker(cb);
        }
        if let Some(concurrency) = self.concurrency {
            definition = definition.with_concurrency(concurrency);
        }
        
        // NEW fields
        if let Some(id) = self.route_id {
            definition = definition.with_route_id(id);
        }
        if let Some(auto) = self.auto_startup {
            definition = definition.with_auto_startup(auto);
        }
        if let Some(order) = self.startup_order {
            definition = definition.with_startup_order(order);
        }
        
        Ok(definition)
    }
}
```

**Step 5: Add unit tests**

```rust
#[test]
fn test_builder_route_id_sets_id() {
    let definition = RouteBuilder::from("timer:tick")
        .route_id("my-route")
        .build()
        .unwrap();
    
    assert_eq!(definition.route_id(), Some("my-route"));
}

#[test]
fn test_builder_auto_startup_false() {
    let definition = RouteBuilder::from("timer:tick")
        .auto_startup(false)
        .build()
        .unwrap();
    
    assert!(!definition.auto_startup());
}

#[test]
fn test_builder_startup_order_custom() {
    let definition = RouteBuilder::from("timer:tick")
        .startup_order(50)
        .build()
        .unwrap();
    
    assert_eq!(definition.startup_order(), 50);
}

#[test]
fn test_builder_defaults() {
    let definition = RouteBuilder::from("timer:tick")
        .build()
        .unwrap();
    
    assert_eq!(definition.route_id(), None);
    assert!(definition.auto_startup());
    assert_eq!(definition.startup_order(), 1000);
}
```

**Step 6: Run tests**

```bash
cargo test -p camel-builder
```

Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/camel-builder/src/lib.rs
git commit -m "feat(camel-builder): add route_id, auto_startup, startup_order builder methods"
```

---

### Task 3: Create RouteController Trait

**Files:**
- Create: `crates/camel-core/src/route_controller.rs`

**Step 1: Create RouteStatus enum**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteStatus {
    Stopped,
    Starting,
    Started,
    Stopping,
    Suspended,
    Failed(String),
}
```

**Step 2: Create RouteAction enum**

```rust
#[derive(Debug, Clone)]
pub enum RouteAction {
    Start,
    Stop,
    Suspend,
    Resume,
    Restart,
    Status,
}
```

**Step 3: Create RouteController trait**

```rust
use async_trait::async_trait;
use camel_api::CamelError;

#[async_trait]
pub trait RouteController: Send + Sync {
    async fn start_route(&mut self, route_id: &str) -> Result<(), CamelError>;
    async fn stop_route(&mut self, route_id: &str) -> Result<(), CamelError>;
    async fn restart_route(&mut self, route_id: &str) -> Result<(), CamelError>;
    async fn suspend_route(&mut self, route_id: &str) -> Result<(), CamelError>;
    async fn resume_route(&mut self, route_id: &str) -> Result<(), CamelError>;
    
    fn route_status(&self, route_id: &str) -> Option<RouteStatus>;
    
    async fn start_all_routes(&mut self) -> Result<(), CamelError>;
    async fn stop_all_routes(&mut self) -> Result<(), CamelError>;
}
```

**Step 4: Update lib.rs to export**

```rust
// crates/camel-core/src/lib.rs
pub mod route_controller;

pub use route_controller::{RouteAction, RouteController, RouteStatus};
```

**Step 5: Commit**

```bash
git add crates/camel-core/src/route_controller.rs crates/camel-core/src/lib.rs
git commit -m "feat(camel-core): add RouteController trait and RouteStatus/RouteAction enums"
```

---

### Task 4: Create ProducerContext

**Files:**
- Create: `crates/camel-component/src/producer.rs`

**Step 1: Create ProducerContext struct**

```rust
use std::sync::Arc;
use tokio::sync::Mutex;
use camel_core::route_controller::RouteController;

pub struct ProducerContext {
    route_controller: Arc<Mutex<dyn RouteController>>,
}

impl ProducerContext {
    pub fn new(route_controller: Arc<Mutex<dyn RouteController>>) -> Self {
        Self { route_controller }
    }
    
    pub fn route_controller(&self) -> &Arc<Mutex<dyn RouteController>> {
        &self.route_controller
    }
}
```

**Step 2: Update lib.rs to export**

```rust
// crates/camel-component/src/lib.rs
pub mod producer;

pub use producer::ProducerContext;
```

**Step 3: Commit**

```bash
git add crates/camel-component/src/producer.rs crates/camel-component/src/lib.rs
git commit -m "feat(camel-component): add ProducerContext for dependency injection"
```

---

### Task 5: Update Endpoint Trait

**Files:**
- Modify: `crates/camel-component/src/endpoint.rs`

**Step 1: Add ProducerContext parameter to create_producer**

```rust
use crate::ProducerContext;

pub trait Endpoint: Send + Sync {
    fn uri(&self) -> &str;
    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError>;
    
    // CHANGED: Add ctx parameter
    fn create_producer(&self, ctx: &ProducerContext) -> Result<BoxProcessor, CamelError>;
}
```

**Step 2: Commit**

```bash
git add crates/camel-component/src/endpoint.rs
git commit -m "feat(camel-component): add ProducerContext parameter to Endpoint::create_producer"
```

---

### Task 6: Migrate Timer Component

**Files:**
- Modify: `crates/components/camel-timer/src/lib.rs:120`

**Step 1: Update create_producer signature**

```rust
impl Endpoint for TimerEndpoint {
    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(TimerProducer::new()))
    }
}
```

**Step 2: Run tests**

```bash
cargo test -p camel-timer
```

Expected: Compilation error in camel-core (expected, will fix in next task)

**Step 3: Commit**

```bash
git add crates/components/camel-timer/src/lib.rs
git commit -m "refactor(camel-timer): update Endpoint::create_producer to accept ProducerContext"
```

---

### Task 7: Migrate Log Component

**Files:**
- Modify: `crates/components/camel-log/src/lib.rs:149`

**Step 1: Update create_producer signature**

```rust
impl Endpoint for LogEndpoint {
    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        // ... existing implementation
    }
}
```

**Step 2: Run tests**

```bash
cargo test -p camel-log
```

**Step 3: Commit**

```bash
git add crates/components/camel-log/src/lib.rs
git commit -m "refactor(camel-log): update Endpoint::create_producer to accept ProducerContext"
```

---

### Task 8: Migrate Direct Component

**Files:**
- Modify: `crates/components/camel-direct/src/lib.rs:98`

**Step 1: Update create_producer signature**

```rust
impl Endpoint for DirectEndpoint {
    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        // ... existing implementation
    }
}
```

**Step 2: Run tests**

```bash
cargo test -p camel-direct
```

**Step 3: Commit**

```bash
git add crates/components/camel-direct/src/lib.rs
git commit -m "refactor(camel-direct): update Endpoint::create_producer to accept ProducerContext"
```

---

### Task 9: Migrate Mock Component

**Files:**
- Modify: `crates/components/camel-mock/src/lib.rs:141`

**Step 1: Update create_producer signature**

```rust
impl Endpoint for MockEndpoint {
    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        // ... existing implementation
    }
}
```

**Step 2: Run tests**

```bash
cargo test -p camel-mock
```

**Step 3: Commit**

```bash
git add crates/components/camel-mock/src/lib.rs
git commit -m "refactor(camel-mock): update Endpoint::create_producer to accept ProducerContext"
```

---

### Task 10: Migrate File Component

**Files:**
- Modify: `crates/components/camel-file/src/lib.rs:199`

**Step 1: Update create_producer signature**

```rust
impl Endpoint for FileEndpoint {
    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        // ... existing implementation
    }
}
```

**Step 2: Run tests**

```bash
cargo test -p camel-file
```

**Step 3: Commit**

```bash
git add crates/components/camel-file/src/lib.rs
git commit -m "refactor(camel-file): update Endpoint::create_producer to accept ProducerContext"
```

---

### Task 11: Migrate HTTP Component

**Files:**
- Modify: `crates/components/camel-http/src/lib.rs:703`

**Step 1: Update create_producer signature**

```rust
impl Endpoint for HttpEndpoint {
    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        // ... existing implementation
    }
}
```

**Step 2: Run tests**

```bash
cargo test -p camel-http
```

**Step 3: Commit**

```bash
git add crates/components/camel-http/src/lib.rs
git commit -m "refactor(camel-http): update Endpoint::create_producer to accept ProducerContext"
```

---

### Task 12: Update CamelContext to Use ProducerContext

**Files:**
- Modify: `crates/camel-core/src/context.rs:144-199`

**Step 1: Add ProducerContext import**

```rust
use camel_component::ProducerContext;
```

**Step 2: Update resolve_steps to create ProducerContext**

```rust
fn resolve_steps(&self, steps: Vec<BuilderStep>) -> Result<Vec<BoxProcessor>, CamelError> {
    // TODO: Will be replaced when RouteController is implemented
    let producer_ctx = ProducerContext::new(Arc::new(Mutex::new(
        crate::route_controller::DefaultRouteController::new(Arc::clone(&self.registry))
    )));
    
    let mut processors: Vec<BoxProcessor> = Vec::new();
    for step in steps {
        match step {
            BuilderStep::Processor(svc) => {
                processors.push(svc);
            }
            BuilderStep::To(uri) => {
                let parsed = parse_uri(&uri)?;
                let component = self.registry.get_or_err(&parsed.scheme)?;
                let endpoint = component.create_endpoint(&uri)?;
                let producer = endpoint.create_producer(&producer_ctx)?; // CHANGED
                processors.push(producer);
            }
            // ... other cases
        }
    }
    Ok(processors)
}
```

**Step 3: Run tests**

```bash
cargo test -p camel-core
```

**Step 4: Commit**

```bash
git add crates/camel-core/src/context.rs
git commit -m "refactor(camel-core): use ProducerContext in resolve_steps"
```

---

### Task 13: Implement DefaultRouteController - Part 1 (Structure)

**Files:**
- Modify: `crates/camel-core/src/route_controller.rs`

**Step 1: Add internal structures**

```rust
use std::collections::HashMap;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use camel_api::BoxProcessor;
use camel_component::ConsumerContext;
use crate::registry::Registry;
use crate::route::RouteDefinition;

struct ManagedRoute {
    definition: RouteDefinition,
    status: RouteStatus,
    consumer_handle: Option<JoinHandle<()>>,
    pipeline_handle: Option<JoinHandle<()>>,
    cancel_token: CancellationToken,
    pipeline: Option<BoxProcessor>,
}

pub struct DefaultRouteController {
    routes: HashMap<String, ManagedRoute>,
    registry: std::sync::Arc<Registry>,
}
```

**Step 2: Implement constructor**

```rust
impl DefaultRouteController {
    pub fn new(registry: std::sync::Arc<Registry>) -> Self {
        Self {
            routes: HashMap::new(),
            registry,
        }
    }
    
    pub fn add_route(&mut self, definition: RouteDefinition) -> Result<(), CamelError> {
        let route_id = definition.route_id()
            .ok_or_else(|| CamelError::RouteError("Route must have an ID".into()))?
            .to_string();
        
        if self.routes.contains_key(&route_id) {
            return Err(CamelError::RouteError(
                format!("Route '{}' already exists", route_id)
            ));
        }
        
        self.routes.insert(route_id, ManagedRoute {
            definition,
            status: RouteStatus::Stopped,
            consumer_handle: None,
            pipeline_handle: None,
            cancel_token: CancellationToken::new(),
            pipeline: None,
        });
        
        Ok(())
    }
    
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }
    
    pub fn route_ids(&self) -> Vec<String> {
        self.routes.keys().cloned().collect()
    }
}
```

**Step 3: Commit**

```bash
git add crates/camel-core/src/route_controller.rs
git commit -m "feat(camel-core): add DefaultRouteController structure and add_route method"
```

---

### Task 14: Implement DefaultRouteController - Part 2 (Lifecycle)

**Files:**
- Modify: `crates/camel-core/src/route_controller.rs`

**Step 1: Implement start_route (simplified - copy logic from context.rs)**

```rust
#[async_trait]
impl RouteController for DefaultRouteController {
    async fn start_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        let managed = self.routes.get_mut(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;
        
        if managed.status != RouteStatus::Stopped {
            return Err(CamelError::RouteError(
                format!("Route '{}' is not stopped (status: {:?})", route_id, managed.status)
            ));
        }
        
        managed.status = RouteStatus::Starting;
        
        // TODO: Resolve pipeline, create consumer, spawn tasks
        // For now, just mark as started
        managed.status = RouteStatus::Started;
        
        tracing::info!("Route '{}' started", route_id);
        Ok(())
    }
    
    async fn stop_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        let managed = self.routes.get_mut(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;
        
        if managed.status != RouteStatus::Started {
            return Err(CamelError::RouteError(
                format!("Route '{}' is not running", route_id)
            ));
        }
        
        managed.status = RouteStatus::Stopping;
        
        // TODO: Cancel tasks, wait for drain
        managed.status = RouteStatus::Stopped;
        
        tracing::info!("Route '{}' stopped", route_id);
        Ok(())
    }
    
    async fn restart_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route(route_id).await?;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        self.start_route(route_id).await
    }
    
    async fn suspend_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route(route_id).await?;
        if let Some(managed) = self.routes.get_mut(route_id) {
            managed.status = RouteStatus::Suspended;
        }
        Ok(())
    }
    
    async fn resume_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.start_route(route_id).await
    }
    
    fn route_status(&self, route_id: &str) -> Option<RouteStatus> {
        self.routes.get(route_id).map(|r| r.status.clone())
    }
    
    async fn start_all_routes(&mut self) -> Result<(), CamelError> {
        let mut sorted_routes: Vec<_> = self.routes.iter()
            .filter(|(_, r)| r.definition.auto_startup())
            .collect();
        sorted_routes.sort_by_key(|(_, r)| r.definition.startup_order());
        
        for (route_id, _) in sorted_routes {
            self.start_route(route_id).await
                .map_err(|e| CamelError::RouteError(
                    format!("Failed to start route '{}': {}", route_id, e)
                ))?;
        }
        
        Ok(())
    }
    
    async fn stop_all_routes(&mut self) -> Result<(), CamelError> {
        let mut sorted_routes: Vec<_> = self.routes.iter().collect();
        sorted_routes.sort_by_key(|(_, r)| std::cmp::Reverse(r.definition.startup_order()));
        
        for (route_id, _) in sorted_routes {
            let _ = self.stop_route(route_id).await;
        }
        
        Ok(())
    }
}
```

**Step 2: Add unit tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_add_route_requires_id() {
        let registry = Arc::new(Registry::new());
        let mut controller = DefaultRouteController::new(registry);
        
        let definition = RouteDefinition::new("timer:tick", vec![]);
        assert!(controller.add_route(definition).is_err());
    }
    
    #[tokio::test]
    async fn test_add_route_with_id_succeeds() {
        let registry = Arc::new(Registry::new());
        let mut controller = DefaultRouteController::new(registry);
        
        let definition = RouteDefinition::new("timer:tick", vec![])
            .with_route_id("test-route");
        assert!(controller.add_route(definition).is_ok());
        assert_eq!(controller.route_count(), 1);
    }
}
```

**Step 3: Run tests**

```bash
cargo test -p camel-core route_controller
```

**Step 4: Commit**

```bash
git add crates/camel-core/src/route_controller.rs
git commit -m "feat(camel-core): implement DefaultRouteController lifecycle methods"
```

---

### Task 15: Integrate RouteController into CamelContext

**Files:**
- Modify: `crates/camel-core/src/context.rs`

**Step 1: Add RouteController to CamelContext**

```rust
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::route_controller::{DefaultRouteController, RouteController};

pub struct CamelContext {
    registry: Arc<Registry>,
    route_controller: Arc<Mutex<DefaultRouteController>>,
    cancel_token: CancellationToken,
    global_error_handler: Option<ErrorHandlerConfig>,
}
```

**Step 2: Update constructor**

```rust
impl CamelContext {
    pub fn new() -> Self {
        let registry = Arc::new(Registry::new());
        let route_controller = Arc::new(Mutex::new(
            DefaultRouteController::new(registry.clone())
        ));
        
        Self {
            registry,
            route_controller,
            cancel_token: CancellationToken::new(),
            global_error_handler: None,
        }
    }
    
    pub fn route_controller(&self) -> &Arc<Mutex<DefaultRouteController>> {
        &self.route_controller
    }
}
```

**Step 3: Update add_route_definition**

```rust
impl CamelContext {
    pub fn add_route_definition(&mut self, mut definition: RouteDefinition) -> Result<(), CamelError> {
        // Auto-generate ID if needed
        if definition.route_id().is_none() {
            let count = self.route_controller.try_lock().unwrap().route_count();
            definition = definition.with_route_id(format!("route-{}", count));
        }
        
        self.route_controller.try_lock().unwrap().add_route(definition)?;
        Ok(())
    }
}
```

**Step 4: Update start() to use RouteController**

```rust
impl CamelContext {
    pub async fn start(&mut self) -> Result<(), CamelError> {
        tracing::info!("Starting CamelContext");
        self.route_controller.lock().await.start_all_routes().await?;
        tracing::info!("CamelContext started");
        Ok(())
    }
}
```

**Step 5: Update stop() to use RouteController**

```rust
impl CamelContext {
    pub async fn stop(&mut self) -> Result<(), CamelError> {
        tracing::info!("Stopping CamelContext");
        self.route_controller.lock().await.stop_all_routes().await?;
        tracing::info!("CamelContext stopped");
        Ok(())
    }
}
```

**Step 6: Run all tests**

```bash
cargo test --workspace
```

**Step 7: Commit**

```bash
git add crates/camel-core/src/context.rs
git commit -m "feat(camel-core): integrate RouteController into CamelContext"
```

---

## Phase 2: ControlBus Component

### Task 16: Create ControlBus Component Structure

**Files:**
- Create: `crates/components/camel-controlbus/Cargo.toml`
- Create: `crates/components/camel-controlbus/src/lib.rs`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "camel-controlbus"
version = "0.1.0"
edition = "2021"

[dependencies]
camel-api = { path = "../../camel-api" }
camel-component = { path = "../../camel-component" }
camel-core = { path = "../../camel-core" }
camel-endpoint = { path = "../../camel-endpoint" }
async-trait = "0.1"
tokio = { version = "1", features = ["sync"] }
tower = "0.5"
tracing = "0.1"
```

**Step 2: Create lib.rs with Component**

```rust
use camel_api::{CamelError, BoxProcessor};
use camel_component::{Component, Endpoint, ProducerContext};
use camel_endpoint::parse_uri;
use camel_core::route_controller::RouteAction;

pub struct ControlBusComponent;

impl Component for ControlBusComponent {
    fn scheme(&self) -> &str {
        "controlbus"
    }
    
    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let parsed = parse_uri(uri)?;
        let command = parsed.path.clone();
        
        let action = parsed.params.get("action").and_then(|a| match a.as_str() {
            "start" => Some(RouteAction::Start),
            "stop" => Some(RouteAction::Stop),
            "suspend" => Some(RouteAction::Suspend),
            "resume" => Some(RouteAction::Resume),
            "restart" => Some(RouteAction::Restart),
            "status" => Some(RouteAction::Status),
            _ => None,
        });
        
        if command == "route" && action.is_none() {
            return Err(CamelError::InvalidEndpoint(
                "controlbus:route requires 'action' parameter".into()
            ));
        }
        
        Ok(Box::new(ControlBusEndpoint {
            uri: uri.to_string(),
            command,
            route_id: parsed.params.get("routeId").cloned(),
            action,
        }))
    }
}

struct ControlBusEndpoint {
    uri: String,
    command: String,
    route_id: Option<String>,
    action: Option<RouteAction>,
}

impl Endpoint for ControlBusEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }
    
    fn create_consumer(&self) -> Result<Box<dyn camel_component::Consumer>, CamelError> {
        Err(CamelError::InvalidEndpoint(
            "controlbus does not support consumers".into()
        ))
    }
    
    fn create_producer(&self, ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        // Will implement in next task
        todo!()
    }
}
```

**Step 3: Add to workspace Cargo.toml**

```toml
# In root Cargo.toml, add to workspace.members:
"crates/components/camel-controlbus",
```

**Step 4: Commit**

```bash
git add crates/components/camel-controlbus/Cargo.toml
git add crates/components/camel-controlbus/src/lib.rs
git add Cargo.toml
git commit -m "feat(camel-controlbus): create ControlBus component structure"
```

---

### Task 17: Implement ControlBusProducer

**Files:**
- Modify: `crates/components/camel-controlbus/src/lib.rs`

**Step 1: Implement ControlBusProducer**

```rust
use std::task::{Context, Poll};
use std::pin::Pin;
use std::future::Future;
use tower::Service;
use camel_api::{Body, Exchange};

struct ControlBusProducer {
    route_id: Option<String>,
    action: RouteAction,
    controller: std::sync::Arc<tokio::sync::Mutex<dyn camel_core::route_controller::RouteController>>,
}

impl Service<Exchange> for ControlBusProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;
    
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    
    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let route_id = self.route_id.clone()
            .or_else(|| exchange.input.header("CamelRouteId").and_then(|v| v.as_text().map(|s| s.to_string())))
            .ok_or_else(|| {
                CamelError::ProcessorError(
                    "controlbus requires routeId parameter or CamelRouteId header".into()
                )
            });
        
        let action = self.action.clone();
        let controller = self.controller.clone();
        
        Box::pin(async move {
            let route_id = route_id?;
            let mut ctrl = controller.lock().await;
            
            match action {
                RouteAction::Start => {
                    ctrl.start_route(&route_id).await?;
                    exchange.input.body = Body::Null;
                }
                RouteAction::Stop => {
                    ctrl.stop_route(&route_id).await?;
                    exchange.input.body = Body::Null;
                }
                RouteAction::Suspend => {
                    ctrl.suspend_route(&route_id).await?;
                    exchange.input.body = Body::Null;
                }
                RouteAction::Resume => {
                    ctrl.resume_route(&route_id).await?;
                    exchange.input.body = Body::Null;
                }
                RouteAction::Restart => {
                    ctrl.restart_route(&route_id).await?;
                    exchange.input.body = Body::Null;
                }
                RouteAction::Status => {
                    let status = ctrl.route_status(&route_id)
                        .ok_or_else(|| CamelError::ProcessorError(
                            format!("Route '{}' not found", route_id)
                        ))?;
                    exchange.input.body = Body::Text(format!("{:?}", status));
                }
            }
            
            Ok(exchange)
        })
    }
}
```

**Step 2: Update create_producer**

```rust
impl Endpoint for ControlBusEndpoint {
    fn create_producer(&self, ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        let route_id = self.route_id.clone();
        let action = self.action.clone().ok_or_else(|| {
            CamelError::InvalidEndpoint("controlbus requires 'action' parameter".into())
        })?;
        let controller = ctx.route_controller().clone();
        
        Ok(BoxProcessor::new(ControlBusProducer {
            route_id,
            action,
            controller,
        }))
    }
}
```

**Step 3: Add unit tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_endpoint_requires_action() {
        let comp = ControlBusComponent;
        let result = comp.create_endpoint("controlbus:route?routeId=foo");
        assert!(result.is_err());
    }
    
    #[test]
    fn test_endpoint_parses_action() {
        let comp = ControlBusComponent;
        let endpoint = comp.create_endpoint("controlbus:route?routeId=foo&action=start").unwrap();
        assert_eq!(endpoint.uri(), "controlbus:route?routeId=foo&action=start");
    }
}
```

**Step 4: Run tests**

```bash
cargo test -p camel-controlbus
```

**Step 5: Commit**

```bash
git add crates/components/camel-controlbus/src/lib.rs
git commit -m "feat(camel-controlbus): implement ControlBusProducer"
```

---

### Task 18: Add Integration Test

**Files:**
- Create: `crates/camel-test/tests/controlbus_test.rs`

**Step 1: Create integration test**

```rust
use camel_api::{CamelError, Value};
use camel_builder::RouteBuilder;
use camel_core::context::CamelContext;
use camel_timer::TimerComponent;
use camel_log::LogComponent;
use camel_controlbus::ControlBusComponent;
use camel_mock::MockComponent;

#[tokio::test]
async fn test_autostartup_false_route_does_not_start() -> Result<(), CamelError> {
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(MockComponent::new());
    
    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=1")
        .route_id("lazy-route")
        .auto_startup(false)
        .to("mock:result")
        .build()?;
    
    ctx.add_route_definition(route)?;
    ctx.start().await?;
    
    // Route should be stopped
    let status = ctx.route_controller().lock().await.route_status("lazy-route");
    assert!(matches!(status, Some(camel_core::RouteStatus::Stopped)));
    
    ctx.stop().await?;
    Ok(())
}

#[tokio::test]
async fn test_controlbus_starts_route() -> Result<(), CamelError> {
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(ControlBusComponent::new());
    ctx.register_component(MockComponent::new());
    
    // Route 1: Lazy route (doesn't auto-start)
    let lazy = RouteBuilder::from("timer:tick?period=100&repeatCount=1")
        .route_id("lazy-route")
        .auto_startup(false)
        .startup_order(10)
        .to("mock:result")
        .build()?;
    
    ctx.add_route_definition(lazy)?;
    
    // Route 2: Trigger route (starts lazy-route via controlbus)
    let trigger = RouteBuilder::from("timer:trigger?period=50&repeatCount=1")
        .route_id("trigger-route")
        .startup_order(20)
        .set_header("CamelRouteId", Value::String("lazy-route".into()))
        .to("controlbus:route?action=start")
        .build()?;
    
    ctx.add_route_definition(trigger)?;
    
    ctx.start().await?;
    
    // Wait for trigger to execute
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    
    // Lazy route should now be started
    let status = ctx.route_controller().lock().await.route_status("lazy-route");
    assert!(matches!(status, Some(camel_core::RouteStatus::Started)));
    
    ctx.stop().await?;
    Ok(())
}
```

**Step 2: Run test**

```bash
cargo test -p camel-test controlbus_test
```

Expected: Tests pass

**Step 3: Commit**

```bash
git add crates/camel-test/tests/controlbus_test.rs
git commit -m "test(camel-test): add integration tests for autoStartup and controlbus"
```

---

## Phase 3: Documentation & Examples

### Task 19: Add Example - Lazy Route

**Files:**
- Create: `examples/lazy-route/Cargo.toml`
- Create: `examples/lazy-route/src/main.rs`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "lazy-route"
version = "0.1.0"
edition = "2021"

[dependencies]
camel-api = { path = "../../crates/camel-api" }
camel-builder = { path = "../../crates/camel-builder" }
camel-core = { path = "../../crates/camel-core" }
camel-timer = { path = "../../crates/components/camel-timer" }
camel-log = { path = "../../crates/components/camel-log" }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal"] }
tracing-subscriber = "0.3"
```

**Step 2: Create main.rs**

```rust
use camel_api::CamelError;
use camel_builder::RouteBuilder;
use camel_core::context::CamelContext;
use camel_log::LogComponent;
use camel_timer::TimerComponent;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();
    
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    
    // Route that doesn't auto-start
    let lazy = RouteBuilder::from("timer:lazy?period=1000")
        .route_id("lazy-route")
        .auto_startup(false)
        .to("log:info?level=WARN")
        .build()?;
    
    ctx.add_route_definition(lazy)?;
    
    // Route that triggers the lazy route after 5 seconds
    let trigger = RouteBuilder::from("timer:trigger?delay=5000&repeatCount=1")
        .route_id("trigger-route")
        .log("Starting lazy route...", camel_processor::LogLevel::Info)
        .build()?;
    
    ctx.add_route_definition(trigger)?;
    
    ctx.start().await?;
    
    println!("Context started. Lazy route status:");
    let status = ctx.route_controller().lock().await.route_status("lazy-route");
    println!("  lazy-route -> {:?}", status);
    
    println!("\nWaiting 5 seconds before manual start...");
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    
    // Manual start from code
    ctx.route_controller().lock().await.start_route("lazy-route").await?;
    
    println!("\nLazy route status after manual start:");
    let status = ctx.route_controller().lock().await.route_status("lazy-route");
    println!("  lazy-route -> {:?}", status);
    
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    
    Ok(())
}
```

**Step 3: Add to workspace**

```toml
# In root Cargo.toml, add to workspace.members:
"examples/lazy-route",
```

**Step 4: Test example**

```bash
cargo run -p lazy-route
```

Expected: Route starts after 5 seconds

**Step 5: Commit**

```bash
git add examples/lazy-route/Cargo.toml
git add examples/lazy-route/src/main.rs
git add Cargo.toml
git commit -m "feat(examples): add lazy-route example"
```

---

### Task 20: Update README

**Files:**
- Modify: `README.md`

**Step 1: Add section on route lifecycle**

```markdown
## Route Lifecycle Management

rust-camel supports controlling when and how routes start:

### Auto Startup

By default, all routes start automatically when `ctx.start()` is called. You can disable this:

```rust
let route = RouteBuilder::from("timer:tick")
    .route_id("lazy-route")
    .auto_startup(false)  // Won't start automatically
    .to("log:info")
    .build()?;
```

### Startup Order

Control the order in which routes start (useful for dependencies):

```rust
let route_a = RouteBuilder::from("direct:a")
    .route_id("route-a")
    .startup_order(10)  // Starts first
    .to("log:info")
    .build()?;

let route_b = RouteBuilder::from("direct:b")
    .route_id("route-b")
    .startup_order(20)  // Starts after route-a
    .to("direct:a")
    .build()?;
```

### Runtime Control

Control routes dynamically from code or from other routes:

```rust
// From code:
ctx.route_controller().lock().await.start_route("lazy-route").await?;
ctx.route_controller().lock().await.stop_route("route-a").await?;

// From another route (using controlbus):
RouteBuilder::from("timer:monitor")
    .set_header("CamelRouteId", Value::String("backup-route".into()))
    .to("controlbus:route?action=start")
    .build()?
```

See `examples/lazy-route` for a complete example.
```

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add route lifecycle management section to README"
```

---

## Final Steps

### Task 21: Run Full Test Suite

**Step 1: Run all tests**

```bash
cargo test --workspace
```

Expected: All tests pass

**Step 2: Run clippy**

```bash
cargo clippy --workspace
```

Fix any warnings

**Step 3: Format code**

```bash
cargo fmt --all
```

**Step 4: Final commit**

```bash
git add .
git commit -m "chore: format and lint"
```

---

## Summary

**Total Tasks**: 21  
**Estimated Time**: 2-3 hours  

**Key Changes**:
- RouteDefinition: Added route_id, auto_startup, startup_order
- RouteController: New trait + DefaultRouteController implementation
- ProducerContext: Dependency injection pattern
- ControlBus: Dynamic route management component
- 6 components migrated to new Endpoint API
- Integration tests + examples

**Breaking Changes**:
- `Endpoint::create_producer()` now requires `&ProducerContext` parameter
- All component authors must update their implementations

**Future Work**:
- SupervisingRouteController (retry logic, health checks)
- Stats action for ControlBus
- More sophisticated suspend/resume
