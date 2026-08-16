# Tasks: add-endpoint-parameters

## Phase 1: EndpointUri value type in camel-api

### camel-api

#### Task 1.1: EndpointUri struct, grammar validation, and canonical rendering

**Files:**
- `crates/camel-api/src/endpoint_uri.rs` (new)
- `crates/camel-api/src/error.rs` (modified — add typed error)
- `crates/camel-api/src/lib.rs` (modified — add `pub mod endpoint_uri;` and re-export `EndpointUri`)

**Steps:**
1. In `error.rs`: add `#[non_exhaustive] pub enum EndpointUriError { DuplicateKey { key: String }, MissingScheme, EmptyQueryKey, InvalidParamKey { key: String } }` with `std::error::Error` + `Display` impls (every variant's Display names the offending key/input), a new `CamelError` variant `EndpointUri(EndpointUriError)`, and `impl From<EndpointUriError> for CamelError`.
2. Create `endpoint_uri.rs` defining `#[non_exhaustive] pub struct EndpointUri { pub scheme: String, pub path: String, pub params: BTreeMap<String,String>, raw_query: Option<String> }` — the private `raw_query` field stores the original query bytes (absent when the base URI had none) for byte-preserving rendering.
3. Implement `pub fn try_from_uri_and_params(base: &str, params: BTreeMap<String,String>) -> Result<Self, EndpointUriError>`: scheme = non-empty substring before first `:` (`MissingScheme` when absent/empty); query = portion after first `?`; pairs split on `&`, each pair splits on the FIRST `=` (no `=` → empty value); query pair with empty key → `EmptyQueryKey`; params key policy — `InvalidParamKey` when a key is empty or contains any of `&`, `=`, `%`, `#`, `?`, `+`, or a space byte; repeated keys within the query string are legal and preserved in order; a params key that equals any raw query key (raw string comparison) → `DuplicateKey { key }` fail-closed. Store the original query bytes in `raw_query`.
4. Implement `pub fn to_canonical_string(&self) -> String`: emit `scheme:path`, then `raw_query` byte-for-byte (if any), then params entries appended in BTreeMap sorted order (`?` if no query existed, `&` otherwise); keys verbatim; values percent-encoded uppercase-hex over UTF-8 bytes encoding exactly `& = % # ? +` and space (space as `%20`); all other bytes (including `:` and multi-byte UTF-8) pass through.
5. Do NOT derive `Serialize`. Derive `Clone` only. `Debug` comes in Task 1.2 — for now write a minimal manual `Debug` printing scheme/path and `params: <N entries redacted>`.
6. In `lib.rs`: add `pub mod endpoint_uri;` and re-export BOTH `EndpointUri` and `EndpointUriError`.
7. Unit tests inline in `endpoint_uri.rs` under `#[cfg(test)]`.

**Tests:** (all in `crates/camel-api/src/endpoint_uri.rs`, `command: cargo test -p camel-api --lib endpoint_uri`, `expected: fail before implementation, pass after`)
- `merge_uri_and_params_canonical`: `try_from_uri_and_params("kafka:orders", {brokers: "my-host:9092", acks: "all"})` → Ok; `to_canonical_string()` == `kafka:orders?acks=all&brokers=my-host:9092`
- `duplicate_key_fails_closed`: `try_from_uri_and_params("kafka:orders?brokers=a", {brokers: "b"})` → `DuplicateKey { key: "brokers" }`; Display contains `brokers`
- `repeated_query_keys_preserved`: `try_from_uri_and_params("list:demo?item=a&item=b", {})` → `to_canonical_string()` == `list:demo?item=a&item=b`
- `malformed_bases_rejected`: each of `noscheme`, `:pathonly`, `timer:tick?=1`, params key `a&b` → the respective typed variant (`MissingScheme` / `EmptyQueryKey` / `InvalidParamKey`), Display names the offending part
- `deterministic_across_insert_orders`: build two EndpointUri from `{b:"2",a:"1"}` and `{a:"1",b:"2"}` (different insertion order) → identical `to_canonical_string()`
- `existing_query_preserved_byte_identical`: `try_from_uri_and_params("timer:tick?period=1000&repeatCount=6", {})` → output byte-equals input
- `golden_reserved_characters`: `try_from_uri_and_params("http:srv?a=1&flag", {z: "100%", q: "a b+c"})` → `http:srv?a=1&flag&q=a%20b%2Bc&z=100%25`
- `pair_without_equals_has_empty_value`: `try_from_uri_and_params("t:x?flag", {a: "1"})` → `t:x?flag&a=1`

**Acceptance:**
- `cargo test -p camel-api --lib endpoint_uri` exits 0
- `cargo clippy -p camel-api -- -D warnings` exits 0
- `cargo fmt --check` clean for the new file
- No `Serialize` derive on `EndpointUri` (verify by reading the struct)
- The duplicate-key error is the typed variant, not `CamelError::Config(String)`

- [x] 1.1

#### Task 1.2: Redacting Debug and catalog-aware to_redacted_string

**Files:**
- `crates/camel-api/src/endpoint_uri.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified — extend re-exports if a prelude exists for component_metadata types)

**Steps:**
1. Replace the minimal `Debug` with a manual implementation that prints scheme, path, and every param VALUE masked as `***` (keys visible in clear; fail-safe — no catalog access). The private `raw_query` field SHALL be omitted from Debug output entirely (it carries unmasked original query bytes). If the path carries RFC 3986 userinfo (`//user:pass@...`), the credential segment (between a leading `//` and the first subsequent `/`, when it contains `@`) SHALL be masked in Debug and the redacted rendering; `to_canonical_string` stays byte-faithful (mirroring the raw-query treatment).
2. Implement `pub fn to_redacted_string(&self, catalog: &dyn ComponentMetadataCatalog) -> String`: resolve `self.scheme` in the catalog; build the option set from BOTH sources — the `raw_query` pairs AND the params map; for each option value, resolve the key by name-or-alias against `metadata.uri_options` (two-phase: exact name match, then alias match; an option with `pattern: Some(_)` does NOT match by its anchor name — add `uo.pattern.is_none()` to both predicates); mask the value with `***` UNLESS the catalog affirmatively resolved the option AND its `secret` flag is false; scheme-not-found or option-unresolved → mask (fail-safe). Same rendering order/encoding as `to_canonical_string` (extract a shared private pair-walker so parity is structural, not test-discipline).
3. Add an ADR-0051 classification marker doc comment on the `EndpointUri` struct (`ADR-0051 credential boundary: redacting-wrapper` — the exact token `lint-secrets` keys off, see scripts/xtask/src/main.rs ~L2835).
4. Unit tests with an inline stub catalog implementing `ComponentMetadataCatalog` (http scheme: `password` secret, `timeout` non-secret, `token` non-secret with alias `apikey` and no pattern, prefix-pattern option anchored `cfg` non-secret; empty otherwise).

**Tests:** (`command: cargo test -p camel-api --lib endpoint_uri`, `expected: fail before, pass after`)
- `debug_masks_all_param_values`: EndpointUri built via `try_from_uri_and_params("http:srv?password=clear", {delay: "1000"})`; `format!("{:?}", uri)` contains `***` and contains neither `1000` nor `clear`
- `debug_and_redacted_mask_userinfo`: `try_from_uri_and_params("http://admin:hunter2@srv/path", {})`; both `format!("{:?}", uri)` and `to_redacted_string(&stub)` contain neither `hunter2` nor `admin:`; `to_canonical_string()` == the input byte-for-byte
- `redacted_string_masks_secret_passes_non_secret`: scheme `http`, params `{password: "hunter2", timeout: "5000"}` + stub catalog → output contains `password=***` and `timeout=5000`
- `redacted_string_unknown_scheme_masks`: scheme `not-a-scheme`, params `{token: "abc"}` + stub catalog → output contains `token=***`, no Err, no `abc`
- `redacted_string_masks_query_string_secrets`: `try_from_uri_and_params("http:srv?password=clear", {})` + stub catalog → `to_redacted_string` contains `password=***` and not `clear`
- `redacted_string_alias_resolves_pattern_anchor_does_not`: params `{apikey: "abc", "cfg.foo": "bar"}` on scheme `http` + stub catalog → output contains `apikey=abc` (alias resolution, non-secret) and `cfg.foo=***` (pattern anchor does not match)

**Acceptance:**
- `cargo test -p camel-api --lib endpoint_uri` exits 0
- `cargo clippy -p camel-api -- -D warnings` exits 0
- `cargo xtask lint-secrets` exits 0 (EndpointUri classified compliant with ADR-0051)

- [x] 1.2

#### Task 1.3: Round-trip test against a real component parser

**Files:**
- `crates/components/camel-timer/tests/endpoint_uri_roundtrip.rs` (new)

**Steps:**
1. Add an integration test in camel-timer (already depends on camel-api) that builds `EndpointUri::try_from_uri_and_params("timer:tick", BTreeMap from [("period","2500")])` — a NON-default value (the timer parser's default for `period` is 1000; a dropped parameter must not coincide with the default or the oracle is vacuous) — renders `to_canonical_string()`, parses it with the timer component's config `from_uri` (camel-timer uses `#[uri_config(skip_impl)]`; `from_uri` is hand-written in `crates/components/camel-timer/src/lib.rs` and reachable via `use camel_component_api::UriConfig;`), and asserts: the parsed config's `period` field equals the value parsed from the literal `timer:tick?period=2500`, AND the two configs' `format!("{:?}")` outputs are equal (`TimerConfig` lacks `PartialEq` — Debug-output equality is the comparison method).

**Tests:**
- `canonical_string_roundtrips_timer_from_uri`: as above, `command: cargo test -p camel-component-timer --test endpoint_uri_roundtrip`, `expected: fail before Task 1.1 lands in camel-api, pass after`

**Acceptance:**
- `cargo test -p camel-component-timer --test endpoint_uri_roundtrip` exits 0
- `cargo clippy -p camel-component-timer --all-targets -- -D warnings` exits 0

- [x] 1.3

## Phase 2: parameters surface in camel-dsl and camel-builder

### camel-dsl (AST)

#### Task 2.1: parameters field on endpoint AST structs

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/tests/parameters_dsl_tests.rs` (new)

**Steps:**
1. Add `#[serde(default)] pub parameters: BTreeMap<String,String>` to: `ToStep` (alongside `pub to`), `WireTapStep` (alongside its uri field), `EnrichConfig` (alongside `uri`), and the route-level `from` surface struct in route_ast.rs that holds the `from` string (locate the struct owning `pub from: String` in this file; add the field there). `EnrichStep` and `PollEnrichStep` each have exactly one field today (the `EnrichBody`); add `#[serde(default)] pub parameters: BTreeMap<String,String>` as a SECOND field on each so the shorthand form composes with a sibling `parameters:` key (`- enrich: db:query` + `parameters:` at step level).
2. Add `#[non_exhaustive]` to `ToStep`, `WireTapStep`, `EnrichStep`, `PollEnrichStep`, and `EnrichConfig`.
3. Fix every in-tree compile error from `#[non_exhaustive]` (struct-literal constructions of these types inside camel-dsl must switch to a constructor or `..Default::default()` — add `#[derive(Default)]` where needed or a `new()`; inspect the actual construction sites with `rg 'ToStep \{|WireTapStep \{|EnrichStep \{|EnrichConfig \{' crates/` and update each).
4. Verify YAML/JSON scalar coercion is rejected naturally by `BTreeMap<String,String>` typing (serde_yaml/serde_json will error on non-string scalars); pin it with a test.

**Tests:** (`crates/camel-dsl/tests/parameters_dsl_tests.rs`, `command: cargo test -p camel-dsl --test parameters_dsl_tests`, `expected: fail before, pass after`)
- `ast_accepts_parameters_on_to`: deserialize YAML step `- to: kafka:orders\n  parameters:\n    brokers: my-host:9092` → `ToStep.parameters["brokers"] == "my-host:9092"` and `to == "kafka:orders"`
- `ast_accepts_parameters_on_from`: a route document with `from: timer:tick` + `parameters: {period: "1000"}` deserializes with the parameters on the from surface
- `ast_accepts_parameters_on_wire_tap_and_enrich_full`: wire_tap step and full-form enrich (`enrich: {uri: db:query, parameters: {dataSource: customers}}`) both carry their maps
- `ast_accepts_parameters_on_enrich_shorthand_and_poll_enrich`: `- enrich: db:query\n  parameters:\n    dataSource: customers` and the poll_enrich equivalents (`- poll_enrich: file:inbox\n  parameters:\n    delay: "500"` and the full form) carry their maps
- `non_string_parameter_value_rejected`: `parameters: {retries: 3}` (YAML int) → deserialization Err whose message contains `retries`
- `json_variant_accepts_parameters`: the same to-step shape as JSON object with `"parameters": {"brokers": "x"}` deserializes

**Acceptance:**
- `cargo test -p camel-dsl --test parameters_dsl_tests` exits 0
- `cargo test -p camel-dsl` exits 0 (existing suite green after `#[non_exhaustive]` fixes)
- `cargo clippy -p camel-dsl -- -D warnings` exits 0

- [x] 2.1

### camel-dsl (lowering)

#### Task 2.2: yaml.rs lowering merges via EndpointUri

**Files:**
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/tests/parameters_dsl_tests.rs` (modified — append lowering tests)

**Steps:**
1. In the AST→model lowering in yaml.rs, wherever `ToStepDef::new(to)`, `WireTapStepDef { uri: wire_tap }`, `EnrichStepDef { uri, .. }` (Enrich L1212 and PollEnrich L1220 shapes), and the route-level `from` string are produced (find all construction sites of these model values in this file), route the raw pair through `EndpointUri::try_from_uri_and_params(raw_uri, params)?.to_canonical_string()`; the `?` propagates `EndpointUriError` (via `From<EndpointUriError> for CamelError` from Task 1.1) through the existing lowering error type (inspect the function signatures — they already return `Result<_, _>` with an error convertible into the crate's parse error; map the typed error into it, preserving the offending key name in the message). Empty params map → skip the EndpointUri call and pass the raw uri through unchanged (preserves byte-identity for existing routes).
2. Append lowering tests to `parameters_dsl_tests.rs`.

**Tests:** (`command: cargo test -p camel-dsl --test parameters_dsl_tests`, `expected: fail before, pass after`)
- `from_parameters_merge_to_canonical`: route `from: timer:tick` + `parameters: {period: "1000"}` lowers to model with `from == "timer:tick?period=1000"`
- `to_parameters_merge_to_canonical`: `to: kafka:orders` + `parameters: {brokers: my-host:9092}` → model step uri `kafka:orders?brokers=my-host:9092`
- `query_string_and_parameters_equivalent`: one route `to: log:out?showBody=true`, another `to: log:out` + `parameters: {showBody: "true"}` → model step uris byte-identical
- `wire_tap_and_enrich_and_poll_enrich_merge`: wire_tap → `log:audit?showBody=true`; enrich shorthand and full form both → `db:query?dataSource=customers` and are byte-identical to each other; poll_enrich → `file:inbox?delay=500`
- `duplicate_key_is_lowering_error_yaml_and_json`: YAML route `to: kafka:orders?brokers=a` + `parameters: {brokers: b}` → lowering Err containing `brokers`; the SAME shape as a JSON document (`{"to": "kafka:orders?brokers=a", "parameters": {"brokers": "b"}}`) → lowering Err containing `brokers` (both authoring paths share the yaml.rs lowering — this pins it)
- `empty_parameters_preserve_uri_bytes`: `to: timer:tick?period=1000&repeatCount=6` + `parameters: {}` (or absent) → model uri byte-equals the raw uri

**Acceptance:**
- `cargo test -p camel-dsl --test parameters_dsl_tests` exits 0
- `cargo test -p camel-dsl` exits 0
- `cargo clippy -p camel-dsl -- -D warnings` exits 0

- [x] 2.2

### schema

#### Task 2.3: route-schema regeneration and camel-lint embedded copy sync

**Files:**
- `schemas/dsl/route-schema.json` (modified — regenerated)
- `crates/camel-lint/schema/route-schema.json` (modified — byte-equal copy)
- `examples/json-dsl/config/parameters.json` (new — the schema-validation suite validates every JSON example under `examples/json-dsl/`; add a route exercising `parameters` alongside `to`/`from`, following the shape of the existing `routes.json`)

**Steps:**
1. Run `cargo xtask schema` to regenerate `schemas/dsl/route-schema.json` from the updated AST types (the generator reads the schemars output; the new `parameters` maps on to/wire_tap/enrich/poll_enrich/from surfaces must appear as optional `object` with `additionalProperties: string`).
2. Copy the regenerated file over `crates/camel-lint/schema/route-schema.json` (byte-equal, exactly as the xtask `--check` error message instructs: `cp schemas/dsl/route-schema.json crates/camel-lint/schema/route-schema.json`).
3. Run `cargo xtask schema --check` and confirm exit 0.
4. Run the schema-validation test suite and the lint schema byte-equal test (locate with `cargo test -p camel-lint schema` and `cargo test -p camel-dsl --test schema_validation`).

**Tests:**
- `schema_check_gate`: `cargo xtask schema --check` exits 0 (`expected: fails before the copy is synced, passes after`)
- `lint_embedded_schema_byte_equal`: the existing byte-equal test in camel-lint passes (`command: cargo test -p camel-lint`, inspect test name containing `schema`)
- `schema_admits_parameters`: with `examples/json-dsl/config/parameters.json` present (a valid route using `parameters` on `to` and `from`), `cargo test -p camel-dsl --test schema_validation` exits 0 — the suite picks the file up automatically from `examples/json-dsl/config` (`expected: fails if the regenerated schema rejects the parameters key, passes when admitted`)

**Acceptance:**
- `cargo xtask schema --check` exits 0
- `cargo test -p camel-lint` exits 0
- `cargo test -p camel-dsl --test schema_validation` exits 0

- [x] 2.3

### camel-builder

#### Task 2.4: RouteBuilder .parameters with pending-slot semantics

**Files:**
- `crates/camel-builder/src/lib.rs` (modified)
- `crates/camel-builder/tests/parameters_builder_tests.rs` (new — camel-builder has a tests/ dir; follow the pattern of `tests/canonical_spec_test.rs`)

**Steps:**
1. Add builder state: `parameter_assignments: Vec<(EndpointSlot, BTreeMap<String,String>)>` where `EndpointSlot` is an enum `From` | `Step(usize)` (index into `steps`). Implement `pub fn parameters(mut self, params: BTreeMap<String,String>) -> Self` that pushes an assignment for the current slot — `From` when no step has been added yet, otherwise `Step(last step index)` — so parameters on DIFFERENT endpoints each persist independently (none overwrites another). A push whose slot already has an assignment, or whose current slot is a non-endpoint step, records a misuse flag; this is errored at `build()`, not at the call site.
2. In `build(self) -> Result<RouteDefinition, CamelError>` (L697): apply each `parameter_assignments` entry to its slot — verify the slot is endpoint-bearing (`From`, or a `BuilderStep::{To, WireTap, Enrich, PollEnrich}` step — inspect the actual `BuilderStep` variant shapes for WireTap/Enrich/PollEnrich and match them); a recorded misuse flag or a non-endpoint slot → return a `CamelError::RouteError` with a message naming the misuse (the variant existing build()-time validation errors use). For endpoint slots, apply `EndpointUri::try_from_uri_and_params(existing_uri, params)?.to_canonical_string()` and replace the slot's uri, propagating `EndpointUriError` into `CamelError::EndpointUri` via `From` (do NOT fold URI failures into `RouteError`). Also apply the same in `build_canonical` (L790) if it builds from the same steps (inspect; if it delegates to build()/RouteDefinition lowering, no extra work — verify with a test).
3. Tests per the spec scenarios.

**Tests:** (`command: cargo test -p camel-builder`, `expected: fail before, pass after`)
- `builder_parameters_on_to`: `RouteBuilder::from("timer:tick").to("log:out").parameters(map{showBody:"true"})` → `build()` Ok, step uri `log:out?showBody=true`
- `builder_parameters_on_from`: `RouteBuilder::from("timer:tick").parameters(map{period:"1000"}).to("log:out")` → `build()` Ok, `from_uri == "timer:tick?period=1000"`
- `builder_multiple_endpoints_each_keep_parameters`: `RouteBuilder::from("timer:tick").parameters(map{period:"1000"}).to("log:a").parameters(map{showBody:"true"}).to("log:b").parameters(map{showHeaders:"true"})` → `build()` Ok; `from_uri == "timer:tick?period=1000"`, step 0 uri `log:a?showBody=true`, step 1 uri `log:b?showHeaders=true` — no assignment lost
- `builder_parameters_on_wire_tap_enrich_poll_enrich`: each of `.wire_tap("log:audit")`, `.enrich("db:query")` (inspect the actual enrich signature for required args and use them), `.poll_enrich("file:inbox", 1000)` followed by `.parameters(map)` → merged uris `log:audit?showBody=true`, `db:query?dataSource=customers`, `file:inbox?delay=500`
- `builder_parameters_no_pending_endpoint_errors_at_build`: `.to("log:x").log("hi").parameters(map{a:"1"})` → `build()` Err (no panic), message names the misuse
- `builder_consecutive_parameters_errors_at_build`: `.to("log:x").parameters(map{a:"1"}).parameters(map{b:"2"})` → `build()` Err (no panic), message names the misuse
- `builder_duplicate_key_errors_at_build`: `.to("kafka:orders?brokers=a").parameters(map{brokers:"b"})` → `build()` Err containing `brokers`

**Acceptance:**
- `cargo test -p camel-builder` exits 0
- `cargo clippy -p camel-builder --all-targets -- -D warnings` exits 0
- No new `panic!`/`unwrap` introduced (verify `cargo xtask lint-unwrap` exits 0)

- [x] 2.4

## Phase 3: camel-lint parameters extraction

### camel-lint

#### Task 3.1: route_view extracts parameters entries as spanned LintOptions

**Files:**
- `crates/camel-lint/src/document.rs` (modified — the CST walk, the URI-key table around L197, and `endpoint_for` around L427 live here; the byte-exact-span tests also live in this file's test module around L472)
- `crates/camel-lint/src/route_view.rs` (modified — the option-population sites)

**Steps:**
1. In `document.rs`, extend the CST walk to collect `parameters` map entries with byte-exact spans for the endpoint-bearing surfaces (`from`, `to`, `wire_tap`, `enrich`/`poll_enrich` via their `uri` leaves), following the existing pattern used to obtain the `to:`/`from:` string values with spans. The walk struct is internal to camel-lint — camel-lint must NOT gain a camel-dsl dependency.
2. In `route_view.rs`, at both option-population sites — the route-level `from` branch in `LintRoute::endpoints()` (uses `LintOption::parse_from_query(&f.value, f.span.clone())` around L145) and the step-endpoint construction sites (search for `LintOption::parse_from_query` call sites and `LintNode::Endpoint` construction) — append the `parameters` entries (from the extended walk) as `LintOption { key, value, spans }` after the query-string options of that endpoint, spans byte-exact into the original source.
3. Tests in `document.rs`'s existing test module, following the pattern of `from_uri_span_is_byte_exact` / `nested_child_step_uri_captured`.

**Tests:** (`command: cargo test -p camel-lint`, `expected: fail before, pass after`)
- `parameters_entries_become_options_with_spans`: route `to: kafka:orders` + `parameters: {brokers: my-host:9092}` where the `brokers` key starts at byte offset X and the value at Y (construct the source string so offsets are known) → `LintRoute::endpoints()` endpoint options include `brokers` with key span start X and value span start Y
- `from_parameters_entries_become_options`: same for the route-level `from` + parameters
- `unknown_param_in_parameters_flagged`: with a stub catalog for scheme `timer` carrying `period` only, `parameters: {perod: "1"}` (typo) → the R-URI-known unknown-option diagnostic fires with span inside the parameters map
- `missing_required_in_parameters_flagged`: catalog marks a timer option required → `parameters:` map omitting it → the R-URI-known missing-required-option diagnostic fires
- `deprecated_in_parameters_flagged`: catalog marks a timer option deprecated with a reason → that option set via `parameters:` → R-DEPRECATED fires with span inside the map
- `secret_in_parameters_flagged`: catalog marks `http.password` secret → `to: http:srv` + `parameters: {password: hunter2}` → R-SECRET fires with span pointing at `hunter2` inside the map

**Acceptance:**
- `cargo test -p camel-lint` exits 0
- `cargo clippy -p camel-lint --all-targets -- -D warnings` exits 0
- `crates/camel-lint/Cargo.toml` gains no new non-dev dependency (runtime-free charter intact)

- [x] 3.1

### corpus

#### Task 3.2: secret-in-parameters corpus fixture and baseline regen

**Files:**
- `crates/camel-cli/tests/fixtures/lint-corpus/parameters-secret.yaml` (new — `discover_corpus()` in `crates/camel-cli/tests/lint_corpus.rs` globs `tests/fixtures/**/*.{yaml,json}`; create the `lint-corpus/` subdirectory there, following the naming convention of the existing fixture files in `tests/fixtures/`)
- `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` (modified — regenerated)

**Steps:**
1. Inspect `discover_corpus()` in `crates/camel-cli/tests/lint_corpus.rs` (glob scoped to `tests/fixtures`) and the existing fixture naming/layout; add `parameters-secret.yaml` following that pattern: a route whose `to:` endpoint is `http://example.com` (the production-catalog `http` scheme flags `authPassword` secret) with the secret supplied via `parameters: {authPassword: <literal>}` instead of the query string — mirroring how existing http fixtures in the corpus form their URIs.
2. Run `cargo test -p camel-cli --test lint_corpus` — it fails naming the new diagnostic; update `lint-corpus-baseline.ron` by adding the emitted R-SECRET entry (follow the RON baseline format of existing entries; regenerate mechanically from the test output, do not hand-invent fields).
3. Re-run to green.

**Tests:**
- `corpus_matches_baseline_with_parameters_fixture`: `cargo test -p camel-cli --test lint_corpus` exits 0 with the new fixture present and its R-SECRET entry in the baseline (`expected: fails before baseline regen, passes after`)
- `secret_in_parameters_has_in_map_span`: the baseline entry's span for the new fixture points inside the parameters map (verify by reading the regenerated RON entry; assert the byte range falls within the `parameters:` block of the fixture)

**Acceptance:**
- `cargo test -p camel-cli --test lint_corpus` exits 0
- `cargo test -p camel-lsp` exits 0 (LSP untouched, still green)

- [x] 3.2
