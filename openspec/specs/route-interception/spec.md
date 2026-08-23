# route-interception Specification

## Purpose
TBD - created by archiving change advice-route-interception. Update Purpose after archive.
## Requirements
### Requirement: Intercept rule model and matching

The system SHALL support an ordered set of interception rules where each rule
maps one exact endpoint URI to one action, duplicate rule URIs are permitted
in declaration order, and the first rule whose URI equals the send URI
determines the action. Rule construction SHALL reject any action target
outside the `mock:` scheme.

#### Scenario: exact URI match with first-match-wins

- **GIVEN** rules `[("seda:out", SkipTo mock:a), ("seda:out", SkipTo mock:b)]` and a route step `to: seda:out`
- **WHEN** the route compiles and an exchange traverses it
- **THEN** `mock:a` records the exchange and `mock:b` records nothing

#### Scenario: empty rule set leaves the send untouched

- **GIVEN** one context built with an empty `InterceptRules` and one identical context built without interception configuration
- **WHEN** the same exchange traverses `to: seda:out` in both
- **THEN** the seda consumer in each context receives exactly one identical exchange

#### Scenario: non-mock action targets are rejected at rule construction

- **GIVEN** the rules `[("kafka:x", SkipTo "direct:y"), ("kafka:z", DivertCopyTo "seda:w")]`
- **WHEN** `InterceptRules::new` is called with them
- **THEN** construction fails with a rule-indexed error naming the `SkipTo` target, and the `DivertCopyTo` case fails identically when constructed alone (table-driven: both actions, both targets, rule index in every message)

### Requirement: Skip semantics replace the send before component resolution

The system SHALL apply `SkipTo` by substituting the target URI before the
original URI's component is resolved, so the original scheme needs no
registered component.

#### Scenario: skipped URI with unregistered real component

- **GIVEN** a rule `("kafka:orders", SkipTo mock:orders)`, no kafka component registered, and a route step `to: kafka:orders`
- **WHEN** the route compiles and an exchange traverses it
- **THEN** compilation succeeds and `mock:orders` records the exchange

#### Scenario: skip target resolution failure is a compile error

- **GIVEN** a rule `("kafka:x", SkipTo mock:x)` and no mock component registered
- **WHEN** the route compiles
- **THEN** compilation fails with an error naming the intercept target

### Requirement: Divert semantics isolate the copy outcome

The system SHALL apply `DivertCopyTo` by composing `WireTapService(copy
target)` before the real producer: a clone of the exchange goes to the copy
target (detached tracked task when admitted; INLINE on the caller's future
with back-pressure when saturated), the real producer then runs, and the real
producer's outcome is returned verbatim. Any copy failure SHALL be logged
and suppressed.

#### Scenario: divert delivers both copy and real message

- **GIVEN** a rule `("seda:out", DivertCopyTo mock:tap)` and a running `from: seda:out` consumer
- **WHEN** an exchange traverses `to: seda:out` and the step completes
- **THEN** the real send's outcome is returned and, after lifecycle drain joins the copy task, `mock:tap` has recorded the clone and the consumer received the real message

#### Scenario: saturated divert runs the copy inline before the real send

- **GIVEN** a divert rule (copy concurrency = `WireTapService` default 20), 20 earlier exchanges whose admitted copies are still held in flight, and a copy target that records arrival order relative to the real target
- **WHEN** the 21st exchange traverses the intercepted send
- **THEN** the 21st copy is recorded BEFORE the 21st real send's effect (inline CallerRuns) and the step outcome is still the real producer's outcome verbatim

#### Scenario: divert survives route stop and restart

- **GIVEN** a started context with a divert rule on a route, and that route stopped and restarted
- **WHEN** an exchange traverses the intercepted send after restart
- **THEN** the copy target records the clone and the real send completes (admission is reopened by restart, not permanently closed by the earlier stop)

#### Scenario: real Ok outcome stays verbatim when the copy call fails

- **GIVEN** a divert rule whose copy target producer fails synchronously on `call` and a real producer that succeeds
- **WHEN** an exchange traverses the intercepted send
- **THEN** the step outcome equals the real producer's `Ok` result verbatim and a warning is logged for the failed copy

#### Scenario: real Err outcome stays verbatim when the copy succeeds

- **GIVEN** a divert rule whose copy target succeeds and a real producer that returns `Err`
- **WHEN** an exchange traverses the intercepted send
- **THEN** the step outcome equals the real producer's `Err` verbatim (copy delivery does not mask or alter it)

#### Scenario: copy poll_ready failure is swallowed

- **GIVEN** a divert rule whose copy target producer errors on `poll_ready`
- **WHEN** an exchange traverses the intercepted send
- **THEN** the step completes with the real producer's outcome and the copy failure is logged, not propagated

#### Scenario: copy target resolution failure is a compile error

- **GIVEN** a `DivertCopyTo` rule whose copy URI has no registered component
- **WHEN** the route compiles
- **THEN** compilation fails with an error naming the copy target

### Requirement: Rules freeze at first route registration or start

The system SHALL accept intercept rules only until the first successful
route registration or context start, whichever occurs first, and SHALL
reject later rule changes; stop/restart SHALL NOT unfreeze.

#### Scenario: setting rules after the first route registration is rejected

- **GIVEN** a context that has already successfully registered one route
- **WHEN** `set_intercept_rules` is called
- **THEN** the call returns a frozen-rules error and the registered route's pipeline is unchanged

#### Scenario: setting rules after start of an empty context is rejected

- **GIVEN** a context with zero routes that has been started successfully
- **WHEN** `set_intercept_rules` is called
- **THEN** the call returns a frozen-rules error

#### Scenario: a failed start does not freeze rules

- **GIVEN** a context with zero routes whose `start` fails
- **WHEN** `set_intercept_rules` is called afterwards
- **THEN** the call succeeds (freeze trips only on successful start or successful route registration)

#### Scenario: recompiled pipelines keep the same rules

- **GIVEN** a started context with a skip rule and a route affected by it
- **WHEN** the route is recompiled through the hot-reload path
- **THEN** the recompiled pipeline applies the same rule

### Requirement: seda interception stays on the send side

The system SHALL intercept `to: seda:` endpoints by wrapping or replacing the
enqueue producer only, and SHALL NOT intercept seda consumers.

#### Scenario: skip replaces the enqueue

- **GIVEN** a rule `("seda:q", SkipTo mock:q)` and a running `from: seda:q` consumer
- **WHEN** an exchange traverses `to: seda:q`
- **THEN** `mock:q` records the exchange and the consumer receives nothing

### Requirement: Divert preserves real-producer readiness discipline

The system SHALL, inside the composed divert step, await readiness on the
same real-producer service instance before invoking `call`, return readiness
errors verbatim, and never invoke `call` after a readiness failure.

#### Scenario: real producer readiness is driven before call

- **GIVEN** a divert rule and a real-producer stub that records a readiness event then a call event and returns a sentinel `Ok` exchange
- **WHEN** an exchange traverses the intercepted send
- **THEN** the recorded order is readiness-before-call and the sentinel `Ok` result is returned verbatim

#### Scenario: real producer readiness failure returns verbatim and skips call

- **GIVEN** a divert rule and a real-producer stub whose readiness fails with a sentinel error
- **WHEN** an exchange traverses the intercepted send
- **THEN** the step returns that readiness error verbatim and the stub's call is never invoked

### Requirement: Interception stays in the data plane

The system SHALL implement interception as compile-time service composition
with no dependency on the control-plane query types.

#### Scenario: interception code has no query-plane dependency

- **GIVEN** the interception modules (`InterceptRules`, the compiler application, and the divert composition)
- **WHEN** the hexagonal architecture boundary test suite analyzes camel-core's dependency edges
- **THEN** no interception-related module declares a dependency on `RuntimeBus`, `RuntimeQuery`, or `RuntimeQueryBus` types

