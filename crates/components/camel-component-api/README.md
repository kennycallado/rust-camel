# camel-component-api

> Component API traits and extension points for rust-camel components.

## Overview

`camel-component-api` defines the core `Component`, `Endpoint`, `Consumer`, and `Producer` contracts used by component crates.

It also re-exports common types from `camel-api` and URI helpers from `camel-endpoint`, so most component crates can depend on a single API crate.

## Features

- `Component` trait for scheme-based endpoint factories
- `Endpoint`, `Consumer`, and producer context contracts
- New extension traits: `ComponentContext`, `ComponentRegistrar`, `ComponentBundle`
- `NoOpComponentContext` helper for tests and examples
- Re-exports from `camel-api` (e.g. `CamelError`, `Exchange`, `BoxProcessor`)
- Re-exports from `camel-endpoint` (`UriConfig`, `UriComponents`, `parse_uri`)
- Shared network retry primitives: `NetworkRetryPolicy`, `retry_async`, `retry_async_cancelable`, `is_retryable_camel_error`

## Installation

```toml
[dependencies]
camel-component-api = "*"
```

## Usage

### Implementing a Custom Component

```rust
use camel_component_api::{CamelError, Component, ComponentContext, Endpoint};

pub struct MyComponent;

impl Component for MyComponent {
    fn scheme(&self) -> &str {
        "my"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(MyEndpoint { uri: uri.to_string() }))
    }
}

struct MyEndpoint {
    uri: String,
}

impl Endpoint for MyEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
        Err(CamelError::NotImplemented("consumer not implemented".into()))
    }

    fn create_producer(
        &self,
        _ctx: &camel_component_api::ProducerContext,
    ) -> Result<camel_component_api::BoxProcessor, CamelError> {
        Err(CamelError::NotImplemented("producer not implemented".into()))
    }
}
```

## Core Types

| Type | Description |
|---|---|
| `Component` | Factory for scheme-specific endpoints |
| `Endpoint` | Consumer/producer endpoint contract |
| `Consumer` | Pulls/receives exchanges from a source |
| `ProducerContext` | Runtime context for producer creation |
| `ConsumerContext` | Runtime context passed to consumers |
| `ComponentContext` | Read-only access to components, languages, and metrics |
| `ComponentRegistrar` | Dynamic component registration interface (`Arc<dyn Component>`) |
| `ComponentBundle` | Multi-scheme registration + TOML config contract |
| `NoOpComponentContext` | Lightweight test context with no registry lookups |

## Extension Traits

```rust
use std::sync::Arc;
use camel_component_api::{CamelError, Component, ComponentContext, ComponentBundle, ComponentRegistrar, Endpoint, NoOpComponentContext};

// ComponentContext: provided to create_endpoint
impl Component for MyComponent {
    fn scheme(&self) -> &str { "my" }

    fn create_endpoint(&self, uri: &str, ctx: &dyn ComponentContext) -> Result<Box<dyn Endpoint>, CamelError> {
        let _maybe_other = ctx.resolve_component("other");
        Ok(Box::new(MyEndpoint { uri: uri.to_string() }))
    }
}

// ComponentBundle: register multiple schemes from one config
pub struct MyBundle;

impl ComponentBundle for MyBundle {
    fn config_key() -> &'static str { "my" }

    fn from_toml(raw: toml::Value) -> Result<Self, CamelError> {
        let _ = raw;
        Ok(Self)
    }

    fn register_all(self, registrar: &mut dyn ComponentRegistrar) {
        registrar.register_component_dyn(Arc::new(MyComponent));
    }
}

// NoOpComponentContext: convenient for tests
let ctx = NoOpComponentContext;
let _endpoint = MyComponent.create_endpoint("my:foo", &ctx)?;
```

## Network Retry Helpers

Shared retry primitives for transient network reconnects. See
[ADR-0013](../../docs/adr/0013-network-retry-policy-and-migration.md) for the
full migration decision tree.

### `NetworkRetryPolicy`

Shared configuration for capped exponential backoff with jitter:

```rust
use camel_component_api::NetworkRetryPolicy;
use std::time::Duration;

let policy = NetworkRetryPolicy {
    max_attempts: 5,             // 1 initial + 4 retries; 0 = unlimited
    initial_delay: Duration::from_millis(100),
    max_delay: Duration::from_secs(30),
    multiplier: 2.0,
    jitter: 0.1,
    enabled: true,
    ..NetworkRetryPolicy::default()
};
```

Components expose this as a TOML `[reconnect_policy]` section:

```toml
[components.ws.reconnect_policy]
max_attempts = 5
initial_delay_ms = 100
max_delay_ms = 30000
multiplier = 2.0
jitter = 0.1
enabled = true
```

### `retry_async`

Bounded retry of an async operation with a caller-provided classifier. The
optional label is emitted as a structured `component` tracing field and in
the retry log message for operator observability:

```rust
use camel_component_api::{retry_async, NetworkRetryPolicy};

let result: Result<MyType, MyError> = retry_async(
    &policy,
    Some("my-component"),           // label appears as "my-component: transient error — retrying"
    || async { /* op */ Err(MyError::Transient) },
    |err| matches!(err, MyError::Transient),
).await;
```

### `retry_async_cancelable`

Same as `retry_async`, plus a [`CancellationToken`](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html)
honored during the inter-retry sleep. Use for consumers/producers tied to
route shutdown so the backoff does not block cancellation:

```rust
use camel_component_api::{retry_async_cancelable, NetworkRetryPolicy};
use tokio_util::sync::CancellationToken;

let result = retry_async_cancelable(
    &policy,
    Some("my-consumer"),
    || async { /* op */ },
    |err| true,
    &cancel_token,
).await;
```

Cancel during `op` itself is the caller's responsibility (e.g. `tokio::select!`
in the op closure). Cancel during sleep returns the last operation error; no
synthetic `Cancelled` variant is added to the caller's error type.

### `is_retryable_camel_error`

Canonical classifier for `CamelError`. Use when your operation already
returns `CamelError` and you want the standard retry-on-transient behavior
(`CamelError::Io(_)` and `[TRANSIENT]`-marked processor errors):

```rust
use camel_component_api::{retry_async, is_retryable_camel_error};

let result = retry_async(
    &policy,
    Some("my-component"),
    || async { /* returns Result<_, CamelError> */ Ok(()) },
    is_retryable_camel_error,
).await;
```

### When to use what

| Primitive | When |
|---|---|
| `retry_async` | Bounded retry of pure async ops, no shutdown-cancel need |
| `retry_async_cancelable` | Same + route/consumer shutdown interrupts backoff sleep |
| Manual loop | `&mut` state across `await`, polling/event-stream loops, async pre-retry side effects (bridge/pool restart), shared counters, non-idempotent writes |

Examples of each branch are listed in ADR-0013.

## Documentation

- API: <https://docs.rs/camel-component-api>
- Repository: <https://github.com/kennycallado/rust-camel>

## License

Apache-2.0
