# Mock

The Mock component is a producer-only testing utility. It records every Exchange a Route sends to it and exposes assertions you call from your test code.

```rust,ignore
use camel_builder::RouteBuilder;
use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;

let mock = MockComponent::new();
let mock_ref = mock.clone();

let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
ctx.register_component(TimerComponent::new());
ctx.register_component(mock);

let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=1")
    .route_id("mock-demo")
    .set_body(camel_api::Body::Text("hello"))
    .map_body(|body: camel_api::Body| {
        camel_api::Body::Text(body.as_text().unwrap_or("").to_uppercase())
    })
    .to("mock:result")
    .build()?;

ctx.add_route_definition(route).await?;
ctx.start().await?;

let endpoint = mock_ref.get_endpoint("result").unwrap();
endpoint.await_exchanges(1, std::time::Duration::from_secs(5)).await;
endpoint.assert_exchange_count(1).await;
endpoint.exchange(0)
    .assert_body_text("HELLO")
    .assert_no_error();
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: mock-route
    from: "direct:input"
    steps:
      - to: "mock:result"
```

The Mock endpoint is the same in both APIs. The Rust builder and the YAML DSL compile to the same `RouteDefinition`. A YAML-defined route produces Exchanges that the Rust test code can still assert on through the cloned `MockComponent` handle.

</details>

## URI

```text
mock:<name>
```

| Segment | Required | Description |
| --- | --- | --- |
| `name` | yes | Logical name for this endpoint. The same name creates the same recorded-exchange store, so two routes that send to `mock:result` share one buffer. |

The Mock has no query parameters. Every behavior is controlled through the Rust API on `MockComponent` and `MockEndpointInner`.

## Why producer-only

The Mock only supports producer mode. The contract crate declares `supports_producer: true` and leaves consumer support disabled. A Mock Consumer would have no source to pull from and no peer to broadcast to. The value is the recording side, not a stub peer.

You use Mock as a `to:` target inside a Route. Your test code holds a clone of the `MockComponent` and reads back what arrived. Clone the `MockComponent` before you register it, because registration moves the value into the `CamelContext`.

## Recording model

Each `MockEndpointInner` keeps a `VecDeque<Exchange>` behind a `tokio::Mutex` and a `tokio::sync::Notify` for wake-ups. The producer appends every Exchange it processes, optionally deep-cloning the body if `MockConfig::copy_on_exchange` is `true`. The default cap is 10 000 retained exchanges. Older entries drop when the cap is exceeded.

Multiple `MockEndpoint` instances with the same name share one `MockEndpointInner` through `Arc`. Two routes that both send to `mock:result` write to the same buffer. A multicast test can assert on every leg from one handle.

The recording is in-memory only. The component persists nothing to disk, supports no replay, and exposes no remote inspection. Stop the `CamelContext` and the recorded Exchanges disappear.

## Assertions

The Rust API offers three assertion styles. Pick by what your test needs to express.

`assert_exchange_count(n)` is the first check in most tests. It panics with a descriptive message if the count does not match. Call it before any deeper inspection so a missing exchange fails the test at the count line, not inside a body assertion.

`exchange(idx)` returns an `ExchangeAssert` for fluent checks. Chain `.assert_body_text("HELLO")`, `.assert_body_json(value)`, `.assert_body_bytes(&[1, 2, 3])`, `.assert_header("x-source", json!("timer"))`, `.assert_header_exists("trace-id")`, `.assert_has_error()`, or `.assert_no_error()`. Every method panics with a message that names the endpoint, the exchange index, the expected value, and the actual value. Test output stays self-explanatory without extra `assert!` calls.

`expect_body`, `expect_header`, and `expect_header_regex` register a batch of expectations up front. Call `assert_satisfied()` after the exchanges have arrived. Batch mode matches in strict order by default. Set `MockConfig::any_order = true` to match each expected body against any received exchange exactly once.

`await_exchanges(n, timeout)` blocks until `n` exchanges arrive or the timeout elapses. It uses `Notify`, not polling, so it returns the instant the producer appends. Call it before `exchange(idx)` to avoid an out-of-bounds panic. The method needs a multi-threaded Tokio runtime; `#[tokio::test(flavor = "multi_thread")]` is the right shape.

## MockConfig

| Field | Default | Effect |
| --- | --- | --- |
| `max_retained` | `10000` | Drop oldest exchanges past this cap. |
| `copy_on_exchange` | `false` | Deep-clone the body on insert. Set `true` when the caller mutates the Exchange after sending. |
| `fail_fast` | `false` | Stop processing after the first failing assertion and record the error. |
| `assert_period_ms` | `0` | Default timeout for `await_exchanges_with_timeout`. `0` means use the explicit fallback. |
| `any_order` | `false` | Match expected bodies against received bodies without position. |

`fail_fast_error()` returns the recorded error after a fail-fast stop. Use it in test cleanup to log the cause.

**Reference**: [Mock crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-mock/README.md). Vocabulary: [Components CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md).
