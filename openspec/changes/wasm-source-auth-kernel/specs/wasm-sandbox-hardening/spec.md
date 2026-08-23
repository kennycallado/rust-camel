## ADDED Requirements

### Requirement: WASM source listener bind governance

The host SHALL treat the listener bind as operator-authoritative.
When the operator declares a `bind` URI query parameter and the guest
declares a matching `listener_spec.bind` (equal after normalization),
the host SHALL bind exactly that address. When the two differ, the
consumer SHALL fail during startup — after the guest reveals its
listener spec and before `TcpListener::bind` — with an error naming
both addresses; the guest-declared bind SHALL NOT be silently
overridden. When the operator declares no bind, the guest-declared
bind SHALL be used but SHALL pass through the same per-bind exposure
gate as any other transport bind before the TCP listener is created.
A `bind` forwarded from the route URI through guest config SHALL NOT
bypass the gate.

#### Scenario: matching binds produce one listener

- **GIVEN** a `wasm:` source route with `?bind=127.0.0.1:8080` and a
  guest declaring `listener_spec.bind = "127.0.0.1:8080"`
- **WHEN** the source consumer starts
- **THEN** the host binds `127.0.0.1:8080` and exactly one listener
  exists

#### Scenario: conflicting binds fail before the socket is bound

- **GIVEN** a `wasm:` source route with an operator-declared bind and
  a guest-declared `listener_spec.bind` that differs after
  normalization
- **WHEN** the source consumer starts
- **THEN** startup fails with an error naming the route, the operator
  bind, and the guest bind, and no TCP listener is bound

#### Scenario: guest-declared non-loopback bind is gated

- **GIVEN** a `wasm:` source route with no operator bind whose guest
  declares `listener_spec.bind = "0.0.0.0:8080"` and a `Public` plan
- **WHEN** the source consumer starts
- **THEN** startup fails unless the operator acknowledged that bind
  with `allow_public_exposure = true`
