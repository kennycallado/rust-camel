# Rust-Camel Core Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:writing-plans to create implementation plan from this design.

**Goal:** Implement a Tower-centric integration framework in Rust, inspired by Apache Camel, starting with core abstractions and essential components.

**Architecture:** Tower Services as processors, enum-based message body, async traits for components/endpoints, builder pattern for route DSL.

**Tech Stack:** Rust 2024, Tokio, Tower, serde, async-trait

---

## Design Decisions Summary

| Aspect | Decision |
|--------|----------|
| **Objective** | Core extensible towards full Apache Camel alternative |
| **Body type** | Enum Body (Empty, Bytes, Text, Json) |
| **Async runtime** | Tokio + Tower |
| **DSL** | Builder pattern (extensible for future DSLs) |
| **MVP Components** | direct, log, timer, mock |
| **Architecture** | Tower-centric (processors as Services) |

---

## Section 1: Core Types

### Body

```rust
pub enum Body {
    Empty,
    Bytes(Bytes),
    Text(String),
    Json(serde_json::Value),
}
```

Rationale: Type-safe for common cases, extensible via Json variant for arbitrary data.

### Message

```rust
pub type Headers = HashMap<String, Value>;
pub type Value = serde_json::Value;

pub struct Message {
    pub headers: Headers,
    pub body: Body,
}
```

### Exchange

```rust
pub struct Exchange {
    pub input: Message,
    pub output: Option<Message>,
    pub properties: HashMap<String, Value>,
    pub error: Option<CamelError>,
    pub pattern: ExchangePattern,
}

pub enum ExchangePattern {
    InOnly,   // fire-and-forget
    InOut,    // request-reply
}
```

### CamelError

```rust
#[derive(Debug, thiserror::Error)]
pub enum CamelError {
    #[error("Component not found: {0}")]
    ComponentNotFound(String),
    
    #[error("Endpoint creation failed: {0}")]
    EndpointCreationFailed(String),
    
    #[error("Processor error: {0}")]
    ProcessorError(String),
    
    #[error("Type conversion failed: {0}")]
    TypeConversionFailed(String),
    
    #[error("Invalid URI: {0}")]
    InvalidUri(String),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

---

## Section 2: Processor = Tower Service

### Core Trait

```rust
use tower::Service;

/// Processor is a Tower Service that transforms Exchange
pub trait Processor: Service<Exchange, Response = Exchange, Error = CamelError> {}

// Blanket implementation
impl<P> Processor for P 
where 
    P: Service<Exchange, Response = Exchange, Error = CamelError> 
{}
```

### Processor Extensions

```rust
pub trait ProcessorExt: Sized {
    fn filter<F>(self, predicate: F) -> FilterLayer<F>
    where
        F: Fn(&Exchange) -> bool + Clone;
    
    fn map_body<F>(self, f: F) -> MapBodyLayer<F>
    where
        F: Fn(Body) -> Body + Clone;
    
    fn set_header<K, V>(self, key: K, value: V) -> SetHeaderLayer
    where
        K: Into<String>,
        V: Into<Value>;
    
    fn to(self, endpoint: &str) -> ToEndpointLayer;
}
```

### Built-in Processors (EIPs)

| Processor | Tower Layer | Purpose |
|-----------|-------------|---------|
| `Filter` | `tower::filter::AsyncFilterLayer` | Conditional routing |
| `MapBody` | Custom | Transform body |
| `SetHeader` | Custom | Add/modify headers |
| `ToEndpoint` | Custom | Send to endpoint |
| `Split` | Custom | Split message into parts |
| `Aggregate` | Custom | Combine messages |
| `DeadLetterChannel` | Custom | Error handling |

---

## Section 3: Components & Endpoints

### Component

```rust
#[async_trait]
pub trait Component: Send + Sync {
    /// URI scheme this component handles (e.g., "timer", "log")
    fn scheme(&self) -> &str;
    
    /// Create an endpoint from URI
    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError>;
}
```

### Endpoint

```rust
#[async_trait]
pub trait Endpoint: Send + Sync {
    fn uri(&self) -> &str;
    
    /// Create consumer (receives messages from external source)
    fn create_consumer(&self) -> Box<dyn Consumer>;
    
    /// Create producer (sends messages to external destination)
    fn create_producer(&self) -> Result<Box<dyn Producer>, CamelError>;
}
```

### Consumer

```rust
pub struct ConsumerContext {
    sender: mpsc::Sender<Exchange>,
}

impl ConsumerContext {
    pub async fn send(&self, exchange: Exchange) -> Result<(), CamelError> {
        self.sender.send(exchange).await.map_err(|_| CamelError::ChannelClosed)
    }
}

#[async_trait]
pub trait Consumer: Send + Sync {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError>;
    async fn stop(&mut self) -> Result<(), CamelError>;
}
```

### Producer

```rust
pub type Producer = Box<dyn Service<Exchange, Response = Exchange, Error = CamelError, Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>> + Send + Sync>;
```

---

## Section 4: CamelContext & RouteBuilder

### CamelContext

```rust
pub struct CamelContext {
    components: HashMap<String, Box<dyn Component>>,
    routes: Vec<Route>,
    type_converters: TypeConverterRegistry,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl CamelContext {
    pub fn new() -> Self;
    
    pub fn register_component<C: Component + 'static>(&mut self, component: C);
    
    pub fn add_route(&mut self, route: Route);
    
    pub async fn start(&mut self) -> Result<(), CamelError>;
    
    pub async fn stop(&mut self) -> Result<(), CamelError>;
    
    pub fn component(&self, scheme: &str) -> Option<&dyn Component>;
}
```

### RouteBuilder

```rust
pub struct RouteBuilder {
    from: String,
    processors: Vec<Box<dyn Processor>>,
}

impl RouteBuilder {
    pub fn from(endpoint: &str) -> Self;
    
    pub fn filter<P>(mut self, predicate: P) -> Self 
    where 
        P: Fn(&Exchange) -> bool + Clone + Send + Sync + 'static;
    
    pub fn to(mut self, endpoint: &str) -> Self;
    
    pub fn log(mut self, level: LogLevel, message: &str) -> Self;
    
    pub fn set_header<K, V>(mut self, key: K, value: V) -> Self
    where
        K: Into<String>,
        V: Into<Value>;
    
    pub fn build(self) -> Result<Route, CamelError>;
}
```

### Usage Example

```rust
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    let mut ctx = CamelContext::new();
    
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    
    let route = RouteBuilder::from("timer:tick?period=1000")
        .set_header("source", "timer")
        .filter(|ex| ex.input.header("active").is_some())
        .to("log:info?showHeaders=true")
        .build()?;
    
    ctx.add_route(route);
    ctx.start().await?;
    
    // Run until ctrl-c
    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;
    
    Ok(())
}
```

---

## Section 5: Crate Structure

```
rust-camel/
├── Cargo.toml                    # Workspace
├── crates/
│   ├── camel-api/                # Core traits & types (zero internal deps)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── body.rs           # Body enum
│   │   │   ├── message.rs        # Message struct
│   │   │   ├── exchange.rs       # Exchange, ExchangePattern
│   │   │   ├── error.rs          # CamelError
│   │   │   ├── value.rs          # Value type alias
│   │   │   └── processor.rs      # Processor trait
│   │   └── Cargo.toml
│   │
│   ├── camel-core/               # Runtime engine
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── context.rs        # CamelContext
│   │   │   ├── route.rs          # Route execution
│   │   │   └── registry.rs       # Component registry
│   │   └── Cargo.toml
│   │
│   ├── camel-component/          # Component trait & abstractions
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── component.rs      # Component trait
│   │   │   ├── endpoint.rs       # Endpoint trait
│   │   │   ├── consumer.rs       # Consumer trait
│   │   │   └── producer.rs       # Producer type
│   │   └── Cargo.toml
│   │
│   ├── camel-endpoint/           # Endpoint utilities
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   └── uri.rs            # URI parsing helpers
│   │   └── Cargo.toml
│   │
│   ├── camel-processor/          # EIP implementations
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── filter.rs
│   │   │   ├── map.rs
│   │   │   ├── log.rs
│   │   │   └── to.rs
│   │   └── Cargo.toml
│   │
│   ├── camel-builder/            # RouteBuilder fluent API
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   └── route_builder.rs
│   │   └── Cargo.toml
│   │
│   ├── camel-dsl/                # Future: YAML, JSON DSLs
│   │   ├── src/lib.rs
│   │   └── Cargo.toml
│   │
│   ├── camel-util/               # Utilities
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   └── type_converter.rs
│   │   └── Cargo.toml
│   │
│   └── components/               # Component implementations
│       ├── camel-timer/
│       ├── camel-log/
│       ├── camel-direct/
│       └── camel-mock/
│
└── examples/
    └── hello-world/
```

### Dependency Graph

```
camel-api (zero internal deps)
    ↓
camel-util
    ↓
camel-component ← camel-endpoint
    ↓
camel-processor
    ↓
camel-core
    ↓
camel-builder
    ↓
components/* (depend on camel-api, camel-component, camel-core)
```

---

## Section 6: MVP Components

### camel-timer

```rust
pub struct TimerComponent;

impl Component for TimerComponent {
    fn scheme(&self) -> &str { "timer" }
    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError>;
}

// URI: timer:name?period=1000&delay=500&repeatCount=10
pub struct TimerConfig {
    pub name: String,
    pub period: Duration,
    pub delay: Duration,
    pub repeat_count: Option<u32>,
}
```

### camel-log

```rust
pub struct LogComponent;

impl Component for LogComponent {
    fn scheme(&self) -> &str { "log" }
    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError>;
}

// URI: log:category?level=info&showHeaders=true&showBody=true
pub struct LogConfig {
    pub category: String,
    pub level: LogLevel,
    pub show_headers: bool,
    pub show_body: bool,
}
```

### camel-direct

```rust
pub struct DirectComponent;

impl Component for DirectComponent {
    fn scheme(&self) -> &str { "direct" }
    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError>;
}

// URI: direct:name
// In-memory synchronous communication between routes
```

### camel-mock

```rust
pub struct MockComponent;

impl Component for MockComponent {
    fn scheme(&self) -> &str { "mock" }
    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError>;
}

// URI: mock:name
// Testing component that records received exchanges
pub struct MockEndpoint {
    received: Arc<Mutex<Vec<Exchange>>>,
}

impl MockEndpoint {
    pub fn get_received_exchanges(&self) -> Vec<Exchange>;
    pub fn assert_exchange_count(&self, expected: usize);
}
```

---

## Section 7: Error Handling

### Strategy

1. **Processor errors**: Stored on `Exchange.error`, route continues with error handler
2. **Component errors**: Return `Result<_, CamelError>` immediately
3. **Dead Letter Channel**: Optional error handler processor at route level

```rust
let route = RouteBuilder::from("timer:tick")
    .error_handler(DeadLetterChannel::new("log:error"))
    .filter(|ex| /* might fail */)
    .to("direct:output")
    .build()?;
```

---

## Section 8: Testing Strategy

### Unit Tests

- Each processor as isolated Tower Service
- Body enum conversions
- URI parsing

### Integration Tests

- Route execution with mock components
- CamelContext lifecycle
- Multi-route scenarios

### Example Test

```rust
#[tokio::test]
async fn test_filter_processor() {
    let mut exchange = Exchange::new(Message::default());
    exchange.input.headers.insert("active".into(), Value::Bool(true));
    
    let filter = FilterProcessor::new(|ex| ex.input.header("active").is_some());
    let result = filter.oneshot(exchange).await;
    
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_route_with_timer_and_mock() {
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(MockComponent::new());
    
    let mock = ctx.get_mock_endpoint("mock:result").unwrap();
    
    let route = RouteBuilder::from("timer:tick?repeatCount=1")
        .to("mock:result")
        .build()?;
    
    ctx.add_route(route);
    ctx.start().await?;
    
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    mock.assert_exchange_count(1);
    ctx.stop().await?;
}
```

---

## Future Extensions (Out of MVP Scope)

1. **More components**: file, http, kafka, database
2. **More EIPs**: Splitter, Aggregator, Routing Slip, Wire Tap
3. **Type converters**: Automatic body type conversion
4. **DSL alternatives**: YAML, JSON route definitions
5. **Management API**: Health checks, metrics, tracing
6. **Clustering**: Distributed route execution

---

## Next Steps

1. Use `superpowers:writing-plans` skill to create detailed implementation plan
2. Implement `camel-api` crate first (core types)
3. Implement `camel-component` traits
4. Implement `camel-core` with basic route execution
5. Implement MVP components
6. Add integration tests
