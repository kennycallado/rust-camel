# camel-test

> Testing utilities for rust-camel

Testing utilities for [rust-camel](https://github.com/kennycallado/rust-camel).

## Overview

`camel-test` provides two main utilities:

1. **`CamelTestContext`** — a test harness that removes boilerplate: registers components, provides simple start/stop methods, and exposes a shared `MockComponent` for assertions.
2. **`TimeController`** — deterministic time control for timer-based tests using tokio's built-in mock clock (no real `sleep` calls).

## Basic usage

```rust
use camel_test::CamelTestContext;
use camel_builder::RouteBuilder;
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

    // Wait for the timer's deliveries — never sleep for system state
    // (see CONTEXT.md; poll or await a condition instead).
    h.mock().get_endpoint("result").unwrap()
        .await_exchanges(3, Duration::from_secs(5)).await;
    h.mock().get_endpoint("result").unwrap().assert_exchange_count(3).await;
    h.stop().await; // deterministic teardown (drop is best-effort fallback)
}
```

## With time control

```rust
use camel_test::CamelTestContext;
use camel_builder::RouteBuilder;
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
| `.with_seda()` | Registers SedaComponent |
| `.with_component(c)` | Registers any custom component |
| `.with_time_control()` | Activates tokio mock clock; `build()` returns `(harness, TimeController)` |

## xml-bridge integration tests

`tests/mtls_bridge_test.rs` and other bridge-backed suites (`xml_bridge_xj_test`,
`xml_bridge_xslt_test`) need the native xml-bridge binary. Provision it from the
workspace root:

```sh
cargo xtask build-xml-bridge
export CAMEL_XML_BRIDGE_BINARY_PATH="$PWD/bridges/xml/build/native/xml-bridge"
cargo test -p camel-test --features integration-tests --test mtls_bridge_test
```

Semantics of `CAMEL_XML_BRIDGE_BINARY_PATH`:

Only `mtls_bridge_test`'s two tests (`start_and_connect_mtls_handshake`,
`wrong_ca_handshake_rejected`) skip-with-diagnostic when the variable is absent.
`xml_bridge_xj_test` and `xml_bridge_xslt_test` call `require_xml_bridge_binary`
and fail loudly when it is unset — unsetting the variable is not a general skip
switch for all bridge-backed suites.

| State | Behavior |
|---|---|
| Absent | `mtls_bridge_test` prints a `SKIP` line to stderr naming this env var and the provisioning command, then returns early. `xml_bridge_xj_test`/`xml_bridge_xslt_test` panic with the provisioning hint. |
| Set to an existing file | The test runs its full assertions against that binary. |
| Set but not an existing file (including empty) | Hard failure with the provisioning hint; a misconfigured value never degrades into a skip. |

## Installation

Add to your `Cargo.toml`:

```toml
[dev-dependencies]
camel-test = "*"
```
