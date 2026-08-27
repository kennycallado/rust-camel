## ADDED Requirements

### Requirement: Bare-controller activation of the cohort barrier

camel-core SHALL expose a public, idempotent activation method for
the cohort barrier on `DefaultRouteController`, performing the same
gate-opening act the CamelContext lifecycle performs through the
controller actor handle, so that consumers driving a bare
`DefaultRouteController` (registered, started, and dispatched
outside a full context lifecycle) can release parked pipeline
dispatch after their startup completes. The method SHALL NOT change
barrier semantics for the context path: reset stays boot-scoped,
activation stays level-triggered, and the method SHALL NOT
re-close or re-arm the gate.

#### Scenario: Bare-controller dispatch releases after activation

- **GIVEN** a bare `DefaultRouteController` with a route added and started, and no CamelContext lifecycle involved
- **WHEN** a consumer sends an exchange and the controller's activation method has been called after startup
- **THEN** the parked dispatch proceeds and the sender's reply resolves within normal call timeouts, instead of parking until the drain timeout

#### Scenario: Unactivated bare controller parks dispatch

- **GIVEN** a bare `DefaultRouteController` with a route added and started, and the activation method never called
- **WHEN** a consumer sends an exchange
- **THEN** pipeline dispatch stays parked (barrier contract unchanged) — activation is the bare consumer's explicit responsibility

#### Scenario: Activation is idempotent and level-triggered

- **GIVEN** a bare controller whose barrier was already activated
- **WHEN** the activation method is called again
- **THEN** the call returns without effect (no re-arm, no reset) and any newly parked dispatch resolves immediately

#### Scenario: Context path unchanged

- **GIVEN** a CamelContext boot with its startup cohort completing normally
- **WHEN** the context lifecycle activates the barrier through the actor handle
- **THEN** the barrier opens, parked dispatch proceeds, and any additional activation call has no effect and requires no ordering relative to the context's act
