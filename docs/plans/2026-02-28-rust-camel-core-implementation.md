# Rust-Camel Core Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the core rust-camel framework with Tower-based processors, enum body types, and MVP components (timer, log, direct, mock).

**Architecture:** Tower Services as processors, enum-based message body, async traits for components/endpoints, builder pattern for route DSL.

**Tech Stack:** Rust 2024, Tokio, Tower, serde, async-trait, thiserror, bytes, tracing

---

## Task 1: Setup Workspace Structure

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Create: `crates/camel-api/Cargo.toml`
- Create: `crates/camel-api/src/lib.rs`
- (similar for all crates)

**Step 1: Update workspace Cargo.toml**

Replace root `Cargo.toml` with complete workspace configuration including all dependencies.

**Step 2: Create crate directories**

Run: `mkdir -p crates/camel-{api,util,component,endpoint,processor,core,builder,dsl}/src`

**Step 3-4: Create all Cargo.toml files for each crate**

**Step 5: Create empty lib.rs files**

Run: `touch crates/camel-{api,util,component,endpoint,processor,core,builder,dsl}/src/lib.rs`

**Step 6: Verify workspace builds**

Run: `cargo check`

**Step 7: Commit**

Run: `git add -A && git commit -m "chore: setup workspace structure"`

---

## Task 2: Implement camel-api Core Types

**Files:**
- `crates/camel-api/src/body.rs` - Body enum
- `crates/camel-api/src/message.rs` - Message struct
- `crates/camel-api/src/exchange.rs` - Exchange struct
- `crates/camel-api/src/error.rs` - CamelError enum
- `crates/camel-api/src/value.rs` - Value type alias

**Key types:**

```rust
pub enum Body {
    Empty,
    Bytes(Bytes),
    Text(String),
    Json(serde_json::Value),
}

pub struct Message {
    pub headers: HashMap<String, Value>,
    pub body: Body,
}

pub struct Exchange {
    pub input: Message,
    pub output: Option<Message>,
    pub properties: HashMap<String, Value>,
    pub error: Option<CamelError>,
    pub pattern: ExchangePattern,
}
```

**Tests:** Body conversions, Message headers, Exchange lifecycle

**Commit:** `feat(camel-api): implement core types`

---

## Task 3: Implement Processor Trait

**Files:**
- `crates/camel-api/src/processor.rs`

**Key trait:**

```rust
pub trait Processor: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + Sync + 'static {}
```

**Include:** IdentityProcessor as basic implementation

**Tests:** IdentityProcessor passes through exchange unchanged

**Commit:** `feat(camel-api): add Processor trait`

---

## Task 4: Implement camel-component Traits

**Files:**
- `crates/camel-component/src/component.rs`
- `crates/camel-component/src/endpoint.rs`
- `crates/camel-component/src/consumer.rs`

**Key traits:**

```rust
pub trait Component: Send + Sync {
    fn scheme(&self) -> &str;
    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError>;
}

pub trait Endpoint: Send + Sync {
    fn uri(&self) -> &str;
    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError>;
    fn create_producer(&self) -> Result<Box<dyn Producer>, CamelError>;
}

pub trait Consumer: Send + Sync {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError>;
    async fn stop(&mut self) -> Result<(), CamelError>;
}
```

**Commit:** `feat(camel-component): implement Component, Endpoint, Consumer traits`

---

## Task 5: Implement camel-processor EIPs

**Files:**
- `crates/camel-processor/src/filter.rs`
- `crates/camel-processor/src/map_body.rs`
- `crates/camel-processor/src/set_header.rs`

**Filter processor:**
```rust
pub struct Filter<P, F> {
    inner: P,
    predicate: F, // Fn(&Exchange) -> bool
}
```

**MapBody processor:**
```rust
pub struct MapBody<P, F> {
    inner: P,
    mapper: F, // Fn(Body) -> Body
}
```

**SetHeader processor:**
```rust
pub struct SetHeader<P> {
    inner: P,
    key: String,
    value: Value,
}
```

**Tests:** Each processor tested in isolation

**Commit:** `feat(camel-processor): implement Filter, MapBody, SetHeader`

---

## Task 6: Implement CamelContext

**Files:**
- `crates/camel-core/src/context.rs`
- `crates/camel-core/src/route.rs`
- `crates/camel-core/src/registry.rs`

**CamelContext:**
```rust
pub struct CamelContext {
    components: Arc<RwLock<HashMap<String, Box<dyn Component>>>>,
    routes: Vec<Route>,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl CamelContext {
    pub fn new() -> Self;
    pub async fn register_component<C: Component>(&mut self, component: C);
    pub fn add_route(&mut self, route: Route);
    pub async fn start(&mut self) -> Result<(), CamelError>;
    pub async fn stop(&mut self) -> Result<(), CamelError>;
}
```

**Commit:** `feat(camel-core): implement CamelContext and Route`

---

## Task 7: Implement Timer Component

**Files:**
- `crates/components/camel-timer/Cargo.toml`
- `crates/components/camel-timer/src/lib.rs`

**URI format:** `timer:name?period=1000&delay=500&repeatCount=10`

**TimerConfig:**
```rust
pub struct TimerConfig {
    pub name: String,
    pub period: Duration,
    pub delay: Duration,
    pub repeat_count: Option<u32>,
}
```

**Tests:** URI parsing, config extraction

**Commit:** `feat(camel-timer): implement timer component`

---

## Task 8: Implement Log Component

**Files:**
- `crates/components/camel-log/Cargo.toml`
- `crates/components/camel-log/src/lib.rs`

**URI format:** `log:category?level=info&showHeaders=true`

**LogConfig:**
```rust
pub struct LogConfig {
    pub category: String,
    pub level: LogLevel,
    pub show_headers: bool,
    pub show_body: bool,
}
```

**Commit:** `feat(camel-log): implement log component`

---

## Task 9: Implement Direct Component

**Files:**
- `crates/components/camel-direct/Cargo.toml`
- `crates/components/camel-direct/src/lib.rs`

**URI format:** `direct:name`

**Purpose:** In-memory synchronous communication between routes

**Commit:** `feat(camel-direct): implement direct component`

---

## Task 10: Implement Mock Component

**Files:**
- `crates/components/camel-mock/Cargo.toml`
- `crates/components/camel-mock/src/lib.rs`

**URI format:** `mock:name`

**MockEndpoint:**
```rust
pub struct MockEndpoint {
    received: Arc<Mutex<Vec<Exchange>>>,
}

impl MockEndpoint {
    pub fn get_received_exchanges(&self) -> Vec<Exchange>;
    pub fn assert_exchange_count(&self, expected: usize);
}
```

**Commit:** `feat(camel-mock): implement mock component for testing`

---

## Task 11: Implement RouteBuilder

**Files:**
- `crates/camel-builder/src/lib.rs`
- `crates/camel-builder/src/route_builder.rs`

**RouteBuilder:**
```rust
pub struct RouteBuilder {
    from: String,
    processors: Vec<Box<dyn Processor>>,
}

impl RouteBuilder {
    pub fn from(endpoint: &str) -> Self;
    pub fn filter<P>(self, predicate: P) -> Self;
    pub fn to(self, endpoint: &str) -> Self;
    pub fn log(self, level: LogLevel, message: &str) -> Self;
    pub fn set_header<K, V>(self, key: K, value: V) -> Self;
    pub fn build(self) -> Result<Route, CamelError>;
}
```

**Commit:** `feat(camel-builder): implement RouteBuilder fluent API`

---

## Task 12: Create Hello World Example

**Files:**
- `examples/hello-world/Cargo.toml`
- `examples/hello-world/src/main.rs`

**Example code:**
```rust
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();
    
    ctx.register_component(TimerComponent::new()).await;
    ctx.register_component(LogComponent::new()).await;
    
    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=5")
        .set_header("source", Value::String("timer".into()))
        .to("log:info?showHeaders=true")
        .build()?;
    
    ctx.add_route(route);
    ctx.start().await?;
    
    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;
    
    Ok(())
}
```

**Commit:** `feat(examples): add hello-world example`

---

## Task 13: Integration Tests

**Files:**
- `tests/integration_test.rs`

**Test scenarios:**
1. Timer → Mock (verify exchanges received)
2. Timer → Filter → Mock (verify filtering works)
3. Timer → SetHeader → Mock (verify headers set)

**Commit:** `test: add integration tests`

---

## Verification Commands

After all tasks:

```bash
cargo test --workspace
cargo clippy --workspace
cargo fmt --check
```

---

## Notes

- Use TDD: write test first, then implementation
- Commit after each task
- Run `cargo test` frequently
- The design document is at `docs/plans/2026-02-28-rust-camel-core-design.md`
