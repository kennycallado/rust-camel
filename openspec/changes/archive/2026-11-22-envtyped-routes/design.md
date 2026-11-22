# Design: envtyped-routes

## Problem shape

`${env:}` substitution at the source-text seam writes strings; typed positions
reject them. The fix pattern is already in the codebase (rc-3c67o family):
resolve with provenance, and after a FAILED typed parse, run a bounded
smallest-first subset probe whose oracle is the real parser. `.2` widens the
camel-dsl arm from `routes:`-only to the whole route document and extends it
to JSON.

## Architecture

Layer: **DSL/loader** (`camel-dsl` only). No changes to `camel-api`,
`camel-core`, `camel-config`, `camel-bundles`, or `camel-cli` sources (the CLI
openapi command benefits automatically through
`extract_rest_blocks_from_file_with_env`).

### 1. Probe generalization — one loop, two trees

`env_int_probe.rs` currently couples the subset-search loop to
`serde_yml::Value` (`try_coerced_subset` serializes with the YAML shim) and to
`Vec<RouteDefinition>` oracles. Generalize:

- `clean_integer`/`clean_boolean` return a tree-neutral `ProbeScalar`
  (`Int(i64)` / `UInt(u64)` / `Bool(bool)`). The clean-lexical rules and the
  SYNC notes (camel-config, camel-lint mirrors) are unchanged.
- A small internal trait `ProbeDoc` exposes the four tree operations the loop
  needs: parse text → tree, serialize tree → text, read a provenance path as
  `&str`, write a `ProbeScalar` at a provenance path. Two impls:
  `YamlDoc(serde_yml::Value)` (existing semantics, byte-for-byte the same
  coercion/serialization behavior) and `JsonDoc(serde_json::Value)` (serialize
  with `serde_json::to_string`, so coerced JSON stays JSON — this removes the
  blocker recorded on `interpolate_for_parse`: "the YAML-shim tree
  re-serializes to YAML, not JSON").
- The subset enumeration (cap 8, lexicographic smallest-first, fresh tree per
  attempt, skip-on-anomaly) stays ONE function, generic over `ProbeDoc` and
  over the oracle return type (`Vec<RouteDefinition>` for route parses,
  `Vec<RouteDslRest>` for the openapi extract). `parse_with_probe` keeps its
  current signature and delegates with `YamlDoc`; a new `parse_json_with_probe`
  delegates with `JsonDoc`. No second probe loop is forked.

### 2. Candidate set — widening with stay-out exclusion

`candidates()` becomes: every provenance path EXCEPT paths whose first segment
is the `templates` or `templated_routes` key, resolving to a clean-integer or
clean-boolean STRING leaf, in document order. The two exclusions are the
documented stay-out subtrees (see below); every other subtree (`rest:`, `mcp:`,
`routes:`) joins the candidate set.

**Why the exclusion is load-bearing (cap-budget argument).** The candidate
filter inspects leaf VALUES, not position types — a numeric-looking parameter
string under `templated_routes.parameters` would pass the value test. But a
coerced number can never repair a `BTreeMap<String, String>` position (a
string-typed field rejects a number), so such candidates are dead weight that
only consumes the shared cap. Widening without exclusion could flip a doc that
probes successfully today (eight `routes:` candidates) into over-cap failure
by adding numeric-looking template inputs to the budget — breaking the
compatibility promise. Excluding the stay-out subtrees keeps the candidate
budget exactly as it is for documents that parse today and admits only
subtrees whose coercion can genuinely repair the parse.

**Correctness argument under the widened oracle (minimum-cardinality).**
The probe oracle is the full typed document parse INCLUDING compilation
(`parse_yaml_for_probe` = deserialize + shape-check + compile). The
smallest-first search guarantees: the FIRST minimum-cardinality parsing
subset in document order wins. When the document has a unique required
repair set — the typed-failure case — every parsing subset must contain it
(any subset omitting a required coercion still fails), so that set is S_min
and the search returns exactly S_min. When multiple independent sufficient
subsets exist (e.g. two polymorphic positions each of whose coerced values
satisfies compilation), the search deterministically returns the
document-order-first minimum — coercion never extends beyond a
minimum-cardinality parsing subset, and never touches non-candidates.
Unsigned bounds, narrowing, and negatives stay enforced by the real parse.

**Compatibility promise, stated precisely.** Pass-1 behavior is unchanged:
every document that parses today still parses, with the same values (the
probe only ever runs after a failed first pass, and interpolation is
untouched). Repair outcomes split by class: (a) routes:-only probe repairs
are preserved — their candidate set and document order are identical under
the widened filter (no `templates`/`templated_routes` paths exist in them) —
EXCEPT when widened candidates from OTHER subtrees push the count over the
cap, in which case the first-pass error stands (the accepted boundary, e.g.
the eight-throttle fixture plus a numeric-looking `$schema` root token — a
string-typed position the candidate value-filter admits but whose coercion
can never repair; pinned, not hidden: the cap already produces the same
outcome today for over-cap documents, and the alternative
(schema-aware position filtering) would replace the real-parser-as-oracle
design with heuristics); (b) collision-guarded documents are EXCLUDED from
the preservation promise: a document whose provenance includes a stale path
— a substituted entry overwritten by a later mapping key — loses that path
to the insert-time collision guard, and a document that probe-loads today
ONLY through the stale path keeps the first-pass error. Reproduced case,
pinned: a routes:-only throttle mapping whose key `'${env:KEY:-max_requests}'`
substitutes onto `max_requests` with value `'${env:VALUE:-404}'`, followed by
a literal `max_requests: '404'` — today it probe-loads with one candidate
(the stale path coerces the surviving literal); after the change the guard
drops the stale path and the first-pass error stands, below the cap. The
guard is a correctness fix (a surviving literal must never coerce on a
token's provenance); the compat promise covers only documents whose
provenance is collision-free; (c) widened-scope repairs (rest:/mcp:
candidates)
turn prior hard failures into loads — that is this requirement's purpose,
not a compat break; string fields such as `host` are not repair examples —
interpolation already loads them and they are unchanged.

**Stay-out subtrees (documented + pinned):**

- **`templated_routes.parameters`** — `BTreeMap<String, String>`:
  string-typed by contract (template-substitution inputs). Excluded from
  candidates; pinned: a numeric-looking `${env:}` parameter value loads with
  the STRING value intact and consumes no cap budget.
- **Template bodies (`templates[].routes[]`)** — `Vec<serde_yml::Value>`:
  untyped at document parse by design; typing happens at MATERIALIZATION,
  which the document-parse probe never observes. Excluded from candidates.
  Typed template numerics flow through the existing typed-parameter machinery
  (`TemplateParamType::Number/Boolean` → whole-node typed substitution in the
  materializer): a template declaring a `number` parameter, instantiated with
  `${env:RC_TPL_DELAY:-500}`, materializes the typed value 500 — fixture
  through discovery. A template-body DIRECT `${env:}` numeric (no typed
  parameter) substitutes at document level to a string, passes the document
  parse, and keeps today's documented materialization limitation — pinned by
  a negative that REACHES materialization (the doc parse succeeds via a
  widened-probe repair elsewhere in the same document, then materialization
  of the direct-token template body fails with today's typed error).

The routes:-only behavior is unchanged except for the two pinned
exclusions: collision-guarded documents (a stale provenance path — a
substituted entry overwritten by a later mapping key — is dropped by the
insert-time collision guard and may lose a probe repair the document had
before; pinned below-cap in Testing) and documents whose widened candidate
count crosses the cap through candidates from other subtrees (the pinned
`$schema` over-cap boundary). Otherwise, for a document whose only
candidates are under `routes:`, the candidate filter result is identical
(`routes:` paths were never under the excluded keys), same search, same cap.

### 3. JSON provenance — two-pass, legacy-first

`env_interpolation.rs` gains `interpolate_json_source_with_provenance(raw,
lookup) -> Result<(String, Option<Vec<ProvenancePath>>), String>`: parse the
document as `serde_json::Value`, run the same tree walk (shared scalar
helpers `interpolate_string`/`has_env_token`/`is_whole_scalar_env_token`;
mirrored recursion arms — the two `Value` enums have no common tree type), and
serialize with `serde_json::to_string`. `ProvenanceSeg::Key/Index` is already
format-neutral. Unresolved variables error with the variable name.

**Two-pass shape (compatibility is why).** The legacy splice and the tree walk
disagree on escape handling: a JSON default containing `\n` decodes to a
newline AFTER splicing, but the tree path decodes first and the interpolation
sanitizer turns the control character into a space; compaction also shifts
error positions; and a substitution that breaks JSON syntax (an unescaped
quote inside a default) hard-fails the splice today while the tree walk would
silently escape it. So JSON documents keep the legacy splice as the FIRST
pass — byte-identical values, error text, and warnings for every document that
loads today. The repair pass runs only when the first pass fails AND the
legacy semantic baseline is intact:

1. the splice-substituted text must still parse as JSON AND carry no
   duplicate object keys (a substitution that broke syntax, or a duplicate
   key anywhere in the document, means no baseline — duplicate-key
   documents are never repaired because default `serde_json` maps are
   key-sorted and cannot preserve authored overwrite order; today's
   first-pass error stands, exactly today's collapse-on-duplicate parse
   outcome);
2. the splice-parsed tree and the tree-walk-parsed tree must be EQUAL
   (`serde_json::Value` equality) — any normalization drift (escape
   sanitization, compaction-induced value changes) means no repair;
3. the coercion subset applied on top must be nonempty — a repair pass with
   zero candidates never re-parses anything and the first-pass error stands.

The tree walk itself detects substitution-produced KEY collisions (a
substituted key equal to an existing sibling key — lookup-based detection,
independent of map iteration order) and drops the collided path plus its
descendants from provenance. Provenance path ORDER for JSON is the walk's
deterministic map-iteration order (key-sorted), not authored source order —
default `serde_json` maps sort keys, so authored order is not recoverable
without the crate-wide `preserve_order` feature (rejected: it would reorder
every JSON value the crate serializes). The probe's minimum-cardinality
guarantee does not depend on order; order only selects among equal-size
winning subsets, and the sorted order is deterministic. Pinned: an
order-sensitive tie test (two candidates where sorted and authored order
differ) and a post-substitution key-collision test.

The final failure returns the ORIGINAL first-pass error (no replay: the JSON
chain has no logging deserializer, and pass-1 already emitted today's output;
its compiler is not logging-free, so warning counts are pinned — see
Testing).

**Provenance collision guard — YAML.** YAML mapping keys can also collide
(substituted key matching a later literal, duplicate keys, same-value
overwrites). `serde_yml::Mapping` preserves insertion order, so the walk
detects insert-time collisions directly: an entry inserted over an existing
one marks the OVERWRITTEN entry's path (and all descendant paths) as collided
and drops them from the provenance set; a substituted key that overwrites an
earlier literal keeps provenance (the final leaf at that path IS the
substituted value). A collided path is never a candidate, regardless of
value — the same-value case (substituted `"404"` overwritten by literal
`"404"`) is pinned for the YAML walker; the JSON walker never reaches it
(duplicate-key documents are rejected from repair above).

### 4. Wiring points

- **`parse_routes_with_env_inner`** (yaml.rs): unchanged structurally — the
  widened candidate set flows through the existing call.
- **Discovery JSON arm** (`discovery.rs`): pass-1 stays the splice + existing
  parse (byte-identical, warnings included). On failure, repair = JSON tree
  walk with provenance, gated by the three baseline checks above (spliced
  text parses without duplicate keys; splice tree == tree-walk tree; nonempty
  coercion subset), then `parse_json_with_probe` with a QUIET oracle: the
  speculative compile must not repeat today's compile warnings, mirroring
  `parse_yaml_with_threshold_and_security_quiet` — a warnings-suppressed twin
  of the threshold/security parse for JSON. When the repair WINS, the winning
  repaired text is what the document's downstream consumers parse: the JSON
  template extractors (`parse_json_templates` / `parse_json_templated_routes`)
  deserialize the WHOLE `RouteDslRoutes` and would reject the still-string
  port if fed the original splice — they run against the repaired text (the
  YAML precedent is section-only extraction,
  `template/yaml.rs` TemplateSections; JSON repair instead repairs the
  document once and feeds the result to every consumer). On final failure the
  original first-pass error maps to `DiscoveryError::Json` as today.
  Templates and templated-routes JSON parses keep today's behavior.
- **New string seam** `parse_json_routes_with_env(raw, lookup)` in json.rs
  (sibling of `parse_routes_with_env`, same two-pass shape and
  `RoutesEnvError` surface) for direct callers and fixtures.
  `load_json_from_file` keeps its documented no-interpolation contract — the
  env seam for JSON route documents is discovery and the new string seam.
- **`extract_rest_blocks_from_file_with_env`** (yaml.rs): YAML arm —
  provenance interpolation + probe with a QUIET oracle
  (`serde_yml::from_str::<RouteDslRoutes>` → `.rest`, no logging), final
  failure replays once through the logging `extract_rest_blocks` (preserves
  today's error log) and returns the first-pass error annotated with the file
  path; JSON arm — legacy splice pass first (today's behavior), then the same
  quiet-probe repair, with its existing single error-log site firing only on
  final failure. Unresolved no-default placeholders keep the named-variable
  error (fires before any probe). The CLI caller (`camel openapi generate`)
  keeps its `&|_| None` default-only lookup unchanged.

## Testing strategy

- Unit (env_int_probe.rs): widened-scope positives/negatives through the real
  seam (`parse_routes_with_env`): rest.port default + explicit lookup,
  negative/leading-zero/overflow/embedded-token failures, templated_routes
  parameter stays string and consumes no cap budget, cap shared across
  `routes:`+`rest:` (nine combined from probed subtrees keeps the first-pass
  error, eight still searches), polymorphic compile-required coercion
  (`request_schema` boolean) coerces, the CAP-COMPATIBILITY BOUNDARY:
  the existing eight-throttle fixture plus a numeric-looking `$schema` root
  token moves from probe-success to the first-pass error (accepted boundary,
  pinned with the qualified promise wording), and the BELOW-CAP COLLISION
  PIN (YAML-ONLY): the reproduced stale-provenance throttle document
  (substituted key `'${env:KEY:-max_requests}'` onto `max_requests` with
  value `'${env:VALUE:-404}'`, followed by literal `max_requests: '404'`)
  keeps the first-pass error — the collision guard drops the stale path a
  document that probe-loads today relies on. JSON glue: same matrix MINUS
  the below-cap collision pin (YAML-only: JSON rejects duplicate-key
  documents before probing, so the stale-provenance regression cannot occur
  there) through `parse_json_routes_with_env`; JSON round-trip probe
  (coerced tree serializes as JSON, oracle is `serde_json`); two-pass
  baseline checks — an unescaped-quote substitution (splice breaks syntax)
  keeps today's first-pass error with no repair, and an `a\nb`-default +
  repairable-port combination keeps the first-pass error (splice/tree trees
  unequal — repair restricted to the legacy semantic baseline); the JSON
  duplicate-key rejection pin — a duplicate-key document is never repaired
  (no candidates consulted), first-pass error stands.
- env_interpolation.rs: collision-guard tests — YAML walker: an entry
  overwriting a substituted one drops the overwritten path (and descendants)
  from provenance, including the same-value collision (substituted `"404"`
  overwritten by literal `"404"` is never a candidate); a substituted key
  overwriting an earlier literal keeps provenance. JSON walker: a
  post-substitution key collision (substituted key equal to an existing
  sibling) drops the collided path + descendants; duplicate-key documents
  are rejected from repair (no candidates consulted); an order-sensitive tie
  test pins sorted-iteration-order determinism (sorted vs authored order
  differ, minimum subset resolves deterministically).
- yaml.rs: REPLACE `extract_rest_from_file_int_field_fails_type_mismatch`
  with `extract_rest_from_file_int_default_coerces`; keep the
  notanumber/leading-zero negatives and the named-variable test; add the JSON
  arm coercion + final-failure single-log test.
- Quietness fixture with exact counts (JSON repair): the NAMED
  `json_repair_quiet_idempotent_warning` fixture — a `set_body` candidate
  whose value is compile-valid both as string and coerced (so the retry
  actually reaches the compile stage), followed by a trailing
  `unmarshal: no-such-format` step whose LATE compile failure keeps the
  document failing (verified replacement: zero-throttle fails earlier at AST
  lowering — before the warning site — and would make the fixture vacuous),
  with the compile passing through the idempotent_consumer warning path
  (`compile.rs:1701-1713`): the legacy pass-1 warning count equals the direct
  splice-parse baseline (exactly ONE warning), each repair attempt adds
  exactly ZERO new error!/warn! events, and final failure adds nothing and
  returns the original first-pass error — asserted for BOTH the string and
  the numeric `set_body` candidate values, count assertions not best-effort.
- Discovery JSON loader pins: a clean `rest:`-block JSON document and a clean
  throttle-under-`routes` JSON document each load through discovery, proving
  the repaired-text consumption feeds every downstream consumer (routes,
  templates, templated specs).
- Stale pins flipped with the behavior: CLI openapi tests
  (`crates/camel-cli/src/commands/openapi.rs` — YAML port/status and JSON
  port failure expectations become coercion expectations asserting actual
  server URLs/statuses), discovery JSON numeric pin (`discovery.rs` test
  expecting the JSON typed failure becomes a coercion expectation), each
  asserting the named-variable error for unresolved no-default and exactly
  one final deserialize error log where the arm logs.
- Integration fixtures (`crates/camel-dsl/tests/`): loader-path documents —
  YAML `rest:` doc, YAML template document (typed-parameter numeric via
  `${env:}` through discovery materialization, plus the direct-body
  negative that reaches materialization), JSON route document — each
  positive plus a negative, using the existing `EnvGuard` lookup-injection
  pattern.

## ADRs / boundaries

- ADR-0026 (JSON is a full-DSL authoring format) — the JSON arm must reach
  parity, not a second-class splice.
- Crate purity: camel-dsl keeps its own probe mirror (SYNC notes in
  `camel-config/src/env_int_probe.rs` and the dsl module header); the TOML
  subset-search is NOT copied wholesale — camel-dsl's own search is reused.
- No new dependencies; no public API changes outside camel-dsl's own surface
  (one new pub string seam, one pub fn re-export).

## Risks

- Probe attempt growth for mixed rest+routes documents: bounded by the
  unchanged cap (8 combined candidates from probed subtrees only); beyond it
  the first-pass error stands.
- JSON number fidelity: probe coercion builds `serde_json::Number` from i64
  then u64 — the same precedence as the YAML shim; floats are never
  candidates (clean-lexical rule).
- Two-pass JSON: repair-pass compaction shifts error positions, but repair
  attempts are quiet and the returned error is always the original first-pass
  error — user-visible text is unchanged.
