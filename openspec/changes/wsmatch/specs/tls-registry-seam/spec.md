## ADDED Requirements

### Requirement: Server-scoped connection registry resolution

camel-ws SHALL resolve the `WsConnectionRegistry` for an accepted
connection from the accepting server's owned per-path map on
`WsAppState` (server-scoped owned state), not from a process-global
path match. `GLOBAL_CONNECTION_REGISTRIES` remains the
producer-facing index keyed by the full `(host, port, path)` triple;
consumer start and stop keep its entries exact-keyed.

#### Scenario: Path-sharing servers are isolated

- **GIVEN** two servers on different host:port serving the same path,
  each with a live client connection
- **WHEN** the consumer on one server stops
- **THEN** only that server's connections receive the stop close;
  the other server's client on the shared path keeps exchanging
  frames

#### Scenario: Accept-path registration is server-local

- **GIVEN** server A registered under `(hostA, portA, path)` and
  server B under `(hostB, portB, path)` in the same process
- **WHEN** a client connects to server B on that path
- **THEN** the connection registers into server B's registry only,
  and `A.registry` contains no connection from server B's accept

#### Scenario: Producer lookup unchanged

- **GIVEN** a running consumer registered under the full triple
- **WHEN** a producer sends with `sendToAll` or a targeted
  `CamelWsConnectionKey`
- **THEN** the exact-key global lookup finds that consumer's
  registry and delivery behaves as before
