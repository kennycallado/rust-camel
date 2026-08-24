# Design: bean-test-registry

## Approach

Three layers, mirroring the intercepts architecture (Stage B) exactly:

1. **Parsing** (`crates/camel-cli/src/commands/test/document.rs`): new
   `beans: Option<BTreeMap<String, BeanDeclDoc>>` on `TestDocument` plus an
   accessor that eagerly constructs validated declarations. `BeanDeclDoc =
   { kind: BeanKindDoc (echo|setBody|fail), methods: Option<Vec<String>>,
   config: Option<BTreeMap<String,String>> }`, `deny_unknown_fields`,
   `rename_all = camelCase`. Validation runs inside `parse_test_document`
   (before the runner boots, and before any registry interaction): unknown
   kind; `methods` present-but-empty; blank bean name checked directly via
   `trim().is_empty()`; blank `methods` entries; per-kind config validation
   (`setBody` requires `body`; `fail` allows only `message`; `echo` allows no
   config keys). All failures are document errors → exit 2.
   `BeanRegistry::register`'s own name validation remains as
   defense-in-depth.

2. **Stub beans** (`crates/camel-cli/src/commands/test/beans.rs`, new): three
   `BeanProcessor` impls over a shared interior config. `echo.call` returns
   `Ok(())` leaving the exchange untouched; `setBody.call` replaces
   `exchange.input.body` (camel-api `Exchange` shape, `exchange.rs:60-64`)
   with the configured string; `fail.call` returns
   `Err(CamelError::ProcessorError(message))` with message = configured
   `message` or exactly `fail bean <name>` when absent — which
   (probe-verified, see below) propagates to the delivery result and surfaces
   as a document error (exit 2) carrying the message. `methods()` returns the
   runner-resolved list (wildcard auto-populated from the routes' `bean:`
   steps). `method_params` returns empty (no overload resolution in v1).
   `on_start`/`on_stop` are no-ops.

3. **Wiring** (`crates/camel-cli/src/commands/test/runner.rs`):
   **order of operations** — parse route sources (inline `routes` or
   `routeFiles`) into `RouteDefinition`s FIRST, recursively collect every
   `BuilderStep::Bean {name, method}` occurrence (including nested
   sub-pipelines: `circuit_breaker_fallback`, `cache.on_miss` — the
   `BuilderStep::Cache` variant whose `on_miss` field holds
   `Vec<BuilderStep>` (route_definition.rs:281-288) — and any other step
   variant holding `Vec<BuilderStep>`), build the
   `BeanRegistry`, register stubs, THEN `boot_context` with
   `builder.beans(Arc<Mutex<BeanRegistry>>)` (context_builder.rs:125) and
   finally `add_route_definition` (lookup is compile-time). Absent block →
   current behavior unchanged. The recursive walk needs one minimal
   camel-core addition: `RouteDefinition::circuit_breaker_fallback()` as a
   public accessor mirroring `steps()` (the field is `pub(crate)` today,
   route_definition.rs:330-344); every nested-step location inside
   `BuilderStep` variants is already reachable from the public
   `camel_core::route::{BuilderStep, RouteDefinition}` path.

**Method allowlist contract (probe-verified 2026-08-23).** The step compiler
checks bean existence at compile time and `bean.methods()` membership **per
call** (`step_compilers/core.rs:515-519`); `methods()` cannot be call-site
aware. Wildcard is therefore resolved by the RUNNER, not the bean: after route
definitions are parsed (inline or files) and BEFORE boot, the runner walks all
`BuilderStep::Bean {name, method}` pairs. Wildcard stubs (`methods` omitted)
are constructed with `methods() = exactly the methods the routes call on that
bean`; explicit `methods` lists are cross-validated (a route method outside the
declared list is a document error, exit 2, before boot). No additional
camel-core changes are required for method allowlist enforcement; the
per-call allowlist then always passes.

**fail semantics (probe-verified).** Without a configured error handler,
a processor `Err` propagates: `PipelineOutcome::Failed` → `send_and_wait` →
direct-producer `oneshot` returns `Err` (`route_compiler.rs:438-440`,
`route_compiler_ext.rs:439-462`) → the runner's delivery loop maps it to a
document error → **exit 2**, downstream mock count 0, route stays alive. So
v1 `fail` is failure-injection assertable via exit code 2 plus the configured
message in the failure output — not a passing-count-0 document.

**API facts (probe-verified).** `builder.beans()` takes
`Arc<std::sync::Mutex<BeanRegistry>>` (`context_builder.rs:125`);
`BeanRegistry::register` takes the bean by value; `BuilderStep`/`RouteDefinition`
import from `camel_core::route::` (the `lifecycle::application` path is
`pub(crate)`).

**camel-run boundary.** `camel run` reads Camel.toml `[beans]` (WASM-only) and
never touches `*.test.yaml`. Non-interference is pinned by a subprocess test
banning the test-document filename, the invalid kind name `teleport`, and the
`unknown variant` fragment, mirroring the intercepts precedent.

## Affected crates

- `camel-cli`: document.rs (`beans:` parsing + validation, 9 new unit tests in document_tests.rs);
  new `commands/test/beans.rs` (stub impls + unit tests); runner.rs
  (parse-walk-register-boot reorder, single call site); integration tests
  (execution matrix); docs + example.
- `camel-core`: ONE minimal addition — `pub fn circuit_breaker_fallback()`
  accessor on `RouteDefinition` (mirrors `steps()`; field stays
  `pub(crate)`). No behavior change.
- No other crate changes.

## Architecture boundaries

- DSL/Components: no new component; the closed lean set
  {direct, log, mock, seda, timer} is untouched (creep rule, ADR-0064 §2).
- Runtime/Services: no RuntimeBus/QueryBus traffic; no IPC; everything is
  in-process data-plane.
- Bean surface: consumes the existing `camel-bean` `BeanProcessor` +
  `BeanRegistry` API verbatim; no camel-bean changes. The single camel-core
change is the fallback accessor named in Affected crates.
- Tier boundary (ADR-0064 §3): stubs need no non-`direct:` stimulus, no
  plugin trust model, no additional filesystem access beyond existing
  route-source resolution.
- Spec amendment is surgical: "SHALL NOT start beans" becomes "SHALL NOT
  start WASM plugins, file-watch, or network servers, and SHALL NOT load user
  beans; it MAY register built-in stub beans declared in the test document".

Single-phase change (one coherent slice, 5 tasks).

## Alternatives considered

- **Load WASM beans in lean boot** — rejected: spec-forbidden, plugin-dir
  trust model violates tier boundary.
- **`to: bean:x` component** — rejected: grows the lean set; needs ADR
  amendment; no user demand yet.
- **Script/expression-backed stubs** — deferred: composes with rc-3kwt
  matchers later; not needed for the load-blocker use case.
- **Declaring stubs in route files (Camel.toml-style)** — rejected: test
  doubles belong to the test document, not production config.
- **Wildcards via camel-core contract change** — rejected: additive CLI-only
  change is the budget; document-level validation achieves the same UX.
