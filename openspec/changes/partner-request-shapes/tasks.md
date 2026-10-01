# Tasks: partner-request-shapes

## camel-matchers

### Task 1.1: RequestShape algebra and first-mismatch judgment

**Files:**
- `crates/camel-matchers/src/lib.rs` (modified)
- `crates/camel-matchers/CONTEXT.md` (modified)
- `crates/camel-cli/src/commands/test/scenario_tests.rs` (modified)
- `crates/camel-integration-test/src/document/validate.rs` (modified)

**Steps:**
1. In `crates/camel-matchers/src/lib.rs`, directly below `pub struct RequestExpectation` (line ~62), add `pub struct RequestShape { pub method: Option<String>, pub path: Option<PathFilter>, pub query: Option<BTreeMap<String, String>>, pub body: Option<Expectation> }` deriving `Debug, Clone, PartialEq`, with doc comments in the crate's existing style: the per-request shape of a `requests` entry — the same filter trio `RequestExpectation` carries plus an optional body `Expectation` over the projected body value.
2. Add field `pub requests: Option<Vec<RequestShape>>` to `RequestExpectation` with doc comment: per-request shape asserts, positional over the filtered recorded sequence; `None` on count-only expectations.
3. Add `#[derive(Debug, Clone, PartialEq)] #[non_exhaustive] pub enum ShapeAspect { Method, Path, Query, Body }` (ADR-0049 posture) with doc comment naming the failed aspect of a shape mismatch.
4. Add the pure judgment function:
   `pub fn request_shape_mismatch<'a>(shapes: &[RequestShape], projections: impl Iterator<Item = (&'a str, &'a str, &'a Value)>) -> Option<(usize, ShapeAspect)>` — zip `shapes` with `projections` (zip's short-circuit gives the absent-element rule); for each present pair, in order, check (a) `method`: `eq_ignore_ascii_case` when `Some`, (b) `path`: the `PathFilter` semantics `matching_count` applies per request (`Exact` strict bytes, `Contains` substring, `Matches` regex), (c) `query`: the same subset predicate `matching_count` applies per request (percent-decoded pairs via `query_pairs`), (d) `body`: `expectation_matches` from line ~260. Factor the per-request method/path/query predicate out of `matching_count` into a private helper BOTH functions call, with the regex PRE-COMPILED and passed in as `Option<&regex::Regex>` — `matching_count` already documents "compiles once per call, not once per recorded request" and hoists the compile above its closure; preserve that contract. In `request_shape_mismatch`, compile each shape's `Matches` pattern exactly once per call before the loop (`Regex::new(..).ok()`, fail-closed like the existing path). Return `Some((index, aspect))` for the FIRST failing index (zero-based), `None` when every present pair matches. Projections shorter than `shapes` are NOT a mismatch of this function — length is the count bound's business.
5. Update the external struct literals broken by the field addition (mechanical compile fixes only, `requests: None`, no grammar work here — the `requests` grammar itself is Task 1.2): (a) `crates/camel-cli/src/commands/test/scenario_tests.rs:333` (the only literal outside the two test-support crates, grep-verified); (b) the full literal `Ok(PartnerExpectation { bound, method, path, query })` at `crates/camel-integration-test/src/document/validate.rs:283` — camel-cli depends on this crate non-optionally, so this lib literal must compile before the `cargo check -p camel-cli` gate can pass.
6. In the inline `mod tests` (line ~388), add unit tests listed under Tests below.
7. In `crates/camel-matchers/CONTEXT.md`, add a `request shape` language entry (`RequestShape` — the per-request shape of a `requests` entry: method, one path filter, query subset, body expectation; avoid: request template) and mention `request_shape_mismatch` in the judgment-functions list of the crate summary sentence.

**Tests:** (in `crates/camel-matchers/src/lib.rs` inline `mod tests`)
- `shape_positional_match_on_present_elements`: shapes `[{method: POST, body: JsonSubset({"k": "1"})}, {method: GET}]` judged against projections `[("POST", "/o", {"k":"1","x":2}), ("GET", "/h", null)]` → `request_shape_mismatch` returns `None`.
- `shape_first_mismatch_wins_with_index_and_aspect`: shapes `[{method: POST}, {method: GET}]` against `[("POST", "/o", null), ("POST", "/h", null)]` → returns `Some((1, ShapeAspect::Method))`.
- `shape_body_contains_over_text_projection`: shape `{body: Contains("idempotency")}` against projection `("POST", "/o", "idempotency-key-42")` (string value) → `None`.
- `shape_short_projection_is_not_a_mismatch`: shapes `[{method: POST}, {method: POST}]` against one projection `("POST", "/o", null)` → `None`.
- `shape_path_and_query_aspects`: shape `{path: Exact("/o?a=1"), query: {"a":"1"}}` against `("POST", "/o?a=1", null)` → `None`; shape `{path: Contains("/o"), query: {"a":"1"}}` against `("POST", "/o?a=2", null)` → `Some((0, ShapeAspect::Query))` (the path filter passes, the declared query pair is absent); shape `{path: Contains("/ord")}` against `("GET", "/x", null)` → `Some((0, ShapeAspect::Path))`. Aspect precedence when multiple fail: method, then path, then query, then body (the check order of step 4).
- `command`: `cargo test -p camel-matchers --lib` (scoped: systemd-run unit `fleet-partnerasserts`, `MemoryMax=12G MemorySwapMax=2G TasksMax=1024 OOMPolicy=kill`, `env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache`, `-j4`); expected: new tests fail before step 1-4 land (compile error), pass after.

**Acceptance:**
- `cargo test -p camel-matchers --lib` passes.
- `cargo clippy -p camel-matchers --all-targets -- -D warnings` exits 0.
- `cargo check -p camel-cli` succeeds (the mechanical literal fixes compile; full camel-cli clippy runs in the final workspace gates).
- `cargo fmt --check` clean on the crate.
- No new dependencies in `crates/camel-matchers/Cargo.toml` (purity law, ADR-0072).

- [x] 1.1

## camel-integration-test grammar

### Task 1.2: `requests` grammar entries and load errors

**Files:**
- `crates/camel-integration-test/src/document/validate.rs` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)
- `crates/camel-integration-test/src/runner/partner_validate_test.rs` (modified)

**Steps:**
1. In `partner_expectation_from_value` (`document/validate.rs` line ~158), extend `KEYS` with `"requests"`.
2. Add a `"requests"` match arm: reject non-arrays with `"{FIELD}: `requests` must be a list of entry maps, got {payload}"`; reject an empty list with `"{FIELD}: `requests` must not be empty — express absence as `atMost: 0`"; parse each entry through a new private helper `request_shape_from_value(entry: &Value, index: usize, entry_no: usize) -> Result<RequestShape, DocError>` so errors name the entry (for example `requests entry 2: unknown field`).
3. `request_shape_from_value` parses an entry map restricted to `method` (string), one path filter at most among `path`/`pathContains`/`pathMatches` (same semantics and compile-verification as the outer readers, exclusivity error naming both keys), `query` (string-to-string map), and `body` (through the existing `expectation_from_value(payload, index, "requests entry body")` — it already returns `camel_matchers::Expectation`). Unknown keys fail with `"{FIELD}: `requests` entry {n}: unknown field `{other}`; expected {backticked(ENTRY_KEYS)}"` where `ENTRY_KEYS = ["method", "path", "pathContains", "pathMatches", "query", "body"]`. Non-object entries fail with a message naming the entry number. The empty map is a valid existence-only shape.
4. Enforce exclusivity: `requests` present together with any of `count`/`atLeast`/`atMost` fails with `"{FIELD}: `requests` and `{key}` are exclusive: `requests` implies the count"` (check after the existing loop, next to the count/atLeast exclusivity check).
5. Synthesize the bound: when `requests` is `Some(list)` set `bound = CountBound::Exact(list.len() as u64)` (the existing no-bound error path is unreachable for `requests`, and the bound-form requirement text stays satisfied); construct the returned `PartnerExpectation` with both `bound`/filters and `requests`.
6. Update the struct literals broken by Task 1.1's field addition to carry `requests: None` (mechanical, same class): the four `PartnerExpectation { .. }` literals in `src/runner/partner_validate_test.rs` (lines ~86, ~488, ~519, ~577) and the four in `src/doc_parse_test.rs` (lines ~800, ~1552, ~1589, ~1771) — this task's test/clippy gates compile both files; leaving any to Task 1.3 would break the per-task gate.
7. In `doc_parse_test.rs`, add the load-error and parse tests listed under Tests. Follow the file's existing pattern: `partner_expectation_from_value` is `pub(super)` (document-module-private), so the neighboring partner-grammar tests are PATH-BASED — they write a temporary `.test.yaml` carrying a `validate` action with the expectation under test and parse it through `crate::parse_scenario_document`, asserting on the `DocError` (or the parsed expectation) that comes back. Only call the function directly if the file's existing partner tests already do; otherwise go through the document path.

**Tests:** (in `crates/camel-integration-test/src/doc_parse_test.rs`, beside the existing partner-expectation tests)
- `partner_requests_parse_into_shapes`: `partner_expectation_from_value` on `{"method":"POST","requests":[{"body":{"jsonSubset":{"k":"1"}}},{}]}` → Ok; expectation has `method == Some("POST")`, `bound == CountBound::Exact(2)`, `requests` len 2 with `requests[0].body == Some(Expectation::JsonSubset(...))` and `requests[1]` fully `None` fields.
- `partner_requests_bare_body_is_equals`: entry `{"body": {"idempotencyKey": "idem-42", "orderId": "ord-7"}}` → `requests[0].body == Some(Expectation::Equals(Value::Object(...)))` (dual grammar: unrecognized-key object is literal equals).
- `partner_requests_mixed_with_count_is_load_error`: `{"count": 2, "requests": [{}]}` → `DocError::Validation` whose message contains "exclusive".
- `partner_requests_empty_is_load_error`: `{"requests": []}` → message contains "atMost: 0".
- `partner_requests_unknown_entry_field_is_load_error`: entry `{"bodySubset": {}}` → message contains "unknown field `bodySubset`" and the expected entry fields.
- `partner_requests_entry_path_exclusivity_is_load_error`: entry `{"path": "/a", "pathContains": "/b"}` → message contains "exclusive".
- `partner_requests_entry_not_a_map_is_load_error`: `{"requests": ["nope"]}` → message names entry 1.
- `command`: `cargo test -p camel-integration-test --lib partner_requests` (scoped systemd-run as in Task 1.1); expected: fail before implementation, pass after.

**Acceptance:**
- `cargo test -p camel-integration-test --lib` passes (all pre-existing tests too — struct-literal `PartnerExpectation { .. }` constructions in the crate's tests gain `requests: None`).
- `cargo clippy -p camel-integration-test --all-features --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean.

- [x] 1.2

## camel-integration-test runner

### Task 1.3: shape projection, fail-fast judging, and per-aspect redacted diagnostics

**Files:**
- `crates/camel-integration-test/src/runner/partner_validate.rs` (modified)
- `crates/camel-integration-test/src/runner/partner_validate_test.rs` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified)

**Steps:**
1. Add `#[cfg(feature = "http")] pub(crate) fn filtered_projections<'a>(requests: &'a [HttpWireRequest], method: Option<&str>, path_filter: Option<&PathFilter>, query: Option<&BTreeMap<String, String>>) -> Vec<(&'a str, &'a str, Value)>`: filter each request by calling the EXISTING `camel_matchers::matching_count` with a single-element iterator — `matching_count(std::iter::once((request.method.as_str(), request.path.as_str())), method, path_filter, query) == 1` — zero new public API, zero predicate duplication, byte-identical filter semantics with the count logic; map each surviving `HttpWireRequest` to `(method.as_str(), path.as_str(), reply_bytes_value(&request.body))` — the existing `pub(crate) fn reply_bytes_value` in `src/runner.rs` (line ~844). At the call site, bridge the owned `Value`s to the judgment's `&Value` items with `.iter().map(|(m, p, v)| (*m, *p, v))`. Do not re-implement the predicates here: the pure predicates live in camel-matchers; this layer only sequences.
2. In `partner_validate_action`'s per-snapshot decision (both the no-deadline and deadline paths), when `expected.requests` is `Some(shapes)`: (a) treat a filtered count ABOVE `shapes.len()` as IMMEDIATE failure — the existing `above_ceiling` returns `false` for `Exact`, so the generic Exact path does NOT fail fast on over-count; the shape path must add this check itself; (b) call `camel_matchers::request_shape_mismatch(shapes, filtered_projections(...))` and on `Some((idx, aspect))` fail immediately (append-only recorder: a present mismatch never heals); (c) settle only when the filtered count equals `shapes.len()` AND the mismatch is `None`. Do NOT change `above_ceiling` or the count-only `Exact` semantics — the baseline "overshoot never passes" scenarios pin them. Keep single-snapshot-per-iteration discipline: derive count and shape evidence from the SAME snapshot clone.
3. Extend the failure path to render shape mismatches: a new `#[cfg(feature = "http")] pub(crate) fn shape_mismatch_detail(partner: &str, mismatch: (usize, ShapeAspect), expected: &PartnerExpectation, observed: &[HttpWireRequest], secret_keys: &[String]) -> String` producing text of the form `partner PARTNER: request N (of the filtered sequence) ASPECT: expected EXPECTED, got OBSERVED` with `N = idx + 1` and per-aspect both-sides rendering: `Method` — plain method texts; `Path` — expected side renders exactly as `render_filters` renders a path filter (`Exact` through `redact_wire_path(path, secret_keys)`, `Contains`/`Matches` kind-only with payload elided), observed side renders `redact_wire_path(observed.path, secret_keys)`; `Query` — expected side renders `key=value` pairs with secret-set keys as `key=<redacted>` (mirroring `render_filters`), observed side renders the redacted wire path; `Body` — both sides render through a small render helper EXTRACTED from the inline `format!` arms of the message-validate dispatch in `src/runner.rs` (around lines 983-1075, the `expected ..., got ...` arms): extract the existing formatting into a `pub(crate)` helper that BOTH the message-validate arms and `shape_mismatch_detail` call, keeping the produced strings byte-identical for the message-validate path. Headers NEVER render for any aspect. Wire the detail into the action's verdict failure alongside the existing `partner_mismatch_detail` counts evidence.
4. Add the unit tests listed under Tests. (The `PartnerExpectation` literal updates in `partner_validate_test.rs` and `doc_parse_test.rs` already landed in Task 1.2 — do not repeat them here.)

**Tests:** (in `src/runner/partner_validate_test.rs`)
- `shape_indexes_the_filtered_sequence`: recorded recorder-order sequence `GET /health`, `POST /order`, `GET /health`, `POST /order`; expectation `method: POST` with `requests: [{path: Exact("/order")}, {}]` (empty second entry) → the judgment passes (entries assert the two POSTs; the health requests never occupy an index) while the same expectation with the entries swapped to `[{}, {method: "GET"}]` fails naming request 2 aspect `method`.
- `shape_mismatch_names_one_based_index_and_aspect`: two recorded `POST /o` requests, second body `{"k":"2"}`; expectation `requests: [{body Equals {"k":"1"}}, {body Equals {"k":"1"}}]` → the judgment itself returns `Some((1, ShapeAspect::Body))` (direct `camel_matchers::request_shape_mismatch` assert — the Body aspect branch has no other negative test) and the produced detail names "request 2" and aspect `body`, and contains expected `{"k":"1"}` and observed `{"k":"2"}` renderings.
- `shape_mismatch_query_redacts_both_sides`: expectation entry `query: {"token": "s3cr3t"}` with `secret_keys = ["token"]`, recorded path `/o?token=leak` → detail contains `token=<redacted>` (expected side) and the observed path with the token value masked; raw `s3cr3t`/`leak` strings absent.
- `shape_mismatch_path_renders_kind_only_for_contains`: entry `path: Contains("/ord")` vs recorded `/x` → expected side renders `pathContains <pattern elided>`; the substring `/ord` payload absent.
- `shape_judgment_fail_fast_in_window`: a recorded sequence whose FIRST filtered element already mismatches, judged through the snapshot decision helper with a deadline-style loop (reuse the existing poll-harness pattern in this test file) → the verdict fails without waiting the full window (assert elapsed < the full deadline or assert immediate fail return, following the file's existing atMost-fail-fast test shape).
- `shape_over_length_fails_fast_in_window`: expectation `requests: [{method: POST}]` (len 1) while the recorder already holds TWO filtered requests, deadline window open → the verdict fails immediately, not at expiry (the count-only `Exact` path sleeps to expiry on over-count; the shape path must not).
- `command`: `cargo test -p camel-integration-test --lib --features http partner` (the crate has NO default features — `--lib` alone compiles an empty feature set; the real gates are `--features http` and `--all-features`) (scoped systemd-run as in Task 1.1); expected: fail before implementation, pass after.

**Acceptance:**
- `cargo test -p camel-integration-test --lib --all-features` and `--features http` both pass.
- `cargo clippy -p camel-integration-test --all-features --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-log-redaction` and `cargo xtask lint-non-exhaustive` exit 0.
- No header value from `HttpWireRequest.headers` appears in any produced diagnostic string (grep the new code: only `method`, `path`, `body` are read).

- [x] 1.3

## camel-integration-test scenario

### Task 1.4: retry-identical-bodies scenario fixture and e2e test

**Files:**
- `crates/camel-integration-test/tests/fixtures/partner-shape-retry.test.yaml` (new)
- `crates/camel-integration-test/tests/fixtures/partner-shape-retry-route.yaml` (new)
- `crates/camel-integration-test/tests/partner_verification_test.rs` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)

**Steps:**
1. Author `partner-shape-retry-route.yaml` modeled on the existing `fixtures/retry-route.yaml`: a route from a `direct:` stimulus to an `http` partner target with `error_handler.retry` (same retry policy family as the existing fixture — maximumRedeliveries enough for exactly one resend on a 500).
2. Author `partner-shape-retry.test.yaml`: a `scenario:` document with one harness-provisioned `http` partner whose `partners:` script answers `500` on the first request then `200` (the existing `PartnerFault`/scripted-response grammar used by the retry fixtures), a `send` of a fixed JSON body `{"idempotencyKey": "idem-42", "orderId": "ord-7"}` to the route's direct stimulus, and a final `validate` targeting the partner through the same reference form the file's existing partner-validate fixtures use, with `deadline: 10s` and `expectation: {method: POST, requests: [{body: {idempotencyKey: "idem-42", orderId: "ord-7"}}, {body: {idempotencyKey: "idem-42", orderId: "ord-7"}}]}` — each bare body map is a literal `equals` proving retry-identical projected body values.
3. Add test `shape_asserts_prove_retry_identical_bodies` to `tests/partner_verification_test.rs`, modeled on the file's FLAGSHIP_DOC retry pattern (inline `const` document at ~line 552 loading `routeFiles: [retry-route.yaml]`, fixture resolved via the `MANIFEST_DIR` join at ~line 802) — either an inline document const or a fixture file pair is acceptable if it matches how neighboring tests in this file are written; assert `DocumentOutcome.verdict == Some(Pass)`; no sleeps (`lint-test-sleep` forbids them — the deadline poll is the only wait).
4. In `crates/camel-integration-test/CONTEXT.md`, extend the `partner validate target` language entry with the `requests` grammar (positional over the filtered sequence, XOR bound forms synthesizing the exact count, entry keys, body dual grammar, fail-fast on present mismatched elements) and add a `request shape` entry pointing at `camel_matchers::RequestShape`.

**Tests:**
- `shape_asserts_prove_retry_identical_bodies` (tests/partner_verification_test.rs): fixture document from steps 1-2 exists → run the document → verdict is `Pass`, and the run recorded exactly two partner requests (the harness's recorded-request count, when the common harness exposes it — otherwise assert via the passing shape expectation itself).
- `command`: `cargo test -p camel-integration-test --test partner_verification_test --all-features shape_asserts -- --test-threads=1` (scoped systemd-run as in Task 1.1); expected: fails before fixture exists, passes after.

**Acceptance:**
- `cargo test -p camel-integration-test --test partner_verification_test --all-features` passes (new + all pre-existing).
- `cargo test -p camel-integration-test --no-default-features --tests` still compiles (feature-gating intact: if the fixture test needs `http`, gate it `#[cfg(feature = "http")]` like the file's other http tests).
- `cargo xtask lint-test-sleep` exits 0 (no sleeps added).
- `cargo fmt --check` clean.

- [x] 1.4
