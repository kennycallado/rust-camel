# staged-listener-binding Specification

## Purpose
TBD - created by archiving change itest-bound-ports. Update Purpose after archive.
## Requirements
### Requirement: Staged listener consumption

The `camel-http` and `camel-component-ws` process-global server registries
SHALL accept a pre-bound `tokio::net::TcpListener` staged under its own
`(host, port)` key and, when a consumer resolves that exact key, SHALL serve
the staged listener instead of binding a new socket. Staging SHALL be
one-shot per key and empty by default (no staged listener, behaviorally
compatible with the unstaged path).

#### Scenario: staged-key-first-spawn-uses-listener

- **GIVEN** a listener bound to `127.0.0.1:0` is staged, and its actual port
  is `P`
- **WHEN** `get_or_spawn("127.0.0.1", P, …)` runs
- **THEN** the server serves on the staged listener's socket without a
  second `bind`, and the registry entry stores `bound_addr` equal to the
  staged listener's local address

#### Scenario: second-caller-reuses-entry

- **GIVEN** an entry was spawned from a staged listener for `(host, P)`
- **WHEN** a second caller runs `get_or_spawn(host, P, …)` with compatible
  limits
- **THEN** the same entry is reused and no listener is staged or bound anew

#### Scenario: empty-slot-behaviorally-compatible

- **GIVEN** no listener is staged for a key
- **WHEN** `get_or_spawn(host, port, …)` runs
- **THEN** the registry binds `host:port` itself, behaviorally compatible
  with the pre-change legacy path (an added empty-slot map lookup is the
  only internal difference)

#### Scenario: wrong-key-staged-fails-deterministically

- **GIVEN** a listener is staged under `("127.0.0.1", P)` but a consumer
  resolves `("localhost", P)`
- **WHEN** `get_or_spawn("localhost", P, …)` runs
- **THEN** the registry returns a deterministic error naming the staged
  port conflict instead of attempting another bind — the host strings can
  resolve to the same socket, so a silent fresh bind risks `EADDRINUSE`
  flakiness — and the staged slot is left untouched

#### Scenario: duplicate-staging-same-key-rejected

- **GIVEN** a listener is already staged under `(host, P)`
- **WHEN** a second listener is staged under the same key
- **THEN** staging is rejected with an error and the first staged listener
  remains in the slot (never replaced or silently dropped)

#### Scenario: concurrent-distinct-keys-independent

- **GIVEN** listeners are staged under `(h1, P1)` and `(h2, P2)`
- **WHEN** consumers resolve both keys
- **THEN** each spawn consumes its own staged listener independently, with
  no cross-key interference, for both the camel-http and camel-component-ws
  registries

#### Scenario: staged-consumption-only-on-vacant-entry

- **GIVEN** a ws entry exists for `(host, P)` — including after its consumer
  released (entries are process-lifetime; release removes the consumer
  reference, not the entry)
- **WHEN** a listener is staged for `(host, P)` and another consumer runs
  `get_or_spawn(host, P, …)`
- **THEN** the existing entry is reused and the staged listener is NOT
  consumed — staged consumption applies only when a vacant entry is created

#### Scenario: tls-prebound-served

- **GIVEN** a staged listener and a TLS-configured spawn on its exact key
- **WHEN** the server starts
- **THEN** TLS is served on the staged socket via the pre-bound listener
  path, not an internally-bound socket

### Requirement: Test port acquisition from staged listeners

The `camel-test` and `camel-component-wasm` Rust library-test and
integration-test suites SHALL obtain component server ports exclusively
from staged bound listeners (bind-0, stage, read actual port), not from
bind-read-drop port probes. Component-internal test listeners follow the
same rule: the `camel-http` consumer test listeners are staged per
ADR-0070 — the pre-bound listener is staged and consumed by the
consumer's spawn path (`get_or_spawn`), so no drop-to-rebind window
exists between the helper bind and the consumer rebind — and readiness
is awaited by polling, never by a wall-clock sleep. (camel-http staging
landed in b9deb92d, bd rc-1dgvg.)

#### Scenario: no-port-probes-remain

- **GIVEN** the change is applied
- **WHEN** `grep -rn find_free_port crates/camel-test/` and
  `grep -rn 'fn free_port' crates/components/camel-component-wasm/`
  run
- **THEN** both return no matches and every former callsite acquires its
  port from a staged listener helper, except sites whose port feeds an
  asserted-unbound address (a staged listener is bound by definition) —
  those name a fixed reserved address instead

#### Scenario: staged-port-survives-to-serve

- **GIVEN** a test stages a listener and formats its route URI with the
  actual port
- **WHEN** the context starts and the route is exercised end-to-end
- **THEN** the exchange is served by the staged listener's socket — the
  same socket the helper bound — proving no drop-to-rebind window existed

#### Scenario: wasm-staged-port-survives-to-serve

- **GIVEN** a wasm test stages a listener and formats the guest's
  `bind` config with the actual port
- **WHEN** the route starts and the webhook is exercised end-to-end
- **THEN** the exchange is served by the staged listener's socket — the
  same socket the helper bound — proving no drop-to-rebind window
  existed between the helper and the host-side bind

#### Scenario: http consumer test listener is staged and polled

- **GIVEN** a camel-http consumer test that needs a served route on a
  known port
- **WHEN** the test stages a pre-bound listener for the consumer's bind
  key and starts the context
- **THEN** the consumer serves on the staged listener's socket — the
  same socket the helper bound, with no drop-to-rebind window — and the
  test proceeds once the readiness poll confirms the listener serves,
  with no fixed wall-clock sleep

### Requirement: Wasm source staged consumption

The `camel-component-wasm` source consumer SHALL accept a pre-bound
`tokio::net::TcpListener` staged under its own exact `(host, port)` key
and, when the resolved bind address matches that key, SHALL serve the
staged listener instead of binding a new socket. Consumption SHALL be
one-shot per key and SHALL occur at the single host-side bind site,
after the operator/guest bind agreement and the ADR-0061 exposure gate.
The staged map SHALL be empty by default (behaviorally compatible with
the unstaged path).

#### Scenario: staged-bind-uses-listener

- **GIVEN** a listener bound to `127.0.0.1:0` is staged, its actual port
  is `P`, and a wasm source route declares guest bind `127.0.0.1:P`
- **WHEN** the route starts and an HTTP request is sent to
  `127.0.0.1:P`
- **THEN** the request is served by the staged listener's socket — the
  same socket the caller bound — with no second `bind` (the helper held
  the socket until consumption, so a re-bind would fail with
  `EADDRINUSE`; a serving route is itself the proof of consumption)

#### Scenario: no-staged-entry-binds-normally

- **GIVEN** no listener is staged for the resolved bind address
- **WHEN** the wasm source route starts
- **THEN** the consumer binds the address itself via
  `TcpListener::bind`, behaviorally compatible with the pre-change path
  (an added empty-map lookup is the only internal difference)

#### Scenario: wrong-host-staged-fails-deterministically

- **GIVEN** a listener is staged under `("0.0.0.0", P)` but a wasm
  source route resolves its bind to `("127.0.0.1", P)` (loopback bind,
  so the ADR-0061 exposure gate stays silent)
- **WHEN** the route starts
- **THEN** the consumer fails deterministically with
  `EndpointCreationFailed` naming the staged port conflict
  (`staged listener conflict on port {p}: staged under host {h},
  requested {r}`) instead of attempting the bind — the host strings can
  cover the same socket, so a silent fresh bind risks `EADDRINUSE`
  flakiness — and the staged slot is left untouched

#### Scenario: duplicate-staging-same-key-rejected

- **GIVEN** a listener is already staged under `(host, P)`
- **WHEN** a second listener is staged under the same key
- **THEN** staging is rejected with
  `listener already staged for {h}:{p}` and the first staged listener
  remains in the slot (never replaced or silently dropped)

#### Scenario: refused-route-preserves-staged-slot

- **GIVEN** a listener is staged under `("0.0.0.0", P)`, a first wasm
  source route declares guest bind `0.0.0.0:P` while its operator `bind`
  entry names `127.0.0.1:1` — refused at the operator/guest bind
  agreement before the bind site — and a second, corrected route declares
  guest bind `0.0.0.0:P` with no operator `bind` entry and the
  ADR-0061 acknowledgment for the non-loopback bind
- **WHEN** the corrected route starts and an HTTP request is sent to
  `127.0.0.1:P`
- **THEN** the request is served by the first route's staged socket —
  the refusal consumed nothing, and the slot was preserved for the
  corrected route, which consumed it exactly once

