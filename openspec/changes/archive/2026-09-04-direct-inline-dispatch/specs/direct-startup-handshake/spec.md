## MODIFIED Requirements

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
