# route-error-prefix

A runtime registration failure surfaces with a doubled `invalid state:` prefix
(bd rc-uqwy2, P3, found by mission 510's
`crates/camel-builder/tests/layer_middleware_routes.rs::layered_producer_readiness_error_surfaces`).
Fix at the command layer so the same failure carries exactly one contextual
prefix, without changing `DomainError` semantics or any port contract.

## Why — traced chain for a `ComponentNotFound` registration failure

`CamelError::ComponentNotFound("mock")` is raised during route compilation
(e.g. `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs:70`; its
Display is `"Component not found: {0}"`, `crates/camel-api/src/error.rs:262`).
Note: the bd and mission order cite `lifecycle/adapters/runtime_bus.rs`; the
file has since moved to `lifecycle/application/runtime_bus.rs` (same code,
lines cited below verified at base `ba192f82e`).

1. **Runtime execution adapter** —
   `crates/camel-core/src/lifecycle/adapters/runtime_execution.rs:21-23`
   (`to_domain`): every `CamelError` becomes
   `DomainError::InvalidState(e.to_string())`. The variant identity is lost
   here; the payload is still prefix-free: `Component not found: mock`.
2. **Command layer, internal path** —
   `crates/camel-core/src/lifecycle/application/commands.rs:254`
   (`handle_register_internal`: `execution.register_route(def).await?`): the
   `?` applies `From<DomainError> for CamelError`
   (`crates/camel-core/src/lib.rs:164-168`), which stringifies via
   `DomainError`'s Display (`crates/camel-core/src/lifecycle/domain/error.rs:19`
   prepends `"invalid state: "`). **First `invalid state:` injected** →
   `CamelError::RouteError("invalid state: Component not found: mock")`.
3. **RuntimeBus port boundary** —
   `crates/camel-core/src/lifecycle/application/runtime_bus.rs:298-301`
   (`RouteRegistrationPort for RuntimeBus::register_route`):
   `CamelError::RouteError(msg) => DomainError::InvalidState(msg)` — the
   prefixed string is carried verbatim into an `InvalidState` payload.
   (`runtime_bus.rs:291-293` wraps journal-recovery errors the same way; that
   hop is not exercised by this scenario.)
4. **Context surface** — the two confirmed add-a-layer callers:
   `crates/camel-core/src/context.rs:660-663`
   (`CamelContext::add_route_definition`, `.map_err(Into::into)`) and
   `crates/camel-core/src/context.rs:187-192`
   (`RuntimeExecutionHandle::add_route_definition`, same shape). `Into::into`
   applies `lib.rs:164-168` again. **Second `invalid state:` injected.**

Observed result: `CamelError::RouteError("invalid state: invalid state:
Component not found: mock")`.

Public command path (`handle_register`,
`commands.rs:157-162,219-221`) stops after layer 1's DomainError and renders
it via Display at `commands.rs:162` (`aggregate.fail(runtime_err.to_string())`)
and `commands.rs:219-221`
(`RouteError(format!("runtime registration failed for route '{route_id}':
{runtime_err}"))`) → `runtime registration failed for route 'x': invalid
state: Component not found: mock` — single wrapper, but the spurious
`invalid state:` from layer 1's Display leaks into the user-facing text.

No existing test pins the doubled text (`grep "invalid state: invalid state"`
over `crates/` returns nothing); the route test above only asserts
`contains("Component not found: mock")` and passes under both forms.

## What changes

**Chosen conversion point: the command layer** (`commands.rs`) — the single
choke point both paths cross right after the execution port returns a
`DomainError`. Structured `match` on the variant (not string-strip), payload
preserved verbatim:

- `handle_register_internal` (`commands.rs:253-255`): replace the naked `?`
  with `DomainError::InvalidState(msg) => CamelError::RouteError(msg)`,
  `other => other.into()`. No re-render through Display, so no prefix is
  injected at this hop.
- `handle_register` (`commands.rs:157-221`): render the compensation reason
  and the surfaced `RouteError` from the `InvalidState` payload (a small
  helper `registration_failure_message(&DomainError) -> String`); other
  variants keep their Display. Log fields keep `%runtime_err` (no log
  changes).

**Before / after** (same failure, `ComponentNotFound("mock")`):

- Internal/context path before: `invalid state: invalid state: Component not
  found: mock` → after: `invalid state: Component not found: mock` (exactly
  one prefix, produced by the final `DomainError → CamelError` surface
  conversion, which is legitimate context).
- Public command path before: `runtime registration failed for route 'x':
  invalid state: Component not found: mock` → after: `runtime registration
  failed for route 'x': Component not found: mock` (exactly one contextual
  prefix, zero `invalid state:`).

**Tests** (in `crates/camel-core/src/lifecycle/application/commands_tests.rs`,
driving `execute_command(RuntimeCommand::RegisterRoute)` with a
`ConfigurableExecutionPort` whose `register_route` returns
`DomainError::InvalidState("Component not found: mock")` — byte-identical to
what `to_domain` produces for a real `CamelError::ComponentNotFound("mock")`):

- `registration_failure_has_no_duplicated_prefix`: message contains
  `runtime registration failed for route` exactly once and `invalid state:`
  zero times.
- `registration_error_keeps_the_original_cause`: message still contains
  `Component not found: mock`.

**Non-goals**: no `DomainError` variant changes (no new message-only variant),
no port signature changes (`RuntimeExecutionPort` / `RouteRegistrationPort`
keep `Result<_, DomainError>`), no new public error type, no changes to
`runtime_bus.rs:112/136` messages (different chain), no log-level changes, no
rewriting of unrelated `RouteError` messages.
