# Tasks: shared-matcher-core

## Phase 1: pure-crate carve

### Task 1.1 — create `crates/camel-matchers` with the carved algebra

**Files:**
- `crates/camel-matchers/Cargo.toml` (new)
- `crates/camel-matchers/src/lib.rs` (new)
- `crates/camel-matchers/CONTEXT.md` (new, minimal stub — full content in 1.3)
- `Cargo.toml` (modified — `[workspace.dependencies]` only; NO members change)

**Steps:**
1. Create `crates/camel-matchers/Cargo.toml` matching sibling scaffolding: `version.workspace = true`, description, license, repository, `[lints] workspace = true`; dependencies exactly `regex = { workspace = true }`, `serde_json = { workspace = true }` (both have root `[workspace.dependencies]` entries), `form_urlencoded = "1.2"` (direct — no workspace entry exists) — ZERO `camel-*` dependencies, no features, no async runtime.
2. Register the dep in the root `Cargo.toml` `[workspace.dependencies]` mirroring the `camel-api` entry (path + exact-version pin). Do NOT touch `[workspace.members]` — the `crates/camel-*` glob already covers the new crate.
3. Create `src/lib.rs` with the types and functions below, moved semantics-verbatim from `camel-integration-test` (source anchors in design.md; adjust intra-doc links like `[PartnerExpectation]` to the new `RequestExpectation` name during the move):
   - `pub enum CountBound { Exact(u64), AtLeast(u64), AtMost(u64), Range(u64, u64) }` (document.rs:321) — keep `#[non_exhaustive]`, doc comments carried over.
   - `pub enum PathFilter { Exact(String), Contains(String), Matches(String) }` (document.rs:336) — `#[non_exhaustive]`.
   - `pub struct RequestExpectation { pub bound: CountBound, pub method: Option<String>, pub path: Option<PathFilter>, pub query: Option<BTreeMap<String, String>> }` — renamed from `PartnerExpectation` (document.rs:349), fields identical.
   - `pub enum Expectation { Equals(serde_json::Value), Regex(String), Contains(String), StartsWith(String), EndsWith(String), Exists, JsonSubset(serde_json::Value) }` (document.rs:284) — `Value` swapped from `camel_api::Value` to `serde_json::Value` (plain alias, zero semantic change), `#[non_exhaustive]`.
   - `pub fn bound_holds(bound: &CountBound, actual: usize) -> bool` (partner_validate.rs:98).
   - `pub fn settles_early(bound: &CountBound, actual: usize) -> bool` (partner_validate.rs:115).
   - `pub fn above_ceiling(bound: &CountBound, actual: usize) -> bool` (partner_validate.rs:127).
   - `pub fn query_pairs(path_and_query: &str) -> Vec<(String, String)>` (partner_validate.rs:85) — percent-decodes `%XX` and `+` via `form_urlencoded`, everything after the first `?`, no pairs without `?`.
   - `pub fn matching_count<'a>(requests: impl IntoIterator<Item = (&'a str, &'a str)>, method: Option<&str>, path_filter: Option<&PathFilter>, query: Option<&BTreeMap<String, String>>) -> usize` — the generic projection of `matching_requests` (partner_validate.rs:43): each item is `(method, path_and_query)`; method comparison `eq_ignore_ascii_case`; path filter `Exact` strict bytes / `Contains` substring / `Matches` regex compiled once per call, invalid pattern matches nothing (fail closed); query subset every declared pair present, order- and encoding-independent.
   - `pub fn render_bound(bound: &CountBound) -> String` (partner_validate.rs:303).
   - `pub fn expectation_matches(expectation: &Expectation, value: &serde_json::Value) -> bool` — the pure per-form boolean of runner.rs:624-679 arms: `Equals` `value == expected`, `Regex` compile-fail matches nothing else `is_match` on the stringified value, `Contains`/`StartsWith`/`EndsWith` on the stringified value, `Exists` value is not `Null`, `JsonSubset` recursive subset via the moved helper. Invalid regex here returns false (the harness keeps the invalid-regex error taxonomy itest-side).
   - `pub fn stringify(value: &serde_json::Value) -> String` (runner.rs:780 — `pub`, not `pub(crate)`: itest's runner.rs:138 substitution imports it cross-crate) and `fn json_subset(expected: &serde_json::Value, actual: &serde_json::Value) -> bool` (runner.rs:790-801, recursive incl. the :796 self-call; may stay private) — moved verbatim.
4. Write the crate's unit tests as `#[cfg(test)] mod tests` in `src/lib.rs` (specs below).
5. Create `CONTEXT.md` stub: one paragraph — "pure matcher algebra shared by the test tiers; see ADR-0072" (ADR-0072 lands in task 1.3; cite it as forthcoming).
6. Run `cargo fmt` and `cargo clippy -p camel-matchers -- -D warnings`; fix findings.

**Tests:**
- `bound_holds_covers_every_form_at_edges` — setup: bounds `Exact(2)`, `AtLeast(2)`, `AtMost(2)`, `Range(1,3)`; action: evaluate counts 0,1,2,3,4; assert: `Exact(2)` holds iff ==2; `AtLeast(2)` iff >=2; `AtMost(2)` iff <=2; `Range(1,3)` iff 1<=n<=3 (inclusive both ends). command: `cargo test -p camel-matchers`. expected: fails before implementation (crate absent), passes after.
- `settles_early_absence_claims_never_settle` — setup: `AtMost(5)`, `Range(0,5)`; action: evaluate counts 0..=5; assert: always `false` for both; `Exact(2)` true only at 2, `AtLeast(2)` true at 2+. same command.
- `above_ceiling_only_upper_breaches` — setup: `AtMost(2)`, `Range(1,2)`; action: count 3; assert: true; same bounds with `Exact(2)`/`AtLeast(2)` at count 99: false; `AtMost(2)` at 2: false. same command.
- `matching_count_query_subset_order_and_encoding_independent` — setup: declared `{"a":"1","b":"2"}`; action: project `/x?b=2&a=1` and `/x?a=%31&b=%32`; assert: count 2 (both match); `/x?a=1` alone: count 0 (ALL-pairs subset — `b` missing); with re-declared subset `{"a":"1"}`: `/x?a=1` matches (count 1); `/x?b=2`: count 0. same command.
- `matching_count_invalid_regex_fails_closed` — setup: `PathFilter::Matches("(")`; action: `matching_count` over `[("GET","/anything")]`; assert: 0. same command.
- `matching_count_method_case_insensitive` — setup: filter method `post`; action: project `("POST","/o")` and `("GET","/o")`; assert: 1. same command.
- `matching_count_path_forms` — setup: `Exact("/o?a=1")` strict bytes vs `Contains("/o")` vs `Matches("^/o")`; action: projections `("GET","/o?a=1")`, `("POST","/o?a=1&x=2")`, `("GET","/diff")`; assert exact counts per form (exact=1, contains=2, matches=2 — `/diff` matches none of the three). same command.
- `query_pairs_plus_decoding` — setup: `/x?a=1+2`; action: `query_pairs`; assert: `[("a","1 2")]` (`+` decodes to space, per spec's "`+` decoded" clause). same command.
- `expectation_matches_string_forms` — setup: value `"hello world"`; action: `Contains("world")`, `StartsWith("hello")`, `EndsWith("world")`, `Exists`, `Regex("^hello")`, `Equals(json!("hello world"))`; assert: all true; `Contains("nope")` false. same command.
- `expectation_matches_object_forms` — setup: value `{"n":"café","s":"hello world"}`; action: `Equals(json!({"n":"café","s":"hello world"}))`, `JsonSubset(json!({"n":"café"}))`, `Regex("caf")`, `Exists`; assert: all true; `JsonSubset(json!({"n":"other"}))` false; `Exists` false on `Value::Null`; `Regex("(")` false (fail closed). same command.
- `json_subset_recursive_objects` — setup: expected `{"user":{"name":"María"}}`, actual `{"user":{"name":"María","role":"admin"},"extra":1}`; action: `expectation_matches(JsonSubset(expected), actual)`; assert: true; expected with `{"user":{"name":"other"}}`: false. same command.
- `render_bound_forms` — setup: each `CountBound` variant; action: render; assert: non-empty distinct strings per variant (exact snapshot not required — itest diagnostics keep their own suite). same command.
- `query_pairs_no_question_mark` — setup: `/noquery`; action: `query_pairs`; assert: empty vec; `/q?a=1` → `[("a","1")]`. same command.

**Acceptance:**
- `cargo build -p camel-matchers` exits 0; `cargo test -p camel-matchers` green.
- `grep -c 'camel-' crates/camel-matchers/Cargo.toml` finds zero camel dependencies (name line excepted).
- `cargo xtask lint-publish-cycles` exits 0.
- `cargo clippy -p camel-matchers -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2 — rewire `camel-integration-test` to consume the crate

**Files:**
- `crates/camel-integration-test/Cargo.toml` (modified — add dependency `camel-matchers`)
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/runner/partner_validate.rs` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/src/lib.rs` (modified)

**Steps:**
1. Add `camel-matchers = { workspace = true }` to itest's `Cargo.toml` (the workspace entry from 1.1 resolves it; sibling convention, no path hedge). After `query_pairs` moves out, itest's `form_urlencoded` optional dependency and its `dep:form_urlencoded` http-feature entry are dead (sole use was partner_validate.rs:87) — remove both; `serde_json` optional stays (adapters/http.rs:1026 still uses it).
2. In `document.rs`: delete the type definitions `CountBound`, `PathFilter`, `PartnerExpectation`, `Expectation` (:284-360 region); import them from `camel_matchers`; the raw serde stage (`expectation_from_value` :1053, `partner_expectation_from_value` :1107) now CONSTRUCTS core types — adjust imports only, construction logic unchanged; `ValidateExpectation` (:367) stays and now wraps `camel_matchers::{Expectation, RequestExpectation}`.
3. In `lib.rs` (:37-40): keep the public re-export surface byte-identical from the outside — `pub use camel_matchers::{CountBound, Expectation, PathFilter}; pub use camel_matchers::RequestExpectation as PartnerExpectation;` (plus whatever the existing list re-exports verbatim).
4. In `runner/partner_validate.rs`: delete `query_pairs`, `bound_holds`, `settles_early`, `above_ceiling`, `render_bound` definitions; import from `camel_matchers`; replace the `matching_requests` body's filtering with a call to `camel_matchers::matching_count` fed by the projection `requests.iter().map(|r| (r.method.as_str(), r.path.as_str()))` — keep `matching_requests` as the `#[cfg(feature = "http")]` adapter (signature unchanged, returns the same count); `render_filters` (:322) and `partner_mismatch_detail` (:259) stay (redaction-coupled), rendering `PathFilter` matches via the moved type — its exhaustive match (:327-337) gains a `_ =>` defensive arm (unreachable for foreign `#[non_exhaustive]` variants; pick a fallback rendering string and keep going).
5. In `runner.rs`: the message-expectation arms (:624-679) delegate their boolean to `camel_matchers::expectation_matches` and keep constructing the SAME `ScenarioFailure` detail strings (subject, redaction, humantime) — the invalid-regex `ValidationMismatch` arm (:635-640) keeps its error taxonomy and short-circuits BEFORE delegating so verdict bytes are unchanged; the enum match gains a `_ =>` defensive failure arm (unreachable, defensive error message defined by you, consistent with the crate's failure voice); import `stringify`/`json_subset` from core where still needed (:138 variable substitution keeps using itest-side logic; only pure definitions were moved).
6. Run the full itest suite with and without the http feature; run `cargo fmt` and `cargo clippy -p camel-integration-test --all-targets -- -D warnings`.

**Tests:** (behavior-preserving rewire — the existing suite is the oracle; no new test files)
- `itest_suite_http_unchanged` — setup: worktree with 1.1 landed; action: `cargo test -p camel-integration-test --features http`; assert: all tests pass with ZERO edits to test files (verify `git diff --stat` shows no `*_test.rs` content changes beyond import adjustments if any are strictly required — expectation texts untouched). expected: green after rewire; red in between is a bug in the rewire, not a test to write.
- `itest_compiles_without_http` — action: `cargo check -p camel-integration-test` (no features); assert: exits 0 (serde_json/form_urlencoded now arrive transitively through camel-matchers unconditionally — acknowledged in design.md).
- `no_duplicate_algebra_left` — action: `grep -rn "pub enum CountBound\|pub enum PathFilter\|pub enum Expectation\|struct PartnerExpectation\|fn bound_holds\|fn settles_early\|fn above_ceiling\|fn json_subset\|fn query_pairs\|fn render_bound\|fn stringify\|fn matching_count\|fn expectation_matches" crates/camel-integration-test/src`; assert: zero hits (all definitions now live in camel-matchers; the last two patterns catch smuggled copies of the new core fns).

**Acceptance:**
- `cargo test -p camel-integration-test --features http` green.
- `cargo check -p camel-integration-test` (no features) exits 0.
- `cargo clippy -p camel-integration-test --all-targets -- -D warnings` exits 0.
- Zero expectation-text edits: `git diff` touches no scenario fixtures or assertion detail strings.

- [x] 1.2

### Task 1.3 — ADR-0072, CONTEXT-MAP, testing-surface map

**Files:**
- `docs/adr/0072-test-pyramid-v2.md` (new)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-matchers/CONTEXT.md` (modified — replace stub)

**Steps:**
1. Write `docs/adr/0072-test-pyramid-v2.md` (match the house ADR format of 0069/0071 — status Proposed, context, decision, consequences), superseding-in-part ADR-0069. Decision sections, pinning exactly what design.md records:
   - Placement: dedicated `camel-matchers` crate; the B2 camel-config precedent weighed and rejected (semantic accretion); crate-count cost accepted and recorded.
   - Purity rule: zero `camel-*` deps; regex/serde_json/form_urlencoded allowed; no harness, no wire types, no redaction, no async runtime, no features.
   - One algebra, per-tier grammar, per-tier observation ("same verbs, different subjects"); grammar never unified (ADR-0069 §2 mixing ban stands); observations never unified into one trait — parameterize the algebra (`matching_count` over projected tuples), not the observation.
   - Staged direction: step 2 = unit-tier `expects` growth consuming the crate (future change); step 3 = observational probes (`mock:probe-N`, registry-only); MUTATING weaving gated exactly as a lean-set change per ADR-0064 §5 AdviceWith Stage A/B frame; wire timeouts never virtualized per ADR-0069 §6; `recipient_list`/dynamic-dispatch force-FULL stays static.
   - Include the **testing-surface map**: one table naming camel-test (unit kit), camel-integration-test (scenario kit), camel-matchers (algebra), camel-cli `test` command (runner/tiering), camel-bundles (boot installers), and the dual-use lean runtime components (mock/direct/seda/timer/log) with their ADRs (0064, 0069, 0055, 0070) — closing the "spread across four ADRs" documentation gap.
2. Add the ADR to `CONTEXT-MAP.md`'s ADR index (match the one-line format of siblings; mention `camel-matchers`, `camel-test`, `camel-integration-test`).
3. Rewrite `crates/camel-matchers/CONTEXT.md`: full crate context — purpose (the algebra), the purity rule, what it is NOT (no grammar, no observation, no redaction), consumers (itest today, camel-test step 2), pointer to ADR-0072.
4. Run `cargo xtask lint-context-citations` to confirm citation hygiene.

**Tests:**
- `adr_index_entry_present` — setup: ADR-0072 written; action: `grep -c "0072" CONTEXT-MAP.md`; assert: >= 1 (index entry exists).
- `context_citations_clean` — action: `cargo xtask lint-context-citations`; assert: exits 0.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- ADR-0072 exists with status Proposed and the supersede-in-part relation to 0069 stated.
- Testing-surface map section present in the ADR naming all six surfaces.

- [x] 1.3
