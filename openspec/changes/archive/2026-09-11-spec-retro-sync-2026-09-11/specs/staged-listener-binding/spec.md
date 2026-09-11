## MODIFIED Requirements

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
