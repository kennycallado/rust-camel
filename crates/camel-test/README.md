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

## Artemis broker-side disposition query

The Artemis JMS disposition cases in `tests/support/jms_disposition_cases.rs`
read broker state through `ArtemisBroker::queue_stat` and `parse_queue_stat` in
`tests/support/artemis.rs`. This section records what those counts prove and
what they do not.

**Mechanism.** `queue_stat` runs `docker exec` into the dedicated Artemis
container and invokes the container's own
`/var/lib/artemis-instance/bin/artemis queue stat --queueName <q>`. The exec has
a 60 s wall-clock deadline. `parse_queue_stat` reads the pinned 2.36.0 table and
returns `MESSAGE COUNT` and `MESSAGES ADDED` per queue; the case queries the
source queue, `DLQ`, and `ExpiryQueue`. A case waits up to 30 s for the source queue to drain before it
records the final counts.

**Stability argument.** Stability is scoped to the verified
`apache/activemq-artemis:2.36.0-alpine` pin: the CLI ships inside the container
and is version-matched to the broker; the image deploys no console wars, so no
Jolokia endpoint exists and the Classic-style Jolokia path is unavailable for
Artemis; no extra port is exposed; and invoking the `docker` CLI from test code
has precedent in `jms_crash_probe_cases.rs` (`docker pause`/`unpause`), which
`docker exec` follows. The test-only `artemis`/`artemis` credentials are passed
on the command line, so they are visible in the container's `ps` output.

**Non-proofs.** The counts are point-in-time management counts under the
isolated dedicated-container scenario only. They do not prove journal layout or
byte persistence, they are not a universal proof against message loss, and they
make no crash-proofness claim. `MESSAGES ADDED` accumulates over the broker's
lifetime; it is exact here only because each case owns a fresh container.

**Terminal-kind distinction.** The bridge's terminal discard is an
acknowledgement: the source queue drains with zero `DLQ` and `ExpiryQueue`
additions. A broker-native dead-letter moves the message to `DLQ`, which shows a
non-zero `MESSAGES ADDED`. With `dead-letter-address=DLQ` under `match="#"` there
is no silent-drop route, so the counts always distinguish the two.

**Positional parsing.** `parse_queue_stat` reads columns by position and depends
on the pinned column order. Its wrapped-header guard requires cell 3 to be
`MESSAGE` on the first header line and `COUNT` on the second, and cell 4 to be
`MESSAGES` on the first and `ADDED` on the second. A missing or damaged pair is
an error, and `queue_stat` turns a missing row for the requested queue into an
error rather than zero counts, so a format change fails closed.

## Installation

Add to your `Cargo.toml`:

```toml
[dev-dependencies]
camel-test = "*"
```
