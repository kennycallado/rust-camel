## ADDED Requirements

### Requirement: WASM source kernel convergence

The WASM source transport SHALL converge on the unified transport
auth kernel using the boundary-authentication shape (Gen B, like ws
and grpc): security wiring arrives via `set_security_context`, the
kernel authenticates at the transport edge where the raw request
exists, denials render in the transport idiom (HTTP status), and
accepted requests enter the pipeline with the typed carrier installed.
A `wasm:` source route without a plan SHALL be `Public` pass-through,
subject to the per-bind exposure gate.

#### Scenario: wasm route authenticates via the kernel

- **GIVEN** an `Authenticated` `wasm:` source route with provider
  wiring and an inbound request with a valid credential
- **WHEN** the host processes the request
- **THEN** the kernel mints the principal, the pipeline Exchange
  carries the carrier, and the 202-immediate-ack semantics for
  accepted requests are unchanged

#### Scenario: wasm route with a public plan is pass-through

- **GIVEN** a `wasm:` source route whose compiled plan is `Public`
  (no declared security policy) on a loopback bind
- **WHEN** an inbound request arrives
- **THEN** it is forwarded to the guest without credential
  extraction, as before

#### Scenario: wasm route with missing wiring fails closed

- **GIVEN** a `wasm:` source route classified non-`Public` whose
  security wiring was never injected into the consumer
- **WHEN** an inbound request arrives
- **THEN** the request is denied before the guest is woken (absent
  wiring never yields pass-through for non-`Public` plans)
