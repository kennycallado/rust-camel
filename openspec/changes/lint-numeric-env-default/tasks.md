# Tasks: lint-numeric-env-default

## camel-lint

### Task 1.1: Regression pins for the rc-jq775 shape family

**Files:**
- `crates/camel-lint/src/rules/rschema/tests.rs` (modified)

**Steps:**
1. Add the five tests below to `crates/camel-lint/src/rules/rschema/tests.rs`, near the existing carve-out tests (after `rschema_int_position_clean_default_no_diagnostic`). Use the module's existing `analyze` / `rschema_only` / `slice` helpers (the `analyze` helper already panics on parse failure, so every pin asserts successful parsing). Write the tests FIRST, run them, and confirm they all pass against the current implementation (they pin existing carve-out behavior — no production code changes in this task).
2. Every fixture is a block mapping — an unquoted `${env:...}` token never appears inside a flow mapping (the token's `}` terminates flow style and is an R-SYN parse error). Retry-policy fixtures include `max_attempts: 4` (RouteDslRedeliveryPolicy has no serde default for it; verified at `crates/camel-dsl/src/route_ast.rs:424`).

**Tests:** (executable spec — name, setup, action, assert)
- `rschema_delay_ms_full_config_default_no_diagnostic`: bare route `id: r1` / `from: direct:start` / `steps:` / `- delay:` / `delay_ms: ${env:OPS_X:-20000}` → `analyze` → `rschema_only(&diags).is_empty()` (carved leaves emit no Error and no Info). Repeat the assertion for all FOUR variants the blessed scenario names: unquoted bare, quoted bare (`"${env:OPS_X:-20000}"`), unquoted wrapped in a `routes:` envelope, and quoted wrapped in the same envelope.
- `rschema_delay_ms_shorthand_body_no_diagnostic`: bare route with steps `- delay: ${env:OPS_X:-20000}` (whole delay body is one token) → `analyze` → `rschema_only(&diags).is_empty()`.
- `rschema_delay_ms_and_dynamic_header_carve_per_leaf`: steps `- delay:` with `delay_ms: ${env:OPS_X:-20000}` and `dynamic_header: ${env:HDR:-3000}` → `analyze` → zero Errors; exactly one Info; `slice(source, &infos[0].span)` == `"${env:HDR:-3000}"` (the header leaf keeps its string semantics while delay_ms is carved).
- `rschema_retry_multiplier_clean_int_default_no_diagnostic`: `error_handler:` / `retry:` / `max_attempts: 4` / `multiplier: ${env:M:-2}` → `analyze` → `rschema_only(&diags).is_empty()` (NUMBER-typed position, clean-integer default, typed copy wins).
- `rschema_on_exceptions_retry_default_no_diagnostic`: `error_handler:` / `on_exceptions:` / `- retry:` / `max_attempts: 4` / `max_delay_ms: ${env:D:-10000}` → `analyze` → `rschema_only(&diags).is_empty()`.

  Command: `cargo test -p camel-lint --lib` (the full library suite — one positional filter maximum; the pin names are suite-covered).
  Expected: all five pass BEFORE any production change (they pin existing behavior). If any fails, STOP and report `test-design-gap: <which pin and actual output>` — that would mean the verification evidence is wrong.

**Acceptance:**
- All five new tests pass; the full `cargo test -p camel-lint --lib` suite passes.
- `cargo fmt --check` exits 0; `cargo clippy -p camel-lint --all-targets -- -D warnings` exits 0.
- No production file touched (`git status` shows only `tests.rs` modified).

- [x] 1.1

### Task 1.2: Suppress the substituted-default Info note under a covering Error

**Files:**
- `crates/camel-lint/src/rules/rschema.rs` (modified)
- `crates/camel-lint/src/rules/rschema/tests.rs` (modified)

**Steps:**
1. In `RSchemaRule::analyze` (crates/camel-lint/src/rules/rschema.rs), in the final Info-note loop (`for (sub, span) in substituted.iter().zip(info_spans)`), after the existing `carved_spans.contains(&span)` skip, add a second skip: if any diagnostic already in `diagnostics` has `severity == Severity::Error` and `span_covers(d.span.clone(), &span)` holds, `continue` (suppress the note). Reuse the existing free function `span_covers` (`fn span_covers(outer: Span, inner: &Span)` — outer by value, and `Span` is not `Copy`, hence the `.clone()` on the borrowed `d.span`); do not add a new helper. The `diagnostics` vector at that point already contains every Error (schema errors are pushed first, unresolved-token Errors second), so a single `diagnostics.iter().any(...)` is the complete check. Update the loop's doc comment: the note is truthful only when the leaf's chain is error-free; an ancestor-anchored Error span may also cover individually valid sibling leaves, suppressing their notes too (bounded to Info — Errors are never removed, downgraded, or relocated).
2. Migrate the one existing guard-arm pin that asserts Info beside an Error: in `bool_position_placeholder_still_error` (crates/camel-lint/src/rules/rschema/tests.rs, ~line 2122), replace the trailing "Info note must ALSO survive" block (`infos.len() == 1` assertion and its comment) with an assertion that `infos.is_empty()` plus a comment stating the suppression rule: the covering Error span suppresses the note; the Error assertion above the block stays unchanged.
3. Extend the two existing Error-only pins that own the delta's "no Info" clauses so they also pin suppression: in `int_position_notanumber_still_error` add a trailing `assert!(infos.is_empty(), ...)` for the substituted-default note (the `notanumber` Error covers its leaf), and in `bool_position_non_boolean_defaults_still_error` add the same zero-Info assertion for the `yes`/`TRUE`/`1` documents (both assertions FAIL before step 1 — the Info notes exist today — and PASS after, consistent with this task's TDD flow).
4. Add the five new tests below (write them first, confirm they FAIL on the current code with the expected Info present, then implement step 1 and confirm they pass).

**Tests:** (executable spec — name, setup, action, assert)
- `rschema_float_default_number_position_error_no_info`: `error_handler:` / `retry:` / `max_attempts: 4` / `multiplier: ${env:M:-2.0}` → `analyze` → exactly one Error whose span covers the placeholder (`slice(source, &errors[0].span).contains("${env:M:-2.0}")`; ancestor anchoring is fine, do not assert the exact anchor node); `infos` (severity Info) empty.
- `rschema_jitter_factor_float_default_error_no_info`: same shape with `jitter_factor: ${env:J:-0.5}` → one Error covering the placeholder; zero Info notes.
- `rschema_empty_default_integer_position_error_no_info`: steps `- delay:` with `delay_ms: "${env:OPS_X:-}"` → one Error covering the placeholder; zero Info notes.
- `rschema_collateral_info_suppression_bounded`: steps `- delay:` with `delay_ms: ${env:OPS_X:-2.0}` (float, boot-rejected) and `dynamic_header: ${env:HDR:-x-custom}` (valid string default under the same `DelayBody` anyOf), plus a disjoint step in the schema-valid block form `- set_header:` with `key: my-title` and `value: ${env:MY_TITLE:-hello}` (a scalar `set_header:` is schema-invalid — SetHeaderData requires a block mapping with `key`/`value` and would add a second Error) → exactly one Error (covering the float placeholder); exactly one Info, and `slice(source, &infos[0].span) == "${env:MY_TITLE:-hello}"` (the sibling header note is suppressed by the ancestor Error span; the disjoint string-position note survives; no Error removed).
- `rschema_int_position_2x_default_error_no_info`: `throttle:` / `max_requests: ${env:MY_LIMIT:-2x}` (the blessed `2x` scenario, not covered by any existing pin — `grep "2x"` finds nothing in tests.rs today) → exactly one Error whose span covers the placeholder; zero Info notes (the Error covers its leaf; `2x` is not a clean integer — boot rejects it too).

  Command: `cargo test -p camel-lint --lib` (the full library suite — cargo test accepts a single positional filter, so the pins are exercised by the suite run, not by a multi-filter invocation; multi-filter rejection must never count as a TDD red).
  Expected: FAIL before step 1 (Info notes present today), PASS after. `bool_position_placeholder_still_error` must pass in both its old form (before) and migrated form (after).

**Acceptance:**
- `cargo test -p camel-lint --lib` passes in full (238+ tests including migrated and new pins).
- `cargo fmt --check` exits 0; `cargo clippy -p camel-lint --all-targets -- -D warnings` exits 0.
- The suppression is present: `sed -n '/for (sub, span) in substituted/,/^        diagnostics$/p' crates/camel-lint/src/rules/rschema.rs | grep -c span_covers` prints ≥ 1 (the check is scoped to the Info-note loop region; a whole-file grep would be vacuous — the file already has 3+ `span_covers` sites).

- [x] 1.2

### Task 1.3: Typing-mirror documentation truth

**Files:**
- `crates/camel-lint/src/env_interpolation.rs` (modified)

**Steps:**
1. Update the `WholeScalarEnvToken::WithDefault` variant doc (crates/camel-lint/src/env_interpolation.rs, ~lines 193-196): replace the stale sentence (its contiguous line-195 fragment is `suppressed, replicating the boot tree-walk` — the phrase `re-inference suppressed` spans a line break and never matches a single-line grep) with wording that states the two-stage behavior — the mirror forces STRING typing and records the leaf as an integer/boolean carve-out candidate when the default is a clean integer / exact-lowercase boolean; `rschema.rs`'s `typed_carve_out_instance` then re-validates the candidate as NUMBER/BOOL when the STRING copy flags the leaf's chain and the typed probe clears it (Condition A/B, ancestor-anchored spans included). This stale comment is what produced bd rc-jq775's root-cause claim; keep it under one sentence-pair per stage, no behavior change.
2. Extend the module-header SYNC comment (~lines 1-9): keep the existing SYNC obligations (they are behavioral and unchanged) and add one line pointing to the lint-side carve-out pass: the whole-scalar token typing is consumed by `rules/rschema.rs` (`typed_carve_out_instance`), whose clean-integer/clean-boolean gates are the SYNC'd mirrors of `camel-dsl::env_int_probe` / `camel-config` — so the typing story cannot be read from this file alone.
3. Cross-check the `rschema.rs` module docs (lines 15-29) still match the corrected wording; fix only if a direct contradiction exists (they were updated in rc-93wct rev 2 and are expected consistent — do not rewrite them).

**Tests:** (executable spec — name, setup, action, assert)
- `mirror_parity_table` (existing): unchanged and passing — comments-only change must not alter behavior.
- `whole_scalar_detection_edges` (existing): unchanged and passing.
- Documentation-consistency check (manual, no test): `grep -n "re-inference suppressed" crates/camel-lint/src/env_interpolation.rs` returns no hits after the edit, and `grep -n "typed_carve_out_instance" crates/camel-lint/src/env_interpolation.rs` returns the new module-header pointer.

  Command: `cargo test -p camel-lint --lib env_interpolation`
  Expected: pass before and after (behavior-neutral task).

**Acceptance:**
- `! grep -q "suppressed, replicating the boot tree-walk" crates/camel-lint/src/env_interpolation.rs` exits 0 (the stale fragment is gone — checking the contiguous line-195 fragment, since the remembered phrase spans a line break and never matched) and `grep -q "typed_carve_out_instance" crates/camel-lint/src/env_interpolation.rs` exits 0 (the module-header pointer exists).
- `cargo test -p camel-lint --lib` passes in full.
- `cargo fmt --check` exits 0; `cargo clippy -p camel-lint --all-targets -- -D warnings` exits 0.

- [x] 1.3
