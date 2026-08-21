# Tasks: mock-expectation-and-uri-surface

Single-phase change. All tasks in `crates/components/camel-mock` unless noted.
Public API is additive; no existing symbol changes signature.

## crates/components/camel-mock

### Task 1: count expectations (expect_count / expect_minimum_count) + expectations.rs split

**Files:**
- `crates/components/camel-mock/src/expectations.rs` (new)
- `crates/components/camel-mock/src/lib.rs` (modified)

**Steps:**
1. Create `expectations.rs`; move `MockExpectations` (struct at lib.rs:146, `Default` impl, `new`, `push_body`, `push_header`, `push_header_regex`) into it. Mark the struct fields `pub(crate)` — `assert_satisfied` in `lib.rs` reads `guard.expected_bodies` etc. directly (lib.rs:479+), and the move must keep that working.
2. Add two fields to `MockExpectations`: `expected_count: Option<usize>` and `minimum_count: Option<usize>` (initialized `None` in `new`), plus `pub(crate) fn set_expected_count(&mut self, n: usize)` and `pub(crate) fn set_minimum_count(&mut self, n: usize)`.
3. In `lib.rs`, add `pub use expectations::MockExpectations;` (public path `camel_mock::MockExpectations` unchanged; the `expectations` field type on `MockEndpointInner` keeps resolving).
4. Add two public methods on `MockEndpointInner` (lib.rs, next to `expect_body` at ~lib.rs:440):
   - `pub fn expect_count(&self, n: usize)` — locks `self.expectations`, calls `set_expected_count(n)`.
   - `pub fn expect_minimum_count(&self, n: usize)` — locks `self.expectations`, calls `set_minimum_count(n)`.
5. In `assert_satisfied` (lib.rs:470), inside the expectations guard, BEFORE the existing `if !guard.expected_bodies.is_empty()` block, add:
   - Exact: if `guard.expected_count == Some(n)` and `received.len() != n` → `self.set_fail_fast_on_mismatch(); panic!("MockEndpoint '{}': expected {} exchanges, got {}", self.name, n, received.len())`.
   - Minimum: if `guard.minimum_count == Some(m)` and `received.len() < m` → `self.set_fail_fast_on_mismatch(); panic!("MockEndpoint '{}': expected at least {} exchanges, got {}", self.name, m, received.len())`.
   Both panics abort the method — no later check runs (short-circuit by panic).

**Tests:** (all in the existing `mod tests` in lib.rs; command `cargo test -p camel-component-mock --lib`)
- `count_exact_mismatch_fails`: component default config, `inner.expect_count(3)`; send 2 exchanges via producer; `AssertUnwindSafe(inner.assert_satisfied()).catch_unwind().await` → caught, message contains endpoint name, "expected 3", "got 2". Fails before implementation (no expect_count).
- `count_exact_satisfied_passes`: `expect_count(2)`, send 2, `assert_satisfied().await` completes without panic. Fails before.
- `count_minimum_satisfied_by_more`: `expect_minimum_count(2)`, send 5, completes without panic. Fails before.
- `count_minimum_violated_fails`: `expect_minimum_count(4)`, send 1, catch_unwind → message contains "at least 4". Fails before.
- `count_exact_and_minimum_enforced_together`: `expect_count(2)` + `expect_minimum_count(1)`, send 3 → panic message reports the exact mismatch ("expected 2"/"got 3"), not the minimum. Fails before.
- `count_checked_before_bodies`: `expect_count(5)` + one `expect_body(Text("x"))`, send 2 non-matching exchanges → panic message reports the count mismatch, not a body mismatch. Fails before.
- `count_coexists_with_bodies_pass`: `expect_count(2)` + two matching bodies, send 2 matching → no panic. Fails before.
- `count_evaluates_retained_snapshot_under_truncation`: component with `MockConfig::new(3)` (max_retained 3), `expect_count(3)`, send 5 → `received_count().await == 3` and `assert_satisfied().await` no panic. Fails before.

**Acceptance:**
- `cargo test -p camel-component-mock --lib` green (all existing tests + 8 new).
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean.
- `camel_mock::MockExpectations` public path still compiles (re-export).

- [x] 1

### Task 2: MockAssertionError + evaluate_expectations + try_assert_satisfied + assert.rs split

**Files:**
- `crates/components/camel-mock/src/assert.rs` (new)
- `crates/components/camel-mock/src/lib.rs` (modified)

**Steps:**
1. Create `assert.rs` with `#[non_exhaustive] #[derive(Debug)] pub enum MockAssertionError` implementing `std::fmt::Display` and `std::error::Error` (manual impls — no new dependencies). Variants and payloads (body detail fields are pre-formatted `String`s via `format!("{:?}", body)`):
   - `CountMismatch { endpoint: String, expected: usize, actual: usize }` → Display: `MockEndpoint '{endpoint}': expected {expected} exchanges, got {actual}`
   - `MinimumCountNotMet { endpoint: String, minimum: usize, actual: usize }` → `MockEndpoint '{endpoint}': expected at least {minimum} exchanges, got {actual}`
   - `BodyCountMismatch { endpoint: String, expected: usize, actual: usize }` → `MockEndpoint '{endpoint}': expected {expected} bodies, got {actual}` (byte-parity with lib.rs:483-488)
   - `BodyMismatch { endpoint: String, index: usize, expected: String, actual: String }` → `MockEndpoint '{endpoint}': body[{index}] expected {expected}, got {actual}` (parity with lib.rs:514-517 — `{expected}`/`{actual}` carry the `{:?}`-formatted body text)
   - `BodyNotFound { endpoint: String, expected: String }` → `MockEndpoint '{endpoint}': expected body {expected} not found in received exchanges (anyOrder mode)` (parity with lib.rs:503-506)
   - `HeaderNotFound { endpoint: String, key: String, value: serde_json::Value }` → `MockEndpoint '{endpoint}': expected header '{key}' = {value} not found in any received exchange` (parity with lib.rs:530-533)
   - `HeaderRegexNotMatched { endpoint: String, key: String, pattern: String }` → `MockEndpoint '{endpoint}': no received exchange has header '{key}' matching regex {pattern:?}` (parity with lib.rs:556-559)
   - `InvalidHeaderPattern { endpoint: String, key: String, pattern: String, source: Box<dyn std::error::Error + Send + Sync> }` → `MockEndpoint '{endpoint}': invalid regex pattern {pattern:?}: {source}` (parity with lib.rs:539-543; `Error::source()` returns `Some(&*self.source)` for this variant, `None` for the rest)
2. In `assert.rs`, add `impl MockEndpointInner { pub(crate) async fn evaluate_expectations(&self) -> Result<(), MockAssertionError> }` containing ALL checks in this order: exact count, minimum count, body-count + bodies + headers + header regexes — the latter four ONLY under the existing `!expected_bodies.is_empty()` gate for the body pair (preserve the lib.rs:479 gate exactly: no expected bodies ⇒ body-count and per-body checks are skipped; header checks run independently of that gate, as today). Body-count check is `guard.expected_bodies.len() != received.len()` → `BodyCountMismatch`. On any error: if the variant is NOT `InvalidHeaderPattern`, call `self.set_fail_fast_on_mismatch()` FIRST, then return the `Err`; `InvalidHeaderPattern` returns `Err` without touching the latch (regex compile failure becomes this variant instead of today's inline panic). Move the `body_eq` helper into `assert.rs` (private).
   Visibility for the split: `evaluate_expectations` needs `pub(crate)` access to `MockEndpointInner` fields (`expectations`, `any_order`, `name`) — mark those fields `pub(crate)` in lib.rs — and to `set_fail_fast_on_mismatch` (mark it `pub(crate)` instead of private).
3. In `lib.rs`, rewrite `assert_satisfied` (lib.rs:470-563) as: `if let Err(e) = self.evaluate_expectations().await { panic!("{e}") }` — delete the inline check bodies (now in evaluate). Add `pub async fn try_assert_satisfied(&self) -> Result<(), MockAssertionError> { self.evaluate_expectations().await }`.
4. Add `pub use assert::MockAssertionError;` to lib.rs.
5. Keep/update doc comments on both public methods (`# Panics` on `assert_satisfied`, `# Errors` on `try_assert_satisfied`).

**Tests:** (`cargo test -p camel-component-mock --lib`)
- `try_assert_satisfied_ok_when_satisfied`: `expect_count(1)` + matching body, send 1 → `try_assert_satisfied().await` is `Ok(())`. Fails before (no such method).
- `try_assert_satisfied_err_with_details`: `expect_count(2)`, send 0 → `Err(e)` where `e.to_string()` contains endpoint name and "expected 2"; no panic escapes. Fails before.
- `try_assert_satisfied_sets_fail_fast_latch`: component `MockConfig { fail_fast: true, ..Default::default() }`, unmet `expect_count(2)`, send 0 → `Err` AND `inner.fail_fast_error()` is `Some`. Fails before.
- `invalid_header_regex_returns_err_not_panic`: fail_fast TRUE component, `expect_header_regex("k", "(unclosed")`, send 1 exchange → `try_assert_satisfied().await` is `Err` matching `InvalidHeaderPattern` (via `matches!`), `fail_fast_error()` is `None` (latch NOT tripped even with fail_fast enabled), and the call does not panic. Fails before (today: inline panic).
- `display_equals_panicking_variant_message`: two identically-configured endpoints (default config), each `expect_count(3)`, send 1; endpoint A: capture panic payload of `AssertUnwindSafe(assert_satisfied()).catch_unwind().await` downcast to `String`; endpoint B: `try_assert_satisfied().await.err().unwrap().to_string()`; assert the two strings are equal. Fails before.
- `no_expected_bodies_with_received_exchanges_still_ok`: no body expectations set, 3 exchanges sent → `try_assert_satisfied().await` is `Ok(())` (regression guard for the `is_empty` gate — body-count check must not fire when no bodies expected). Passes before AND after (guard test).

**Acceptance:**
- `cargo test -p camel-component-mock --lib` green (Task 1 tests still pass — same messages).
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-unwrap` clean.
- `cargo xtask lint-non-exhaustive` passes (`MockAssertionError` is `#[non_exhaustive]`).

- [x] 2

### Task 3: URI parameter surface (descriptor + manual parsing + precedence + catalog parity)

**Files:**
- `crates/components/camel-mock/src/lib.rs` (modified)

**Steps:**
1. Extend the `MockUriConfig` descriptor (lib.rs:92-107) with five fields, controlbus style (`camel-controlbus/src/metadata.rs`), keeping `skip_impl` and the existing `metadata(..)` block:
   `#[uri_param(name = "retain")] pub _retain: String`, same for `copy`, `failFast`, `expectedCount`, `anyOrder`. None required. Remove the `_name: String` placeholder field if the macro permits an empty path-only descriptor; otherwise keep `_name` unchanged.
2. Add two private parse helpers in lib.rs:
   - `fn parse_usize_param(uri_value: &str, name: &str) -> Result<usize, CamelError>` — `uri_value.parse::<usize>()` mapping errors to `CamelError::EndpointCreationFailed(format!("mock: invalid value for URI parameter '{name}': '{uri_value}' is not a non-negative integer"))`.
   - `fn parse_bool_param(uri_value: &str, name: &str) -> Result<bool, CamelError>` — accepts `true`/`false` case-insensitively; anything else → `EndpointCreationFailed(format!("mock: invalid value for URI parameter '{name}': '{uri_value}' is not a boolean (true|false)"))`.
3. In `create_endpoint` (lib.rs:244), after the name check and BEFORE the registry lock, resolve effective values:
   - `retain`: if `parts.params.get("retain")` present → `parse_usize_param` then reject `0` with `EndpointCreationFailed("mock: URI parameter 'retain' must be >= 1, got 0")`; else `self.config.max_retained`.
   - `copy`: param ? `parse_bool_param` : `self.config.copy_on_exchange`.
   - `fail_fast`: param ? `parse_bool_param` : `self.config.fail_fast`.
   - `any_order`: param ? `parse_bool_param` : `self.config.any_order`.
   - `expectedCount`: validation only in this task — if present, `parse_usize_param(v, "expectedCount")?` and discard the value (the `?` propagates malformed-value errors now; the `Option<usize>` wiring lands in Task 4, keeping this task free of unused-value warnings).
4. Use the resolved `retain`/`copy`/`fail_fast`/`any_order` values in the `MockEndpointInner` construction inside `or_insert_with` (first-creation-wins is preserved by the existing entry API — pre-existing inners are not reconfigured).
5. Add the inline catalog parity test (see tests) and run `cargo xtask schema --check` to confirm no generated-schema drift.

**Tests:** (`cargo test -p camel-component-mock --lib`)
- `uri_retain_override_truncates`: default component; `create_endpoint("mock:cap?retain=50")`; send 55 exchanges → `get_endpoint("cap").received_count().await == 50` (default would be 10 000 → 55). Fails before (param ignored).
- `uri_any_order_overrides_matching`: default component (any_order false); `create_endpoint("mock:relaxed?anyOrder=true")`; `expect_body(Text("a"))` + `expect_body(Text("b"))`; send "b" then "a" → `assert_satisfied().await` no panic (default strict order would fail). Fails before.
- `uri_fail_fast_overrides_latching`: default component (fail_fast false); `create_endpoint("mock:tight?failFast=true")`; `inner.expect_count(1)`, send 0, `assert_satisfied` in catch_unwind → `fail_fast_error()` is `Some` (default false would leave None). Fails before.
- `uri_absent_params_fallback_to_config`: component `MockConfig { fail_fast: true, ..Default::default() }`; `create_endpoint("mock:audit")` → `expect_count(1)` + 0 sent + `assert_satisfied` in catch_unwind → `fail_fast_error()` is `Some` (component fail_fast applied); a fresh no-param endpoint with 0 sent and no expectations → `try_assert_satisfied` returns `Ok` (no default count expectation). Fails before.
- `uri_malformed_numeric_rejected`: `create_endpoint("mock:x?retain=abc")` → `Err` whose message contains "retain". Fails before.
- `uri_malformed_expected_count_rejected`: `create_endpoint("mock:x?expectedCount=abc")` → `Err` whose message contains "expectedCount". Fails before.
- `uri_zero_retain_rejected`: `create_endpoint("mock:x?retain=0")` → `Err` message contains "retain" and ">= 1". Fails before.
- `uri_malformed_boolean_rejected`: `create_endpoint("mock:x?copy=maybe")` → `Err` message contains "copy". Fails before.
- `uri_first_creation_wins_on_conflict`: create `mock:single?retain=5`, then `mock:single?retain=100`, send 7 → `get_endpoint("single").received_count().await == 5`. Fails before.
- `catalog_parity_five_params`: `MockConfig::metadata()` `uri_options` names sorted == `["anyOrder", "copy", "expectedCount", "failFast", "retain"]`. Fails before.

**Acceptance:**
- `cargo test -p camel-component-mock --lib` green.
- `cargo xtask schema --check` exits 0.
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings` exits 0; `cargo fmt --check` clean.

- [x] 3

### Task 4: expectedCount expectation wiring + live-traffic inertness

**Files:**
- `crates/components/camel-mock/src/lib.rs` (modified)

**Steps:**
1. In `create_endpoint`, extend Task 3's `expectedCount` handling from validation-only to full wiring: resolve `expected_count: Option<usize>` (param present → `Some(parse_usize_param(v, "expectedCount")?)`, absent → `None`). Capture registry-entry freshness (restructure the existing `or_insert_with` into an explicit `match registry.entry(name.clone())` — `Entry::Vacant(_) => fresh`, insert the inner; `Entry::Occupied(_) => not fresh`) and, after obtaining `inner`, if `fresh && expected_count == Some(n)` call `inner.expect_count(n)`. Pre-existing inners are never reconfigured (first-creation-wins).
2. Producer untouched: verify NO change to `MockProducer::poll_ready`/`call` (lib.rs:659-736) — `expectedCount` is not consulted anywhere in the producer path.
3. Document the inertness contract in the crate-level doc comment and on `MockUriConfig`: `expectedCount` records intent; it is enforced only when an explicit assertion method runs (`assert_satisfied`/`try_assert_satisfied`), never by the live producer; under `camel run` it never rejects or drops traffic. Note: `copy` has no positive behavioral contrast (both producer branches clone identically) — its URI parsing is proven by malformed-value rejection and catalog parity.

**Tests:** (`cargo test -p camel-component-mock --lib`)
- `expected_count_never_rejects_live_exchanges`: `create_endpoint("mock:sink?expectedCount=2&failFast=true")`; process 7 exchanges via producer → all 7 `Ok`, `received_count().await == 7`, `inner.fail_fast_error()` is `None`. Regression guard after Task 3+4 wiring (fails at any point the wiring regresses).
- `expected_count_enforced_only_at_assertion`: `mock:sink?expectedCount=2`, send 3, `try_assert_satisfied().await` → `Err` (2 vs 3 mismatch). Fails before this task's wiring (param validated but inert).
- `failed_assertion_then_applies_normal_fail_fast`: `mock:sink?expectedCount=2&failFast=true`, send 3, `try_assert_satisfied().await` → `Err`; then process one more exchange via producer → that call returns `Err` with the fixed "fail-fast mode" message (latch tripped via the explicit assertion).
- `expected_count_not_reset_on_second_creation`: create `mock:once` (no params), then `mock:once?expectedCount=5`, send 2 → `try_assert_satisfied().await` is `Ok` (no expectation registered — first creation wins; a reconfigured inner would report a 5-vs-2 mismatch).

**Acceptance:**
- `cargo test -p camel-component-mock --lib` green.
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings` exits 0; `cargo fmt --check` clean.

- [x] 4

### Task 5: exchange(idx) current-thread runtime guard

**Files:**
- `crates/components/camel-mock/src/lib.rs` (modified)

**Steps:**
1. In `exchange` (lib.rs:422), BEFORE the `block_in_place` call, add: `if let Ok(handle) = tokio::runtime::Handle::try_current() { if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread { panic!("MockEndpoint '{}': exchange(idx) cannot be used from a current-thread tokio runtime; use #[tokio::test(flavor = \"multi_thread\")] or the async accessors get_received_exchanges()/await_exchanges()", self.name); } }`. MultiThread and no-runtime (`try_current` Err) fall through to the existing `block_in_place` path unchanged.
2. Update the method's doc comment: keep the existing panic notes, add that the current-thread case fails immediately with a remedy-naming message (the deadlock/opaque-panic case is intercepted up front).

**Tests:** (`cargo test -p camel-component-mock --lib`)
- `exchange_current_thread_clear_panic`: `#[tokio::test]` (default current-thread flavor); endpoint with 1 recorded exchange; `std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner.exchange(0)))` (sync closure — `exchange` is sync) → caught, downcast payload contains "current-thread" and "multi_thread". Test completes without hanging.
- `exchange_multi_thread_unchanged`: `#[tokio::test(flavor = "multi_thread")]`; 2 recorded exchanges; `inner.exchange(1)` returns an `ExchangeAssert` (no panic).
- `exchange_no_runtime_returns_assert`: plain `#[test]` (no async); build the component and send 1 exchange inside a manually spawned `tokio::runtime::Runtime` (`std::thread`-scoped or `Runtime::new()?.block_on(...)` then drop the runtime guard), then call `inner.exchange(0)` outside any runtime context → returns an `ExchangeAssert` for the recorded exchange without panicking.

**Acceptance:**
- `cargo test -p camel-component-mock --lib` green (existing multi_thread-flavored tests using `exchange` keep passing).
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings` exits 0; `cargo fmt --check` clean.

- [x] 5

### Task 6: README documentation + crate gate sweep

**Files:**
- `crates/components/camel-mock/README.md` (modified)

**Steps:**
1. Add a URI parameter table to README.md: 5 rows (`retain`, `copy`, `failFast`, `expectedCount`, `anyOrder`) with type, default (falls back to `MockConfig`), and semantics; `expectedCount` row carries the inertness note (assertion-time only, never rejects live traffic).
2. Add a short "Count expectations" section: `expect_count`/`expect_minimum_count` + `try_assert_satisfied` example (5-10 line code block, English).
3. Gate sweep, each command from the worktree root, all must exit 0: `cargo fmt --check --all`, `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings`, `cargo clippy -p camel-component-mock --all-targets -- -D warnings`, `cargo test -p camel-component-mock --lib`, `cargo xtask lint-unwrap`, `cargo xtask lint-secrets`, `cargo xtask lint-log-levels`, `cargo xtask lint-non-exhaustive`, `cargo xtask schema --check`.
4. Fix anything the sweep surfaces (docs wording, fmt drift) — zero functional changes expected in this task.

**Tests:**
- `readme_param_table_complete`: manual verification listed in the task report — README contains all 5 param names and the inertness note for `expectedCount` (`grep -c 'expectedCount' crates/components/camel-mock/README.md` ≥ 1).

**Acceptance:**
- All 9 gate commands in step 3 exit 0 (record exit codes in the report).
- README renders the 5-param table with the inertness note.

- [x] 6
