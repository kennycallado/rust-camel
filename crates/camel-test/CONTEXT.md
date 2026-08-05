# Test Harness

Testing utilities for rust-camel. This crate builds a `CamelContext` for integration tests,
registers common Components, controls Tokio test time, and stops Routes after each test.

> **Scope boundary.** [`crates/camel-api/CONTEXT.md`](../camel-api/CONTEXT.md) defines Exchange and
> other shared contract terms. Component terms belong to
> [`crates/components/CONTEXT.md`](../components/CONTEXT.md). This file defines only camel-test
> helpers and their test-lifecycle contracts.

## Language

**Test harness**:
`CamelTestContext`, which wraps a `CamelContext` with component registration, mock access, and
teardown helpers. It is the rust-camel counterpart to Apache Camel's `CamelTestSupport`.
_Avoid_: test context (too broad), CamelContext wrapper

**Typestate builder**:
`CamelTestContextBuilder<S>`. Callers obtain it only through `CamelTestContext::builder()`.
`NoTimeControl` and `WithTimeControl` select different `build()` return types.
_Avoid_: test fixture builder, context builder

**Time controller**:
`TimeController`, a zero-sized handle for Tokio mock time. Only
`.with_time_control().build()` returns it. It advances paused time or resumes real time.
_Avoid_: mock clock, virtual clock

**Mock component accessor**:
`CamelTestContext::mock()`, which returns the always-registered `MockComponent`. Tests use its
Endpoints to inspect Exchanges.
_Avoid_: mock exchange (Exchange is the shared data type, not the mock facility)

**Teardown guard**:
The private `TestGuard` that starts best-effort shutdown when a harness is dropped. Explicit
`stop()` or `shutdown()` remains the deterministic teardown path.
_Avoid_: shutdown hook, runtime guard

## `#[non_exhaustive]` posture

| Public enums | Posture | Reason |
|---|---|---|
| None | N/A | camel-test has no public enums. ADR-0049 does not include test utility crates in its contract-crate scope. |

The public structs are test helpers, not contract enums that external implementations match.

## Architecture notes

### Drop guard uses best-effort shutdown

`TestGuard::drop()` blocks for cleanup on a Tokio multi-thread Runtime. On a current-thread Runtime,
it spawns cleanup because blocking in `Drop` is not possible. Outside a Tokio Runtime, it performs
no asynchronous cleanup. Tests that require completion must call `stop()` or `shutdown()`.

### Builder state controls the result type

`CamelTestContextBuilder<NoTimeControl>::build()` returns `CamelTestContext`.
`CamelTestContextBuilder<WithTimeControl>::build()` pauses Tokio time and returns
`(CamelTestContext, TimeController)`. The type system prevents callers from requesting a time
controller without enabling time control.

### `ctx()` exposes the storage type

`CamelTestContext::ctx()` returns `&Arc<Mutex<CamelContext>>`. This escape hatch lets tests use
Runtime APIs that the harness does not wrap. Its concrete return type is part of the public API.
Changing the lock or ownership model is a breaking change.

### `with_mock()` is an intent marker

Every harness registers `MockComponent`. `with_mock()` changes no state. It lets a test state its
mock dependency at the builder call site.

## Related decisions

- ADR-0049: camel-test is outside the contract-enum scope.
- ADR-0007: Runtime crash propagation; the test teardown guard is not part of that path.
- ADR-0012: camel-test has no `error!` policy sites.
