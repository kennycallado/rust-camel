# AutoStart + RouteController Design

**Date**: 2026-03-02  
**Status**: Design approved  
**Target**: rust-camel 0.1.x (pre-release, API breaks allowed)

## Overview

Implement Apache Camel's route lifecycle management in rust-camel:

- **autoStartup**: Control which routes start automatically
- **startupOrder**: Define startup sequence for inter-dependent routes
- **RouteController**: Runtime control (start/stop/suspend/resume/restart)
- **controlbus component**: Dynamic route management from within routes

This enables:
1. Route dependencies (route B waits for route A to be ready)
2. Dynamic control (activate backup route when primary fails)
3. Optimized startup (lazy-load expensive routes)

## Architecture

### Design Philosophy

- **Fidelity to Apache Camel**: Maintain concepts and behavior from Camel 4.x
- **Idiomatic Rust**: Use traits, explicit parameters, type safety
- **Extensible**: Start with DefaultRouteController, enable SupervisingRouteController later
- **Non-breaking**: Existing code continues to work (all new fields have defaults)

### Key Components

```
┌─────────────────────────────────────────────────────┐
│ CamelContext                                         │
│  ├─ Registry (components)                           │
│  ├─ RouteController (Arc<Mutex<dyn RouteController>>)│
│  └─ start() → delegates to RouteController          │
└─────────────────────────────────────────────────────┘
         │
         ├─ creates ProducerContext ──────┐
         │                                 │
         ▼                                 ▼
┌──────────────────────┐      ┌──────────────────────┐
│ RouteBuilder         │      │ ControlBusComponent  │
│  ├─ route_id()       │      │  └─ receives         │
│  ├─ auto_startup()   │      │     RouteController  │
│  └─ startup_order()  │      │     via context      │
└──────────────────────┘      └──────────────────────┘
         │                                 │
         ▼                                 ▼
┌──────────────────────┐      ┌──────────────────────┐
│ RouteDefinition      │      │ ControlBusProducer   │
│  ├─ route_id         │      │  └─ calls            │
│  ├─ auto_startup     │      │     route_controller │
│  └─ startup_order    │      │     .start_route()   │
└──────────────────────┘      └──────────────────────┘
```

## Component Details

### 1. RouteDefinition Enhancement

**Location**: `crates/camel-core/src/route.rs`

**New fields**:
```rust
pub struct RouteDefinition {
    pub(crate) from_uri: String,
    pub(crate) steps: Vec<BuilderStep>,
    pub(crate) error_handler: Option<ErrorHandlerConfig>,
    pub(crate) circuit_breaker: Option<CircuitBreakerConfig>,
    pub(crate) concurrency: Option<ConcurrencyModel>,
    
    // NEW:
    pub(crate) route_id: Option<String>,      // Unique ID (auto-generated if None)
    pub(crate) auto_startup: bool,             // Default: true
    pub(crate) startup_order: i32,             // Default: 1000
}
```

**Builder methods**:
```rust
impl RouteBuilder {
    pub fn route_id(mut self, id: impl Into<String>) -> Self;
    pub fn auto_startup(mut self, auto: bool) -> Self;
    pub fn startup_order(mut self, order: i32) -> Self;
}
```

**Auto-generation**:
- If `route_id` is `None`, CamelContext generates `"route-0"`, `"route-1"`, etc.
- Routes without explicit `startup_order` get auto-assigned starting from 1000
- Camel starts routes in ascending `startup_order`, stops in reverse

### 2. RouteController Trait

**Location**: `crates/camel-core/src/route_controller.rs`

**Trait definition**:
```rust
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

**RouteStatus enum**:
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

### 3. DefaultRouteController

**Location**: `crates/camel-core/src/route_controller.rs`

**Behavior**:
- Fail-fast: if any route fails to start, CamelContext fails to start
- Sequential startup: routes start one-by-one in `startup_order`
- Stores route state, consumer/pipeline handles, cancel tokens
- Resolves pipelines lazily on first start

**Internal structure**:
```rust
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
    registry: Arc<Registry>,
}
```

**Key implementation notes**:
- `start_route()` moves consumer/pipeline spawning logic from `CamelContext::start()`
- `stop_route()` signals cancellation, waits for graceful drain
- Pipeline resolution is reused across restarts

### 4. ProducerContext Pattern

**Location**: `crates/camel-component/src/producer.rs`

**Breaking change**: Modify `Endpoint::create_producer()` signature:

```rust
// Before:
fn create_producer(&self) -> Result<BoxProcessor, CamelError>;

// After:
fn create_producer(&self, ctx: &ProducerContext) -> Result<BoxProcessor, CamelError>;
```

**ProducerContext**:
```rust
pub struct ProducerContext {
    route_controller: Arc<Mutex<dyn RouteController>>,
}

impl ProducerContext {
    pub fn route_controller(&self) -> &Arc<Mutex<dyn RouteController>> {
        &self.route_controller
    }
}
```

**Migration impact**:
- All existing components need to add `ctx: &ProducerContext` parameter
- Most components ignore it (just use `_ctx`)
- ControlBus uses it to access RouteController

### 5. ControlBus Component

**Location**: `crates/components/camel-controlbus/src/lib.rs`

**URI format**: `controlbus:route?routeId=xxx&action=yyy`

**Supported actions**:
- `start`: Start a stopped route
- `stop`: Stop a running route
- `suspend`: Suspend (pause) a route
- `resume`: Resume a suspended route
- `restart`: Stop + start
- `status`: Get route status (returned in message body)

**Implementation**:
```rust
struct ControlBusProducer {
    route_id: Option<String>,
    action: RouteAction,
    controller: Arc<Mutex<dyn RouteController>>,
}

impl Service<Exchange> for ControlBusProducer {
    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        // Get route_id from parameter or CamelRouteId header
        // Lock controller, execute action
        // For status: put result in body
    }
}
```

**Usage example**:
```rust
RouteBuilder::from("timer:monitor?period=60000")
    .set_header("CamelRouteId", Value::String("backup-route".into()))
    .to("controlbus:route?action=start")
    .build()?
```

### 6. CamelContext Integration

**Location**: `crates/camel-core/src/context.rs`

**Changes**:
```rust
pub struct CamelContext {
    registry: Arc<Registry>,
    route_controller: Arc<Mutex<DefaultRouteController>>,  // NEW
    cancel_token: CancellationToken,
    global_error_handler: Option<ErrorHandlerConfig>,
}

impl CamelContext {
    pub fn route_controller(&self) -> &Arc<Mutex<DefaultRouteController>>;
    
    pub fn add_route_definition(&mut self, definition: RouteDefinition) -> Result<(), CamelError> {
        // Auto-generate ID if needed
        // Delegate to route_controller.add_route()
    }
    
    pub async fn start(&mut self) -> Result<(), CamelError> {
        // Delegate to route_controller.start_all_routes()
    }
}
```

## Migration Guide

### For Component Authors

**Before**:
```rust
impl Endpoint for MyEndpoint {
    fn create_producer(&self) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(MyProducer))
    }
}
```

**After**:
```rust
impl Endpoint for MyEndpoint {
    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(MyProducer))
    }
}
```

### For Route Authors

**Before**:
```rust
let route = RouteBuilder::from("timer:tick")
    .to("log:info")
    .build()?;

ctx.add_route_definition(route)?;
ctx.start().await?;  // All routes start immediately
```

**After** (no changes required, but new options available):
```rust
let route = RouteBuilder::from("timer:tick")
    .route_id("my-timer")        // Optional: explicit ID
    .auto_startup(false)          // Optional: don't auto-start
    .startup_order(10)            // Optional: startup order
    .to("log:info")
    .build()?;

ctx.add_route_definition(route)?;
ctx.start().await?;  // Only routes with auto_startup=true start

// Later: manual control
ctx.route_controller().lock().await.start_route("my-timer").await?;
```

## Future Extensions

### SupervisingRouteController (Phase 2)

**When**: After validating DefaultRouteController API

**Features**:
- Backoff retry on startup failures
- Health checks integration
- Concurrent startup for independent routes
- Route filtering (include/exclude patterns)

**Implementation**:
```rust
pub struct SupervisingRouteController {
    inner: DefaultRouteController,
    backoff_delay: Duration,
    backoff_max_attempts: Option<usize>,
    include_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
}
```

**Configuration**:
```rust
let controller = SupervisingRouteController::new(registry)
    .with_backoff_delay(Duration::from_secs(5))
    .with_max_attempts(3)
    .exclude_routes("aws*");  // Fail fast for critical routes

ctx.set_route_controller(controller);
```

### Stats Action (Phase 2)

**ControlBus extension**:
```rust
.to("controlbus:route?routeId=foo&action=stats")
// Returns XML/JSON with performance metrics
```

**Requires**:
- Metrics collection in RouteController
- Integration with `metrics` crate

## Testing Strategy

### Unit Tests

1. **RouteBuilder**:
   - Auto-generation of route IDs
   - Default values for auto_startup and startup_order
   - Builder methods preserve immutability

2. **RouteController**:
   - Start/stop lifecycle transitions
   - Status tracking accuracy
   - Startup order enforcement
   - Auto_startup filtering

3. **ControlBus**:
   - All actions execute correctly
   - Status returned in body
   - Error handling for invalid route IDs

### Integration Tests

1. **Route dependencies**:
   - Route B depends on route A (startup order)
   - Verify B starts after A

2. **Dynamic control**:
   - Start route from controlbus
   - Stop route from controlbus
   - Get status from controlbus

3. **Graceful shutdown**:
   - Stop in reverse startup order
   - In-flight exchanges complete

### Example Programs

1. **lazy-route**: Route that doesn't auto-start
2. **route-dependencies**: Multiple routes with startup order
3. **controlbus-example**: Dynamic route management from another route

## Implementation Checklist

### Phase 1: Core Infrastructure
- [ ] Add route_id, auto_startup, startup_order to RouteDefinition
- [ ] Add builder methods to RouteBuilder
- [ ] Create RouteController trait
- [ ] Implement DefaultRouteController
- [ ] Add ProducerContext and update Endpoint trait
- [ ] Migrate all existing components to new Endpoint signature
- [ ] Update CamelContext to use RouteController

### Phase 2: ControlBus
- [ ] Implement ControlBusComponent
- [ ] Implement ControlBusEndpoint
- [ ] Implement ControlBusProducer
- [ ] Add unit tests for controlbus
- [ ] Add integration tests for dynamic control

### Phase 3: Documentation & Examples
- [ ] Update README with new features
- [ ] Add example: lazy-route
- [ ] Add example: route-dependencies
- [ ] Add example: controlbus-example
- [ ] Write migration guide for component authors

## References

- [Apache Camel - Route Controller](https://camel.apache.org/manual/route-controller.html)
- [Apache Camel - Auto Startup](https://camel.apache.org/manual/configuring-route-startup-ordering-and-autostartup.html)
- [Apache Camel - ControlBus Component](https://camel.apache.org/components/4.18.x/controlbus-component.html)
