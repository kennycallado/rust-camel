# Design: transacted-route-definition-walk

## Approach

**Chosen mechanism: run the SAME registry-backed walk at the registration
seam.** The manager registry is reachable there, so no separate rejection
path is invented.

Evidence (verified on `main` @ `e1b81ed86`):

- `handle_register_internal` (`crates/camel-core/src/lifecycle/application/
  commands.rs:270`) receives `CommandDeps`, which carries
  `transaction_registry: Arc<TransactionManagerRegistry>` and
  `datasource_catalog: Option<Arc<dyn DatasourceCatalog>>`
  (`commands.rs:41-51`).
- `RuntimeBus::deps()` populates both from the bus
  (`lifecycle/application/runtime_bus.rs:195-207`); the bus is the same
  object that owns `CamelContext::transaction_registry()` and is the target
  of `install_datasource_catalog` (`context.rs:746-782`).
- `handle_register_internal` is the single landing for
  `RouteRegistrationPort::register_route` calls with a pre-lowered
  definition: `CamelContext::add_route_definition` (`context.rs:650`) and
  `RuntimeExecutionHandle::add_route_definition` (`context.rs:183`) both call
  the port → `RuntimeBus::register_route` (`runtime_bus.rs:341`) →
  `handle_register_internal`. Direct route-controller entry points
  (`RouteController::add_route`) are downstream of this seam and outside
  this slice.
- The resolver shape already exists: the canonical lowering arm calls
  `walk_transacted_block(&built, |ds| transaction_registry.get(ds),
  |ds| catalog.get_config(ds).map(|c| c.db_url))` (`commands.rs:1369-1377`).

### Mechanics

1. **Scan** (`crates/camel-core/src/transacted_walk.rs`): add
   `walk_route_definition_transacted_blocks(definition, resolve_manager,
   resolve_url)`. It walks `RouteDefinition::steps()` and
   `RouteDefinition::circuit_breaker_fallback()` in document order, recursing
   through the existing `child_steps` map. At each node: if it is a
   `BuilderStep::Transacted`, call `walk_transacted_block` (the same visitor,
   depth-0 semantics); otherwise recurse. Because `child_steps` never yields
   a `Transacted` body, only OUTERMOST blocks are entered and a nested block
   surfaces as the visitor's own `NestedBlock` rejection — the same
   precedence rule as the canonical arm (R3-2). Error-handler policies on a
   lowered `RouteDefinition` hold compiled processors, not `BuilderStep`s,
   so those two fields are the complete pre-lowered surface.
2. **Call site** (`commands.rs::handle_register_internal`): run the scan
   BEFORE the duplicate check and BEFORE `execution.register_route(def)`,
   mirroring the canonical `handle_register`, which runs
   `spec.validate_contract()?` before its duplicate check. A rejection
   therefore never leaves a persisted aggregate or a runtime side effect.
3. **Typed error across the port**: `RouteRegistrationPort::register_route`
   is a `pub(crate)` in-process port. Change its return type from
   `Result<(), DomainError>` to `Result<(), CamelError>`. Today the impl
   flattens every error through `DomainError::InvalidState(e.to_string())`
   (`runtime_bus.rs:346-357`) and `From<DomainError> for CamelError` renders
   `RouteError(e.to_string())` (`lib.rs:173-177`), so
   `TransactionError::ManagerMissing` reaches callers as prose. The two
   production callers (`context.rs:183`, `context.rs:654`) already return
   `CamelError`; the flattening disappears with the signature change.

   Scope of the typed pass-through: ONLY `CamelError::Transaction(..)` from
   the scan is forwarded intact. Every other registration failure keeps the
   existing formatting — `CamelError::RouteError(msg)` becomes
   `RouteError("invalid state: {msg}")` and any other variant becomes
   `RouteError("invalid state: {display}")`, exactly the text today's
   `DomainError::InvalidState` round-trip produced. The `JoinSessionMissing`
   backstop still surfaces through the unchanged `RuntimeExecutionAdapter`,
   whose `to_domain` maps it to `DomainError::InvalidState(e.to_string())`
   (`runtime_execution.rs:21-31`); it is not turned typed by this change.

### Unchanged

- `StepCompilerRegistry::compile_step` keeps the typed
  `JoinSessionMissing` dark gate byte-for-byte
  (`lifecycle/adapters/step_compilers/mod.rs:293-306`); `rc-ggo3d` removes
  it in its own slice.
- Both lowering fan-ins keep their existing walks. Re-validating a
  declarative-compiled definition at registration reuses the structural
  checks and adds dialect resolution: with a registered manager the second
  pass may newly reach the dialect stage. A registered manager with no
  installed catalog, or no URL entry for the datasource, fails
  `DialectUnprovable` at this seam — not the `JoinSessionMissing` dark gate;
  with a catalog entry it produces the verdict the canonical boundary
  already produces.
- Parity is scoped to offences representable as lowered `BuilderStep`s at
  each entry: unknown block keys stay parser-only, and the canonical form
  cannot author a dynamic-routing step at all.
- `TransactionError` gains no variants.

### ADR truth

ADR-0101 §h currently records that the registration seam does not run the
walk and that the dark gate may not be removed before it does. After this
change the ADR is updated in place: the seam runs the registry-backed walk
(steps + circuit-breaker fallback), scan rejections keep their typed
`TransactionError` across the registration port (other registration
failures keep their existing `invalid state: …` formatting), and the dark
gate stays until `rc-ggo3d`.

## Affected crates

- `camel-core`: `transacted_walk` (scan helper), `commands` (registration
  seam call), `runtime_bus` + `ports/registration_port.rs` (typed error),
  `context_tests` (four seam tests).
- `docs`: ADR-0101 §h prose.

## Architecture boundaries

Runtime layer only: the application command handler calls a domain-free
visitor that already exists and already reads `camel-api` types. The DSL
keeps its own deferred entries; no DSL code changes. No component, service,
language or function surface is touched, and `camel-api` gains no API.

## Alternatives considered

- **Reject any pre-lowered `BuilderStep::Transacted` at registration with a
  named error.** Rejected: the registry and catalog are both in
  `CommandDeps` at the seam, and the visitor already takes exactly those
  resolvers. A blanket rejection would also reject definitions the seam can
  validate correctly (e.g. a future `rc-ggo3d`-lowered route re-registered
  through the same path), and it would create a second, weaker rejection set
  that can drift from the walk.
- **Walk in `CamelContext::add_route_definition` / `RuntimeExecutionHandle::
  add_route_definition`.** Rejected: two call sites would duplicate the
  seam, and the internal bus path (`RuntimeBus::register_route` callers)
  would stay unvalidated.
- **Keep the port returning `DomainError` and add a
  `DomainError::Transaction` variant.** Rejected as the larger change: it
  widens the lifecycle domain error surface for one transient in-process
  call and still requires a second conversion in `RuntimeBus`. The port is
  `pub(crate)`; returning `CamelError` is the smaller, honest signature.
