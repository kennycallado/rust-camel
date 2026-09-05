## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: Test port acquisition from staged listeners

The `camel-test` and `camel-component-wasm` Rust library-test and
integration-test suites SHALL obtain component server ports exclusively
from staged bound listeners (bind-0, stage, read actual port), not from
bind-read-drop port probes.

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
