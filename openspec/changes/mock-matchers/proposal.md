# Proposal: mock-matchers

## Why

Body and header expectations in `*.test.yaml` are exact-match only. The mock
component already ships a tested header-regex engine
(`expect_header_regex`, `assert.rs`) that the CLI never wires — stranded
infrastructure. Tests that produce nondeterministic content (ids, timestamps,
trace ids) cannot be asserted today without weakening to `count`-only checks.
`bd rc-3kwt` (epic rc-7roi, last feature dep) asks for regex/predicate
matchers; the reply-capture change pinned exact-match deliberately so this
change lands once, in one vocabulary.

## What Changes

- New public matcher vocabulary in `camel-component-mock`:
  `BodyMatcher` and `HeaderMatcher` enums with a `matches` predicate.
  v1 set: `equals` (default), `regex`, `contains`, `startsWith`,
  `endsWith`, `exists`, `jsonSubset` (bodies); `equals`, `regex`,
  `exists` (headers). The `regex` crate is already a dependency
  (linear-time, no catastrophic backtracking).
- Test-document syntax: `expects.bodies` entries were strings-only and
  gain strict matcher maps — `- regex: "^order-[0-9]+$"`. The
  dual-grammar positions (`expects.headers`, `expectReply.headers`,
  `expectReply.body`) keep every literal value as structural `equals`
  while a sole-key matcher map selects a matcher —
  `X-Trace: { regex: "^[a-f0-9]{8}$" }`. A sole `predicate:` key errors
  "not supported in v1" (grammar stays stable for a future language hook).
- Compatibility note: the only old documents that change meaning are
  JSON objects whose sole key happens to be a matcher key, in the
  dual-grammar positions (`expectReply.body`, `expects.headers` values,
  `expectReply.headers` values) — e.g. `body: {equals: "x"}` meaning
  literal JSON equality of that object; they migrate by wrapping —
  `body: {equals: {equals: "x"}}`. `expects.bodies` entries were
  strings-only before, so matcher maps there are pure additions. All
  other existing documents parse and behave identically.
- CLI parses and validates matchers at document load (bad regex or
  non-object `jsonSubset` fail with exit 2 naming the field); the runner
  wires them through new component setters and the reply path calls the
  public matcher instead of its private `reply_body_eq` copy.
- Assertion failures name the matcher kind, its pattern, and the received
  body; exit taxonomy unchanged (parse error 2, assertion failure 1).

Excluded: jsonpath/xpath matchers, script predicates (rhai/js/wasm),
numeric/size comparators, variable interpolation, changing the async
`Language` trait, any producer-side behavior in the mock component
(rc-i2qf rejection stands — matchers are assertion-side only).

## Acceptance criteria

- Existing `.test.yaml` documents parse and behave identically, except
  the documented sole-matcher-key wrap migration in dual-grammar
  positions.
- Each v1 matcher is enforced on mock endpoints and on `expectReply`,
  with ordered-body semantics preserved; failures render matcher + pattern
  + received body and exit 1.
- Malformed matchers fail at parse with exit 2: an unrecognized or
  multi-key map in strict `expects.bodies` entries, an invalid regex, a
  non-object `jsonSubset`, `jsonSubset` on a header, or a sole
  `predicate:` key.
- `jsonSubset` matches recursively over objects; arrays compare exactly.
- The public enums live in `camel-component-mock`; `camel-cli` consumes
  them; no new crate, no camel-core dependency.

## Risk budget

Acceptable: additive public API on the mock component; wider `ExpectSet`
types in the CLI. Out of bounds: mock producer behavior changes, async
evaluation, exit-code taxonomy changes, breaking existing documents.
