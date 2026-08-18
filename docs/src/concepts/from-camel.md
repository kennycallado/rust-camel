# Coming from Apache Camel

rust-camel shares Apache Camel's pattern vocabulary because the vocabulary is
proven. The implementation is independent. The runtime, the type system, and
the execution model are all different. A Camel user recognizes Filter,
Content-Based Router, and Splitter. rust-camel is not a drop-in replacement
and never claims Camel compatibility. See
[ADR-0046](../adr/0046-apache-camel-inspiration-not-conformance.md) for the
design stance.

## Core vocabulary

| Apache Camel | rust-camel | Notes |
|---|---|---|
| CamelContext | CamelContext | Same name. Built with `CamelContext::builder()`. No Spring or CDI. |
| Route | RouteDefinition | Built with `RouteBuilder` (Rust) or parsed from YAML. |
| RouteBuilder (Java DSL) | RouteBuilder (Rust) | Same fluent style: `.from().to().build()`. |
| XML DSL | YAML DSL | No XML DSL. Declarative routes use YAML. |
| Processor | `Service<Exchange>` | Every processor is a Tower `Service`. No Java interface to implement. |
| Exchange | Exchange | Same concept. Carries input Message, optional output Message, headers, properties. |
| Message | Message | Body + headers container inside Exchange. |
| Body | Body | Enum: `Text`, `Json`, `Bytes`, `Stream`, `Empty`. Not `Object`. |
| Endpoint | Endpoint | Resolved from a URI scheme (e.g. `timer:tick`, `log:info`). |
| Component | Component | Registered on CamelContext. Same scheme names as Camel (timer, log, file, http, kafka). |
| Channel | (no equivalent) | No first-class Channel type. The `from:`/`to:` URI pair fills this role. |
| BeanRegistry | BeanRegistry | Named instances, resolved at route start time. |

## EIP names

Most EIP step names match between Apache Camel and rust-camel. The YAML DSL uses snake\_case (`wire_tap`, `load_balance`) where Apache Camel's XML uses camelCase (`wireTap`, `loadBalance`).

| Apache Camel (Java/XML) | rust-camel YAML | rust-camel Rust |
|---|---|---|
| `choice` | `choice` | `.choice()` |
| `when` | `when` (under `choice`) | `.when(predicate)` |
| `otherwise` | `otherwise` (under `choice`) | `.otherwise()` |
| `filter` | `filter` | `.filter(predicate)` |
| `split` | `split` | `.split(config)` |
| `aggregate` | `aggregate` | `.aggregate(config)` |
| `multicast` | `multicast` | `.multicast()` |
| `wireTap` | `wire_tap` | `.wire_tap(uri)` |
| `loadBalance` | `load_balance` | `.load_balance()` |
| `recipientList` | `recipient_list` | `.recipient_list(expr)` |
| `routingSlip` | `routing_slip` | `.routing_slip(expr)` |
| `throttle` | `throttle` | `.throttle(n, duration)` |
| `delay` | `delay` | `.delay(duration)` |
| `loop` | `loop` | `.loop_count(n)` |
| `marshal` | `marshal` | `.marshal(format)` |
| `unmarshal` | `unmarshal` | `.unmarshal(format)` |
| `enrich` | `enrich` | `.enrich(uri)` |
| `pollEnrich` | `poll_enrich` | `.poll_enrich(uri, timeout)` |
| `validate` | `validate` | `.validate(predicate)` |
| `doTry` | `do_try` | `.do_try()` |
| `doCatch` | `catch` (under `do_try`) | `.do_catch_exception(&[...])` |
| `doFinally` | `finally` (under `do_try`) | `.do_finally()` |
| `circuitBreaker` | `circuit_breaker` (route-level) | `.circuit_breaker(config)` |
| `transform` | `transform` | `.transform(body)` (alias for `set_body`) |
| `setBody` | `set_body` | `.set_body(value)` |
| `setHeader` | `set_header` | `.set_header(key, value)` |
| `script` | `script` | `.script(language, source)` |

## Execution model

Apache Camel runs processors in a pipeline backed by a Java service architecture. rust-camel runs processors as Tower `Service<Exchange>` steps in a Tower middleware chain. This means:

- Every step is `Clone + Send + Sync + 'static`.
- Backpressure is explicit through Tower's `poll_ready`.
- The pipeline outcome is `Completed`, `Stopped`, or `Failed` (see [ADR-0024](../adr/0024-pipeline-outcome-replaces-camel-error-stopped.md)). `Stopped` is not an error.

## Error handling

| Apache Camel | rust-camel | Notes |
|---|---|---|
| `onException(Exception.class)` | `on_exceptions: [{ kind: "..." }]` | Match by error `kind`. |
| `errorHandler(deadLetterChannel)` | `error_handler: { dead_letter_channel: ... }` | Route-level config. |
| `handled(true)` | `disposition: handled` | Absorbs the error. Route terminates normally. |
| `continued(true)` | `disposition: continued` | Clears the error. Advances to the next step. |
| `maximumRedeliveries` | `retry(max).with_backoff(...)` | Retry on the builder. |

See [ADR-0019](../adr/0019-error-disposition-pipeline-recovery.md) for the exception disposition contract.

## What rust-camel does not have

| Apache Camel feature | Status | Alternative |
|---|---|---|
| Spring XML DSL | Not planned | YAML DSL |
| CDI / Spring DI | Not planned | Rust trait system, manual registration |
| JMX | Not planned | OpenTelemetry, metrics endpoints |
| Bean annotation scanning | Not planned | Explicit bean registration |
| Normalizer EIP | Not implemented | Compose from Convert Body + Content-Based Router |
| Content Filter EIP | Not implemented | Use Script or process closure to strip fields |
| Detour EIP | Not implemented | Compose from filter + to |
| Transaction Client | Not implemented | Future work |

## Body types

Apache Camel uses `Object` as the body type and relies on type converters. rust-camel uses a `Body` enum with six variants:

| Variant | Holds |
|---|---|
| `Body::Text` | UTF-8 string |
| `Body::Json` | Parsed JSON value |
| `Body::Bytes` | Raw byte buffer |
| `Body::Stream` | Async stream (materialized by [Stream Cache](../steps/stream-cache.md)) |
| `Body::Xml` | XML document |
| `Body::Empty` | No payload |

The [Exchange and Message](exchange-message.md) page covers typed body access. The [Convert Body](../eip/convert-body.md) step changes between variants. The [Marshal and Unmarshal](../eip/marshal-unmarshal.md) step converts between wire formats (JSON, CSV, XML, ZIP) and these variants.

## Getting started

If you are new to rust-camel, start with these pages:

| Page | Covers |
|---|---|
| [Getting started](../getting-started/index.md) | Install and run your first route |
| [Core concepts](index.md) | Exchange, routes, components |
| [EIP patterns](../eip/index.md) | The pattern catalogue |
| [YAML DSL route structure](../yaml-dsl/route-structure.md) | Declarative route syntax |
