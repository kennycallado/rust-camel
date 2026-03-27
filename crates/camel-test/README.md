# camel-test

Testing utilities for [rust-camel](https://github.com/rust-camel/rust-camel).

## Overview

`camel-test` provides testing utilities for writing integration tests against the rust-camel framework. It re-exports commonly needed types and provides test-specific utilities.

## Features

- **MockComponent re-export** - Direct access to the mock component for test assertions
- **Test dependencies** - Includes commonly needed testing crates (tokio, tower)
- **Component test utilities** - Pre-configured components for integration testing

## Usage

```rust
use camel_test::MockComponent;
use camel_builder::RouteBuilder;
use camel_core::CamelContext;
use camel_component_timer::TimerComponent;

#[tokio::test]
async fn test_route() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());
    
    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .to("mock:result")
        .build()
        .unwrap();
    
    ctx.add_route_definition(route).await.unwrap();
    ctx.start().await.unwrap();
    
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ctx.stop().await.unwrap();
    
    // Assert exchanges were received
    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(3).await;
}
```

## Installation

Add to your `Cargo.toml`:

```toml
[dev-dependencies]
camel-test = "0.1"
```

## Documentation

For detailed mock component documentation and assertion methods, see the [`camel-component-mock`](../components/camel-mock/) crate.
