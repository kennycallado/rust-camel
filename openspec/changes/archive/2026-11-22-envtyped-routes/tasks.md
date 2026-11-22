# Tasks: envtyped-routes

Single-phase change: all tasks belong to one delivery group (design.md has no
`## Phases` section — the whole change is one deliverable in one crate plus
its cli/doc surface). Tasks are ordered by dependency: probe core first, then
the two interpolation guards, then the JSON seam, then the wiring points, then
fixtures and docs.

Conventions for every task: `cargo fmt --check --all` and
`cargo clippy -p camel-dsl --all-targets -- -D warnings` must pass before
reporting done; new code follows ADR-0091 style norms (module-qualified calls,
no `unwrap()`, `expect("reason")` only); tests use injected lookups (never the
process environment) and the existing `EnvGuard`/lookup patterns.

## Task 1.1 — probe core: ProbeScalar/ProbeDoc/run_probe + widened candidates

Files:
- `crates/camel-dsl/src/env_int_probe.rs` (modified)

Steps:
1. Add `pub(crate) enum ProbeScalar { Int(i64), UInt(u64), Bool(bool) }`.
   Change `clean_integer` and `clean_boolean` to return
   `Option<ProbeScalar>` (lexical rules unchanged: `-?(0|[1-9][0-9]*)` with
   i64-then-u64 parse; exactly `true`/`false`). Keep the SYNC comments
   (camel-config, camel-lint mirrors) — those crates are NOT touched.
2. Add `pub(crate) trait ProbeDoc: Sized` with four operations:
   `fn from_text(text: &str) -> Option<Self>`,
   `fn to_text(&self) -> Option<String>`,
   `fn string_leaf_at(&self, path: &[ProvenanceSeg]) -> Option<&str>`,
   `fn coerce_leaf_at(&mut self, path: &[ProvenanceSeg], value: &ProbeScalar) -> bool`.
3. Implement `ProbeDoc` for `YamlDoc(serde_yml::Value)`:
   `serde_yml::from_str` / `serde_yml::to_string`; coercion maps
   `Int`/`UInt` to `Value::Number` (i64 first, then u64) and `Bool` to
   `Value::Bool`. This must preserve today's coercion/serialization behavior
   byte-for-byte.
4. Implement `ProbeDoc` for `JsonDoc(serde_json::Value)`:
   `serde_json::from_str` / `serde_json::to_string` (JSON stays JSON — this
   is what removes the `interpolate_for_parse` re-serialization blocker);
   coercion mirrors the YAML mapping with `serde_json::Number::from`.
5. Rewrite `candidates()`: drop the `routes`-first-segment filter; EXCLUDE
   any provenance path whose first segment is `Key("templates")` or
   `Key("templated_routes")` (stay-out subtrees); accept a path when
   `string_leaf_at` yields a `&str` that `clean_integer` or `clean_boolean`
   accepts; preserve document order (provenance order).
6. Rewrite the subset search as one generic function
   `fn run_probe<T: ProbeDoc, R>(doc_text: &str, provenance: &[ProvenancePath], parse: impl Fn(&str) -> Result<R, CamelError>) -> Result<R, CamelError>`:
   same smallest-first lexicographic enumeration, same `MAX_PROBE_CANDIDATES`
   cap (8), fresh tree per attempt, skip-on-anomaly, first-pass error on
   exhaustion. The oracle return type is generic (`Vec<RouteDefinition>` for
   route parses, `Vec<RouteDslRest>` for the openapi extract in task 1.6).
7. `parse_with_probe` keeps its exact current signature and delegates to
   `run_probe::<YamlDoc, _>`; add
   `pub(crate) fn parse_json_with_probe` delegating to `run_probe::<JsonDoc, _>`.
   No second subset loop exists anywhere.
8. Update the module doc: minimum-cardinality argument (the search returns
   the FIRST minimum-cardinality parsing subset in document order; unique
   repair set ⇒ exactly S_min; multiple sufficient subsets ⇒ deterministic
   document-order-first minimum), stay-out exclusions, compat classes
   (a) routes:-only preserved except over-cap push and collision-guarded
   documents, (b) widened repairs turn prior failures into loads,
   (c) JSON duplicate-key documents never repair (their gate lives in
   task 1.4).

Tests (all in `env_int_probe.rs` `mod tests`, through the REAL seam
`crate::yaml::parse_routes_with_env` — not `run_probe` directly; reuse the
`error_counting_subscriber` harness for log-count tests):
- EXISTING tests keep passing unchanged (they pin routes:-only behavior:
  `probe_fixes_single_int_position`, `probe_minimal_subset_skips_polymorphic`,
  `probe_rejects_negative_at_unsigned`, `probe_rejects_leading_zero_and_overflow`,
  `probe_cap_nine_candidates_fails`, `probe_cap_eight_candidates_succeeds`,
  bool suite, logging suite).
- `probe_fixes_rest_port_default` — setup: YAML doc whose `rest:` block
  carries `port: ${env:RC_REST_PORT:-8080}`, `host: 127.0.0.1`, `path: /api`
  and one GET operation with `to: direct:list`, `RC_REST_PORT` unset;
  action: `parse_routes_with_env`; assert: Ok, and the rest-expanded route's
  `from_uri()` contains `"http://127.0.0.1:8080/api"` (rest lowers to
  `http://host:port/path?httpMethod=…`).
- `probe_fixes_rest_port_from_env_lookup` — same doc with the lookup
  returning `Some("9090".into())` for `RC_REST_PORT`; assert the URI
  contains `":9090/api"`.
- `probe_rest_port_notanumber_and_leading_zero_and_overflow_fail` — same doc
  with default resolving to `notanumber`, `007`, `99999999999999999999999`;
  assert Err each time.
- `probe_rest_port_embedded_token_fails` — `port: p-${env:RC_EMB:-8080}`;
  assert Err (first-pass type error stands).
- `probe_rest_port_unresolved_names_variable` — `port: ${env:RC_MISSING}`
  (no default); assert the `RoutesEnvError::Unresolved("RC_MISSING")`
  variant.
- `probe_rest_negative_port_fails` — default `-2` at the `u16` position;
  assert Err.
- (the below-cap stale-provenance compat test is deliberately NOT here —
  owned by Task 1.2, which introduces the collision guard it requires.)
- `probe_cap_shared_across_routes_and_rest` — a doc with 4 throttle
  candidates under `routes:` + 5 rest-block port candidates (9 combined) ⇒
  Err; the same doc with 4+4 ⇒ Ok (ports coerced). Count only probed-subtree
  candidates.
- `probe_cap_boundary_schema_token_fails` — the existing 8-throttle fixture
  PLUS a root `$schema: ${env:RC_SCHEMA:-1}` token; assert Err (accepted
  compat boundary).
- `probe_request_schema_compile_required_coerces` — a `rest:` operation with
  `request_schema: ${env:SCHEMA:-true}` plus a rest `port` placeholder;
  assert both coerce (schema carries boolean `true`, port typed) — the
  compile-required polymorphic position enters the winning subset.
- `probe_templated_routes_parameter_stays_string` — a doc with
  `templated_routes.parameters.delay_ms: ${env:RC_TR_P:-500}` and an
  8-candidate `routes:` section; assert Ok, parameter value is the STRING
  `"500"` (excluded subtree consumes no cap budget).

Acceptance:
- `cargo test -p camel-dsl --lib env_int_probe` passes including all
  pre-existing probe tests.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.
- `grep -rn "routes" crates/camel-dsl/src/env_int_probe.rs` shows no
  first-segment `routes` filter left in `candidates`.
- Only ONE subset-enumeration loop exists (`rg "next_combination" -c` = 1
  call site).

- [x] 1.1

## Task 1.2 — YAML provenance collision guard (insert-time detection)

Files:
- `crates/camel-dsl/src/env_interpolation.rs` (modified)
- `crates/camel-dsl/src/env_int_probe.rs` (modified — one test)

Steps:
1. In the YAML tree walk (`interpolate_value` / the mapping arm): detect
   insert-time collisions. `serde_yml::Mapping` preserves insertion order and
   exposes `contains_key`/`get`/`insert` — when inserting an entry whose key
   already exists in the mapping, mark the OVERWRITTEN entry's provenance
   path (and all descendant paths) as collided and remove them from the
   recorded set; when the inserted entry IS the substituted one (a
   substituted key overwriting an earlier literal), KEEP its provenance (the
   final leaf at that path is the substituted value). Track paths under
   construction so descendant prefixes can be dropped in one pass.
2. Apply the same collision handling for a substituted KEY that lands on an
   existing sibling key: the entry the substituted key overwrites loses its
   provenance; the substituted entry keeps its own.
3. Unit-test hook: the recorded-paths set is the value
   `interpolate_yaml_source_with_provenance` already returns — assert on it
   directly, no new API.

Tests (in `env_interpolation.rs` `mod tests`):
- `yaml_same_value_collision_drops_stale_provenance` — input mapping with
  `'${env:K:-a}': '${env:V:-404}'` followed by `a: '404'` (same surviving
  value); with lookup returning None (defaults resolve); assert the
  provenance set does NOT contain the path whose final leaf is the literal
  (the substituted path was overwritten).
- `yaml_substituted_key_over_literal_keeps_provenance` — input with literal
  `a: '1'` followed by `'${env:K:-a}': '${env:V:-2}'`; assert provenance
  contains the substituted path and the surviving value is `"2"`.
- `yaml_descendants_of_collided_path_dropped` — a collided mapping entry
  carrying a nested sequence/mapping with its own token leaves; assert NO
  descendant path survives in provenance.
- `probe_below_cap_stale_provenance_yaml_only_fails` (moved here from 1.1) —
  the reproduced routes:-only throttle doc from the compat-promise text:
  mapping with `'${env:KEY:-max_requests}': '${env:VALUE:-404}'` followed by
  literal `max_requests: '404'`, plus `period_secs: 1`; through
  `parse_routes_with_env` assert Err — today it probe-loads via the stale
  path; the guard removes it below the cap (pinned compat boundary).

Acceptance:
- `cargo test -p camel-dsl --lib env_interpolation` passes.
- `cargo test -p camel-dsl --lib env_int_probe` passes (guard test green).
- The collision guard references only `serde_yml::Mapping` order-preserving
  APIs (no sort-dependent logic).

- [x] 1.2

## Task 1.3 — JSON provenance walk: interpolate_json_source_with_provenance

Files:
- `crates/camel-dsl/src/env_interpolation.rs` (modified)

Steps:
1. Add
   `pub(crate) fn interpolate_json_source_with_provenance(raw: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<(String, Option<Vec<ProvenancePath>>), String>`:
   parse `raw` as `serde_json::Value`; run the tree walk mirroring the YAML
   recursion arms (`Value::String` / `Array` / `Object` cases) reusing the
   shared scalar helpers `interpolate_string`, `has_env_token`,
   `is_whole_scalar_env_token`; record `ProvenancePath` (Key/Index segments)
   for whole-scalar substitutions; serialize with `serde_json::to_string`.
   Unresolved variable ⇒ `Err(var_name)`. Return `Ok((text, None))` when the
   document does not survive the JSON round-trip (fallback semantics
   mirrored from the YAML seam — the caller's splice is the first pass; this
   function is only invoked in repair paths, but keep the None fallback for
   safety).
2. Provenance ORDER for JSON: the walk's deterministic key-sorted iteration
   order (`serde_json::Map` default). Document this in the fn doc: authored
   source order is not recoverable (default serde_json sorts keys; the
   crate-wide `preserve_order` feature is deliberately rejected).
3. Substitution-produced KEY collision detection by lookup (order-
   independent): when a substituted KEY string equals an existing sibling
   key in the target object, drop the collided path and its descendants from
   provenance. (serde_json's Map silently overwrites on insert — check
   `contains_key` before inserting and mark collided.)
4. Escapes: the JSON walk substitutes into the ALREADY-DECODED string values;
   do not add escape handling beyond what the YAML walker does — the
   baseline-equality gate in task 1.4 is what prevents escape drift from
   producing repairs.

Tests (in `env_interpolation.rs` `mod tests`):
- `json_provenance_records_whole_scalar_paths` — a JSON object with a
  whole-scalar placeholder leaf; assert the returned provenance contains the
  exact `ProvenancePath` (Key segments) and the text re-serializes as JSON.
- `json_post_substitution_key_collision_drops_provenance` — object with key
  `"${env:K:-shared}"` (value `${env:V:-1}`) and an existing sibling
  `"shared"`; assert the collided path is absent from provenance.
- `json_provenance_order_is_sorted_not_authored` — object with two
  placeholder leaves whose key-sorted order differs from authored order
  (e.g. keys `b` authored before `a`); assert provenance lists the `a` path
  first (sorted), documenting the deterministic tie-break.
- `json_unresolved_var_errors_with_name` — `${env:RC_J_MISSING}` no default;
  assert `Err("RC_J_MISSING")`.

Acceptance:
- `cargo test -p camel-dsl --lib env_interpolation` passes.
- No `serde_json` preserve_order feature added to any Cargo.toml
  (`git diff -- '*.toml'` empty for this task).

- [x] 1.3

## Task 1.4 — JSON route-document seam: parse_json_routes_with_env + repair gates + quiet oracle

Files:
- `crates/camel-dsl/src/json.rs` (modified)
- `crates/camel-dsl/src/env_int_probe.rs` (modified — quiet-attempts helper, if not already generic)
- `crates/camel-dsl/src/lib.rs` (modified — pub re-export of the new seam)

Steps:
1. Add a duplicate-key detector for JSON text:
   `pub(crate) fn json_has_duplicate_keys(text: &str) -> bool` — a raw-text
   scan that reports whether any object literal in the document declares the
   same key twice (same nesting level). Plain
   `serde_json::from_str::<serde_json::Value>` (collapses silently) and
   lenient duplicate-tracking deserializers are NOT acceptable; the expected
   shape is a compact recursive raw-text scanner with in-string/in-escape
   state tracking (precedent: the raw pre-scan
   `reject_unknown_transacted_keys` in route_ast.rs).
2. Add the warnings-suppressed quiet oracle:
   `pub(crate) fn parse_json_with_threshold_and_security_quiet(json: &str, stream_cache_threshold: usize, security_ctx: SecurityCompileContext, script_ctx: &ScriptLoadContext) -> Result<Vec<RouteDefinition>, CamelError>`
   in `json.rs`, mirroring `parse_yaml_with_threshold_and_security_quiet`
   (yaml.rs — same staged shape: `parse_json_to_declarative_inner` +
   `compile_declarative_route_with_stream_cache_threshold`) PLUS compile
   warning suppression: wrap the speculative call so no `warn!`/`error!`
   event reaches the dispatcher (implement a scoped suppression guard, e.g.
   `pub(crate) struct QuietCompileGuard` in `env_int_probe.rs` that installs
   a thread-local `tracing::Dispatch` filtering WARN+ERROR events for its
   lifetime — precedent: the test `ErrorCountLayer` in env_int_probe.rs and
   camel-core's `capture_warns`). The oracle exists so probe attempts repeat
   nothing.
3. Add
   `pub fn parse_json_routes_with_env(raw: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<Vec<RouteDefinition>, RoutesEnvError>`
   in `json.rs` (re-export from lib.rs next to `parse_routes_with_env`),
   implementing the two-pass shape:
   - PASS 1 (legacy): `crate::env_interpolation::interpolate_env_with(raw, lookup)`
     → `parse_json_with_threshold_and_security_with_script_ctx` (exact
     today's behavior — values, error text, warnings). Success ⇒ return.
   - REPAIR (only on pass-1 failure): run the baseline gates IN ORDER —
     (i) the splice-substituted text must parse as JSON AND
     `json_has_duplicate_keys(spliced) == false`, else first-pass error
     stands; (ii) `interpolate_json_source_with_provenance` on the raw text
     — an unresolved variable there maps to
     `RoutesEnvError::Unresolved(var)` (it is the same var pass-1 failed
     on); the splice-parsed tree must EQUAL the tree-walk-parsed tree
     (`serde_json::Value` equality), else first-pass error stands;
     (iii) candidates (task 1.1 widened rules, via
     `JsonDoc`) must be non-empty, else first-pass error stands;
   - probe: `crate::env_int_probe::parse_json_with_probe` with the QUIET
     oracle (pass threshold
     `camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD` and
     `SecurityCompileContext::default()`); success ⇒ return Ok;
     failure ⇒ `RoutesEnvError::Parse(first_pass_error)` (the ORIGINAL
     pass-1 error, no replay — the JSON chain has no logging deserializer).
4. Factor the shared two-pass repair into ONE helper both this seam and
   discovery (task 1.5) and the openapi extract (task 1.6) consume —
   ORACLE-GENERIC so the same gate sequence serves every arm:
   `pub(crate) fn repair_json_with_probe<R>(spliced: &str, raw: &str, lookup: &dyn Fn(&str) -> Option<String>, probe: &dyn Fn(&str) -> Result<R, CamelError>) -> Result<Option<R>, CamelError>`
   — the caller supplies its quiet probe closure (route seam:
   `parse_json_with_threshold_and_security_quiet` with default threshold /
   security / script ctx; discovery: its threshold/security/script_ctx;
   extract: `extract_rest_blocks_json_for_probe`). `Ok(None)` when no repair
   applies (any gate fails; the caller keeps its first-pass error),
   `Ok(Some(r))` when the probe wins, `Err(var)` only for the
   unresolved-variable case which the string seam maps to
   `RoutesEnvError::Unresolved`, discovery to `DiscoveryError::Env`.
5. Tests use injected lookups only (no process environment).

Tests (in `json.rs` `mod tests`):
- `json_routes_env_fixes_rest_port_default` — JSON doc with `"rest":
  [{"host":"127.0.0.1","port":"${env:RC_J_PORT:-8080}","path":"/api",
  "operations":[{"method":"GET","operation_id":"listUsers","to":"direct:listUsers"}]}]`
  (as a full `{"rest":[…]}` document), lookup None; assert Ok and the
  rest-expanded `from_uri()` contains `"http://127.0.0.1:8080/api"`.
- `json_routes_env_fixes_rest_port_from_lookup` — same doc, lookup
  `RC_J_PORT=9090`; assert `":9090/api"`.
- `json_routes_env_fixes_throttle_under_routes` — JSON doc with a
  `throttle.max_requests: "${env:RC_J_MAX:-2}"` step; assert
  `max_requests == 2`.
- `json_first_pass_preserves_escape_value_byte_identical` — doc with
  `"${env:RC_J_ESC:-a\nb}"` at a STRING position (the JSON source contains
  the two-character escape `\n`); assert Ok and the field value is
  `"a\nb"` WITH the real newline (splice semantics) — the repair pass never
  ran, so no sanitization to `"a b"`.
- `json_unescaped_quote_no_repair_first_pass_error` — a default containing a
  raw `"` that breaks JSON syntax after splicing at a typed position; assert
  Err with today's splice syntax error text.
- `json_escape_default_plus_repairable_port_no_repair` — a doc with BOTH an
  `a\nb` escape-bearing default (string position) AND a repairable port
  placeholder; assert Err (baseline inequality — splice tree ≠ tree-walk
  tree; no partial repair silently changing the escape default).
- `json_duplicate_key_document_never_repaired` — a doc carrying a duplicate
  object key anywhere plus a repairable port placeholder; assert Err
  (first-pass error stands, no candidates consulted).
- `json_unresolved_names_variable` — `"host": "${env:RC_J_MISSING}"`;
  assert `RoutesEnvError::Unresolved("RC_MISSING")`.
- `json_repair_quiet_idempotent_warning_string_body` — the NAMED fixture:
  JSON doc with (a) a `set_body` candidate whose value is compile-valid as
  BOTH string and coerced value, (b) a trailing
  `"unmarshal": "no-such-format"` step whose LATE compile failure keeps the
  document failing (zero-throttle is NOT used — it fails at AST lowering
  before the warning site); count events via the `ErrorCountLayer` pattern
  extended to count WARN+ERROR (add `warn_count` alongside `error_count` in
  the harness); assert: baseline (direct splice parse of the same doc) emits
  exactly ONE warning from the idempotent_consumer path; the full seam call
  adds ZERO additional warn/error events during repair attempts; final
  failure returns the original first-pass error.
- `json_repair_quiet_idempotent_warning_numeric_body` — the same fixture
  with the set_body candidate value NUMERIC-coercing (e.g. `42`); same
  count assertions.
- `json_routes_env_malformed_json_first_pass_unchanged` — invalid JSON (no
  placeholders); assert Err identical to today's `parse_json` error class.
- `json_embedded_token_and_notanumber_keep_first_pass_error` — (a) a doc
  with an embedded token at a typed position
  (`"port": "p-${env:RC_J_E:-8080}"`) and (b) a doc whose placeholder
  resolves to `notanumber`; assert BOTH keep the first-pass error with
  today's error text, and the repair attempts emit zero warn/error events
  (warn-count harness).

Acceptance:
- `cargo test -p camel-dsl --lib json` passes.
- `cargo test -p camel-dsl --lib env_int_probe` passes (quiet guard has no
  cross-test leakage — the guard is scoped, not global).
- `grep -n "parse_json_routes_with_env" crates/camel-dsl/src/lib.rs` shows
  the re-export.
- `rg "TODO|TBD|FIXME" crates/camel-dsl/src/json.rs` — zero hits.

- [x] 1.4

## Task 1.5 — discovery JSON arm wiring (two-pass + repaired-text consumption)

Files:
- `crates/camel-dsl/src/discovery.rs` (modified)
- `crates/camel-dsl/src/env_int_probe.rs` (modified — winning-text accessor,
  step 3)

Steps:
1. Keep `interpolate_for_parse` returning `None` provenance for JSON
   (discovery's pass-1 stays the splice + existing parse — byte-identical,
   warnings included). Do NOT route discovery's YAML arm through any new
   code.
2. In `parse_document_routes`'s `Some("json")` arm: on failure of the
   existing routes parse, call the SHARED repair helper from task 1.4
   (`repair_json_with_probe::<Vec<RouteDefinition>>` with the quiet route
   oracle built from this document's threshold / security_ctx / script_ctx).
   `Ok(None)` ⇒ keep the first-pass error and
   map to `DiscoveryError::Json` exactly as today; `Ok(Some(routes))` ⇒ the
   repair won; `Err(var)` ⇒ `DiscoveryError::Env` (same unresolved mapping
   as today).
3. When the repair WINS, the winning repaired text feeds ALL downstream
   consumers of that document: `crate::template::json::parse_json_templates`
   and `crate::template::json::parse_json_templated_routes` parse the
   REPAIRED text instead of the original splice (they deserialize the whole
   `RouteDslRoutes` and would reject the still-string port otherwise). The
   YAML precedent for section-tolerance is TemplateSections
   (template/yaml.rs) — JSON repairs the document once and feeds the result
   to every consumer. To obtain the winning text: extend the probe result —
   add `pub(crate) fn parse_json_with_probe_with_text(...) -> Result<(Vec<RouteDefinition>, String), CamelError>`
   in `env_int_probe.rs` (task 1.1's `run_probe` already serializes each
   winning subset; return the winning serialization alongside). This new fn
   wraps `parse_json_with_probe`; the string seam keeps using the plain one.
4. Templates and templated-route parses keep today's behavior for all
   non-repaired documents (spliced text as today).

Tests (in `discovery.rs` `mod tests`, using the existing discovery test
harness/`EnvGuard` patterns; temp files via the existing helpers):
- `discovery_json_rest_doc_loads_through_repair` — a `.json` route file with
  a `rest` block whose `port` is `"${env:RC_D_PORT:-8081}"`; run discovery
  with an injected None lookup; assert routes discovered and the URI carries
  `":8081/"`.
- `discovery_json_routes_throttle_loads` — a `.json` file with
  `throttle.max_requests: "${env:RC_D_MAX:-3}"`; assert the compiled route
  throttles at 3.
- `discovery_json_repaired_text_feeds_template_extractors` — a `.json` file
  with BOTH a repairable rest port placeholder AND a `templates` section
  with one template (valid); assert discovery succeeds — proving the
  template extractor consumed the REPAIRED text (a still-string port would
  fail the whole-`RouteDslRoutes` template extraction with the old wiring).
- `discovery_json_numeric_pin_flipped` — find the existing discovery test
  that pins a JSON numeric typed failure (the test asserting a JSON doc with
  a placeholder at a numeric position fails; it references the pre-probe
  behavior); FLIP it to expect the typed value (coercion), preserving the
  test's document shape.

Acceptance:
- `cargo test -p camel-dsl --lib discovery` passes including the flipped pin.
- YAML discovery tests unchanged and passing (no YAML-arm behavior change).
- One shared repair helper exists (`rg "repair_json" -c` ≥ 2 call sites:
  json.rs seam + discovery).

- [x] 1.5

## Task 1.6 — openapi extract parity: extract_rest_blocks_from_file_with_env

Files:
- `crates/camel-dsl/src/yaml.rs` (modified)

Steps:
1. Add a QUIET extract oracle:
   `fn extract_rest_blocks_for_probe(yaml: &str) -> Result<Vec<RouteDslRest>, CamelError>`
   — `serde_yml::from_str::<RouteDslRoutes>` → `.rest`, mapped to the same
   error wording `extract_rest_blocks` produces, with ZERO logging (the
   logging `extract_rest_blocks` replays exactly once on final failure).
   For JSON: a quiet twin
   `fn extract_rest_blocks_json_for_probe(json: &str) -> Result<Vec<RouteDslRest>, CamelError>`
   using `serde_json::from_str::<RouteDslRoutes>`.
2. Rewrite `extract_rest_blocks_from_file_with_env`:
   - YAML arm: `interpolate_yaml_source_with_provenance` → probe via
     `crate::env_int_probe::run_probe::<YamlDoc, _>` DIRECTLY (NOT
     `parse_with_probe` — that fn's signature is pinned to
     `Vec<RouteDefinition>` by task 1.1 and design.md; the extract oracle
     returns `Vec<RouteDslRest>`), with the quiet extract oracle; on
     final failure replay ONCE through logging `extract_rest_blocks` (today's
     single error log) and return the first-pass error annotated with the
     file path (today's annotation shape).
   - JSON arm: legacy splice first pass (`interpolate_env_with` +
     `serde_json::from_str::<RouteDslRoutes>` → `.rest`, today's behavior
     incl. today's single error-log site on failure) → on failure, the
     ORACLE-GENERIC shared helper from task 1.4
     (`repair_json_with_probe::<Vec<RouteDslRest>>` with
     `extract_rest_blocks_json_for_probe` as the probe closure — one gate
     sequence shared with the route seam and discovery, so an extract arm
     can never repair a duplicate-key document the route arm refuses);
     final failure keeps the original
     first-pass error and logs exactly once (the existing log site).
   - Unresolved no-default placeholders keep the named-variable error (the
     `not_set` closure fires before any probe).
   - The CLI caller (`camel-cli/src/commands/openapi.rs`) is NOT modified —
     it keeps `&|_| None` (defaults-only).
3. REPLACE the pinned failure test with the coercion contract (see Tests).
4. `RouteDslRest` field types, DSL keys, and `extract_rest_blocks`'s public
   signature are NOT changed.

Tests (in `yaml.rs` `mod tests`, next to the replaced pin):
- `extract_rest_from_file_int_default_coerces` (REPLACES
  `extract_rest_from_file_int_field_fails_type_mismatch`) — the SAME
  document shape as the old pin (`rest:\n  - host: 127.0.0.1\n    port:
  ${env:PORT:-8080}`); assert Ok and `rest[0].port == 8080`.
- `extract_rest_from_file_int_notanumber_fails_type_mismatch` — same doc,
  default `notanumber`; assert the type-mismatch wording class is preserved
  (`type mismatch` + `expected unsigned integer` + `found string`), and NO
  named-variable error.
- `extract_rest_from_file_no_default_names_variable` — EXISTING test keeps
  passing unchanged (named-variable error before any probe).
- `extract_rest_json_int_default_coerces` — a `.json` file with a rest block
  port `"${env:RC_OJ_PORT:-8082}"`; assert `rest[0].port == 8082`.
- `extract_rest_final_failure_logs_exactly_once` — the notanumber YAML doc;
  count ERROR-level events with the existing subscriber harness; assert
  exactly one (the replay), matching today's single-log behavior; probe
  attempts add nothing.
- `extract_rest_json_final_failure_logs_exactly_once` — the notanumber JSON
  doc; assert exactly one error-level event (the existing JSON log site).
- `extract_rest_json_duplicate_key_never_repaired` — a `.json` file with a
  duplicate object key plus a repairable port placeholder; assert Err
  (first-pass error stands — proving the extract arm shares the route
  arm's baseline gates via the oracle-generic helper).

Acceptance:
- `cargo test -p camel-dsl --lib yaml::tests::extract_rest` passes (all
  extract tests).
- `rg "extract_rest_from_file_int_field_fails_type_mismatch" -c` = 0.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 1.6

## Task 1.7 — CLI openapi pin flips (camel-cli)

Files:
- `crates/camel-cli/src/commands/openapi.rs` (modified — tests only; the
  production body keeps `&|_| None`)

Steps:
1. Find the CLI tests that pin the OLD hard failures (the tests expecting a
   YAML rest doc with a `port`/`status` placeholder and a JSON rest doc with
   a `port` placeholder to FAIL with a type mismatch — around the
   `extract_rest_blocks_from_file_with_env` call sites in the test module).
2. Flip each pinned failure to the coercion contract, asserting ACTUAL
   results: the generated OpenAPI document reflects the coerced server URL
  (e.g. the server URL contains `:8080` for a `${env:PORT:-8080}` default),
   a genuinely broken document still errors, and an unresolved no-default
   placeholder errors naming the variable.
3. Add NO new production code paths; the lookup stays `&|_| None`
   (defaults-only) in the CLI.

Tests (in the `camel-cli` test module, same harness as the flipped pins):
- `openapi_generate_yaml_rest_port_default_coerces` — YAML file with rest
  `port: ${env:CLI_OA_PORT:-8083}` + one GET operation; run the generate
  path; assert success and the output server/URL carries `:8083`.
- `openapi_generate_json_rest_port_default_coerces` — same via a `.json`
  file; assert `:8083` in the output.
- `openapi_generate_unresolved_names_variable` — rest `host:
  ${env:CLI_OA_UNSET}` (no default); assert the error names
  `CLI_OA_UNSET`.
- `openapi_generate_broken_document_still_errors` — port default
  `notanumber`; assert the first-pass typed error.
- Keep every OTHER existing CLI openapi test passing unchanged.

Acceptance:
- `cargo test -p camel-cli -- openapi` passes.
- `git diff crates/camel-cli/src/commands/openapi.rs` touches ONLY the test
  module (`#[cfg(test)]` region).
- The declared camel-cli lane command
  (`cargo test -p camel-cli --no-default-features --features
  flavor-regular,exec --no-fail-fast`) passes in STAGE 4.

- [x] 1.7

## Task 1.8 — loader-path integration fixtures (crates/camel-dsl/tests/)

Files:
- `crates/camel-dsl/tests/envtyped_route_docs.rs` (new)

Steps:
1. Create the integration file using the existing patterns from
   `tests/discovery_test.rs` (discovery harness) and the `EnvGuard`
   lookup-injection helper from `camel_component_api::test_support` (as used
   in yaml.rs tests). Fixture docs live inline in the test file (small
   constants) — no goldens.
2. YAML rest fixture (positive + negative): a `.yaml` route file with a
   `rest:` block, port placeholder default `8184`, host `127.0.0.1`, one
   GET operation; load via `discover_routes` (the loader path) with injected
   lookups; assert the discovered route's `from_uri()` carries `:8184/`;
   negative: default `notanumber` keeps the first-pass error.
3. YAML template fixture (positive): a `.yaml` document with a `templates:`
   section declaring a `number` parameter used at a numeric body position
   (e.g. `delay_ms: "{{delay}}"` with parameter spec `{"name":"delay",
   "type":"number"}` — mirror an existing template fixture shape from
   `tests/route_template_parity.rs` or template/yaml.rs tests) and a
   `templated_routes:` instantiation supplying `delay:
   "${env:RC_TPL_DELAY:-500}"`; assert discovery + materialization produce a
   route carrying the typed delay 500 (assert via the compiled route's
   delay step config or the declarative route — read the value the same way
   template/materializer tests do).
4. Template-body direct-token negative (reaches materialization): a document
   whose template BODY carries a direct `${env:RC_BODY_DELAY:-500}` at a
   numeric position (no typed parameter) alongside a `rest:` port
   placeholder the widened probe repairs; assert the DOCUMENT loads (the
   widened probe repaired rest) and MATERIALIZATION of the templated route
   fails with today's typed error — the stay-out is pinned end-to-end.
5. JSON route-document fixture (positive + negative): a `.json` route file
   with a rest block port placeholder (default `8186`) and a `routes` array
   with a throttle numeric placeholder; assert both coerce through
   discovery; negative: `notanumber` port keeps the first-pass error.
6. Every lookup is injected; every fixture doc uses `RC_*`/`CLI_*`-prefixed
   variable names unique to this file.

Tests:
- `yaml_rest_doc_loader_path_coerces` / `yaml_rest_doc_loader_path_negative_keeps_error`
- `yaml_template_numeric_via_typed_parameter_materializes`
- `yaml_template_body_direct_token_stays_limited`
- `json_route_doc_loader_path_coerces` / `json_route_doc_loader_path_negative_keeps_error`

Acceptance:
- `cargo test -p camel-dsl --test envtyped_route_docs` passes.
- No fixture reads the process environment (`EnvGuard` injected lookups
  only; `grep -n "std::env::var" crates/camel-dsl/tests/envtyped_route_docs.rs`
  = 0 hits).

- [x] 1.8

## Task 1.9 — docs: env-interpolation typed-leaf section + openapi parity

Files:
- `docs/src/configuration/env-interpolation.md` (modified)

Steps:
1. Extend the typed-leaf section (landed with the components slice) with the
   route-document surface: `${env:}` at typed positions under `rest:` (and
   other probed subtrees) in route documents coerces after a failed first
   parse; same clean-integer/clean-boolean lexical gates, same cap (8
   combined candidates from probed subtrees), same failure shapes.
2. Document the boundaries exactly as blessed: `templated_routes.parameters`
   stays string (template-substitution inputs); template-body direct tokens
   keep the materialization limitation — use the typed-parameter machinery
   (`type: number`/`boolean` parameters) for template numerics; JSON route
   documents repair through the same probe with a legacy-splice first pass
   (duplicate-key documents never repair); collision-guarded and over-cap
   documents keep the first-pass error.
3. Add the `camel openapi generate` parity note: the REST extractor shares
   the probe, so a rest port placeholder coerces there too; unresolved
   no-default placeholders still fail naming the variable.
4. Write in English (LANGUAGE POLICY), matching the page's existing voice;
   no new pages.

Tests:
- `cd docs && mdbook test` passes (the page's inline examples, if any, must
  still build — check whether the page uses mdbook's rust/rhai runnable
  blocks before editing around them).

Acceptance:
- `cd docs && mdbook test` exits 0.
- The page contains the words "openapi generate" in the parity note and
  names the cap (8) and the duplicate-key JSON rule.
- No other docs files changed (`git diff --stat -- docs/` shows exactly one
  file).

- [x] 1.9

## Task 1.10 — D5 pins: JSON ordering under feature unification, collision both-orders, unresolved-var alignment (bd rc-3c67o.2.3)

Files:
- `crates/camel-dsl/src/env_interpolation.rs` (modified — provenance order)
- `crates/camel-dsl/src/json.rs` (modified — repair unresolved mapping, fixture strength)

Steps:
1. `serde_json`'s `preserve_order` feature is ACTIVE in the confirmed CLI
   feature graph (`camel-language-rhai/Cargo.toml` enables it — feature
   unification can swap key-sorted maps for authored-order IndexMap). Make
   JSON provenance order EXPLICITLY deterministic regardless of map backing:
   after the tree walk, SORT the recorded provenance paths lexicographically
   (segment-wise: Key by string, Index by number) before returning; update
   the fn doc (sorted-order determinism is a walker guarantee, not a
   serde_json map guarantee).
2. Align the repair-pass unresolved-variable mapping (task 1.4 step 3) with
   the variable pass-1 failed on: the tree-walk's unresolved var must equal
   the pass-1 splice's unresolved var for the same document — assert both
   name the same variable in the test.
3. Strengthen `json_repair_quiet_idempotent_warning_*` so the fixture
   PROVES retries compile: assert the repair path executed (pass-1 failed,
   baseline gates passed, at least one speculative attempt ran — observable
   via the suppressed-warning plumbing: the quiet guard counts suppressed
   events; assert ≥ 1 suppressed event during repair).

Tests:
- `json_repair_tie_uses_sorted_keys` — JSON object authoring key `b` (value
  `${env:RC_TIE_B:-1}`) BEFORE key `a` (value `${env:RC_TIE_A:-1}`) at two
  typed positions whose oracle accepts EITHER single coercion but NOT both
  strings; run under BOTH `cargo test -p camel-dsl --lib
  json_repair_tie_uses_sorted_keys` AND the same with
  `--features camel-core/lang-rhai` (pulls preserve_order into the build
  graph); assert ONLY the `a`-path coercion wins in both configurations
  (explicit sort, not map-iteration luck).
- `json_post_substitution_collision_drops_descendants_in_both_orders` —
  same-value collision authored in BOTH orders (substituted entry first vs
  literal first, keys `shared` / `!shared` shape from the bd issue); assert
  the collided path and descendants are dropped in both, and the surviving
  value's provenance is correct in both.
- `json_repair_unresolved_matches_pass1_variable` — a doc whose repair-pass
  tree walk and pass-1 splice both resolve to the same missing variable;
  assert `RoutesEnvError::Unresolved` carries that exact name.

Acceptance:
- Both feature configurations of the tie test pass (the command matrix is
  the acceptance).
- `cargo test -p camel-dsl --lib env_interpolation json` passes.

- [x] 1.10

## Task 1.11 — D5 buildability: visibility, winning-text handoff, task-order fix (bd rc-3c67o.2.4)

Files:
- `crates/camel-dsl/src/env_int_probe.rs` (modified — visibility; helper
  signature)
- `crates/camel-dsl/src/json.rs` (modified — helper signature)
- `crates/camel-dsl/src/discovery.rs` (modified — consume (routes, text))

Steps:
1. Visibility: every symbol introduced in task 1.1 that sibling modules
   (`json.rs`, `discovery.rs`, `yaml.rs`) consume — `ProbeScalar`,
   `ProbeDoc`, `YamlDoc`, `JsonDoc`, `run_probe`, `parse_json_with_probe` —
   must be reachable with `pub(crate)` (trait methods included) so per-task
   `cargo clippy -p camel-dsl --all-targets -- -D warnings` passes; fix any
   private-item leakage the earlier tasks left.
2. Task-order amendment (sanctioned by the standing rule): move the
   JSON-only glue (`JsonDoc` impl + `parse_json_with_probe`) from task 1.1
   INTO task 1.4 — task 1.1 keeps the trait + `YamlDoc` + generic
   `run_probe` + widened candidates + `parse_with_probe` (all used
   immediately), so no JSON-only production code sits unused under
   `-D warnings` between tasks. Apply the reorder now (tasks 1.1/1.4 already
   executed: if 1.1 already introduced them, this step is the audit that
   clippy passes per-task; if not, they land in 1.4).
3. Winning-text handoff: change `repair_json_with_probe` to return
   `Ok(Some((R, String)))` — the winning coercion subset's serialization
   alongside the routes (the probe already serializes each subset; keep it).
   The string seam discards the text; discovery uses it for the template
   extractors. `parse_json_with_probe_with_text` (task 1.5's wrapper) becomes
   the helper's internal probe call — ONE probe execution per repair, no
   second search.
   As landed (bd rc-3c67o.2.4 design): the helper stays oracle-generic —
   `Result<Option<R>, String>` — and discovery's probe is the text-returning
   `parse_json_with_probe_with_text`, so `R = (Vec<RouteDefinition>, String)`
   carries the winning text with zero helper coupling. One `next_combination`
   call site in `crates/camel-dsl/src` (single search); the string seam's
   plain-`R` probe discards the text by construction.

Tests:
- `discovery_json_repaired_text_feeds_template_extractors` (from task 1.5)
  now ALSO asserts the materialized instance route exists in the discovery
  output (repaired REST port URI + templated instance), proving the winning
  text reached BOTH template consumers with a single probe execution. As
  landed: the test asserts the exact lowered REST URI
  (`http://127.0.0.1:8084/api?httpMethod=GET`) and the exact instance route
  (`rc-d-inst` → `direct:repaired`), and `run_probe_with_text` gained a pin
  that the returned text is exactly the serialization the oracle accepted.
- `extract_rest_json_duplicate_key_never_repaired`,
  `extract_rest_from_file_int_default_coerces` (task 1.6) still pass after
  the signature change; `cargo clippy -p camel-dsl --all-targets --
  -D warnings` exits 0 at this task's end.

Acceptance:
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.
- `rg "parse_json_with_probe_with_text" -c` ≥ 1 (single probe execution
  path for text+routes).
- All 1.4/1.5/1.6 tests pass unchanged.

- [x] 1.11

## Task 1.12 — D5 regression pins: templated param assertion, CLI status flip, stale extract pin, first-pass error texts (bd rc-3c67o.2.5)

Files:
- `crates/camel-dsl/src/env_int_probe.rs` (modified — one test assertion)
- `crates/camel-dsl/src/yaml.rs` (modified — tests)
- `crates/camel-cli/src/commands/openapi.rs` (modified — tests)
- `crates/camel-dsl/tests/envtyped_route_docs.rs` (modified — tests, if
  already created by 1.8)

Steps:
1. `probe_templated_routes_parameter_stays_string` (task 1.1) asserts
   through `Vec<RouteDefinition>`, which cannot carry template parameters:
   keep the probe-seam load assertion (Ok), and assert the parameter value
   via a direct `RouteDslRoutes` deserialization of the same document
   (`templated_routes[0].parameters["delay_ms"] == "500"` as STRING).
2. YAML negatives in task 1.1 assert the FIRST-PASS error explicitly, not
   just `Err`: for notanumber/007/overflow/negative/embedded-token assert
   the error text contains the shim's type-mismatch class
   (`type mismatch` + `expected unsigned integer` + `found string`); add
   `extract_rest_from_file_int_leading_zero_keeps_first_error` (default
   `007`) alongside.
3. Replace the stale JSON extract failure pin
   `extract_rest_from_file_json_int_fails` (yaml.rs, near line 6681) with
   `extract_rest_json_int_default_coerces` (already named in task 1.6) —
   delete the old pin, keep the coercion test as its replacement.
4. CLI status pin: flip
   `generate_success_status_placeholder_errors` to
   `openapi_generate_yaml_success_status_default_coerces` — a YAML rest doc
   with a POST operation whose response status is
   `${env:CLI_OA_STATUS:-201}`; assert the generated OpenAPI document has
   `responses["201"]` for `/api/users` POST; RETAIN the existing YAML and
   JSON port URL assertions.

Tests: the four named above plus the retained pins.

Acceptance:
- `rg "generate_success_status_placeholder_errors|extract_rest_from_file_json_int_fails" -c`
  = 0.
- `cargo test -p camel-dsl --lib env_int_probe` and
  `cargo test -p camel-cli -- openapi` pass.
- Every negative test asserts error TEXT, not just the Err variant.

- [x] 1.12

