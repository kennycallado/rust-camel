# camel-bean

The `camel-bean` crate provides a bean/registry system for rust-camel, enabling dependency injection and business logic integration with routes defined in YAML DSL or Rust code.

## Overview

The bean system allows you to:
- Define business logic as structured beans with typed methods
- Register beans in a central registry for lookup by name
- Invoke bean methods from routes with automatic parameter binding
- Integrate seamlessly with YAML DSL via the `bean` step type

This enables clean separation between route configuration and business logic, similar to Spring beans in Apache Camel.

## Quick Start

1. Add `camel-bean` to your `Cargo.toml`:
```toml
[dependencies]
camel-bean = "0.6.0"
camel-bean-macros = "0.6.0"
```

2. Define a bean with handlers:
```rust
use camel_bean::{bean_impl, handler};

pub struct OrderService;

#[bean_impl]
impl OrderService {
    #[handler]
    pub async fn process(&self, body: Order) -> Result<ProcessedOrder, String> {
        // Business logic here
        Ok(processed_order)
    }
}
```

3. Register and use the bean:
```rust
use camel_bean::BeanRegistry;

let mut registry = BeanRegistry::new();
registry.register("orderService", OrderService);

// Invoke from code
registry.invoke("orderService", "process", &mut exchange).await?;
```

4. Use in YAML DSL:
```yaml
routes:
  - id: "process-order"
    from: "direct:orders"
    steps:
      - bean:
          name: "orderService"
          method: "process"
```

## API Reference

### BeanRegistry

The central registry for storing and retrieving beans.

```rust
use camel_bean::{BeanRegistry, BeanProcessor};

let mut registry = BeanRegistry::new();

// Register a bean
registry.register("myBean", MyService);

// Get a bean
if let Some(bean) = registry.get("myBean") {
    // Use bean
}

// Invoke a method directly
registry.invoke("myBean", "methodName", &mut exchange).await?;

// Registry information
registry.len()  // Number of registered beans
registry.is_empty()  // Whether registry is empty
```

### BeanProcessor Trait

The trait that all beans must implement. The `#[bean_impl]` macro handles this automatically.

```rust
use async_trait::async_trait;
use camel_api::{CamelError, Exchange};
use camel_bean::BeanProcessor;

#[async_trait]
pub trait BeanProcessor: Send + Sync {
    async fn call(&self, method: &str, exchange: &mut Exchange) -> Result<(), CamelError>;
    
    fn methods(&self) -> Vec<&'static str>;
}
```

### Attributes

#### `#[bean_impl]`

Applied to an `impl` block to generate the `BeanProcessor` implementation:

```rust
#[bean_impl]
impl MyService {
    // Methods with #[handler] will be callable
}
```

#### `#[handler]`

Applied to methods within a `#[bean_impl]` block to make them invocable:

```rust
#[bean_impl]
impl MyService {
    #[handler]
    pub async fn process(&self, body: MyData) -> Result<ResultData, String> {
        // Method implementation
    }
}
```

## Parameter Binding

The bean system supports automatic parameter binding from the exchange to handler methods:

### Supported Parameter Types

| Parameter Type | Source | Description |
|---------------|--------|-------------|
| `body: T` | Message body | Deserialized from JSON if needed |
| `headers: Headers` | Exchange headers | All headers as a struct |
| `exchange: &mut Exchange` | Full exchange | Complete exchange object |

### Example with Multiple Parameters

```rust
use camel_bean::{bean_impl, handler};
use camel_api::headers::Headers;

#[bean_impl]
impl MyService {
    #[handler]
    pub async fn process(
        &self,
        body: MyData,           // From message body
        headers: Headers,       // All headers
        exchange: &mut Exchange, // Full exchange
    ) -> Result<ResponseData, String> {
        // Access all parameters
    }
}
```

## Error Handling

The bean system provides comprehensive error handling:

### BeanError

```rust
use camel_bean::BeanError;

pub enum BeanError {
    #[error("Bean not found: {0}")]
    NotFound(String),

    #[error("Bean method not found: {0}")]
    MethodNotFound(String),

    #[error("Parameter binding failed: {0}")]
    BindingFailed(String),

    #[error("Handler execution failed: {0}")]
    ExecutionFailed(String),
}
```

### Error Conversion

`BeanError` automatically converts to `CamelError`:

```rust
impl From<BeanError> for camel_api::CamelError {
    fn from(err: BeanError) -> Self {
        camel_api::CamelError::ProcessorError(err.to_string())
    }
}
```

### Handler Return Types

Handlers can return:
- `Result<T, E>` where `E: Display` - for successful/failed processing
- `T` - simple successful response
- Any type that implements `serde::Serialize` for JSON response

## Integration with camel-core

The bean system integrates with the core route controller:

```rust
use camel_core::context::CamelContext;
use camel_bean::BeanRegistry;

// Inside an async function
let mut ctx = CamelContext::builder().build().await?;
let bean_registry = Arc::new(BeanRegistry::new());

// Register beans
bean_registry.register("myService", MyService);

// Set in context
ctx.set_bean_registry(bean_registry);
```

This enables:
- Bean invocation from YAML DSL routes
- Bean discovery and management
- Integration with route lifecycle

## Examples

See the following examples for complete usage:
- `examples/bean-demo` - Comprehensive bean usage demonstration
- `examples/yaml-dsl` - YAML DSL integration example

## Best Practices

1. **Use structured types**: Define clear request/response types for handlers
2. **Error handling**: Return descriptive error messages from handlers
3. **Validation**: Validate input parameters before processing
4. **Async operations**: Leverage async I/O for external service calls
5. **Bean naming**: Use consistent naming conventions for beans

## Type Safety

The bean system provides compile-time type safety:
- Handler signatures are validated at compile time
- Parameter binding is checked for supported types
- Return types must be serializable
- Method names are checked during route parsing

## Performance Considerations

- Bean lookup is O(1) via HashMap
- Parameter binding uses zero-copy where possible
- JSON deserialization only when needed
- Minimal runtime overhead after initial registration

## Advanced Usage

### Dynamic Bean Registration

```rust
// Runtime bean registration
let mut registry = BeanRegistry::new();

// Register different implementations based on config
if cfg.feature_enabled {
    registry.register("service", PremiumService);
} else {
    registry.register("service", BasicService);
}
```

### Bean Composition

```rust
// Beans can use other beans
pub struct Orchestrator {
    order_service: Arc<dyn OrderProcessor>,
    payment_service: Arc<dyn PaymentProcessor>,
}

#[bean_impl]
impl Orchestrator {
    #[handler]
    pub async fn process_order(&self, body: Order) -> Result<CompleteOrder, String> {
        // Use other beans/services
        // ...
    }
}
```

## Future Enhancements

Planned features include:
- Constructor dependency injection
- Lifecycle callbacks
- Bean scopes (singleton, prototype)
- Conditional bean registration
- Integration with service discovery

## License

Licensed under the same terms as rust-camel.
