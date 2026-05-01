# camel-test

> Testing utilities for rust-camel

Testing utilities for [rust-camel](https://github.com/rust-camel/rust-camel).

## Overview

`camel-test` provides two main utilities:

1. **`CamelTestContext`** — a test harness that removes boilerplate: registers components, provides simple start/stop methods, and exposes a shared `MockComponent` for assertions.
2. **`TimeController`** — deterministic time control for timer-based tests using tokio's built-in mock clock (no real `sleep` calls).

## Basic usage

```rust
use camel_test::CamelTestContext;
use camel_builder::{RouteBuilder, StepAccumulator};
use std::time::Duration;

#[tokio::test]
async fn test_route() {
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    tokio::time::sleep(Duration::from_millis(300)).await;

    h.mock().get_endpoint("result").unwrap().assert_exchange_count(3).await;
    h.stop().await; // deterministic teardown (drop is best-effort fallback)
}
```

## With time control

```rust
use camel_test::CamelTestContext;
use camel_builder::{RouteBuilder, StepAccumulator};
use std::time::Duration;

#[tokio::test]
async fn test_route_fast() {
    let (h, time) = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_time_control()
        .build()
        .await;

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .to("mock:result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    time.advance(Duration::from_millis(200)).await; // no real sleep
    tokio::task::yield_now().await;

    h.mock().get_endpoint("result").unwrap().assert_exchange_count(3).await;
}
```

## Available builder methods

| Method | Effect |
|---|---|
| `.with_mock()` | Signals mock usage (always registered) |
| `.with_timer()` | Registers `TimerComponent` |
| `.with_log()` | Registers `LogComponent` |
| `.with_direct()` | Registers `DirectComponent` |
| `.with_component(c)` | Registers any custom component |
| `.with_time_control()` | Activates tokio mock clock; `build()` returns `(harness, TimeController)` |

## Installation

Add to your `Cargo.toml`:

```toml
[dev-dependencies]
camel-test = "*"
```
