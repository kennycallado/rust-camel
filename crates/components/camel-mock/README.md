# camel-component-mock

> Mock component for rust-camel testing

## Overview

The Mock component is a testing utility that records every exchange it receives. It's essential for writing unit tests and integration tests for your routes.

This is a **producer-only** component - it records exchanges sent to it and provides assertions for testing.

## Features

- Record all received exchanges
- Assert exchange count
- Inspect recorded exchanges
- Shared state across endpoint instances
- Perfect for testing route outputs

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-mock = "*"
```

## URI Format

```
mock:name
```

## Usage

### Basic Testing

```rust
use camel_builder::RouteBuilder;
use camel_component_mock::MockComponent;
use camel_core::CamelContext;

#[tokio::test]
async fn test_route() {
    let mock = MockComponent::new();
    let mock_ref = mock.clone(); // Keep reference for assertions

    let mut ctx = CamelContext::new();
    ctx.register_component("mock", Box::new(mock));

    let route = RouteBuilder::from("direct:input")
        .to("mock:result")
        .build().unwrap();

    ctx.add_route(route).await.unwrap();
    ctx.start().await.unwrap();

    // ... send exchanges through route ...

    // Assert on received exchanges
    let endpoint = mock_ref.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges[0].input.body.as_text(), Some("expected"));
}
```

### Integration Test Example

```rust
use camel_builder::RouteBuilder;
use camel_component_mock::MockComponent;
use camel_component_direct::DirectComponent;
use camel_core::CamelContext;

#[tokio::test]
async fn test_transformation_route() {
    // Setup
    let mock = MockComponent::new();
    let mock_ref = mock.clone();

    let mut ctx = CamelContext::new();
    ctx.register_component("direct", Box::new(DirectComponent::new()));
    ctx.register_component("mock", Box::new(mock));

    // Route that transforms messages
    let route = RouteBuilder::from("direct:input")
        .map_body(|body: Body| {
            body.as_text()
                .map(|t| Body::Text(t.to_uppercase()))
                .unwrap_or(body)
        })
        .to("mock:output")
        .build().unwrap();

    ctx.add_route(route).await.unwrap();
    ctx.start().await.unwrap();

    // Test
    let producer = ctx.create_producer("direct:input").await.unwrap();
    let exchange = Exchange::new(Message::new("hello"));
    producer.oneshot(exchange).await.unwrap();

    // Verify
    let endpoint = mock_ref.get_endpoint("output").unwrap();
    endpoint.assert_exchange_count(1).await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges[0].input.body.as_text(), Some("HELLO"));
}
```

### Multiple Endpoints

```rust
let mock = MockComponent::new();

let route = RouteBuilder::from("direct:input")
    .multicast()
        .to("mock:a")
        .to("mock:b")
    .end_multicast()
    .build()?;

// Later, assert on both
let endpoint_a = mock.get_endpoint("a").unwrap();
let endpoint_b = mock.get_endpoint("b").unwrap();

endpoint_a.assert_exchange_count(1).await;
endpoint_b.assert_exchange_count(1).await;
```

## MockEndpointInner Methods

| Method | Description |
|--------|-------------|
| `get_received_exchanges()` | Get all recorded exchanges |
| `assert_exchange_count(n)` | Assert exact count (panics if mismatch) |
| `await_exchanges(n, timeout)` | Async wait until at least `n` exchanges arrive or timeout |
| `exchange(idx)` | Get an `ExchangeAssert` for the exchange at index `idx` |

## ExchangeAssert (fluent assertions)

```rust
endpoint
    .await_exchanges(1, Duration::from_secs(2))
    .await;

endpoint
    .exchange(0)
    .assert_body_text("HELLO")
    .assert_header("x-source", json!("timer"))
    .assert_no_error();
```

| Method | Description |
|--------|-------------|
| `assert_body_text(expected)` | Assert body is `Body::Text` with the given value |
| `assert_body_json(expected)` | Assert body is `Body::Json` matching the given `serde_json::Value` |
| `assert_body_bytes(expected)` | Assert body is `Body::Bytes` with the given bytes |
| `assert_header(key, value)` | Assert a header equals the given JSON value |
| `assert_header_exists(key)` | Assert a header is present |
| `assert_has_error()` | Assert the exchange carries an error |
| `assert_no_error()` | Assert the exchange has no error |

## Best Practices

1. **Clone the MockComponent** before registering to keep a reference for assertions
2. **Use descriptive names** for mock endpoints to make tests readable
3. **Assert early** - check exchange count before inspecting contents
4. **Clean shutdown** - stop the context before assertions to ensure all exchanges are processed

## Documentation

- [API Documentation](https://docs.rs/camel-component-mock)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
