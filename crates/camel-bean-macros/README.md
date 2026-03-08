# camel-bean-macros

Proc-macros for the [`camel-bean`](../camel-bean) crate that enable ergonomic bean definition and automatic `BeanProcessor` trait implementation.

## Overview

This crate provides three proc-macros:

1. **`#[bean_impl]`** - Main macro for generating `BeanProcessor` impl from impl blocks
2. **`#[handler]`** - Marker attribute for bean handler methods
3. **`#[derive(Bean)]`** - Placeholder derive macro (not recommended)

## Macros

### `#[bean_impl]` - Generate BeanProcessor Implementation

The primary macro that transforms an impl block into a `BeanProcessor` implementation with automatic parameter binding.

```rust
use camel_bean::{bean_impl, handler};

pub struct OrderService;

#[bean_impl]
impl OrderService {
    #[handler]
    pub async fn process(&self, body: Order) -> Result<ProcessedOrder, String> {
        // Business logic
        Ok(ProcessedOrder { /* ... */ })
    }

    #[handler]
    pub async fn validate(&self, body: Order, headers: Headers) -> Result<bool, String> {
        // Validation logic
        Ok(true)
    }
}
```

**What it generates:**

```rust,ignore
impl OrderService {
    // ... your methods unchanged ...
}

#[async_trait]
impl BeanProcessor for OrderService {
    async fn call(&self, method: &str, exchange: &mut Exchange) -> Result<(), CamelError> {
        match method {
            "process" => {
                // Extract body parameter
                let body: Order = exchange.input.body.try_into()?;
                // Call method
                let result = self.process(body).await;
                // Handle result (set response body or error)
                // ...
            }
            "validate" => {
                // Extract body and headers
                let body: Order = exchange.input.body.try_into()?;
                let headers = exchange.input.headers.clone();
                // Call and handle result
                // ...
            }
            _ => return Err(CamelError::from(format!("Unknown method: {}", method)))
        }
        Ok(())
    }

    fn methods(&self) -> Vec<&'static str> {
        vec!["process", "validate"]
    }
}
```

### `#[handler]` - Mark Handler Methods

Marker attribute that identifies which methods should be exposed as bean handlers.

```rust
#[bean_impl]
impl MyService {
    #[handler]  // This method is exposed
    pub async fn process(&self, body: Data) -> Result<Response, Error> {
        // ...
    }

    // This method is NOT exposed (no #[handler])
    fn helper(&self) -> String {
        "internal".to_string()
    }
}
```

**Rules:**
- Must be `async`
- First parameter must be `&self`
- Supported parameter types:
  - `body: T` - Extracted from exchange body (requires `TryFrom<Body>`)
  - `headers: Headers` - Cloned from exchange headers
  - `exchange: &mut Exchange` - Full exchange access
- Return type must be `Result<T, E>` where `E: Display`

### `#[derive(Bean)]` - Placeholder (Not Recommended)

Derive macro that generates a placeholder `BeanProcessor` implementation.

**⚠️ Warning:** This macro is incomplete and requires manual impl block with `#[handler]` methods. Use `#[bean_impl]` instead.

```rust,ignore
// NOT RECOMMENDED
#[derive(Bean)]
struct MyService;

// You still need to write impl block manually
impl MyService {
    #[handler]
    pub async fn process(&self, body: Data) -> Result<Response, Error> {
        // ...
    }
}
```

## Parameter Binding

The `#[bean_impl]` macro automatically extracts parameters by name:

```rust
#[bean_impl]
impl UserService {
    // Extract only body
    #[handler]
    pub async fn create(&self, body: CreateUser) -> Result<User, Error> {
        // ...
    }

    // Extract body + headers
    #[handler]
    pub async fn update(&self, body: UpdateUser, headers: Headers) -> Result<User, Error> {
        let user_id = headers.get("user-id").unwrap();
        // ...
    }

    // Full exchange access
    #[handler]
    pub async fn complex(&self, exchange: &mut Exchange) -> Result<(), Error> {
        // Read body, headers, properties, set response, etc.
        // ...
    }
}
```

**Parameter name inference:**
- `body` → Extracted from `exchange.input.body` via `TryFrom<Body>`
- `headers` → Cloned from `exchange.input.headers`
- `exchange` → Mutable reference to the full exchange

## Return Type Handling

Return types are automatically handled:

```rust
#[bean_impl]
impl MyService {
    // Success: body set to Result<T>
    // Error: exchange.set_error()
    #[handler]
    pub async fn process(&self, body: Input) -> Result<Output, String> {
        Ok(Output { /* ... */ })
    }
}
```

**Result handling:**
- `Ok(T)` → Body set to JSON-serialized `T` (if `T` is not `()`)
- `Err(E)` → `exchange.set_error()` called with error message

## Error Handling

The macro generates proper error handling:

```rust
#[bean_impl]
impl MyService {
    #[handler]
    pub async fn process(&self, body: Data) -> Result<Response, String> {
        if body.invalid() {
            return Err("Invalid data".to_string());
        }
        Ok(Response { /* ... */ })
    }
}
```

**Generated error handling:**
- Parameter extraction failures → `CamelError::ProcessorError`
- Method not found → `BeanError::MethodNotFound`
- Handler errors → Set on exchange via `set_error()`

## Integration with camel-bean

This crate is a dependency of `camel-bean` and is re-exported for convenience:

```rust
// Usually you don't depend on camel-bean-macros directly
use camel_bean::{bean_impl, handler, BeanRegistry};

// camel-bean re-exports the macros
```

## Examples

See the [`bean-demo` example](../../examples/bean-demo/) for a complete working example.

## Implementation Details

### Code Generation

The `#[bean_impl]` macro:

1. **Parses** the impl block to find `#[handler]` methods
2. **Validates** method signatures (async, &self, valid parameters)
3. **Generates** parameter extraction code for each handler
4. **Generates** method dispatch via `match` on method name
5. **Generates** result handling (set body or error)
6. **Preserves** original impl block unchanged

### Supported Types

**Parameter types:**
- `body: T` where `T: TryFrom<Body>`
- `headers: Headers` (cloned)
- `exchange: &mut Exchange`

**Return types:**
- `Result<T, E>` where `E: Display`
- `T` is serialized to JSON on success
- `()` is allowed (no body set on success)

## See Also

- [camel-bean](../camel-bean) - Main bean registry and usage examples
- [examples/bean-demo](../../examples/bean-demo/) - Complete working example
- [Apache Camel Bean Binding](https://camel.apache.org/manual/bean-binding.html) - Inspiration

## License

Apache-2.0
