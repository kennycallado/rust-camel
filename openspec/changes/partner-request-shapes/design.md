# Design: partner-request-shapes

## Approach

Three layers, mirroring ADR-0072's grammar/algebra/observation split:

1. **Algebra** (`camel-matchers`): `RequestShape { method: Option<String>,
   path: Option<PathFilter>, query: Option<BTreeMap<String, String>>,
   body: Option<Expectation> }` — the same filter trio
   `RequestExpectation` already carries, plus a body expectation.
   `RequestExpectation` grows `requests: Option<Vec<RequestShape>>`
   (public struct, field addition is the sanctioned growth path; the
   crate-local constructors in `camel-integration-test` are the only
   literal constructors — verified). One pure function
   `request_shape_mismatch(shapes, projections) -> Option<ShapeMismatch>`:
   iterates shapes against the projected sequence positionally, returns
   the first mismatch with its index and failed aspect. Projection input:
   `(method, path_and_query, body: &Value)` — the byte-to-value step stays
   tier-side (purity law: no wire types in the algebra). `ShapeMismatch`
   is `#[non_exhaustive]` (ADR-0049).
2. **Grammar** (`camel-integration-test/src/document/validate.rs`):
   `partner_expectation_from_value` accepts `requests` as a list of
   entry maps. `requests` XOR (`count` | `atLeast` | `atMost`) — the
   `RowsExpectation` rows-XOR-bound precedent; with `requests` the parser
   synthesizes `CountBound::Exact(len)`. Entry keys reuse the existing
   field readers (`method`, `path`, `pathContains`, `pathMatches`,
   `query`) so errors stay field-naming; `body` parses through
   `expectation_from_value` (the shared `Expectation` dual grammar — bare
   value is `equals`). Unknown entry keys fail listing the expected set.
   Empty `requests` fails ("use `atMost: 0`"). No `bodySubset` verb: it
   would be a new matcher key outside the shared algebra.
3. **Observation** (`camel-integration-test/src/runner/partner_validate.rs`):
   the recorder already stores method, path, headers, and exact body
   bytes; the runner projects each RECORDED request as
   `(method, path_and_query, reply_bytes_value(body))` — the sql blob
   precedent: JSON when parseable, else lossy UTF-8 string. Judgement:
   the filtered sequence (outer filters applied, recorder order =
   body-completion order) must have exactly `requests.len()` elements,
   each satisfying its shape. Recorder is append-only, so a present
   mismatched element can never heal: polls fail fast on the first
   shape mismatch or on a filtered count above `len`; success settles at
   equality-with-all-shapes-matching; deadline expiry decides on the
   final snapshot — the existing `Exact` bound machinery unchanged.

Diagnostics (ADR-0051, both sides of every shape mismatch): mismatch
names the partner, the 1-based filtered index, the aspect (`method`,
`path`, `query`, `body`), expected versus observed. Aspect `method`:
plain text both sides. Aspect `path`: expected renders exactly as the
outer path-filter rule (`Exact` through `redact_wire_path`,
`Contains`/`Matches` kind-only elided); observed renders the recorded
wire path through the redactor. Aspect `query`: expected pairs render
`key=value` with secret-set keys redacted; observed renders the
redacted wire path. Aspect `body`: both sides render as message-validate
renders pattern and observed value today. Headers never render, either
side, any aspect. Diagnostics tests cover the redaction of both sides.

Demo scenario: partner scripted 500-then-200 on a route with a retry
policy; both `requests` entries carry the same bare body map (a literal
`equals`) — proving retry-identical projected body values. No sleeps
(`lint-test-sleep`); the retry fixture family already exists in the
partner verification tests.

## Affected crates

- `camel-matchers`: `RequestShape`, `requests` field, shape judgment
  function, unit tests. Pure, additive, no new dependencies.
- `camel-integration-test`: grammar entries + load errors, runner
  projection + judgement + diagnostics, `http_partner_test.rs` /
  `partner_validate_test.rs` unit tests, one `.test.yaml` scenario
  fixture + harness test.

## Architecture boundaries

Testing-tiers boundary only; no Runtime/DSL/Components surface moves.
ADR-0072 purity (types + pure functions, deps unchanged), ADR-0069
scenario vocabulary (single-key action maps, field-naming load errors),
ADR-0051 redaction (positive secret rule; no new raw payload surfaces
in diagnostics beyond the existing message-validate body rule).
One carve-out outside the two test-support crates: the struct-literal
`PartnerExpectation { .. }` at
`crates/camel-cli/src/commands/test/scenario_tests.rs:333` (the only
external literal, grep-verified) gains `requests: None` — a mechanical
compile fix forced by the field addition, no behavior change in
camel-cli.

## Alternatives considered

- **`requests` composing with count bounds** (bd's literal
  `{count: 2, requests: [...]}`): rejected — redundant and ambiguous
  under `atLeast`; the rows-XOR-bound precedent is the settled pattern.
- **Indexing the FULL recorded sequence**: rejected — unrelated traffic
  on the same partner key breaks positional asserts; filtered sequence
  is the deterministic subject.
- **`bodySubset` verb**: rejected — a new matcher key outside the shared
  algebra violates ADR-0072; `body: {jsonSubset: ...}` composes from
  the existing verbs.
- **Header asserts now**: rejected — no header-secret redaction rule
  exists; deferred with a bd (ADR-0051 extension required first).
