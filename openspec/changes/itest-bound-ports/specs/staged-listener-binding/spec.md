## ADDED Requirements

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

The `camel-test` Rust library-test and integration-test suites SHALL obtain
component server ports exclusively from staged bound listeners (bind-0,
stage, read actual port), not from bind-read-drop port probes.

#### Scenario: no-port-probes-remain

- **GIVEN** the change is applied
- **WHEN** `grep -rn find_free_port crates/camel-test/` runs
- **THEN** it returns no matches and every former callsite acquires its
  port from a staged listener helper

#### Scenario: staged-port-survives-to-serve

- **GIVEN** a test stages a listener and formats its route URI with the
  actual port
- **WHEN** the context starts and the route is exercised end-to-end
- **THEN** the exchange is served by the staged listener's socket — the
  same socket the helper bound — proving no drop-to-rebind window existed
