# direct-startup-handshake Specification

## Purpose
TBD - created by archiving change audit-fix-direct-startup-race. Update Purpose after archive.
## Requirements
### Requirement: DirectConsumer uses Explicit startup mode

The `DirectConsumer` SHALL declare `ConsumerStartupMode::Explicit` so that the
runtime awaits consumer readiness before completing `StartRoute` for the
consumer's route.

#### Scenario: Startup mode is Explicit

- **GIVEN** a `DirectConsumer` created from a `direct:name` endpoint
- **WHEN** `startup_mode()` is called
- **THEN** it SHALL return `ConsumerStartupMode::Explicit`

### Requirement: DirectConsumer signals readiness after registry insert

The `DirectConsumer::start()` SHALL call `ConsumerContext::mark_ready()`
immediately after the registration block's lock guard is dropped (registry
insert committed and visible) and before entering the event loop. The
`DirectConsumer` SHALL NOT return a `background_task_handle()`; its `start()`
runs the event loop inline, so readiness is signalled by the explicit
`mark_ready()` call, not the runtime's defensive fallback.

#### Scenario: mark_ready resolves StartupReceiver after registration

- **GIVEN** a `DirectConsumer` with an injected `StartupSignal` pair and a name
  not yet present in the shared `DirectRegistry`
- **WHEN** `start()` is spawned and runs the registration block
- **THEN** the paired `StartupReceiver::await_ready()` SHALL resolve `Ok` within
  a short timeout
- **AND** the registry SHALL contain the consumer's name at the point readiness
  resolves

#### Scenario: Duplicate consumer returns Err before mark_ready

- **GIVEN** a `DirectRegistry` with an existing live consumer for a name
- **WHEN** a second `DirectConsumer::start()` attempts to register the same name
- **THEN** `start()` SHALL return `Err(EndpointCreationFailed)` before calling
  `mark_ready()`
- **AND** the runtime SHALL map the error to `mark_failed` on the
  `StartupReceiver`

### Requirement: poll_ready behavior unchanged

The `DirectProducer::poll_ready()` SHALL retain its registry-based readiness
behavior: when the registry contains no entry for the producer's name and
`fail_if_no_consumers` is not `Some(false)`, it SHALL return
`Poll::Ready(Err(EndpointCreationFailed))`, and when the registry entry is
closed (the consumer is gone) it SHALL return
`Poll::Ready(Err(EndpointCreationFailed))`. The
semaphore permit acquisition SHALL move out of `poll_ready` into `call()`'s
future (see stateful-producer-readiness specification) — `poll_ready` SHALL NOT
acquire or hold a semaphore permit. The startup handshake eliminates the
auto-startup race window that previously caused spurious `None` observations;
outside the residual startup-ordering window documented below, this error path
fires only for genuinely-misconfigured routes (no consumer exists at all).

#### Scenario: poll_ready on truly-absent consumer still fails fast

- **GIVEN** a `DirectProducer` for a name with no consumer ever registered
- **WHEN** `poll_ready()` is called with `fail_if_no_consumers` unset (default)
- **THEN** it SHALL return `Poll::Ready(Err(EndpointCreationFailed(_)))`

#### Scenario: poll_ready acquires no semaphore permit

- **GIVEN** a `DirectProducer` with a live consumer registered for its name
- **WHEN** `poll_ready()` is called
- **THEN** it SHALL return `Poll::Ready(Ok(()))` without acquiring a semaphore
  permit, deferring acquisition to `call()`

#### Scenario: Producer ordered before consumer — residual operator window

- **GIVEN** a producer route with `startup_order` less than or equal to the
  consumer route's `startup_order`
- **WHEN** both routes auto-start via `start_context`
- **THEN** `poll_ready()` MAY return `Err(EndpointCreationFailed)` if the
  producer's route is driven before the consumer's `StartRoute` completes
- **AND** this residual window SHALL be documented in `camel-direct/CONTEXT.md`
  as operator-ordering responsibility (the operator sets `startup_order` so the
  consumer starts first)

