# Proposal: bean-test-registry

## Why

Production routes containing `bean:` steps cannot be tested with `camel test`.
The lean boot in `crates/camel-cli/src/commands/test/runner.rs` never wires a
bean registry, and bean lookup happens at route-compile time
(`crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs:501-529`):
`add_route_definition` fails with `Bean not found: {name}`, the runner maps it
to a `doc_error`, and `camel test` exits 2. The route never loads, so no other
step in it can be exercised declaratively. This is the largest remaining
adoption blocker of the two-tier testing program (ADR-0064) and one of the two
gates on Stage C's lint escalation (e_opus advisory A1, recorded on rc-car5).

## What Changes

Add a declarative `beans:` block to `*.test.yaml`. For each entry the test
runner registers an in-process built-in **stub bean** in a `BeanRegistry`
threaded through `CamelContextBuilder::beans()` before routes load, so `bean:`
steps compile against it.

- `beans: {<name>: {kind, methods?, config?}}` — `BTreeMap`, camelCase,
  `deny_unknown_fields` (nested unknown fields rejected), mirroring the
  `intercepts:` block conventions.
- Built-in kinds (v1): `echo` (exchange passes unchanged), `setBody`
  (config `body`), `fail` (processor returns `Err`; config `message`
  optional, default exactly `fail bean <name>`; observable as a document
  error: exit 2 with the message in the failure output).
- Bean names and `methods` entries SHALL be non-blank; blank values are
  document errors (exit 2).
- `methods`: optional list of method names the stub accepts, mirroring the
  production bean surface (step-compiler allowlist). Omitted = accepts any
  method name; present = each route `bean:` method must be declared.
- `config`: per-kind validated (`BTreeMap<String, String>`); unknown keys and
  missing required keys are document errors (exit 2), consistent with the
  `TestDocError` protocol.
- Amend the mock-testkit lean-boot clause: lean boot still SHALL NOT start
  WASM plugins, file-watch, or network servers, and SHALL NOT load user beans;
  it MAY register built-in in-process stub beans declared in the test document.

Explicitly excluded:
- WASM beans (spec forbids them in lean boot; plugin-dir trust model stays out).
- Script/expression-backed beans (defer; may compose with rc-3kwt later).
- A `bean:` URI component (`to: bean:x` stays `ComponentNotFound` — growing the
  closed lean component set would need an ADR-0064 amendment).
- The `camel-test` programmatic harness (same gap at `harness.rs:158-178`;
  follow-up if demanded).
- Assertion matchers on bean behavior (rc-3kwt territory).

## Acceptance criteria

- A test document whose route has a `bean:` step and a matching `beans:` entry
  loads and runs; downstream `mock:` expectations see the stubbed effect
  (`setBody` body, `echo` passthrough, `fail` stops propagation).
- Method allowlist honored: route method outside a declared `methods` list is
  a route-load document error (exit 2).
- All `beans:` validation failures (unknown kind, bad config, empty `methods`)
  are document errors (exit 2) with precise messages.
- `camel run` remains non-interfering: it never reads `beans:` (subprocess
  test banning the test-document filename, the invalid kind name `teleport`,
  and the `unknown variant` fragment, mirroring the `intercepts:` precedent).
- `intercepts:` + `beans:` compose in one document.
- Lean component set unchanged; no new crate; camel-core change limited to
  one minimal public accessor (`RouteDefinition::circuit_breaker_fallback()`,
  mirroring the existing `steps()` accessor) so the runner can collect bean
  calls from fallback sub-pipelines.

## Risk budget

Additive camel-cli surface only; wiring uses the existing
`BeanRegistry`/`CamelContextBuilder::beans()` API verbatim. Accepted risks:
spec-clause amendment wording must not open the door to WASM in lean boot;
`BeanProcessor` lifecycle (`on_start`/`on_stop`) defaults must be no-ops for
stubs; camel-core changes are bounded to exactly one public accessor
(`RouteDefinition::circuit_breaker_fallback()`, mirroring `steps()`). Out of
bounds: any other camel-core change, any lean-set growth, any plugin
loading, any network/filesystem stimulus.
