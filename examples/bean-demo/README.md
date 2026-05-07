# Bean Demo Example

Demonstrates native beans in rust-camel — multi-method components registered in a `BeanRegistry` and invoked from routes or directly.

## What Are Beans?

Beans are **multi-method components** — unlike processors (single `process()` method), a bean exposes multiple named handlers on one struct. Think of it as a service object with several entry points, each callable independently from routes.

| | Processor | Bean |
|---|---|---|
| Methods | One (`process`) | Many (`#[handler]` per method) |
| Use case | Transform/filter a message | Service with multiple operations (validate, process, stats) |
| Invocation | `to: "bean:name"` | `bean: { name, method }` or `registry.invoke(name, method)` |

## What This Example Demonstrates

- **`#[bean_impl]`** — attribute macro on an `impl` block that generates `BeanProcessor`
- **`#[handler]`** — marks individual methods as callable bean handlers
- **`BeanRegistry`** — register beans by name, invoke by `(name, method)` pair
- **Typed parameters** — body deserialized from JSON, headers, or full exchange access
- **Typed returns** — return values serialized back into the exchange body

## Quick Start

```bash
cargo run --example bean-demo
```

## Defining a Bean

```rust
use camel_bean::{bean_impl, handler};

pub struct OrderService;

#[bean_impl]
impl OrderService {
    #[handler]
    pub async fn process(&self, body: Order) -> Result<ProcessedOrder, String> {
        // body is deserialized from exchange.input.body (JSON)
        // return value is serialized back into exchange.input.body
        Ok(ProcessedOrder { .. })
    }

    #[handler]
    pub async fn validate(&self, body: Order) -> Result<Order, String> {
        // validation logic
        Ok(body)
    }

    #[handler]
    pub async fn get_stats(&self) -> Result<String, String> {
        // no body parameter — handler reads nothing from exchange
        Ok("stats...".to_string())
    }
}
```

The `#[bean_impl]` macro generates a `BeanProcessor` impl that:
- Dispatches `call(method, exchange)` to the correct `#[handler]` via match
- Deserializes the exchange body into the handler's body parameter type
- Serializes the handler's return value back into the exchange body
- Exposes `methods()` listing all handler names

## Registering Beans

```rust
use camel_bean::BeanRegistry;

let mut registry = BeanRegistry::new();
registry.register("orderService", OrderService).expect("register");

// Invoke by name + method
registry.invoke("orderService", "validate", &mut exchange).await?;
registry.invoke("orderService", "process", &mut exchange).await?;
```

## Configuring Beans in Camel.toml

For WASM bean plugins, add a `[beans]` section:

```toml
[beans.orderService]
plugin = "order-service-bean"

[beans.auth]
plugin = "my-auth"
```

Each entry maps a bean name to a WASM plugin file (without `.wasm` extension). The plugin is loaded at startup and registered under the given name.

## Using Beans in YAML Routes

```yaml
routes:
  - id: "order-processing"
    from: "direct:start"
    steps:
      - bean:
          name: "orderService"
          method: "validate"
      - bean:
          name: "orderService"
          method: "process"
      - to: "log:processed"
```

The `bean:` step invokes the named bean's method, passing the current exchange. The method's return value replaces the exchange body.

## Handler Parameter Types

Handlers support three parameter patterns (detected automatically by `#[bean_impl]`):

| Signature | Behavior |
|---|---|
| `fn(&self, body: T) -> Result<R, E>` | Deserializes exchange body as `T`, serializes `R` back |
| `fn(&self) -> Result<R, E>` | No body input, serializes `R` back |
| `fn(&self, exchange: &mut Exchange) -> Result<(), E>` | Full exchange access, manages body manually |

Headers can also be accessed via a `headers` parameter (type `serde_json::Value`).

## Running the Example

```bash
cargo run --example bean-demo
```

Output shows five demos:
1. **Valid order validation** — passes all checks
2. **Invalid order validation** — catches missing fields, negative total
3. **High-value order processing** — applies 10% discount (>$1000)
4. **Statistics retrieval** — handler with no body parameter
5. **Regular order processing** — no discount applied

## Next Steps: WASM Bean Plugins

This example uses **native beans** (compiled into your binary). For runtime-loadable beans, use WASM plugins:

```bash
# Create a WASM bean plugin project
camel plugin new my-bean --type bean

# Build and install
cd my-bean
camel plugin build
```

WASM beans implement `BeanPlugin` instead of `Guest`:

```rust
use camel_wasm_sdk::{export_bean, BeanPlugin, WasmExchange, WasmError};

struct MyBean;

impl BeanPlugin for MyBean {
    fn methods() -> Vec<&'static str> {
        vec!["validate", "transform"]
    }

    fn invoke(method: &str, exchange: WasmExchange) -> Result<WasmExchange, WasmError> {
        match method {
            "validate" => Ok(exchange),
            "transform" => Ok(exchange),
            _ => Err(WasmError::ProcessorError(format!("unknown method: {method}"))),
        }
    }
}

export_bean!(MyBean);
```

See `camel-wasm-sdk` README for full WASM bean documentation.

## Related Crates

| Crate | Purpose |
|---|---|
| `camel-bean` | BeanRegistry, BeanProcessor trait |
| `camel-bean-macros` | `#[bean_impl]`, `#[handler]` proc macros |
| `camel-wasm-sdk` | WASM plugin SDK (`BeanPlugin`, `export_bean!`) |
| `camel-config` | Camel.toml parsing including `[beans]` section |
