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

## Documentation

- API: <https://docs.rs/camel-component-api>
- Repository: <https://github.com/kennycallado/rust-camel>

## License

Apache-2.0
