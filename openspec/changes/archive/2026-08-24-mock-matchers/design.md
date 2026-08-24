# Design: mock-matchers

## Approach

Extend the assertion engine where it already lives. `camel-component-mock`
gains two public enums — `BodyMatcher`, `HeaderMatcher` — each with a
fallible `matches(&Body|&header value)` predicate and a Display form that
names the matcher. Evaluation stays synchronous and after-the-fact: the mock
records exchanges, `try_assert_satisfied` compares. This honors the
component identity ruling (rc-i2qf): producers stay sinks; matchers never
simulate behavior.

The CLI's `ExpectSet` widens. `expects.bodies` list entries were
strings-only before, so their syntax is strict: a bare string is
`equals`; a map must carry exactly one recognized body matcher key
(`equals`, `regex`, `contains`, `startsWith`, `endsWith`, `exists`,
`jsonSubset`); anything else is a document error naming the field.
Header values (`expects.headers`, `expectReply.headers`) and
`expectReply.body` accepted arbitrary JSON before, so they use a dual
grammar: a value that is not a single-recognized-key matcher map stays a
literal `equals` by structural equality (header matcher keys: `equals`,
`regex`, `exists`); a sole-key matcher map is that matcher. The reserved
`predicate` key and a sole `jsonSubset` key on headers are rejected
everywhere they appear as the sole key. The parser validates eagerly —
regexes compile and `jsonSubset` must be an object at load time — so
malformed matchers fail with exit 2 before any route starts.

`jsonSubset` semantics: recursive deep-subset. Object fields in the
matcher must exist in the received JSON and match (scalar equality or
recursive subset); fields absent from the matcher are ignored. Arrays
compare exactly (length and order). Against `Body::Text`, the matcher
first parses the text as JSON; non-JSON text fails the matcher with a
"body is not JSON" message.

The reply path replaces its private `reply_body_eq` restatement with a
call to the public `BodyMatcher::matches`, removing the duplication debt
reply-capture left. The existing header-regex engine
(`expect_header_regex`) is reached through `HeaderMatcher::Regex` wiring
instead of staying stranded.

## Affected crates

- `camel-component-mock`: new public `BodyMatcher`/`HeaderMatcher` enums,
  setters `expect_body_matcher` / `expect_header_matcher`, evaluation in
  `assert.rs` beside `body_eq`, new `MockAssertionError` variants naming
  matcher + pattern + received body.
- `camel-cli`: `document.rs` parses and validates matchers (new
  `InvalidMatcher`-class errors, exit 2); `runner.rs` wires matchers to
  the new setters for mock endpoints and the reply path.

## Architecture boundaries

Components layer owns the vocabulary; the CLI (consumer) parses YAML and
maps to component setters — the same direction as `expect_count` today.
No camel-core change, no registry or query-plane handle (ADR-0002/0045
untouched), no async in the matcher path (the `Language`/`Predicate`
engine is deliberately not reused — wrong shape, and ADR-0046 forbids
porting foreign API shapes). Tier-agnostic reuse is satisfied: the future
integration tier (rc-kk69) depends on this crate for its receive
validation anyway. Matcher-mismatch failures are assertion failures
(exit 1); malformed matchers are parse errors (exit 2); the 2 > 1 > 0
precedence is unchanged.

## Alternatives considered

- Sigil strings (`regex:/…/`) — rejected: collides with legitimate bodies
  starting with `regex:`.
- `oneOf` serde discriminator — rejected: awkward with
  `deny_unknown_fields` and worse error text.
- Matchers in `camel-core` or a new `camel-testkit` crate — rejected:
  fragments the existing engine, drags matcher types into the runtime
  (hexarch smell). The mock crate is already the shared test surface.
- Reusing the async `Language` engine — deferred: exchange-scoped, async,
  registry-bound; a `predicate:` escape hatch stays possible later
  without grammar breakage thanks to the reserved key.

Single-phase change: one coherent slice, no milestone split.
