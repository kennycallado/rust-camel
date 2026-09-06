# Tasks: scenario-assertions

## Phase 1: Partner body parity (rc-cv8u2)

### Task 1.1: Route partner script bodies through value_to_wire semantics

**Files:**
- `crates/camel-integration-test/src/adapters/http.rs` (modified)
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/partner_script.rs` (modified)
- `crates/camel-integration-test/tests/http_partner_scripting_test.rs` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)

**Steps:**
1. In `crates/camel-integration-test/src/adapters/http.rs`, change `fn value_to_wire` (line ~966) from private to `pub(crate) fn value_to_wire`. Body unchanged: `Value::Null => Vec::new()`, `Value::String(text) => text.clone().into_bytes()`, `other => other.to_string().into_bytes()`.
2. In `crates/camel-integration-test/src/document.rs` `partner_scripts_for` (line ~246), replace `serde_json::to_vec(value).unwrap_or_default()` with `crate::adapters::http::value_to_wire(value)`. Verify no `serde_json::to_vec` call on a body remains in the function.
3. In `crates/camel-integration-test/src/partner_script.rs`, update the doc comment on the wire-form `body` mapping (the `body` field comment around line 63 — it currently describes the JSON serialization of the body) to state the `value_to_wire` semantics: string → exact bytes, `null` → empty, other → compact JSON, absent → empty.
4. Audit `crates/camel-integration-test/src/` and `tests/` for tests asserting a QUOTED partner-served body (e.g. expecting `"text"` with quotes from a string script body). If any exist, they pinned the bug: update them to the verbatim expectation and say so in the report. If none exist, state "no existing test pinned the double-encoding" in the report.
5. Add the `partners:` body-encoding vocabulary entry to `crates/camel-integration-test/CONTEXT.md` (partner body encoding mirrors the client send path: string verbatim, null empty, structured compact).

**Tests:**
- name: `plain_string_body_served_verbatim`
  setup: a test scenario document in `tests/http_partner_scripting_test.rs` with a `partners:` entry whose `response.body` is the YAML string `exact text` (declared as `body: "exact text"`, a JSON string — not a mapping) and NO `content-type` header scripted (or `text/plain`) — with `application/json` the decoder's string-fallback masks the bug (`wire_body_to_value` parses the quoted form back to the same string)
  action: client-role `send` to the partner endpoint, then `receive` on it (sends do NOT populate `lastReceived` — `vars.remember` is called only from `receive_action`), then `validate` with a `lastReceived` target on that endpoint and `expectation: {equals: "exact text"}`
  assert: the action passes and the wire bytes equal `exact text` — no quotes, no escapes; with no JSON content-type the validated string is the raw wire bytes decoded as text
  command: `cargo test -p camel-integration-test --features http --test http_partner_scripting_test plain_string_body_served_verbatim`
  expected: fails before the fix (wire is `"exact text"` with quotes → equals mismatch under text decode), passes after
- name: `null_body_serves_empty`
  setup: a `partners:` entry whose `response.body` is explicit `null`, no JSON content-type
  action: client-role `send`, `receive`, then validate `lastReceived` with `expectation: {equals: ""}` — an empty wire body decodes to the empty string under text content-types
  assert: the response body is empty (zero bytes), not the 4-byte literal `null`
  command: `cargo test -p camel-integration-test --features http --test http_partner_scripting_test null_body_serves_empty`
  expected: fails before the fix (validated `"null"`), passes after
- name: `partner_client_body_parity` (unit test in `crates/camel-integration-test/src/partner_script.rs` tests module, following that file's existing unit-test pattern)
  setup: a table of `serde_json::Value` cases paired with LITERAL expected byte constants — `Value::String("a\"b".into())` → `b"a\"b"` (raw bytes with the quote, no JSON escaping), `Value::Null` → `b""` (empty), `json!({"k": 1})` → `b"{\"k\":1}"` (compact, no space after `:`), `json!([1, 2])` → `b"[1,2]"`
  action: for each case, call `crate::adapters::http::value_to_wire(value)` — the function the partner call site now delegates to — and compare against the literal byte constant (NOT against itself)
  assert: byte-equality for every case; the string case is raw UTF-8 without surrounding quotes; the null case is empty; the object case is compact
  command: `cargo test -p camel-integration-test --features http --lib partner_client_body_parity`
  expected: fails before (partner path used `serde_json::to_vec`: the string case yields `"a\"b"` WITH surrounding quotes and escaped inner quote; the null case yields `b"null"`), passes after
- name: existing suites stay green
  setup: the full http-gated suites of the crate
  action: run both feature configurations
  assert: all pass
  command: `cargo test -p camel-integration-test --features http && cargo test -p camel-integration-test`
  expected: pass before and after (the fix must not regress existing scripting tests)

**Acceptance:**
- `rg -n "serde_json::to_vec" crates/camel-integration-test/src/document.rs` returns no match inside `partner_scripts_for`
- `cargo test -p camel-integration-test --features http --test http_partner_scripting_test` exits 0
- `cargo test -p camel-integration-test --features http` and `cargo test -p camel-integration-test` (no-feature) both exit 0
- `cargo clippy -p camel-integration-test --features http --all-targets -- -D warnings` exits 0
- `cargo fmt --check --all` exits 0

- [x] 1.1

## Phase 2: Matcher expressiveness (rc-s0e5)

### Task 2.1: Partner-expectation grammar — bounds, path filters, query subset

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified — type usage only)
- `crates/camel-integration-test/src/lib.rs` (modified only if re-exports need adjusting)

**Steps:**
1. In `document.rs`, replace the `PartnerExpectation` struct (line ~297) with:
   `pub struct PartnerExpectation { pub bound: CountBound, pub method: Option<String>, pub path: Option<PathFilter>, pub query: Option<BTreeMap<String, String>> }`.
2. Add `#[non_exhaustive] pub enum CountBound { Exact(u64), AtLeast(u64), AtMost(u64), Range(u64, u64) }` with doc comments stating the poll semantics each bound carries (design.md Decision 2 table). Add `#[non_exhaustive] pub enum PathFilter { Exact(String), Contains(String), Matches(String) }` — `#[non_exhaustive]` is the crate's public-enum posture (ADR-0049, enforced by `cargo xtask lint-non-exhaustive`; `Expectation` is the precedent). `Matches` stores the pattern string; compile-verification happens at load (step 4), matching the `Expectation::Regex` precedent.
3. Rewrite `partner_expectation_from_value` (line ~1034): accepted keys are `count`, `atLeast`, `atMost`, `method`, `path`, `pathContains`, `pathMatches`, `query`. Rules with exact error messages naming the field: (a) exactly one bound form — `count` exclusive with `atLeast`/`atMost`; `atLeast`+`atMost` combine into `CountBound::Range` and REQUIRE `atLeast <= atMost` (inverted → error "partner expectation: atLeast 4 exceeds atMost 3"-style, naming both values); bound values extract via `payload.as_u64()` exactly as `count` does today (negative, fractional, or non-numeric → the same non-negative-integer error naming the key); (b) at most ONE of `path`/`pathContains`/`pathMatches` (two → error naming both keys); (c) `pathMatches` value compiled with `regex::Regex::new` — failure → `doc-validation` naming the pattern and the regex error; (d) `query` must be a map of string keys to STRING values (non-string value → error naming the key); (e) unknown key → existing error style updated to list the accepted keys.
4. Update every construction/usage site of the old `{ count, method, path }` shape: `runner.rs` reads `expected.count` in `partner_validate_action` — leave a temporary `CountBound::Exact`-equivalent read for now (Task 2.3 replaces the logic); adapt `matching_requests` signature in Task 2.2 — for THIS task make the crate compile by having `matching_requests` accept `Option<&str>` path derived from `PathFilter::Exact` only (temporary shim, replaced next task; note it in the report). Update `PartnerExpectation` re-export in `lib.rs` (~line 38) if the path changed.
5. Update existing doc-parse tests constructing partner expectations to the new shape (they are the "constructor tests" the design discloses as part of the pre-1.0 API break).

**Tests (all in `doc_parse_test.rs`, following that file's existing style):**
- name: `partner_at_least_parses`
  setup: a raw validate action with partner target and `expectation: {atLeast: 3}`
  action: parse the document
  assert: `PartnerExpectation { bound: CountBound::AtLeast(3), method: None, path: None, query: None }`
  command: `cargo test -p camel-integration-test --lib partner_at_least_parses`
  expected: fails before (unknown field `atLeast`), passes after
- name: `partner_range_parses_and_inverted_fails`
  setup: one document with `{atLeast: 2, atMost: 4}`; one with `{atLeast: 4, atMost: 3}`
  action: parse both
  assert: first yields `CountBound::Range(2, 4)`; second yields `DocError::Validation` naming both bound values
  command: `cargo test -p camel-integration-test --lib partner_range_parses_and_inverted_fails`
  expected: fails before, passes after
- name: `partner_count_mixed_with_at_least_fails`
  setup: expectation `{count: 2, atLeast: 1}`
  action: parse
  assert: `DocError::Validation` naming `count` and `atLeast` as mutually exclusive
  command: `cargo test -p camel-integration-test --lib partner_count_mixed_with_at_least_fails`
  expected: fails before (no rule), passes after
- name: `partner_two_path_filters_fail`
  setup: expectation `{count: 1, path: "/a", pathContains: "b"}`
  action: parse
  assert: `DocError::Validation` naming both path keys
  command: `cargo test -p camel-integration-test --lib partner_two_path_filters_fail`
  expected: fails before, passes after
- name: `partner_invalid_path_matches_fails`
  setup: expectation `{count: 1, pathMatches: "([unclosed"}`
  action: parse
  assert: `DocError::Validation` naming the pattern
  command: `cargo test -p camel-integration-test --lib partner_invalid_path_matches_fails`
  expected: fails before, passes after
- name: `partner_query_subset_parses`
  setup: expectation `{atLeast: 1, query: {a: "1+1", b: "2"}}`
  action: parse
  assert: `query == Some(BTreeMap { "a" → "1+1", "b" → "2" })`
  command: `cargo test -p camel-integration-test --lib partner_query_subset_parses`
  expected: fails before, passes after
- name: `partner_query_non_string_value_fails`
  setup: expectation `{atLeast: 1, query: {a: 1}}` (numeric value)
  action: parse
  assert: `DocError::Validation` naming key `a`
  command: `cargo test -p camel-integration-test --lib partner_query_non_string_value_fails`
  expected: fails before, passes after
- name: `partner_missing_bound_fails` (adapted from the old missing-count test)
  setup: expectation `{method: GET}` — no `count`/`atLeast`/`atMost`
  action: parse
  assert: `DocError::Validation` requiring a bound
  command: `cargo test -p camel-integration-test --lib partner_missing_bound_fails`
  expected: passes in adapted form (the old test asserted missing `count`; keep both phrasings consistent with the new message)
- name: `partner_at_least_non_integer_fails`
  setup: two documents — expectation `{atLeast: -1}` and expectation `{atLeast: 1.5}`
  action: parse both
  assert: both yield `DocError::Validation` naming `atLeast` and requiring a non-negative integer
  command: `cargo test -p camel-integration-test --lib partner_at_least_non_integer_fails`
  expected: fails before (`atLeast` unknown-key error differs from the integer error), passes after

**Acceptance:**
- `cargo test -p camel-integration-test --lib` exits 0 (all doc-parse tests)
- `cargo test -p camel-integration-test` (no-feature) exits 0 — grammar parsing is un-gated
- `cargo clippy -p camel-integration-test --all-targets -- -D warnings` exits 0 (both feature configs compile)
- Existing load-error scenarios (undeclared partner URI, deadline on non-partner) still pass unchanged

- [x] 2.1

### Task 2.2: Matching layer — pathContains/pathMatches/query subset filters

**Files:**
- `crates/camel-integration-test/Cargo.toml` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/src/runner_test.rs` (modified)

**Steps:**
1. Add `form_urlencoded` as a direct OPTIONAL dependency of `camel-integration-test` in its `Cargo.toml`: `form_urlencoded = { version = "1.2", optional = true }` and add it to the crate's `http` feature list (crate convention: wire-only deps are optional behind `http`, like `bytes`/`http`/`hyper`/`serde_json`; it is NOT declared in `[workspace.dependencies]` — version 1.2.x matches the lockfile via `url`).
2. In `runner.rs`, add `fn query_pairs(path_and_query: &str) -> Vec<(String, String)>` (cfg: same as `matching_requests`, i.e. `#[cfg(feature = "http")]`): split at the first `?`, parse the remainder with `form_urlencoded::parse` (percent-decoding `+` and `%XX`), collect `(String, String)` pairs.
3. Replace the temporary shim from Task 2.1: change `matching_requests(requests, method, path_filter: Option<&PathFilter>, query: Option<&BTreeMap<String, String>>) -> usize`. Filter logic: method ASCII-case-insensitive (unchanged); path filter — `Exact(p)` equals `request.path` byte-for-byte, `Contains(s)` `request.path.contains(s)`, `Matches(pat)` compiles `Regex::new(pat)` once per call and `is_match(&request.path)`; query subset — for every declared `(k, v)`, `query_pairs(&request.path)` contains a pair equal to `(k, v)` (decoded comparison). All filters AND.
4. Keep the function `pub(crate)` and its existing cfg attribute; update the doc comment to describe the three path-filter kinds and the subset rule (leniency lives HERE, never in the lane key — cite Wave A law).

**Tests (in `runner_test.rs`, cfg-gated to feature `http` following the file's existing pattern; build `HttpWireRequest` values directly as existing tests do):**
- name: `matching_exact_path_is_byte_strict`
  setup: requests with paths `/q?bbox=1.5%2C2.5` and `/q?bbox=1.5,2.5`
  action: `matching_requests(&reqs, None, Some(&PathFilter::Exact("/q?bbox=1.5%2C2.5")), None)`
  assert: 1 — the comma form does not match (strict bytes preserved)
  command: `cargo test -p camel-integration-test --features http --lib matching_exact_path_is_byte_strict`
  expected: fails before (no PathFilter), passes after
- name: `matching_contains_tolerates_encoding`
  setup: the same two requests
  action: filter `Some(&PathFilter::Contains("bbox="))`
  assert: 2
  command: `cargo test -p camel-integration-test --features http --lib matching_contains_tolerates_encoding`
  expected: fails before, passes after
- name: `matching_regex_narrows`
  setup: requests `/orders/42`, `/health`
  action: filter `Some(&PathFilter::Matches("^/orders/\\d+$"))`
  assert: 1
  command: `cargo test -p camel-integration-test --features http --lib matching_regex_narrows`
  expected: fails before, passes after
- name: `matching_query_subset_decodes_and_ignores_order`
  setup: request `/q?b=2&a=1%2B1`
  action: query `Some(&{"a": "1+1", "b": "2"})`, no path filter
  assert: 1
  command: `cargo test -p camel-integration-test --features http --lib matching_query_subset_decodes_and_ignores_order`
  expected: fails before, passes after
- name: `matching_query_subset_absent_pair_excludes`
  setup: request `/q?a=1`
  action: query `Some(&{"a": "1", "c": "3"})`
  assert: 0
  command: `cargo test -p camel-integration-test --features http --lib matching_query_subset_absent_pair_excludes`
  expected: fails before, passes after
- name: `matching_method_composes_with_query`
  setup: two requests `/q?a=1`, methods `POST` and `GET`
  action: method `Some("post")` (lowercase declared), query `Some(&{"a": "1"})`
  assert: 1
  command: `cargo test -p camel-integration-test --features http --lib matching_method_composes_with_query`
  expected: fails before, passes after

**Acceptance:**
- `cargo test -p camel-integration-test --features http --lib` exits 0
- `cargo check -p camel-integration-test` (no-feature) exits 0
- `cargo clippy -p camel-integration-test --features http --all-targets -- -D warnings` exits 0
- No lane-key code touched: `rg -n "lane_key|path_and_query" crates/camel-integration-test/src/adapters/http.rs` shows unchanged adapter keying (no diffs in that region)

- [x] 2.2

### Task 2.3: Bound-aware partner validation + redacted diagnostics

**Files:**
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/src/runner_test.rs` (modified)
- `crates/camel-integration-test/tests/partner_verification_test.rs` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)

**Steps:**
1. Rewrite `partner_validate_action` (line ~690) to decide per `CountBound` (design.md Decision 2 table):
   - No deadline: one immediate snapshot decides for every bound.
   - With deadline: `Exact` — current behavior unchanged (poll until equal; above never passes; final snapshot reports actual). `AtLeast(n)` — return Ok as soon as a snapshot count `>= n`; at expiry the final snapshot decides, failing below `n`. `AtMost(n)` — fail immediately when any snapshot count `> n`; otherwise sleep the REMAINING window (poll at `PARTNER_POLL_INTERVAL` to re-check the ceiling) and decide on the final snapshot (`<= n` passes). `Range(min, max)` — fail immediately above `max`; otherwise wait the full window and decide on the final snapshot (within `[min, max]` passes).
2. Extend `partner_mismatch_detail` (line ~792): the bound renders in its own grammar via a new `fn render_bound(bound: &CountBound) -> String` — `Exact(3)` → `expected 3`, `AtLeast(3)` → `expected at least 3`, `AtMost(2)` → `expected at most 2`, `Range(2,4)` → `expected between 2 and 4`. Keep the `Exact` rendering byte-identical to today's so existing mismatch tests stay green.
3. Redaction of filter payloads in diagnostics (Wave A law extension): add `fn render_filters(expected: &PartnerExpectation, secret_keys: &[String]) -> String` (slice type — `router.secret_query_keys()` returns `Vec<String>` and `partner_mismatch_detail` already takes `&[String]`; membership checks may linearly scan or collect a set inside) — method renders `method GET`; path filter renders by KIND only for `Contains`/`Matches` (e.g. `pathContains <pattern elided>`, `pathMatches <pattern elided>` — never the payload); `Exact` path renders via `redact_wire_path`; `query` renders each pair `k=v` EXCEPT keys in `secret_keys` which render `k=<redacted>`. Thread `router.secret_query_keys()` in (the call site already has it).
4. Add the matcher vocabulary entry to `CONTEXT.md` (bounds with window semantics, path-filter kinds, query subset, lane-key strictness unchanged).

**Tests:**
- Integration tests in `tests/partner_verification_test.rs` (the file's existing fixture style — partner endpoints, scripted sends/boots as present there):
  - name: `at_least_settles_early`
    setup: a partner holding 2 recorded requests with 2 more landing near-simultaneously shortly after (snapshots jump 2 → 4 without ever observing exactly 3 — e.g. a burst delivery; the fixture must guarantee no snapshot sees 3, mirroring the existing "never settles" fixture's jump-past technique)
    action: validate `{atLeast: 3}` with `deadline: 5s`
    assert: passes once the burst lands (4 ≥ 3) without waiting the full deadline — RED-state note: before Task 2.3's semantics (grammar from 2.1 + matching from 2.2 already in), Exact-read of `atLeast: 3` never equals 4, so this fails red
    command: `cargo test -p camel-integration-test --features http --test partner_verification_test at_least_settles_early`
    expected: red before Task 2.3 (Exact semantics: snapshots 2, 4, 4… never equal 3), green after (atLeast: 4 ≥ 3 passes)
  - name: `at_least_fails_at_deadline_naming_actual`
    setup: a partner holding exactly 1 recorded request
    action: validate `{atLeast: 3}` with `deadline: 1s`
    assert: `validation-mismatch` whose detail names `at least 3` and the actual count 1
    command: `cargo test -p camel-integration-test --features http --test partner_verification_test at_least_fails_at_deadline_naming_actual`
    expected: fails before, passes after
  - name: `at_most_decides_immediately_without_deadline`
    setup: partner holding 1 request
    action: validate `{atMost: 2}` (no deadline)
    assert: passes on the immediate snapshot — RED-state note: before Task 2.3, Exact-read of `atMost: 2` compares 1 == 2 and FAILS; bound semantics pass (1 ≤ 2)
    command: `cargo test -p camel-integration-test --features http --test partner_verification_test at_most_decides_immediately_without_deadline`
    expected: red before Task 2.3 (Exact: 1 ≠ 2), green after (atMost: 1 ≤ 2)
  - name: `at_most_waits_full_window`
    setup: partner holding 1 matching request; no further matching request will arrive during the window (a nonmatching request MAY land via script — it must not satisfy the method filter)
    action: validate `{atMost: 1, method: POST}` with `deadline: 2s`
    assert: total observed wait ≈ the full 2s window (measure elapsed around the scenario run, asserting elapsed >= 1.9s with scheduler tolerance), then passes on the final snapshot
    command: `cargo test -p camel-integration-test --features http --test partner_verification_test at_most_waits_full_window`
    expected: fails before, passes after
  - name: `at_most_fails_fast_above_bound`
    setup: a route/script delivering 3 matching requests quickly
    action: validate `{atMost: 2}` with `deadline: 5s`
    assert: fails with `validation-mismatch` (`at most 2`, actual 3) well BEFORE the 5s window (elapsed < 2s)
    command: `cargo test -p camel-integration-test --features http --test partner_verification_test at_most_fails_fast_above_bound`
    expected: fails before, passes after
  - name: `at_most_zero_proves_absence_over_window`
    setup: partner bound but never dialed with a matching request
    action: validate `{atMost: 0}` with `deadline: 1s`; measure elapsed around the scenario run
    assert: passes on the final snapshot AFTER the window, with observed elapsed >= 0.9s — the elapsed measurement is the red-state discriminator: before Task 2.3, Exact-read of `atMost: 0` compares 0 == 0 and passes INSTANTLY (elapsed ≈ 0), so the elapsed assertion fails red; bound semantics wait the window (elapsed ≈ 1s) and pass
    command: `cargo test -p camel-integration-test --features http --test partner_verification_test at_most_zero_proves_absence_over_window`
    expected: red before Task 2.3 (instant pass fails the elapsed >= 0.9s assertion), green after (window waited, final snapshot passes)
  - name: `range_fails_fast_above_max`
    setup: count settles at 5 quickly
    action: validate `{atLeast: 2, atMost: 4}` with `deadline: 5s`
    assert: fails immediately on observing 5 (elapsed < 2s), naming `between 2 and 4`
    command: `cargo test -p camel-integration-test --features http --test partner_verification_test range_fails_fast_above_max`
    expected: fails before, passes after
  - name: `range_passes_on_final_snapshot`
    setup: count settles at 3
    action: validate `{atLeast: 2, atMost: 4}` with `deadline: 1s`
    assert: passes on the final snapshot after the window
    command: `cargo test -p camel-integration-test --features http --test partner_verification_test range_passes_on_final_snapshot`
    expected: fails before, passes after
  - name: `path_matches_and_query_subset_end_to_end`
    setup: a route sending TWO requests that both match the filters — `/q?bbox=1.5%2C2.5` and `/q?x=1&bbox=1.5%2C2.5` (both match `^/q\?` and both carry the decoded pair `bbox=1.5,2.5`) — plus one `/health` request; the partner records all three
    action: validate `{atLeast: 1, pathMatches: "^/q\\?", query: {bbox: "1.5,2.5"}}`
    assert: passes — regex and query subset together narrow to exactly the 2 matching requests without pinning encoded byte sequences or pair order — RED-state note: before Task 2.3 (grammar 2.1 + matching 2.2 in), the filtered count is 2 and Exact-read of `atLeast: 1` compares 2 == 1 and FAILS; bound semantics pass (2 ≥ 1)
    command: `cargo test -p camel-integration-test --features http --test partner_verification_test path_matches_and_query_subset_end_to_end`
    expected: red before Task 2.3 (Exact: 2 ≠ 1), green after (atLeast: 2 ≥ 1)
  - name: `path_contains_tolerates_encoding_drift_end_to_end`
    setup: the same two `/q?bbox=…` requests (both contain `bbox=`), partner records both
    action: validate `{atLeast: 1, pathContains: "bbox="}`
    assert: passes — both requests match the substring without pinning encoded byte sequences; RED-state note: before Task 2.3, Exact-read of `atLeast: 1` compares 2 == 1 and FAILS; bound semantics pass (2 ≥ 1)
    command: `cargo test -p camel-integration-test --features http --test partner_verification_test path_contains_tolerates_encoding_drift_end_to_end`
    expected: red before Task 2.3 (Exact: 2 ≠ 1), green after (atLeast: 2 ≥ 1)
- Diagnostics tests (in `runner_test.rs`, unit level — construct a `PartnerExpectation` and call `render_bound`/`render_filters` directly, following the file's unit-test style):
  - name: `render_bound_grammar`
    setup: the four `CountBound` values
    action: `render_bound` each
    assert: `expected 3`, `expected at least 3`, `expected at most 2`, `expected between 2 and 4`
    command: `cargo test -p camel-integration-test --features http --lib render_bound_grammar`
    expected: fails before, passes after
  - name: `render_filters_redacts_secret_query_and_elides_patterns`
    setup: expectation with `query: {"token": "abc", "bbox": "1,2"}`, `PathFilter::Contains("secret")`; `secret_keys = {"token"}`
    action: `render_filters`
    assert: contains `token=<redacted>` and `bbox=1,2`; contains `pathContains <pattern elided>`; does NOT contain `abc` or `secret` verbatim
    command: `cargo test -p camel-integration-test --features http --lib render_filters_redacts_secret_query_and_elides_patterns`
    expected: fails before, passes after
- Regression: the file's existing exact-count tests (immediate pass, mismatch naming, deadline polling, never-settles) must remain green UNCHANGED — if any test's expected detail string must move, justify it in the report (the `Exact` rendering is byte-identical by design, so no move is expected).
  command: `cargo test -p camel-integration-test --features http --test partner_verification_test`
  expected: all pass

**Acceptance:**
- `cargo test -p camel-integration-test --features http` exits 0 (full suite)
- `cargo test -p camel-integration-test` (no-feature) exits 0
- `cargo clippy -p camel-integration-test --features http --all-targets -- -D warnings` exits 0
- `cargo fmt --check --all` exits 0
- Wave A pin tests (lane-key strictness, redaction sweep) pass unchanged: `cargo test -p camel-integration-test --features http redact` exits 0

- [x] 2.3

## Phase 3: Minimum-elapsed assertion (rc-1alu)

### Task 3.1: Wire-arrival instants on IncomingMessage

**Files:**
- `crates/camel-integration-test/src/adapters.rs` (modified)
- `crates/camel-integration-test/src/adapters/http.rs` (modified)
- `crates/camel-integration-test/src/adapters_test.rs` (modified)

**Steps:**
1. In `adapters.rs`, add a `pub arrival: std::time::Instant` field to `IncomingMessage` (line ~57) with a doc comment: "Monotonic wire-arrival instant — when the transport finished receiving this message — NOT when a receive action consumed it (ADR-0069 §5: the wire is the proof)."
2. Update EVERY `IncomingMessage` construction site: the http server-role listener stamps `std::time::Instant::now()` where the transport finishes receiving (the `enqueue_arrival` site, before queueing — the stamp belongs at record time, not dequeue time); the http client-role response path stamps at response receipt; the scripted-queue fixture constructor (`adapters.rs` ~585 `IncomingMessage::scripted` queue) stamps at construction. Find all sites with `rg -n "IncomingMessage \{" crates/camel-integration-test/src/` — every hit must set the field.
3. Do NOT change any trait signature; the field travels inside the struct.

**Tests (in `adapters_test.rs` or `http_partner_test.rs` — wherever the existing adapter unit tests live; cfg `http` where needed):**
- name: `incoming_message_carries_arrival_instant`
  setup: a scripted-queue adapter with one queued message
  action: call `receive` on it (the adapter trait's receive, with the arguments the file's existing scripted-queue tests use)
  assert: the returned `IncomingMessage.arrival` is set (elapsed since stamp is small, < 5s sanity) — and equals the construction stamp, not the receive time (stamp then `tokio::time::sleep(50ms)` then receive; `arrival.elapsed() >= 50ms` proves it predates the receive)
  command: `cargo test -p camel-integration-test --features http --lib incoming_message_carries_arrival_instant`
  expected: fails before (field does not exist), passes after
- name: `server_arrival_stamp_predates_consumption`
  setup: a bound partner listener; a direct client-role send delivers one request to it (the file's existing inbound fixtures show the pattern)
  action: send, then `tokio::time::sleep(100ms)`, then `receive` on the partner endpoint
  assert: `arrival.elapsed()` measured at receive time is >= 100ms (the stamp was taken at enqueue, before the sleep)
  command: `cargo test -p camel-integration-test --features http --lib server_arrival_stamp_predates_consumption`
  expected: fails before, passes after
- name: existing suites stay green
  command: `cargo test -p camel-integration-test --features http && cargo test -p camel-integration-test`
  expected: both pass (pure additive field; all constructors updated)

**Acceptance:**
- `rg -n "IncomingMessage \{" crates/camel-integration-test/src/` — every construction site sets `arrival`
- `cargo test -p camel-integration-test --features http` and no-feature run both exit 0
- `cargo clippy -p camel-integration-test --features http --all-targets -- -D warnings` exits 0

- [x] 3.1

### Task 3.2: elapsedAtLeast grammar + wire-arrival validation

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/tests/http_partner_scripting_test.rs` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)

**Steps:**
1. In `document.rs`, the validate-action grammar gains optional `elapsedAtLeast: <humantime>` (parse via the same humantime helper `deadline` uses). Pairing rule mirroring the deadline-partner-only rule: `elapsedAtLeast` is valid ONLY on a validate whose target is `lastReceived`; on `partner` or `variable` targets → `DocError::Validation` naming the action and `elapsedAtLeast`. Unparseable duration → `DocError::Validation` naming the field and the value. Store on the parsed validate action as `elapsed_at_least: Option<Duration>`.
2. In `runner.rs`, anchor the scenario start in BOTH run entries — `run_scenario` (line ~292) and `run_scenario_document` (line ~334): `let started_at = std::time::Instant::now();` taken once per scenario run, before any action executes (post-boot; boot time only adds margin to pass cases), and thread it through `run_action` (its signature gains the timing parameter) into `validate_action` (parameter or a small `struct ScenarioTiming { started_at: Instant }` — pick one, use it consistently across all three functions).
3. In `validate_action`'s `LastReceived` arm: when `elapsed_at_least` is `Some(bound)`, read `vars.last_received(&endpoint.endpoint)`; compute `let actual = message.arrival.checked_duration_since(started_at).unwrap_or_default();` pass iff `actual >= bound`; on failure return `ScenarioFailure::ValidationMismatch { action: index, detail: format!("{}: expected arrival at least {} after scenario start, arrived after {:?}", subject, humantime-ish bound formatting, actual) }` where `subject` is the redacted endpoint subject already built in that arm. The assertion uses `message.arrival` — NOT the current time.
4. Add the timing vocabulary entry to `CONTEXT.md` (elapsedAtLeast: wire-arrival anchored to scenario start; the not-before-X control run.sh expresses with awk).

**Tests:**
- Parse tests (doc_parse_test.rs):
  - name: `elapsed_at_least_parses_on_last_received`
    setup: validate with `target: {lastReceived: http://127.0.0.1:0/x}` and `elapsedAtLeast: 250ms`
    action: parse
    assert: action carries `elapsed_at_least == Some(250ms)`
    command: `cargo test -p camel-integration-test --lib elapsed_at_least_parses_on_last_received`
    expected: fails before, passes after
  - name: `elapsed_at_least_on_partner_target_fails`
    setup: validate with partner target and `elapsedAtLeast: 1s`
    action: parse
    assert: `DocError::Validation` naming `elapsedAtLeast` and the target kind
    command: `cargo test -p camel-integration-test --lib elapsed_at_least_on_partner_target_fails`
    expected: fails before, passes after
  - name: `elapsed_at_least_unparseable_fails`
    setup: `elapsedAtLeast: "not-a-duration"`
    action: parse
    assert: `DocError::Validation` naming the field and value
    command: `cargo test -p camel-integration-test --lib elapsed_at_least_unparseable_fails`
    expected: fails before, passes after
- Integration tests (tests/http_partner_scripting_test.rs — sends do NOT populate `lastReceived`; `vars.remember` fires only from `receive_action`, so every fixture pairs a `receive` action, exactly as the file's existing lastReceived fixtures do; the send's response parks in the client lane until the receive consumes it — the parked-response pattern of `http_client_lane_test.rs`):
  - name: `waited_arrival_passes_elapsed_bound`
    setup: partner script with `delay: 300ms`, body `{"ok": true}`; scenario: `send`, `receive` (with a deadline, per the grammar's mandatory receive deadline), then `validate` with `lastReceived` target and `elapsedAtLeast: 200ms`
    action: run the scenario
    assert: passes (the response's wire arrival is ≥ 300ms after start ≥ the 200ms bound)
    command: `cargo test -p camel-integration-test --features http --test http_partner_scripting_test waited_arrival_passes_elapsed_bound`
    expected: fails before (field rejected at load), passes after
  - name: `early_arrival_fails_even_when_consumed_late`
    setup: partner script with NO delay (response arrives ~tens of ms after start and parks in the client lane); scenario: `send`, then `sleep: 1s`, then `receive` (consumes the parked response at ~1s), then `validate` with `lastReceived` and `elapsedAtLeast: 500ms`
    action: run the scenario
    assert: fails with `validation-mismatch` naming the endpoint, the 500ms bound, and an actual elapsed well under 500ms — the assertion measured the WIRE arrival (stamped at response receipt), not the receive-consumption moment 1s in. This is the regression pin for the round-1 critical finding and the delta-spec scenario "early arrival fails even when consumed late"
    command: `cargo test -p camel-integration-test --features http --test http_partner_scripting_test early_arrival_fails_even_when_validated_late`
    expected: fails before, passes after
  - name: `too_early_arrival_fails_naming_actual`
    setup: no-delay script; `send`, `receive`, then immediately `validate` with `elapsedAtLeast: 10s`
    action: run
    assert: fails naming `10s` and the actual elapsed
    command: `cargo test -p camel-integration-test --features http --test http_partner_scripting_test too_early_arrival_fails_naming_actual`
    expected: fails before, passes after
- name: existing suites stay green
  command: `cargo test -p camel-integration-test --features http && cargo test -p camel-integration-test`
  expected: both pass

**Acceptance:**
- `cargo test -p camel-integration-test --features http` exits 0; no-feature run exits 0
- `cargo clippy -p camel-integration-test --features http --all-targets -- -D warnings` exits 0
- `cargo fmt --check --all` exits 0
- `rg -n "Instant::now" crates/camel-integration-test/src/runner.rs` shows the scenario-start anchor in both `run_scenario` and `run_scenario_document`, each taken once before the action loop

- [x] 3.2
