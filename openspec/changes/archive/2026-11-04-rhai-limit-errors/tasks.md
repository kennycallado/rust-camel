# Tasks: rhai-limit-errors

Target crate: `camel-language-rhai` (single crate, single-phase). All
paths repo-relative. Exact detail formats are contractual (blessed
spec): `{limit-name} {bound} exceeded`, timeout
`execution-timeout {ms}ms exceeded`, compile-time reason
`max-expression-depth {d} or max-function-expression-depth {f} exceeded`.

Conventions for every task: work only in this worktree; never build in
`/home/kenny/dev/rust-camel`; TDD (write the Tests-block tests first,
verify they FAIL against the current code — capture the failure output
as the baseline evidence — then implement until green); run
`cargo fmt --check` and `cargo clippy -p camel-language-rhai --all-targets -- -D warnings` before reporting; end the task with exactly
one commit `feat(rhai-language-rhai): <imperative summary>` (caveman
style, ≤50 char subject).

## camel-language-rhai

### Task 1.1: Thread ResolvedRhaiLimits into the detail mapper (behavior-neutral)

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)

**Steps:**
1. Change `eval_alt_detail` to
   `fn eval_alt_detail(e: &rhai::EvalAltResult, limits: &ResolvedRhaiLimits) -> Option<String>`;
   keep the match arms and returned strings EXACTLY as today
   (`"evaluation limit exceeded"`, `"script terminated"`, `"function
   not found"`). The `limits` parameter is intentionally unused in the
   arms for now — do NOT add `#[allow(unused)]`, instead prefix the
   binding with `_` only if the compiler demands it; the parameter is
   consumed again in Task 1.2.
2. Change `map_eval_alt` to
   `fn map_eval_alt(e: &rhai::EvalAltResult, limits: &ResolvedRhaiLimits) -> LanguageError`
   and pass `limits` through to `eval_alt_detail`.
3. Change `eval_error` to
   `fn eval_error(e: &rhai::EvalAltResult, target: &str, limits: &ResolvedRhaiLimits) -> LanguageError`
   and pass through to `map_eval_alt` (the `stream_body::map_stream_refusal`
   first-arm is unchanged).
4. Add `#[derive(Clone, Copy)]` to `ResolvedRhaiLimits` — the
   predicate-group clause path and `eval_async` call eval helpers
   inside `spawn_blocking`'s `'static` move closures, so a
   `&ResolvedRhaiLimits` cannot be borrowed there; pass owned copies.
   Update every `eval_error`/`map_eval_alt` call site to pass the
   resolved limits. Exact call-site map (verify with the compiler):
   `eval_error` is called at `eval_with_engine` (read-only path) and
   directly in `eval_mut_sync`; `map_eval_alt` has exactly one caller
   (`eval_error`); `eval_with_engine` is called from `eval_sync` and
   the predicate-group clause path.
5. Verify NO user-visible string changed: run the full lib test suite.

**Tests:** (existing suite only — this task is a pure refactor)
- `command`: `cargo test -p camel-language-rhai --lib`
- `expected`: all pre-existing tests pass, zero new tests, zero
  behavioral diffs in error strings.

**Acceptance:**
- `cargo test -p camel-language-rhai --lib` exits 0.
- `cargo fmt --check` exits 0.
- `cargo clippy -p camel-language-rhai --all-targets -- -D warnings`
  exits 0.
- `git diff main...HEAD -- crates/languages/camel-language-rhai/src/lib.rs`
  contains no change to any string literal.

- [x] 1.1

### Task 1.2: Eval-time limit details name the limit and its bound

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)

**Steps:**
1. Add a private helper:
   `fn data_too_large_detail(label: &str, limits: &ResolvedRhaiLimits) -> String`
   that exact-matches the rhai engine labels:
   `"Length of string"` → `format!("max-string-size {} exceeded", limits.max_string_size)`;
   `"Size of array/BLOB"` or `"Size of BLOB"` → `format!("max-array-size {} exceeded", limits.max_array_size)`;
   `"Size of object map"` → `format!("max-map-size {} exceeded", limits.max_map_size)`;
   anything else → `"data-size limit exceeded"` (name-only, no
   invented bound). The label text is never interpolated into the
   result.
2. In `eval_alt_detail`, replace the generic arm:
   `ErrorTooManyOperations(_)` →
   `format!("max-operations {} exceeded", limits.max_operations)`;
   `ErrorDataTooLarge(label, ..)` → `data_too_large_detail(label, limits)`;
   `ErrorStackOverflow(_)` →
   `format!("max-call-levels {} exceeded", limits.max_call_levels)`;
   `ErrorTooManyVariables(_)` → `"max-variables exceeded"`;
   `ErrorTooManyModules(_)` → `"max-modules exceeded"`.
   Keep `ErrorTerminated(..)` → `"script terminated"` and
   `ErrorFunctionNotFound(..)` → `"function not found"` unchanged.
   Remove the `_` prefix from the `limits` binding left by Task 1.1.
3. Do NOT touch `eval_alt_class` (stays payload-blind) or the
   json-guard rewrite in `map_eval_alt`.
4. Write the Tests-block tests FIRST (they fail on Task 1.1's code —
   capture the failing assertions as baseline evidence), then make
   them pass.

**Tests:** (all in the existing `#[cfg(test)] mod`; use the existing
helpers `RhaiLanguage::with_limits`, `RhaiLimitsConfig`,
`exchange_with_body`; every scripted test asserts BOTH the exact
detail string AND `ExpressionErrorClass::Limit` via a
`LanguageError::EvalFailure` pattern match)

1. `limit_detail_names_operations_with_custom_bound`
   - setup: `RhaiLanguage::with_limits(RhaiLimitsConfig { max_operations: Some(500), ..Default::default() })`
   - action: create + evaluate the expression
     `"let x = 0; loop { x += 1; if x > 1000 { break; } } x"`
   - assert: error is `EvalFailure` with class `Limit` and
     `detail == Some("max-operations 500 exceeded".into())` (exact)
   - command: `cargo test -p camel-language-rhai --lib limit_detail_names_operations_with_custom_bound`
   - expected: FAIL before implementation (detail is the generic
     string), PASS after.
2. `limit_detail_names_string_size_with_custom_bound`
   - setup: `max_string_size: Some(64)`
   - action: evaluate
     `"let s = \"\"; loop { s = s + \"aaaaaaaaaaaaaaaaaaaa\"; if s.len() > 200 { break; } } s"`
   - assert: class `Limit`, `detail == Some("max-string-size 64 exceeded".into())`
   - command: `cargo test -p camel-language-rhai --lib limit_detail_names_string_size_with_custom_bound`
   - expected: FAIL before, PASS after.
3. `limit_detail_names_array_size_with_custom_bound`
   - setup: `max_array_size: Some(50)`
   - action: evaluate
     `"let a = []; loop { a.push(1); if a.len() > 200 { break; } } a"`
   - assert: class `Limit`, `detail == Some("max-array-size 50 exceeded".into())`
   - command: `cargo test -p camel-language-rhai --lib limit_detail_names_array_size_with_custom_bound`
   - expected: FAIL before, PASS after.
4. `limit_detail_names_map_size_with_custom_bound`
   - setup: `max_map_size: Some(50)`
   - action: evaluate
     `"let m = #{}; loop { let k = m.len().to_string(); m[k] = 1; if m.len() > 200 { break; } } m"`
   - assert: class `Limit`, `detail == Some("max-map-size 50 exceeded".into())`
   - command: `cargo test -p camel-language-rhai --lib limit_detail_names_map_size_with_custom_bound`
   - expected: FAIL before, PASS after.
5. `limit_detail_names_call_levels_with_custom_bound`
   - setup: `max_call_levels: Some(8)`
   - action: evaluate `"fn f() { f() } f()"`
   - assert: class `Limit`, `detail == Some("max-call-levels 8 exceeded".into())`
   - command: `cargo test -p camel-language-rhai --lib limit_detail_names_call_levels_with_custom_bound`
   - expected: FAIL before, PASS after.
6. `limit_detail_shows_default_operations_bound`
   - setup: `RhaiLanguage::new()` (all defaults)
   - action: evaluate `"loop {}"`
   - assert: class `Limit`, `detail == Some("max-operations 100000 exceeded".into())`
     (proves the DEFAULT bound appears. Determinism evidence: the
     existing green `test_rhai_infinite_loop_trips_max_operations`
     (defaults, `loop {}`) proves the 100_000 ops budget trips before
     the 5s default timeout in `opt-level 2` test builds — no hedge
     needed, the race cannot happen)
   - command: `cargo test -p camel-language-rhai --lib limit_detail_shows_default_operations_bound`
   - expected: FAIL before, PASS after.
7. `data_too_large_detail_maps_known_labels_exactly` (pure unit test,
   no engine): construct `rhai::EvalAltResult::ErrorDataTooLarge`
   with each of `"Length of string"`, `"Size of array/BLOB"`,
   `"Size of BLOB"`, `"Size of object map"` and a
   `ResolvedRhaiLimits { max_string_size: 64, max_array_size: 50, max_map_size: 77, .. }`
   (others irrelevant). Pass every constructed `EvalAltResult`
   through `map_eval_alt(&error, &limits)`.
   - assert: each mapped error pattern-matches
     `LanguageError::EvalFailure` with
     `class == ExpressionErrorClass::Limit` and `detail` respectively
     `Some("max-string-size 64 exceeded")`,
     `Some("max-array-size 50 exceeded")`,
     `Some("max-array-size 50 exceeded")`,
     `Some("max-map-size 77 exceeded")` (direct `eval_alt_detail`
     assertions may remain, but do not replace the mapped-error
     assertions)
   - command: `cargo test -p camel-language-rhai --lib data_too_large_detail_maps_known_labels_exactly`
   - expected: FAIL before, PASS after.
8. `data_too_large_detail_unknown_label_and_defensive_variants_stay_name_only`
   (pure unit test): labels
   `"unrecognized string-map SENTINEL_xyzzy"` (contains both category
   keywords), `EvalAltResult::ErrorTooManyVariables(Position::NONE)`,
   `EvalAltResult::ErrorTooManyModules(Position::NONE)` — pass each
   through `map_eval_alt(&error, &limits)` and pattern-match
   `LanguageError::EvalFailure`, asserting
   `class == ExpressionErrorClass::Limit`.
   - assert: mapped details respectively
     `Some("data-size limit exceeded")`,
     `Some("max-variables exceeded")`,
     `Some("max-modules exceeded")`; none contains
     `SENTINEL_xyzzy`; the data-too-large detail never contains the
     word `"unrecognized"` (raw label never rendered)
   - command: `cargo test -p camel-language-rhai --lib data_too_large_detail_unknown_label_and_defensive_variants_stay_name_only`
   - expected: FAIL before, PASS after.
9. `limit_detail_stays_operand_free_on_map_trip`
   - setup: `max_map_size: Some(50)`
   - action: evaluate
     `"let m = #{}; loop { let k = \"SECRET_value_\" + m.len().to_string(); m[k] = 1; if m.len() > 200 { break; } } m"`
   - assert: `detail == Some("max-map-size 50 exceeded".into())` exactly
     AND the rendered `LanguageError` does not contain
     `SECRET_value_` (format it with `{err:?}` and assert
     `!formatted.contains("SECRET_value_")` — note `ParseError`-class
     errors may echo the script by contract, but this is an
     `EvalFailure` and must not).
   - command: `cargo test -p camel-language-rhai --lib limit_detail_stays_operand_free_on_map_trip`
   - expected: FAIL before, PASS after.

**Acceptance:**
- All nine tests above pass; `cargo test -p camel-language-rhai --lib`
  exits 0 (pre-existing suite included).
- `cargo fmt --check` and
  `cargo clippy -p camel-language-rhai --all-targets -- -D warnings`
  exit 0.
- No string other than the contracted details changed.

- [x] 1.2

### Task 1.3: Timeout details name the configured duration at all three sites

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)

**Steps:**
1. In `eval_async` (shared by `RhaiExpression::evaluate` and
   `RhaiPredicate::matches`): the `.map_err(|_elapsed| ...)` closure
   that currently builds `EvalFailure { class: Timeout, position: None,
   detail: None }` must emit
   `detail: Some(format!("execution-timeout {}ms exceeded", r.execution_timeout_ms))`.
   Capture the resolved value before the async block
   (`let timeout_ms = r.execution_timeout_ms;`) so the closure only
   captures a `u64` copy.
2. In `PredicateGroup for RhaiPredicateGroup::first_match`: locate its
   tokio-timeout `Elapsed` mapping (same shape) and apply the same
   detail using the `r` already resolved there. The per-clause
   `IndexedPredicateError` wrapping is unchanged.
3. In `MutatingExpression for RhaiMutatingExpression::evaluate`: the
   `Err(_elapsed)` arm of the `tokio::time::timeout(...).await` match
   must emit the same detail (this site matches on
   `Ok(Ok(Ok(..)))/...` arms — add/change only the `Err(_elapsed)`
   arm's `LanguageError`).
4. Do NOT change `ErrorTerminated` handling or any class.
5. Write the Tests-block tests FIRST (fail on current `detail: None`),
   then make them pass.

**Tests:** (limits pattern for all four:
`RhaiLimitsConfig { max_operations: Some(500_000_000), execution_timeout_ms: Some(50), ..Default::default() }`,
script `"loop {}"`, `exchange_with_body("test")`; each asserts class
`Timeout` AND `detail == Some("execution-timeout 50ms exceeded".into())`)

1. `timeout_detail_names_custom_bound_expression`
   - action: `RhaiLanguage::with_limits(limits).create_expression("loop {}")` → `Expression::evaluate`
   - command: `cargo test -p camel-language-rhai --lib timeout_detail_names_custom_bound_expression`
   - expected: FAIL before (detail `None`), PASS after.
2. `timeout_detail_names_custom_bound_predicate`
   - action: same limits, `create_predicate("loop {}")` → `Predicate::matches` → assert `matches` returned `Err` with the contracted error
   - command: `cargo test -p camel-language-rhai --lib timeout_detail_names_custom_bound_predicate`
   - expected: FAIL before, PASS after.
3. `timeout_detail_names_custom_bound_group_first_match`
   - action: `create_predicate_group(&["loop {}", "true"])` → `PredicateGroup::first_match` → assert `IndexedPredicateError` index `0` wrapping the contracted error (the group must fail on the first clause, never reach `"true"`; also assert the error detail survived the wrapping)
   - command: `cargo test -p camel-language-rhai --lib timeout_detail_names_custom_bound_group_first_match`
   - expected: FAIL before, PASS after.
4. `timeout_detail_names_custom_bound_mutating`
   - action: `create_mutating_expression("loop {}")` → `MutatingExpression::evaluate` → assert contracted error
   - command: `cargo test -p camel-language-rhai --lib timeout_detail_names_custom_bound_mutating`
   - expected: FAIL before, PASS after.

**Acceptance:**
- All four tests pass; `cargo test -p camel-language-rhai --lib`
  exits 0 (existing timeout tests included — they assert only
  `is_err` and elapsed time, they must stay green untouched).
- `cargo fmt --check` and
  `cargo clippy -p camel-language-rhai --all-targets -- -D warnings`
  exit 0.

- [x] 1.3

### Task 1.4: Compile-time depth reason names both depth limits; final green

**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)

**Steps:**
1. Add a private helper
   `fn compile_error_to_parse_error(e: &rhai::ParseError, script: &str, limits: &RhaiLimitsConfig) -> LanguageError`:
   when `e.err_type()` is `rhai::ParseErrorType::ExprTooDeep`, build
   `LanguageError::ParseError { expr: script.to_string(), reason: format!("max-expression-depth {} or max-function-expression-depth {} exceeded", resolve_rhai_limits(limits).max_expression_depth, resolve_rhai_limits(limits).max_function_expression_depth) }`
   (resolve ONCE and reuse, do not call twice); any other parse error
   keeps today's `reason: e.to_string()`. API verified in vendored
   rhai 1.27.0: `src/types/parse_error.rs` defines
   `pub const fn err_type(&self) -> &ParseErrorType`, and
   `ParseErrorType::ExprTooDeep` exists.
2. Replace the three compile-error closures with the helper:
   `compile_read_only`'s walk-AST and exec-AST `.map_err(...)`, and
   `create_mutating_expression`'s `.map_err(...)`. The
   `debug!(error = %e, ...)` log lines keep their current placement
   and text.
3. Do NOT change `find_read_only_mutation`'s separate ParseError path
   or any other ParseError producer.
4. Write the Tests-block tests FIRST (fail on current `e.to_string()`
   reasons), then make them pass.
5. Final full-crate verification (this task closes the change):
   `cargo test -p camel-language-rhai` (runs lib + all three
   integration suites, including `engine_reuse_isolation`),
   `cargo fmt --check`,
   `cargo clippy -p camel-language-rhai --all-targets -- -D warnings`.

**Tests:** (every depth test asserts the returned error is
`LanguageError::ParseError` and `reason` equals the contracted string
EXACTLY — no `contains`)

1. `depth_error_reason_names_top_level_depth_custom_bound`
   - setup: `RhaiLimitsConfig { max_expression_depth: Some(4), ..Default::default() }`
     (function depth stays at default 32, non-binding for this script)
   - action: `RhaiLanguage::with_limits(limits).create_expression("let x = (((((((1)))))); x")`
     (nesting depth 8 > 4 trips at parse/create time)
   - assert: `ParseError` with reason exactly
     `"max-expression-depth 4 or max-function-expression-depth 32 exceeded"`
   - command: `cargo test -p camel-language-rhai --lib depth_error_reason_names_top_level_depth_custom_bound`
   - expected: FAIL before (reason is rhai's own text), PASS after.
2. `depth_error_reason_names_function_depth_custom_bound`
   - setup: `RhaiLimitsConfig { max_function_expression_depth: Some(3), ..Default::default() }`
     (top-level depth stays at default 64, non-binding for this script)
   - action: `create_expression("fn f() { let x = (((((1))))); x } f()")`
     (the function BODY trips the function depth setting at parse time;
     the top-level statement stays shallow)
   - assert: `ParseError` with reason exactly
     `"max-expression-depth 64 or max-function-expression-depth 3 exceeded"`
   - command: `cargo test -p camel-language-rhai --lib depth_error_reason_names_function_depth_custom_bound`
   - expected: FAIL before, PASS after.
3. `depth_error_reason_applies_on_mutating_path`
   - setup: `max_expression_depth: Some(4)`
   - action: `create_mutating_expression("let x = (((((((1)))))); body = x;")`
   - assert: `ParseError` with reason exactly
     `"max-expression-depth 4 or max-function-expression-depth 32 exceeded"`
   - command: `cargo test -p camel-language-rhai --lib depth_error_reason_applies_on_mutating_path`
   - expected: FAIL before, PASS after.

If a script in tests 1–3 does NOT trip, report the actual engine
behavior — but no substitution is expected: `ExprTooDeep` is raised in
the PARSER (`ParseSettings::level_up`, vendored
`src/parser.rs:262-267`) before any optimizer runs, so paren nesting
trips deterministically on BOTH engines in `compile_read_only`
(`OptimizationLevel::None` and `Simple`); function bodies parse under
`max_function_expr_depth` (`src/parser.rs:3370`).

**Acceptance:**
- All three tests pass with the exact reason strings.
- Full crate verification green: lib tests, both integration test
  files, fmt, clippy (commands in Steps 5) — record each exit code in
  the task report.
- `git diff main...HEAD` contains no change to
  `eval_alt_class`, `find_read_only_mutation`, or any public enum.

- [x] 1.4
