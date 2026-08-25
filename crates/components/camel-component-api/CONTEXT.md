# Component SPI

Contract crate for the [Components](../CONTEXT.md) bounded context. It defines the traits that each
component implements. It also defines the startup and shutdown contracts that the Runtime uses.
Concrete component crates own all I/O.

> **Scope boundary.** The parent [`crates/components/CONTEXT.md`](../CONTEXT.md) defines Component,
> Endpoint, Consumer, Producer, PollingConsumer, ExchangeEnvelope, ConcurrencyModel,
> ComponentContext, and ConsumerContext. [`crates/camel-api/CONTEXT.md`](../../camel-api/CONTEXT.md)
> defines shared data and runtime contract terms such as Exchange and RuntimeBus. This file covers
> only the component SPI view of those terms.

## Startup handshake

`Consumer::startup_mode()` selects one of two readiness contracts:

- `Immediate` is the default. `start()` is the consumer lifetime loop. The Runtime does not wait
  for a separate readiness signal. A promptly returning `Err` from an Immediate consumer's
  `start()` transitions the Route to `Failed` asynchronously. A detached failure watcher issues
  one `FailRoute`, with at most one defensive retry under the same `command_id`. The lifecycle
  operation returns without waiting, so Immediate startup timing is unchanged. An error arriving
  after the grace keeps the existing logged/crash-notified behavior. For loop-style Immediate
  consumers (start runs until cancellation), the watcher exits when the grace budget elapses.
- `Explicit` is for consumers that bind a resource inside `start()`. They call
  `ConsumerContext::mark_ready()` only after the bind succeeds. This lets the Runtime report bind
  failures before it marks the Route as started.

`StartupSignal` and `StartupReceiver` form the watch-channel handshake. `mark_ready()` and
`mark_failed()` are idempotent. The Runtime also detects an `Explicit` consumer that returns without
sending a signal. This fallback prevents the route controller from waiting forever.

## Shutdown contract

The Runtime owns each Consumer and applies this sequence:

1. It calls `start()` once in a spawned task.
2. A route stop cancels `ConsumerContext::cancel_token()`.
3. The task calls `stop()` after `start()` succeeds, on every exit path.
4. `background_task_handle()` reports task failure for supervision. It is not a shutdown API.

`stop()` must cancel all component-owned tasks and release registrations and resources. Inner tasks
should use the `ConsumerContext` cancellation token, or a child token. This lets them stop before
`stop()` waits for them. ADR-0007 defines crash propagation and Route supervision.

## Network retry helpers

`NetworkRetryPolicy`, `retry_async`, `retry_async_cancelable`, and
`is_retryable_camel_error` are the common retry primitives for networked components. ADR-0013
defines the retry policy. Retry warnings are ADR-0012 category (e), for operator visibility.

The helpers log an error's `Display` value before a retry. Callers must sanitize errors that can
contain credentials, connection strings, or other secrets.

## Polling consumers

`Endpoint::polling_consumer()` returns `None` by default. An Endpoint opts in when it supports
pull-based reads. A PollingConsumer does not start a Route. The EIP-7 `pollEnrich` operation and the
WASM `camel_poll` host function use this contract. ADR-0015 records the decision.

## `#[non_exhaustive]` posture

ADR-0049 places this contract crate in its mandatory scope.

| Enum | Bound posture | Reason |
|---|---|---|
| `ConsumerStartupMode` | Yes | ADR-0049 names it in the application set. External components return it from `startup_mode()`. |
| `ConcurrencyModel` | Yes | ADR-0049 names it in the application set. External components return it from `concurrency_model()`. |

Both enums carry `#[non_exhaustive]` (landed in rc-3pw3). New contract enums in this crate use
`#[non_exhaustive]` from birth. ADR-0049 Rule 3 governs public structs and external struct
literals. Compliance is enforced by `cargo xtask lint-non-exhaustive`.

## `test-support` feature

The optional `test-support` feature exposes the `test_support` module,
`NoopRuntimeObservability`, and `PanicRuntimeObservability`. The panic stub is only for downstream
tests. Production builds must not enable this feature. The feature also enables optional `rcgen`
support.

## Related decisions

- ADR-0007: Consumer failure and Route supervision.
- ADR-0012: retry warning level.
- ADR-0013: network retry policy.
- ADR-0015: PollingConsumer and `pollEnrich`.
- ADR-0049: mandatory `#[non_exhaustive]` policy for contract enums.
