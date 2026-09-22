# Design — sedamatch

## Context

The seda crate owns the exclusion discriminator
`is_no_active_consumers_gate` (rc-tgaxf): the no-active-consumers gate must
fail fast because a retry replays already-executed route steps (rc-ucemm).
The discriminator currently tests two SUBSTRINGS against every
`EndpointCreationFailed` payload. Foreign endpoint-creation failures that
carry the wording collide with the test and lose their retry window.

`CamelError::EndpointCreationFailed(String)` is a shared camel-api variant,
so the variant alone cannot separate the gate from foreign failures. A first
draft proposed exact-shape message parsing; the spec blessing rejected it:
type information re-derived from bytes is text sniffing. The discriminator
must survive inside the error — and it must not be extractable from a
genuine gate error and replayed into a fabricated one.

Doctrine: rc-fr20u (variant carries classification), retryclass 11be1863
(taxonomy matching replaces text sniffing, bd rc-fr20u), rediserr db512039
(typed markers preserved in the source chain, probed by downcast, bd
rc-tielu); this change is bd rc-3px7o.

## Chosen Design — opaque typed provenance in the source chain

### camel-api: opaque source handle + additive variant

Forgery analysis: a source field of `Arc<dyn Error>` (the
ProcessorErrorWithSource shape) is reachable through `source()` as the
std `Error for Arc<T>` wrapper, which downcasts to `Arc` and CLONES —
anyone holding a genuine marker can replay it into a fabricated error.
To close that, the new variant's source field is an opaque handle:

```rust
/// Owned source for source-preserving error variants. The inner handle
/// is private: `Error::source()` exposes only the pointee as `&dyn
/// Error`, and no public `Clone` exists, so callers can neither clone
/// the handle nor name concrete cause types they did not construct.
/// Provenance cannot be extracted and replayed (short of `unsafe`).
#[derive(Debug)]
pub struct OpaqueErrorSource(Arc<dyn std::error::Error + Send + Sync>);

impl OpaqueErrorSource {
    pub fn new(source: Arc<dyn std::error::Error + Send + Sync>) -> Self;
}
impl fmt::Display for OpaqueErrorSource { /* delegates to the pointee */ }
impl std::error::Error for OpaqueErrorSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())   // pointee directly — no Arc wrapper hop
    }
}
```

`CamelError` drops `derive(Clone)` for a manual `impl Clone` in camel-api:
each variant clones normally; the source-preserving variant arc-clones the
private inner through crate-private access. Semantics unchanged — a cloned
gate error still carries its marker — but external code cannot clone the
handle (no public Clone on `OpaqueErrorSource`), cannot reach the `Arc`
(private field), and cannot name the marker type (crate-private in seda).

```rust
/// Like `EndpointCreationFailed` but preserves the source error chain
/// for downstream inspection (e.g. typed gate-rejection classification).
#[error("Endpoint creation failed: {0}")]
EndpointCreationFailedWithSource(String, #[source] OpaqueErrorSource),
```

Mirrors `ProcessorErrorWithSource` on every alias axis:

- `variant_name()` → `"EndpointCreationFailed"` (doTry catch-by-variant
  unchanged).
- `classify()` → `"endpoint"` (metrics error family unchanged).
- Top-level `CamelError` Display is identical to the plain variant.
  Rendering contract, precisely: top-level Display is unchanged, while
  chain-aware diagnostics (renderers that walk `source()`) gain the
  marker's explicitly non-canonical source text — an intended, documented
  addition, not drift.

Spec deltas: new `error-taxonomy` capability (opaque-source contract +
variant aliases) and `dsl` capability MODIFIED (kind grouping — below).

### camel-component-seda (typed rejection, single constructor)

```rust
/// Typed provenance marker for the no-active-consumers gate rejection.
/// Crate-private: foreign code cannot construct or name it, so gate
/// provenance cannot be forged through the public source-carrying
/// variant.
#[derive(Debug, PartialEq, Eq)]
enum NoActiveConsumersGate { Single, Fanout }

// Display is NON-canonical diagnostic text, deliberately different from
// the gate message: "seda no-active-consumers gate rejection (single
// mode)" / "... (fanout mode)". Only the OUTER variant detail carries the
// canonical wording; marker Display renders in source-chain diagnostics
// only and never participates in classification.
```

- `NoActiveConsumersGate::rejection(&self, endpoint_name) -> CamelError`
  (crate-private) is the ONLY constructor of gate errors:
  `EndpointCreationFailedWithSource(detail, OpaqueErrorSource::new(
  Arc::new(self.clone())))` where `detail` (built by `fn detail(&self,
  endpoint_name)`) is byte-identical to today's wording
  (`SEDA endpoint '{name}' has no active consumers` / `... subscribers`).
- The three gate sites (single pre-enqueue, fanout pre-enqueue mode match,
  fanout subscriber-list check) route through it. Each site calls a
  dedicated crate-private fn that delegates to the constructor —
  `single_mode_gate_rejection(name)`, `fanout_preenqueue_gate_rejection(
  name)`, `fanout_subscriber_list_gate_rejection(name)` — so every site's
  construction path is deterministically unit-testable (the test executes
  the exact function the production branch calls and asserts its output
  byte-exactly).
- `is_no_active_consumers_gate(err)` matches the new variant and walks the
  source chain for `NoActiveConsumersGate` by downcast: start at the
  variant's `OpaqueErrorSource` pointee; at each hop unwrap the std
  `Arc<dyn Error>` wrapper if present (rediserr `unwrap_arc_dyn_error`
  precedent — std 1.98 resolves Arc-carrying sources through
  `Error for Arc<T>`, so direct `downcast_ref::<T>()` sees the wrapper),
  probe, then follow `source()`. The walk is bounded at 8 hops
  (`MAX_SOURCE_HOPS`, aligned with rediserr): a marker found at hop 1..=8
  classifies; deeper markers do not; the cap also bounds cyclic chains.
- `is_direct_startup_race(err)` = plain `EndpointCreationFailed` OR new
  variant whose bounded walk finds no marker. Text plays no part.

### DSL kind grouping (authorized exception)

camel-dsl `exception_kind_matches` (`compile.rs`) maps the kind value
`"EndpointCreationFailed"` to the plain variant only. The canonical dsl
spec requires variant-exact matching (`ProcessorError` does NOT match
`ProcessorErrorWithSource`). Our delta MODIFIES the vocabulary requirement:
`"EndpointCreationFailed"` denotes the endpoint-creation failure FAMILY and
matches both variants — an intentional, documented exception because
endpoint-creation catch clauses predate the variant split and must not
silently stop firing. No new kind value is introduced; unknown kind
`"EndpointCreationFailedWithSource"` stays rejected. The vocabulary guard
test (`test_exception_kind_vocabulary_classification_guard`) classifies the
new variant as grouped.

### Matcher sweep (no-drift rule)

Verified by grep during spec drafting: the ONLY non-test matcher of plain
`EndpointCreationFailed` is the camel-dsl kind matcher above. The
`tls_source.rs` and `camel-component-exec/endpoint.rs` occurrences are
test-side assertions (their errors stay plain — still valid). Rule: any
future match meaning "an endpoint-creation failure" includes the new
variant; narrower meanings get case-by-case treatment with justification.

### Manual Clone for CamelError (mechanical note)

`derive(Clone)` is replaced by a manual impl enumerating variants; the
opaque-source variant arc-clones its inner through crate-private access.
Adding a future variant without extending the manual Clone fails to
compile, same exhaustive-coverage discipline as `variant_name()`.
Exhaustiveness cannot detect an arm mapped to the WRONG variant, so the
existing `all_error_samples()` inventory (error.rs) backs an executable
regression gate: for every sample, clone and assert identical
`variant_name()`, `classify()`, and Display
(`clone_preserves_variant_identity_for_all_error_samples`).
(`derive(Debug)` stays — `OpaqueErrorSource` implements Debug by
delegating to the pointee.)

Opacity is pinned executably, not by comment: `compile_fail` doctests on
`OpaqueErrorSource` prove external code cannot clone the handle and
cannot destructure the private inner.

## Alternatives Rejected

1. Exact-shape parsing of the canonical message — rejected at spec
   blessing: type re-derived from bytes is text sniffing; byte-exact
   foreign imitation stays misclassified.
2. Dedicated gate-specific camel-api variant — camel-api must not grow
   component-specific semantics; the generic WithSource form keeps the
   taxonomy component-neutral.
3. Marker text appended to the detail — drifts observable wording; still
   text-based.
4. Public marker type + public constructor — rejected: foreign code could
   forge gate provenance. Marker and constructor stay crate-private;
   downstream tests obtain gate errors behaviorally.
5. Distinct DSL kind value for the new variant — rejected in favor of
   family grouping: existing catch clauses keep firing; the dsl delta
   documents the exception to the variant-exact precedent.
6. Source field shaped as plain `Arc<dyn Error>` (ProcessorErrorWithSource
   shape) — rejected at re-bless: the wrapper downcasts to `Arc` and
   clones, so a held marker can be replayed into a fabricated error.
   `OpaqueErrorSource` (private inner, no public Clone, pointee-only
   `source()`) closes extraction-replay at the type level. Residual: code
   that already HOLDS a genuine gate error can destructure the enum and
   MOVE the `OpaqueErrorSource` handle into a same-variant error with an
   altered detail — classification follows the handle (single-use,
   requires holding a genuine gate error first). Same trust class as any
   owned error value; documented rather than closed.

## Affected Crates and Boundaries

- crates/camel-api (Core): `OpaqueErrorSource`, one additive variant,
  manual `Clone` for `CamelError`, alias entries, tests.
- crates/components/camel-component-seda (Component): typed rejection,
  constructor, three-site rewire, bounded source-chain classification,
  unit tests.
- crates/camel-cli (CLI): no code change; classification tests extend with
  adversarial cases (gate errors obtained behaviorally — the marker is
  crate-private).
- crates/camel-dsl (DSL): kind matcher arm extension + guard-table entry.

Boundary discipline: seda owns wording and classification (rc-fr20u — no
caller-side Display sniffing); camel-api stays component-neutral.

## Edge Cases

- Foreign `EndpointCreationFailedWithSource` with a non-gate source:
  retryable (correct — it is not the gate).
- Byte-exact foreign imitation as a PLAIN `EndpointCreationFailed`:
  retryable — text never classifies.
- Marker at hop depth 1..=8 (including behind foreign wrappers):
  classifies. Depth > 8: does not classify (bounded walk, tested at the
  boundary); the cap bounds cyclic chains.
- Cloned gate error: still classifies (manual Clone arc-clones the
  marker's inner Arc — one more handle to the same marker).
- Other seda `EndpointCreationFailed` wordings (queue-full, enqueue/fanout
  timeout, multipleConsumers config, passthrough creation failures) remain
  plain-variant and retryable — rc-ucemm residual unchanged.
- Empty endpoint name cannot occur (`SedaConfig::from_uri` rejects
  trimmed-empty names — confirmed at spec blessing); the typed rejection
  needs no name-shape rule.

## Test Strategy (executable)

### crates/camel-api/src/error.rs (existing test module)

- `opaque_error_source_exposes_only_pointee` —
  ARRANGE: `let src = OpaqueErrorSource::new(Arc::new(SomeErr));`
  ACT/ASSERT: `src.source().unwrap().downcast_ref::<SomeErr>().is_some()`
  (pointee reachable, no Arc wrapper hop).
- `opaque_error_source_is_not_cloneable` — `compile_fail` doctest on
  `OpaqueErrorSource`: ARRANGE: doc example constructing a source and
  calling `.clone()` on it; ASSERT: does not compile (no public `Clone`).
- `opaque_error_source_inner_is_not_destructurable` — `compile_fail`
  doctest: doc example matching `OpaqueErrorSource(inner) = ...` from an
  external position; ASSERT: does not compile (private field).
- `endpoint_creation_failed_with_source_aliases_to_plain` —
  ARRANGE: `let e = CamelError::EndpointCreationFailedWithSource("d".into(),
  OpaqueErrorSource::new(Arc::new(SomeErr)));` ACT: `e.variant_name()`,
  `e.classify()`, `e.to_string()`; ASSERT: `"EndpointCreationFailed"`,
  `"endpoint"`, `"Endpoint creation failed: d"`.
- `clone_preserves_variant_identity_for_all_error_samples` —
  ARRANGE: `for e in all_error_samples()` (existing inventory) plus the
  new variant sample; ACT: `let c = e.clone();` compare
  `c.variant_name()`, `c.classify()`, `c.to_string()` against the
  original; ASSERT: equal for every sample (manual-Clone regression
  gate).
- `camel_error_clone_preserves_source_provenance` —
  ARRANGE: gate-shaped error `e` (typed source); ACT: `let c = e.clone();`
  then walk `c.source()`; ASSERT: marker still reachable (the manual
  Clone arc-clones).

### crates/components/camel-component-seda/src/lib.rs (tests)

- `gate_rejection_single_round_trip` — ARRANGE:
  `let e = single_mode_gate_rejection("q")` (crate-private); ACT:
  `is_no_active_consumers_gate(&e)`, `is_direct_startup_race(&e)`,
  `e.to_string()`; ASSERT: true, false, and top-level Display is
  `Endpoint creation failed: SEDA endpoint 'q' has no active consumers`.
- `gate_rejection_fanout_round_trip` — ARRANGE:
  `fanout_preenqueue_gate_rejection("q")`; ACT: same probes; ASSERT:
  true, false, Display `Endpoint creation failed: SEDA endpoint 'q' has
  no active subscribers`.
- `gate_rejection_nested_source_chain_classifies_within_bound` —
  ARRANGE: marker behind 2 foreign wrapper errors carried in
  `EndpointCreationFailedWithSource`; ACT: classifiers; ASSERT: gate
  true, race false.
- `gate_rejection_at_hop_limit_classifies` — ARRANGE: marker at hop
  depth exactly 8; ACT/ASSERT: gate true, race false.
- `gate_rejection_beyond_hop_limit_stays_retryable` — ARRANGE: marker at
  hop depth 9; ACT/ASSERT: gate false, race true.
- `marker_display_is_non_canonical` — ARRANGE: gate error source pointee
  Display; ACT/ASSERT: equals the documented diagnostic text
  (`seda no-active-consumers gate rejection (single mode)` / `... (fanout
  mode)`); does NOT equal either canonical gate message.
- `foreign_text_never_classifies_gate` (table) — messages: kafka-style
  containing "has no active consumers"; containing
  "has no active subscribers"; byte-exact single canonical
  (`SEDA endpoint 'q' has no active consumers`); byte-exact fanout
  canonical (`SEDA endpoint 'q' has no active subscribers`);
  prefix-decorated; suffix-decorated; case-modified; empty-name canonical
  — each as PLAIN `EndpointCreationFailed`; ASSERT per row:
  `is_no_active_consumers_gate == false` AND
  `is_direct_startup_race == true`.
- `typed_non_gate_source_stays_retryable` —
  `EndpointCreationFailedWithSource` with a foreign source type; ASSERT:
  gate false, race true.
- `other_seda_wordings_stay_retryable` (table) — queue-full
  (`SEDA queue 'q' is full (size=1)`), enqueue timeout, fanout timeout,
  multipleConsumers wordings; ASSERT per row: gate false, race true.
- Deterministic site tests — each executes the exact function the
  production branch calls and asserts its output byte-exactly:
  - `single_gate_site_detail_byte_exact` — ARRANGE/ACT:
    `single_mode_gate_rejection("site-q")`; ASSERT: top-level Display
    `Endpoint creation failed: SEDA endpoint 'site-q' has no active
    consumers`, gate classification true.
  - `fanout_preenqueue_gate_site_detail_byte_exact` — ARRANGE/ACT:
    `fanout_preenqueue_gate_rejection("site-q")`; ASSERT: Display
    `Endpoint creation failed: SEDA endpoint 'site-q' has no active
    subscribers`, gate classification true.
  - `fanout_subscriber_list_gate_site_detail_byte_exact` —
    ARRANGE/ACT: `fanout_subscriber_list_gate_rejection("site-q")`;
    ASSERT: Display `Endpoint creation failed: SEDA endpoint 'site-q'
    has no active subscribers`, gate classification true.
- Behavioral wording tests (replace the current
  `.contains("no active consumers")` assertions with byte-exact
  equality): the existing single-mode and fanout end-to-end producer
  tests assert the captured error's detail equals the exact canonical
  string for the endpoint under test.

### crates/camel-cli/src/commands/job/startup_retry_classification_tests.rs

- `foreign_byte_exact_single_imitation_stays_retryable` — ARRANGE: plain
  `EndpointCreationFailed("SEDA endpoint 'q' has no active consumers")`;
  ACT: `is_retryable_startup_failure(&e)`; ASSERT: true.
- `foreign_byte_exact_fanout_imitation_stays_retryable` — ARRANGE: plain
  `EndpointCreationFailed("SEDA endpoint 'q' has no active subscribers")`;
  ACT/ASSERT: retryable true.
- `foreign_wording_collision_stays_retryable` — ARRANGE: plain
  `EndpointCreationFailed("kafka topic 'orders' has no active consumers
  (broker=1)")`; ACT/ASSERT: retryable true.
- `typed_non_gate_source_stays_retryable` — ARRANGE:
  `EndpointCreationFailedWithSource` with a foreign source; ACT/ASSERT:
  retryable true.
- `behavioral_single_gate_fails_fast_classification` — ARRANGE:
  SedaComponent single-mode endpoint with no consumer; ACT: capture the
  producer error behaviorally (as `startup_retry_pipeline_tests.rs`
  does); ASSERT: `!is_retryable_startup_failure(&err)`.
- `behavioral_fanout_gate_fails_fast_classification` — ARRANGE: fanout
  endpoint, no subscriber; ACT/ASSERT: `!is_retryable_startup_failure`.
- Existing pipeline tests `gate_fails_fast_with_exactly_one_side_effect`
  and `fanout_gate_fails_fast_with_exactly_one_side_effect`
  (`startup_retry_pipeline_tests.rs`) keep passing with ZERO
  modifications.

### crates/camel-dsl/src/compile.rs (tests)

- `endpoint_creation_failed_kind_matches_both_variants` — ARRANGE: plain
  and source-preserving endpoint-creation errors; ACT:
  `exception_kind_matches("EndpointCreationFailed", &err)` for each;
  ASSERT: true for both.
- `endpoint_creation_failed_with_source_kind_value_rejected` — ARRANGE:
  route with `on_exceptions kind: "EndpointCreationFailedWithSource"`;
  ACT: compile; ASSERT: unknown-kind error listing supported kinds.
- `test_exception_kind_vocabulary_classification_guard` — extend the
  table entry classifying the new variant as grouped under
  `EndpointCreationFailed`.

No phases — single delivery group.
