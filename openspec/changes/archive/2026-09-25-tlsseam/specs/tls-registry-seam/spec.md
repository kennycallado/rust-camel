## ADDED Requirements

### Requirement: Dual-handle process-global access

`TlsReloadRegistry` and camel-ws `ServerRegistry` SHALL expose the
process global through two handles with a single shared instance:
`global()` returning `&'static Self` (existing signature preserved)
and `global_arc()` returning an `Arc<Self>` clone of the same
allocation.

#### Scenario: Handle identity

- **GIVEN** the process global initialized via `global_arc()`
- **WHEN** a caller invokes `global()`
- **THEN** the returned reference points at the same instance the
  `Arc` wraps (registration through either handle is observable
  through the other)

#### Scenario: Existing callers unchanged

- **GIVEN** production code calling `TlsReloadRegistry::global()`
  (runtime reload bus, grpc/http components, core tests)
- **WHEN** the dual-handle backing store lands
- **THEN** those call sites compile unchanged and observe the same
  registrations as before

### Requirement: Isolated registry construction

`ServerRegistry::new()` SHALL construct a registry pair that is
isolated from the process-global `ServerRegistry` and
`TlsReloadRegistry`: empty port map, empty staged map, and a fresh
private `TlsReloadRegistry`.

#### Scenario: No mutation of global server and TLS registries

- **GIVEN** a test-owned `ServerRegistry::new()` instance
- **WHEN** the test spawns and releases servers through it (including
  instance-scoped `reset()`)
- **THEN** the process-global `ServerRegistry::global()` map and
  `TlsReloadRegistry::global()` handlers are untouched
  (`GLOBAL_CONNECTION_REGISTRIES` writes by consumer start/stop are
  existing behavior and remain)

#### Scenario: Reset scope

- **GIVEN** two isolated registries each hosting a live server
- **WHEN** one registry's `reset()` runs
- **THEN** only that registry's servers abort; the other registry's
  servers keep running

### Requirement: TLS handler registration follows the owning registry

`ServerRegistry::get_or_spawn` and `get_or_spawn_with_listener` SHALL
register TLS reload handlers into the `TlsReloadRegistry` the owning
`ServerRegistry` instance holds — the process global for the global
instance, the private instance otherwise.

#### Scenario: Production reload flow preserved

- **GIVEN** a production-path `WsConsumer` (default constructor)
  started with a wss endpoint
- **WHEN** the runtime reload bus calls
  `TlsReloadRegistry::global().find(scheme, host, port)`
- **THEN** the handler registered by `get_or_spawn` is found and the
  server reloads its certificate material

#### Scenario: Isolated handler visibility

- **GIVEN** a test server spawned through an isolated registry
- **WHEN** the test queries the instance's `tls_registry().find(...)`
- **THEN** the handler is found there and NOT in
  `TlsReloadRegistry::global()`

### Requirement: Consumer constructor injection

`WsConsumer` SHALL accept an injected `Arc<ServerRegistry>` for tests
while the default constructor keeps the process global, and its
start/stop paths SHALL operate on the injected registry.

#### Scenario: Default keeps global identity

- **GIVEN** `WsConsumer::new(cfg, rt)` (production and endpoint path)
- **WHEN** the consumer starts a server
- **THEN** the server registers in `ServerRegistry::global()` exactly
  as before this change

#### Scenario: Injected registry end-to-end

- **GIVEN** `WsConsumer::with_server_registry(cfg, rt, isolated_reg)`
- **WHEN** the consumer starts and stops
- **THEN** spawn, ref-count, and release all act on `isolated_reg`,
  observable via its test accessors

### Requirement: camel-ws test isolation

camel-ws lib tests SHALL run against isolated registry instances
without holding the cross-test `REGISTRY_TEST_LOCK`, except tests
that specifically pin the lockdeadline contract or global-singleton
semantics.

#### Scenario: Parallel execution

- **GIVEN** two migrated server-hosting tests running concurrently in
  one test binary
- **WHEN** one calls its instance `reset()`
- **THEN** the other's servers are unaffected and no cross-test lock
  is contested

#### Scenario: Suite serialization removed

- **GIVEN** the camel-ws lib test suite
- **WHEN** registry-mechanics and consumer tests are migrated
- **THEN** the `REGISTRY_TEST_LOCK` chain (43 serialized holders at
  audit time) no longer forces sequential execution of migrated tests
