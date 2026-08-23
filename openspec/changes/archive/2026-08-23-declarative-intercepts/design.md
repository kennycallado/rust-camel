# Design: declarative-intercepts

Stage B of ADR-0064 §5 (bd rc-7f0n). Consumes the Stage A camel-core
primitive (`openspec/specs/route-interception/spec.md`) from the declarative
test surface. No camel-core code changes.

## D1 — Document shape

```yaml
intercepts:
  kafka:orders:    { skipTo: mock:orders }
  seda:audit:      { divertCopyTo: mock:audit }
```

Map keyed by **real** endpoint URI (any scheme except `mock:`), value is an
action object with exactly one of `skipTo` / `divertCopyTo`, both values
full `mock:` URIs.

**URI boundary semantics.** Source keys receive no trimming or
normalization; query parameters participate in Stage A exact matching, so
`kafka:orders` and `kafka:orders?x=1` are distinct rules. An empty source
URI is a document error. Targets must be valid `mock:` URIs with a
non-empty endpoint path; query parameters are allowed and ignored for
lookup — the resolved endpoint path (not the raw text after `mock:`) drives
which mock endpoint receives exchanges.

Determinism: `BTreeMap` iteration order feeds `Vec<InterceptRule>`
construction. Map keys are distinct by construction, so runtime first-match
is unaffected; the ordering is observable only in diagnostics (which
invalid rule index reports first, per Stage A rule-indexed errors).

**Ownership.** `parse_test_document` constructs and validates the
`InterceptRules` eagerly (like `settle_parsed`) and stores it on the
document; parse-time failures are exit 2, preserving the mock-testkit
exit-code contract. The runner only applies the stored rules.

`TestDocument` (camel-cli `document.rs`) gains
`intercepts: Option<BTreeMap<String, InterceptActionDoc>>`.
`InterceptActionDoc` is `deny_unknown_fields` + custom validation:
- both keys present → document error; neither → document error;
- empty or `mock:`-prefixed source URI → document error naming the rule;
- targets validated by delegating to Stage A `InterceptRules::new` — its
  `CamelError::Config` message (rule index + offending URI) is surfaced
  verbatim in the document error.

Parsing constructs the `InterceptRules` eagerly (like `settle_parsed`) and
stores it on the document; parse-time failures are exit 2, preserving the
mock-testkit exit-code contract.

## D2 — Runner application

`boot_context(intercept: Option<InterceptRules>)` passes rules through the
Stage A builder surface (`with_intercept_rules`) before `.build()` and
before any component registration or route load. The Stage A freeze (first
successful route registration or start) is therefore respected by
construction. Fallback: `set_intercept_rules` exists but is not used —
builder-path is the single construction point.

## D3 — Semantics surfaced (inherited from Stage A, documented not re-specified)

- `skipTo` substitutes **before component resolution**: the real component
  never needs to exist. `kafka:`/`http:` routes run in the lean boot
  {direct, log, mock, seda, timer} unchanged (ADR-0064 §2).
- `divertCopyTo` composes the resolved real producer: the real component
  **must** be registered (lean set). Diverting an unregistered URI fails at
  route compile with the Stage A-enriched `ComponentNotFound`, surfacing as
  a route-load document error (exit 2, the unchanged failure class) —
  inherent to WireTap-exact semantics, not a Stage B limitation.
- Naming bridge (e_opus L2-1): `expects` keys are normalized to bare
  endpoint names at parse; intercept targets are `mock:` URIs resolved by
  endpoint path. `skipTo: mock:orders` and `expects: {mock:orders: …}` meet
  on endpoint `orders` — two resolutions, one endpoint.
- seda send-side works via the pinned §6 carve-out; its fences
  (consumer-side, fanout-partial, post-queue) remain fenced and are not
  re-opened here.

## D4 — Boundaries

- Crates touched: `camel-cli` only (+ docs). No new crate (ADR-0055); no
  RuntimeBus/QueryBus traffic (ADR-0002/0045) — interception is data-plane.
- `camel run` never reads `intercepts` (non-interference requirement).
- rc-3kwt (matchers) stays separate; no matcher surface here.

## D5 — Tests

Unit tests in `document.rs` (parse/validation matrix). Runner-level
integration tests in `crates/camel-cli/tests/` following the existing
`test_runner.rs` harness: skip-to-unregistered happy path, divert happy path
(seda real), divert-unregistered failure, naming-bridge assertion, exit-2
matrix. Docs: testing guide + mock-testkit spec delta stay in sync.
