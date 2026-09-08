# Tasks: doc-identifier-interpolation-parity

## camel-cli: test document parser

### Task 1.1: interpolation helper, EnvUnresolved error, step (a0) for repositories and beans

**Files:**
- `crates/camel-cli/src/commands/test/document.rs` (modified)
- `crates/camel-cli/tests/test_doc_identifier_interpolation.rs` (new)

**Steps:**
1. In `document.rs`, add the import `use camel_dsl::env_interpolation::interpolate_env_with;` (camel-cli already depends on camel-dsl; verify with `grep -n 'camel-dsl' crates/camel-cli/Cargo.toml`).
2. Add the private helper below `parse_test_document`'s existing helpers (near `validate_repositories`):
   `fn interpolate_identifier(value: &str, position: &str) -> Result<String, TestDocError>`
   Body: call `interpolate_env_with(value, &|_| None)`; on `Err(var)` return `TestDocError::EnvUnresolved { var, field: position.to_string() }`; on `Ok(resolved)` return it. The closure is passed verbatim as `&|_| None` — the future LayeredEnv injection point (rc-l7m7t); do not inline or rename the lookup concept.
3. Add the `TestDocError` variant (after `InvalidMatcher`):
   `EnvUnresolved { var: String, field: String }` with doc comment "A `${env:NAME}` placeholder in an identifier field resolved to nothing (default-only lookup, ambient env never consulted)". Add its `Display` arm in the existing `impl fmt::Display for TestDocError`: `write!(f, "Environment variable '{var}' not set (required by {field})")` — mirrors the route-side wording of `camel_dsl` `load_from_file_with_env` ("Environment variable '{var}' not set (required by {path})").
4. Add the step-(a0) function:
   `fn interpolate_identifier_fields(doc: &mut TestDocument) -> Result<(), TestDocError>`
   It rebuilds identifier maps key-by-key using `interpolate_identifier`, with collision rejection (an already-present resolved key is an error naming the map and value). In this task wire TWO groups:
   - `repositories`: for each of `cache`, `idempotent`, `claim_check` (`RepositoriesDoc` fields), take the `BTreeMap<String, String>`, rebuild with interpolated keys (position labels `repositories.cache`, `repositories.idempotent`, `repositories.claimCheck`), values untouched (stub target `memory` stays literal). Collision → `TestDocError::InvalidRepositories(format!("repositories.{kind}: duplicate repository name `{resolved}` after interpolation"))`.
   - `beans`: rebuild the `BTreeMap<String, BeanDeclDoc>` with interpolated keys (position `beans`), values untouched. Collision → `TestDocError::InvalidBeans(format!("beans: duplicate bean name `{resolved}` after interpolation"))`.
5. Call `interpolate_identifier_fields(&mut doc)?` in `parse_test_document` immediately after the `serde_yaml::from_str` succeeds and BEFORE the route-source check (step (a)).
6. Update the `parse_test_document` doc comment: prepend "(a0) identifier fields interpolate `${env:...}` default-only (parity with route sources; see the mock-testkit spec requirement)" to the validation-order list.
7. Create `crates/camel-cli/tests/test_doc_identifier_interpolation.rs` modeled on `tests/test_repository_stubs.rs` (imports `camel_cli::commands::test::document::parse_test_document`). No test may read or write environment variables (`env::set_var` / `env::var` are forbidden in this file) — the lookup is default-only by construction; assert env-independence by never touching the env API (`std::env::temp_dir()` for scratch dirs is allowed, as in the precedent file).

**Tests:** (executable spec — write FIRST, verify each FAILS for the resolve/collision cases and the error cases return no EnvUnresolved, then implement)
- `repository_key_default_resolves`: a minimal doc with `repositories: { cache: { "${env:CACHE_REPO_NAME:-persistent}": memory } }` + `routes` (inline minimal) + `expects: { mock:out: { count: 1 } }` → `parse_test_document` OK → `doc.repository_stubs().unwrap().cache.unwrap()` contains key `persistent` and no key containing `${env:`. Expected before: FAIL (key stays raw).
- `bean_key_default_resolves`: doc with `beans: { "${env:BEAN_NAME:-audit}": { kind: echo } }` → parse OK → `doc.bean_decls().unwrap()` contains key `audit`. Expected before: FAIL.
- `repository_no_default_fails_naming_var`: doc with `repositories: { cache: { "${env:NO_SUCH_VAR}": memory } }` → `parse_test_document` returns `Err(TestDocError::EnvUnresolved { var, field })` with `var == "NO_SUCH_VAR"` and `field == "repositories.cache"`; the `Display` string contains `Environment variable 'NO_SUCH_VAR' not set (required by repositories.cache)` and does NOT contain `memory`. Expected before: FAIL (parses OK).
- `empty_default_blank_name_rejected`: key `"${env:NAME:-}"` → `Err(TestDocError::InvalidRepositories(..))` whose message contains `non-blank` (the blank guard sees the resolved empty name). Expected before: FAIL (raw key is non-blank).
- `memory_default_rejected`: key `"${env:NAME:-memory}"` → `Err` whose message contains `built-in repository name`. Expected before: FAIL.
- `repository_key_collision_rejected`: keys `"${env:A:-x}"` and `"x"` in one cache map → `Err(InvalidRepositories)` message contains `duplicate repository name` and `` `x` ``. Expected before: FAIL.
- `bean_key_collision_rejected`: same shape for `beans` → `Err(InvalidBeans)` naming `audit`-analogous resolved name. Expected before: FAIL.

**Acceptance:**
- `cargo test -p camel-cli --test test_doc_identifier_interpolation` exits 0 (all 7 tests pass).
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: step (a0) for intercepts, mock references, and inputs

**Files:**
- `crates/camel-cli/src/commands/test/document.rs` (modified)
- `crates/camel-cli/tests/test_doc_identifier_interpolation.rs` (modified)

**Steps:**
1. Extend `interpolate_identifier_fields` with the remaining three groups:
   - `intercepts`: rebuild the `BTreeMap<String, InterceptActionDoc>` — interpolate each source key (position `intercepts`), then interpolate `action.skip_to` (position `intercepts.skipTo`) and `action.divert_copy_to` (position `intercepts.divertCopyTo`) when present. Collision → `TestDocError::InterceptInvalid(format!("intercepts: duplicate source `{resolved}` after interpolation"))`.
   - `expects`: rebuild the `BTreeMap<String, ExpectSet>` keys with interpolation (position `expects`), values (matcher contents) untouched. Collision → `TestDocError::Yaml(format!("duplicate expectation endpoint `{resolved}` after interpolation"))`.
   - `sequence`: interpolate each `Vec<String>` entry IN PLACE (position `sequence[{index}]`); duplicates remain allowed per the arrival-sequence canon — NO collision guard.
   - `inputs`: interpolate each `input.to` in place (position `inputs[{index}].to`); `body`, `headers`, `expect_reply` untouched.
2. Update the (a0) doc-comment sentence from Task 1.1 to enumerate all five field-groups exactly as the spec requirement lists them.
3. Keep the call site unchanged (one call covers all groups; ordering before step (a) is already correct).

**Tests:**
- `intercept_source_and_target_default_resolve`: doc with `intercepts: { "direct:${env:TARGET:-archive}": { skipTo: "mock:${env:SINK:-skipped}" } }` (+ minimal `routes`, `inputs`, `expects`) → parse OK → `doc.intercepts` key is `direct:archive` with `action.skip_to == "mock:skipped"` (primary assertion: `doc.intercepts.get("direct:archive")` + pub field checks, document.rs:197; `InterceptRules` is built inside `parse_test_document` and not reachable from integration tests — `InterceptRules::lookup` at camel-core/src/intercept.rs:53 is the run-time analogue). Expected before: FAIL (key stays raw, the map holds the unresolved text).
- `expects_and_sequence_mock_refs_resolve`: doc with `expects: { "mock:${env:EP:-result}": { count: 1 }, "mock:${env:EP2:-other}": { count: 1 } }` and `sequence: ["mock:${env:EP:-result}", "mock:${env:EP2:-other}"]` → parse OK → `doc.expects` keys are `result` and `other` (bare, post-(c) normalization) and `doc.sequence` equals `["result", "other"]`. Expected before: parses OK with RAW keys (the scheme check `starts_with("mock:")` at document.rs:861 passes for the raw form, normalizing to the bare unresolved text) — the resolved-value assertions fail.
- `input_to_default_resolves`: doc with `inputs: [{ to: "direct:${env:IN:-start}" }]` → parse OK → `doc.inputs[0].to == "direct:start"`. Expected before: parses OK with the raw `to` (the scheme check at document.rs:921 passes for the raw form) — the resolved-value assertion fails.
- `intercept_no_default_fails`: intercepts key `"${env:SRC}"` → `Err(EnvUnresolved)` with `var == "SRC"`, `field == "intercepts"`. Expected before: FAIL.
- `intercept_key_collision_rejected`: keys `"${env:T:-direct:archive}"` and `"direct:archive"` → `Err(InterceptInvalid)` naming the duplicate. Expected before: FAIL.
- `expects_collision_rejected`: keys `"mock:${env:A:-x}"` and `"mock:x"` → `Err(Yaml(..))` message contains `duplicate expectation endpoint` and `` `mock:x` `` (at step (a0) the key still carries the `mock:` scheme — scheme stripping is step (c), document.rs:860-868 — so the collision message names the full resolved key). Expected before: FAIL.

**Acceptance:**
- `cargo test -p camel-cli --test test_doc_identifier_interpolation` exits 0 (13 tests total).
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.2

## camel-cli: integration and driver tests

### Task 1.3: end-to-end rc-4hexo flip, parity pins, and non-goal pins

**Files:**
- `crates/camel-cli/tests/test_doc_identifier_interpolation.rs` (modified)
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified)

**Steps:**
1. Add the end-to-end repro (mirror `tests/test_repository_stubs.rs::cache_stub_miss_then_hit` scaffolding — `temp_dir`, write `route.yaml` with `fs::write`, `parse_test_document`, `run_test_doc`, `assert_green`):
2. Add the parity/anti-widening/non-goal parse-level and run-level tests listed below.
3. In `driver_tests.rs`, add one driver-level exit-code pin using the file's existing driver scaffolding (follow how neighboring tests build a driver run over a temp dir and assert `summary.exit_code`).

**Tests:**
- `rc4hexo_repro_repository_matches_route` (the FAIL→PASS flip witness): temp dir with `route.yaml` containing a `cache_peek_stale` (or plain `cache`) step whose `repository` is `"${env:CACHE_REPO_NAME:-persistent}"` routing to `mock:out`, and a doc string with `routeFiles: [route.yaml]`, `repositories: { cache: { "${env:CACHE_REPO_NAME:-persistent}": memory } }`, one input, `expects: { mock:out: { count: 1 } }` → `run_test_doc` → `assert_green(&result, 1)`. Honest before-prediction (verified by the bd ticket on v0.42.0): the run fails at route-add with `repository 'persistent' is not registered`; after: green.
- `bean_key_matches_route_e2e` (spec scenario "bean key default matches", run-level): temp dir with `route.yaml` invoking a `bean:` step whose bean name is `"${env:BEAN_NAME:-audit}"` routing to `mock:out`, doc declaring `beans: { "${env:BEAN_NAME:-audit}": { kind: echo } }` + one input + `expects: { mock:out: { count: 1 } }` → `assert_green(&result, 1)` — the stub bean handles the invocation under the resolved name `audit` (mirror the bean route scaffolding in `tests/test_beans.rs`).
- `intercept_applies_to_resolved_uri_e2e` (spec scenario "intercept source and target defaults match", run-level): temp dir with `route.yaml` sending to `direct:${env:TARGET:-archive}` and `mock:real`, doc declaring `intercepts: { "direct:${env:TARGET:-archive}": { skipTo: "mock:${env:SINK:-skipped}" } }` + `expects: { mock:skipped: { count: 1 }, mock:real: { count: 0 } }` (use `minCount: 0`/`maxCount: 0` per the count grammar — mirror `tests/test_intercepts.rs` skipTo scaffolding) → `assert_green` — the intercept applied to the resolved `direct:archive` and the copy landed on `mock:skipped`.
- `escaped_placeholder_key_stays_literal`: doc repositories key `"$${env:CACHE_REPO_NAME:-persistent}"` → parse OK → the parsed cache map's single key is exactly `${env:CACHE_REPO_NAME:-persistent}` (one `$` stripped by the shared escape grammar, inner text literal); additionally an end-to-end variant with a `route.yaml` whose repository field is the same escaped form → `assert_green` (both sides keep the literal name).
- `assertion_data_stays_literal` (anti-widening witness): doc with `inputs[0].body = "${env:PAYLOAD:-leaked}"`, `inputs[0].headers = { "X-T": "${env:T:-v}" }`, `inputs[0].expectReply = { body: "${env:R:-replied}" }`, `expects` entry with `bodies: ["${env:PAYLOAD:-leaked}"]`, a `beans` entry `{ kind: fail, config: { message: "${env:MSG:-m}" }, methods: ["${env:M:-run}"] }` → parse OK → assert `doc.inputs[0].body` is `InputBody::Text("${env:PAYLOAD:-leaked}")` (match on the enum), the header value still carries the raw text, the parsed reply body matcher and the `bodies[0]` matcher still target their raw texts (assert via the matcher's equals target or its `Debug` output containing `${env:`), and the bean `config` and `methods` values are the raw texts. No interpolation anywhere in a value position.
- `settle_placeholder_not_interpolated`: doc with `settle: "${env:S:-500ms}"` → `parse_test_document` FAILS with `TestDocError::SettleOutOfRange` — the raw text is not a duration, proving interpolation never rescues `settle`. Expected before AND after: same failure (regression pin).
- `stub_target_not_interpolated`: doc with `repositories: { cache: { ok-name: "${env:TGT:-memory}" } }` → `Err(InvalidRepositories)` naming the RAW target text `${env:TGT:-memory}` as unsupported — stub targets stay literal (regression pin; fails identically before and after).
- `route_files_paths_never_interpolate`: two parse-level pins and one run-level pin. (i) doc with only `routeFiles: ["${env:ROUTE_DIR:-routes}/demo.yaml"]` (+ minimal expects) parses OK with `doc.route_files` carrying the literal entry; `run_test_doc` then fails naming the literal path `${env:ROUTE_DIR:-routes}/demo.yaml` (no substitution happened). (ii) doc with only `routeFilesFromRoot: ["${env:ROOT_DIR:-cfg}/r.yaml"]` (+ minimal expects) parses OK with `doc.route_files_from_root` carrying the literal entry (root resolution is run-level and needs a `Camel.toml`; the parse-level pin suffices for the non-goal).
- `env_unresolved_identifier_exits_2` (in `driver_tests.rs`, lib target): a temp-dir document whose `repositories.cache` key is `"${env:NO_SUCH_VAR}"` → the driver summary reports exit code 2 (doc-validation class) with a message naming `NO_SUCH_VAR`. Follow the existing `summary.exit_code` assertion style at `driver_tests.rs` (see the `exit 2` assertions already present there).

**Acceptance:**
- `cargo test -p camel-cli --test test_doc_identifier_interpolation` exits 0 (22 tests in-file; 14 after Task 1.2 precedence pin + 8 new — plan originally said 21 before the mid-flight precedence test).
- `cargo test -p camel-cli --lib` exits 0 (driver_tests included, +1 new test).
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 1.3

## camel-cli: docs

### Task 1.4: CONTEXT.md hermeticity carve-out

**Files:**
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. Locate the `camel test` hermeticity note (the passage documenting that ambient env is never consulted, near the current line-71 area) and extend it — or add a short adjacent paragraph — stating: unit-tier doc IDENTIFIER fields (`repositories`/`beans` keys, `intercepts` sources and targets, `mock:` refs in `expects`/`sequence`, `inputs[].to`) interpolate `${env:NAME:-default}` default-only through the camel-dsl scanner for name-match parity with route sources; assertion data and path fields (`routeFiles`, `routeFilesFromRoot`) never interpolate; a no-default identifier placeholder fails doc-validation at exit 2 naming the variable. Reference bd rc-4hexo and ADR-0069 §4 (the doc-env layering track rc-l7m7t) following the citation format already used in that file.
2. Do not touch any other section; keep STE-compliant plain technical English.

**Tests:**
- None (prose). Verification is the citation lint plus the grep below.

**Acceptance:**
- `grep -c 'interpolate' crates/camel-cli/CONTEXT.md` returns ≥ 1.
- `cargo xtask lint-context-citations` exits 0.
- `cargo fmt --check --all` still exits 0 (docs change cannot break it, but the gate stays green).

- [x] 1.4
