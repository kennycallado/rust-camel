# Design: diagnose-premature-explicit-consumer-exit

## Approach

The Explicit branch of `spawn_consumer_task` obtains `background_task_handle()` after `start()` succeeds. When it is absent, retain the idempotent `mark_ready()` fallback, record whether it resolved readiness, and emit one `tracing::warn!` only when the consumer context is not cancelled. Include `route_id`, a `readiness_fallback` boolean, and a fixed message explaining immediate `stop()` and the two supported lifetime patterns. Do not log endpoint URIs or other untrusted values.

Keep background monitoring, finally-stop, `OuterOutcomeCell`, and the termination watcher unchanged. Startup signalling controls readiness, not task lifetime. An Explicit consumer may either keep `start()` alive until cancellation or return a monitored background handle. `None` remains valid when work completes within `start()`, including natural completion; this diagnostic does not turn that completion into a route failure.

Place regression tests in a separate `consumer_exit_tests.rs` test module beside consumer_management.rs. Use a current-thread Tokio test runtime with a locally installed tracing layer that records actual dispatched warning events and fields. Scope capture to this module's tracing target and the new message. Install a bare global registry using the established test pattern so callsite caching cannot hide events. Join spawned tasks with timeouts; use cancellation tokens or immediately completing background handles, not sleeps or external services. Cover already-ready and fallback cases for both initial start and resume. Verify stop calls and the startup receiver. Suppression cases cover cancellation, Immediate startup, and a returned background handle. Existing outer-task watcher tests verify failure publication remains unchanged.

## Affected crates

- camel-core: runtime diagnostic and regression tests.
- camel-component-api: Consumer rustdoc and CONTEXT.md clarify the existing lifecycle.

## Architecture boundaries

This is a Runtime-to-Component-SPI lifecycle diagnostic. ADR-0007 governs route-supervised consumer failure; no new failure publication is added. ADR-0012 governs logs; the diagnostic warns about a recoverable author contract mistake while existing cleanup proceeds. The data plane, DSL, Services, Languages, and Functions do not change. ADR-0091 applies to new tests and code. Existing test and module layout is retained apart from adding a focused test file.

## Alternatives considered

- Reject startup or publish FailRoute: would change legitimate finite-completion semantics and exceed the diagnostic scope recorded in rc-4uuy8.
- Wait indefinitely for cancellation without a handle: would conceal detached consumer work and alter cleanup timing.
- Warn only when readiness is absent: reproduces the existing blind spot after `mark_ready()`.
- Require all Explicit consumers to return a handle: excludes loop-style consumers such as direct, whose `start()` owns their lifetime.
