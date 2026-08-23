# Proposal: declarative-intercepts

## Why

Stage A (merged, `0b0be06a`) delivered the route-interception primitive in
camel-core: `InterceptRules` with `SkipTo` (pre-resolution substitution) and
`DivertCopyTo` (WireTap-isolated copy, real outcome verbatim), frozen at boot.
Today that motor is reachable only from Rust code. The declarative test
surface (`*.test.yaml` via `camel test`) still has no way to use it, so
production routes that reference real components (`kafka:`, `http:`) remain
untestable in the unit tier and inline `to: mock:` remains the only pattern
(ADR-0064 §5 Stage B, bd rc-7f0n).

## What Changes

- `*.test.yaml` gains an optional `intercepts` block: a map from real
  endpoint URI to exactly one action — `skipTo: mock:<name>` (replace the
  send before component resolution) or `divertCopyTo: mock:<name>` (copy to
  the mock, real send continues).
- `crates/camel-cli/src/commands/test/document.rs`: new field on
  `TestDocument` (`deny_unknown_fields` still enforced), action objects
  validate exactly-one-of `skipTo`/`divertCopyTo`, rules are constructed via
  Stage A `InterceptRules::new` at parse time so invalid rules are document
  errors (exit 2).
- `crates/camel-cli/src/commands/test/runner.rs`: `boot_context` applies the
  document's rules through the Stage A builder surface before any route
  registration (freeze contract respected).
- `expects`, `settle`, exit codes, and `camel run` behavior are unchanged.
  Naming bridge (e_opus advisory L2-1): an intercept target `mock:orders`
  and an `expects` key `mock:orders` each independently resolve to the mock
  endpoint name `orders` (expects keys are normalized at parse; mock URIs
  resolve by endpoint path), so the two surfaces meet on the same endpoint.
- No camel-core changes, no new crate (ADR-0055), no lean-boot set change
  (ADR-0064 §2 creep rule).

## Acceptance Criteria

- A route referencing an **unregistered** component (`kafka:orders`) runs
  under `camel test` with `intercepts: {kafka:orders: {skipTo: mock:orders}}`
  and expectations on `mock:orders` pass (exit 0).
- `divertCopyTo` records the pre-send copy on the mock while the real
  endpoint (a lean-boot component such as `seda:`) still receives traffic.
- `divertCopyTo` on an unregistered real component fails at route load
  naming the component, reported as a document error (exit 2, the unchanged
  route-load failure class) — the Stage A contract surfacing, not a new
  failure mode.
- Invalid intercept documents (non-`mock:` target, `mock:` source URI,
  both/neither action key, unknown action field) exit 2 at parse time.
- All existing mock-testkit scenarios and exit-code behavior unchanged.

## Risk Budget

Low. Pure additive surface over triple-blessed Stage A primitives; the
runner change is one construction-path edit; the document change follows the
existing `deny_unknown_fields` discipline. Largest risk is spec drift in the
`mock-testkit` delta — mitigated by preserving every existing scenario
verbatim in the MODIFIED requirement.
