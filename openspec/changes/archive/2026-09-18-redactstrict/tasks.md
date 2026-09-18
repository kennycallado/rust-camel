# Tasks: redactstrict

## Phase 1: Call-shape + binding-local redemption

### scripts/xtask

#### Task 1.1: TDD — red tests for name-based redemption bypass, then call-shape + binding-local rewrite

**Files:**
- `scripts/xtask/src/main.rs` (modified — `redaction_violation_reason` ~L3116-3198, its doc comment, and `mod lint_log_redaction_tests` ~L7235-7378)

**Steps:**
1. RED: add these four tests to `mod lint_log_redaction_tests` (reuse the `violations_for` harness). Three currently pass the lint (real RED) — verify they FAIL before implementing. The fourth is an always-green regression guard for the spec's fn-pointer scenario:
   - `redact_named_ident_no_longer_redeems` (RED): file `crates/foo/src/lib.rs` = `fn f(redacted_url: &str) { tracing::debug!(url = %redacted_url, "req"); }` → assert exactly 1 violation.
   - `fn_pointer_does_not_redeem` (regression guard, GREEN today and must stay green): `fn f(u: &[String]) { tracing::debug!(url = %u.iter().map(redact_url).count(), "req"); }` → assert exactly 1 violation. Nested tokens inside a parenthesized Group are invisible to the top-level segment scan, so this is already flagged; the test guards against a future recursive-redemption regression.
   - `binding_with_redact_named_ident_no_longer_redeems_capture` (RED): `fn f(redacted_url: &str) { tracing::warn!("failed {url}", url = redacted_url); }` → assert exactly 1 violation.
   - `implicit_capture_not_redeemed_by_unrelated_call` (RED): `fn f(x: i32) { tracing::debug!("at {url}", other = redact_seed(x)); }` → assert exactly 1 violation.
2. Verify red: `RUSTC_WRAPPER= cargo test -p xtask lint_log_redaction` — exactly the three RED tests fail, the regression guard and every existing module test pass.
3. GREEN — rewrite the redemption predicates inside `redaction_violation_reason`:
   a. Add a helper `fn has_redact_call(seg: &[&proc_macro2::TokenTree]) -> bool` returning true iff some index `i` has `seg[i]` = Ident whose lowercase form contains `redact` AND `seg[i+1]` = `TokenTree::Group` with `delimiter() == Delimiter::Parenthesis`. Use the same `seg.get(idx + 1)` lookahead style as the existing `is_punct_dot` closure (clippy-clean pattern).
   b. Replace `seg_has_redact` (name-based `contains("redact")` over segment idents) with `seg_has_redact_call = has_redact_call(seg)` in rule 1 (sensitive-identifier use).
   c. Delete `macro_has_redact` (macro-wide redemption). Rule 2 (message-capture): extract the captured identifier NAMES, not just a boolean match — hoist the capture pattern into a module-level `const LOG_REDACTION_CAPTURE_PATTERN: &str` (same regex text, with capture groups added around the `[?#!]?` prefix and the name alternation), build `capture_re` from it inside `lint_log_redaction`, and iterate `capture_re.captures_iter(&literal)` per string literal in a segment (group 2 = the binding key; handles multiple captures per literal). Redeem a capture only when the segment binding that ident satisfies `has_redact_call`. Binding detection: scan segments for one whose first token is the Ident equal to the captured name followed by Punct `=`. If no binding segment exists → violation (implicit capture is unredeemable). Each capture resolves against its own binding segment independently.
   d. The capture pattern's NAME SET stays unchanged in this phase (only grouping syntax changes).
4. Add these green-path tests (same harness):
   - `pre_redacted_local_outside_macro_flagged`: `fn f(u: &str) { let url = redact_url(u); tracing::debug!(url = %url, "req"); }` → assert exactly 1 violation (span-local contract).
   - `path_qualified_call_redeems`: `fn f(u: &str) { tracing::debug!(url = %camel_api::redact::redact_url(u), "req"); }` → assert 0 violations.
   - `captures_resolve_independently`: `fn f(u: &str, r: &str) { tracing::debug!("{url} {uri}", url = redact_url(u), uri = r); }` → assert exactly 1 violation.
   - `bound_raw_not_redeemed_by_unrelated_call`: `fn f(r: &str, x: i32) { tracing::debug!("at {url}", url = r, other = redact_seed(x)); }` → assert exactly 1 violation (verbatim spec scenario: raw binding AND an unrelated redact call; green today via rule 1, guards the binding-local rule 2).
   - `event_level_segment_survives`: `fn f(u: &str) { tracing::event!(tracing::Level::DEBUG, url = %u); }` → assert exactly 1 violation; and `fn f(u: &str) { tracing::event!(tracing::Level::DEBUG, url = %redact_url(u)); }` → assert 0 violations.
5. Update the `lint_log_redaction` / `redaction_violation_reason` doc comments: redemption is call-shape (a redact-named helper invoked with a parenthesized argument list — free fn, path-qualified fn, or method call) and span-local; message captures are redeemed only by their own binding segment; implicit captures are unredeemable; a pre-redacted local outside the macro must wrap at the call site or use an escape hatch.
6. Verify green: `RUSTC_WRAPPER= cargo test -p xtask lint_log_redaction` — every test passes (all existing + the 9 new). Existing tests `allows_redacted_url_field`, `allows_to_redacted_string_helper`, `allows_inline_capture_with_redacted_binding` must remain green unchanged (call-shape covers them).
7. Corpus delta check: `RUSTC_WRAPPER= cargo run -q -p xtask -- lint-log-redaction` — must report OK (0 violations); any hit means a live site relied on name-based redemption — report it, do not allowlist in this task.
8. `RUSTC_WRAPPER= cargo fmt --all --check` and `RUSTC_WRAPPER= cargo clippy -p xtask --all-targets -- -D warnings` clean.

**Tests:** (executable spec — name, arrange, act, assert)
- `redact_named_ident_no_longer_redeems`: bare ident `redacted_url` as sensitive value → run lint → exactly 1 violation. Expected RED before step 3, GREEN after.
- `fn_pointer_does_not_redeem`: `map(redact_url)` value-passed fn → exactly 1 violation. GREEN throughout (regression guard; rationale in step 1).
- `binding_with_redact_named_ident_no_longer_redeems_capture`: `"failed {url}", url = redacted_url` → exactly 1 violation. RED before, GREEN after.
- `implicit_capture_not_redeemed_by_unrelated_call`: `"at {url}", other = redact_seed(x)` → exactly 1 violation. RED before, GREEN after.
- `pre_redacted_local_outside_macro_flagged`: `let url = redact_url(u); debug!(url = %url)` → exactly 1 violation. GREEN after implementation (span-local contract).
- `path_qualified_call_redeems`: `url = %camel_api::redact::redact_url(u)` → 0 violations. GREEN after.
- `captures_resolve_independently`: `"{url} {uri}", url = redact_url(u), uri = r` → exactly 1 violation. GREEN after.
- `bound_raw_not_redeemed_by_unrelated_call`: `"at {url}", url = r, other = redact_seed(x)` → exactly 1 violation. GREEN throughout (rule 1 catches the raw binding today; guards binding-local rule 2).
- `event_level_segment_survives`: `event!(Level::DEBUG, url = %u)` → 1 violation; with `redact_url(u)` → 0. GREEN after.
- Command: `RUSTC_WRAPPER= cargo test -p xtask lint_log_redaction`

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p xtask lint_log_redaction` exits 0 (all existing + 9 new tests).
- `RUSTC_WRAPPER= cargo run -q -p xtask -- lint-log-redaction` prints OK (0 violations).
- `RUSTC_WRAPPER= cargo clippy -p xtask --all-targets -- -D warnings` exits 0.
- `RUSTC_WRAPPER= cargo fmt --all --check` exits 0.
- No name-set change to `LOG_REDACTION_SENSITIVE` or the capture pattern in this task (step 3c's grouping refactor excepted).

- [x] 1.1

## Phase 2: Sensitive-list extension + atomic corpus remediation

### scripts/xtask

#### Task 2.1: TDD — extend sensitive set with endpoint/address/host/remote

**Files:**
- `scripts/xtask/src/main.rs` (modified — `LOG_REDACTION_SENSITIVE` ~L2990, `LOG_REDACTION_CAPTURE_PATTERN` (introduced by Task 1.1), `mod lint_log_redaction_tests`)

**Steps:**
1. RED: add tests (existing harness) that fail against the current 9-name list:
   - `flags_endpoint_field_name`: `fn f(v: &str) { tracing::debug!(endpoint = %v, "req"); }` → exactly 1 violation.
   - `flags_host_leaf_access`: `fn f(s: &Srv) { tracing::debug!(host = %s.host, "req"); }` → exactly 1 violation.
   - `flags_address_capture_without_binding`: `fn f() { tracing::warn!("bound to {address}"); }` → exactly 1 violation.
   - `flags_qualified_sensitive_leaf`: `fn f(c: &Cfg) { tracing::debug!(host = %c.host, "req"); }` → exactly 1 violation (`config.host`-shape: `host` is the sensitive leaf).
2. Verify red, then GREEN: append `"endpoint", "address", "host", "remote"` to `LOG_REDACTION_SENSITIVE` (after the existing 9, same order as written here) and append `endpoint|address|host|remote` to the name alternation inside `LOG_REDACTION_CAPTURE_PATTERN` (keep the existing order, append the four at the end).
3. Add false-positive guard tests (GREEN immediately — prove detection stays `{ident}`-capture/exact-match only):
   - `suffixed_variants_not_caught`: `tracing::debug!(remote_addr = %a, host_name = %n, endpoint_id = %e)` → 0 violations.
   - `prose_words_do_not_fire`: `tracing::info!("direct endpoint created")` and `tracing::debug!("MCP remote '{name}' rejected", name = n)` → 0 violations.
   - `object_position_not_caught`: `tracing::debug!(topic = config.topic, "sub")` → 0 violations.
   - `sensitive_set_and_capture_re_parity`: extract the name alternation from `LOG_REDACTION_CAPTURE_PATTERN`, split it into whole tokens on `|`, and assert set equality with `LOG_REDACTION_SENSITIVE` in BOTH directions (a plain `contains(name)` substring check would false-pass: `url` is a substring of `base_url` — whole-token comparison only).
4. Update doc comments to match the extended set: the `LOG_REDACTION_SENSITIVE` doc (~L2983-2989) and the `lint_log_redaction` doc (~L3005-3008) must name the endpoint/address/host/remote additions. The two violation reason strings (~L3177, ~L3191) stay VERBATIM — the spec quotes them as-is.
5. Verify: `RUSTC_WRAPPER= cargo test -p xtask lint_log_redaction` all green; fmt + clippy clean. Do NOT commit separately from Task 2.2 — Phase 2 lands as a single atomic commit (design.md): the list extension leaves the workspace corpus red until 2.2's site fixes land with it.

**Tests:**
- `flags_endpoint_field_name` / `flags_host_leaf_access` / `flags_address_capture_without_binding` / `flags_qualified_sensitive_leaf`: new names caught as field, shorthand-adjacent leaf, unbound capture, qualified leaf → exactly 1 violation each. RED before step 2, GREEN after.
- `suffixed_variants_not_caught` / `prose_words_do_not_fire` / `object_position_not_caught`: exact-match + capture-regex-only semantics → 0 violations each. GREEN throughout.
- `sensitive_set_and_capture_re_parity`: `LOG_REDACTION_SENSITIVE` set == `LOG_REDACTION_CAPTURE_PATTERN` name alternation (both directions). GREEN after step 2.
- Command: `RUSTC_WRAPPER= cargo test -p xtask lint_log_redaction`

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p xtask lint_log_redaction` exits 0 (26+ tests).
- `LOG_REDACTION_SENSITIVE` has exactly 13 entries; parity with the capture pattern is test-enforced.
- `RUSTC_WRAPPER= cargo clippy -p xtask --all-targets -- -D warnings` and `RUSTC_WRAPPER= cargo fmt --all --check` exit 0.
- Not committed separately from Task 2.2.

- [x] 2.1

### workspace corpus

#### Task 2.2: Corpus audit — remediate every newly-caught site atomically

**Files:**
- `scripts/xtask/allowlist-log-redaction.txt` (modified — justified entries only)
- Corpus log sites enumerated by the audit command (expected hot crates per pre-flight: `crates/camel-ws`, `crates/camel-redis`, `crates/camel-grpc`, `crates/camel-http`, `services/` otel+prometheus; ~12-15 true sites) — each becomes a `(modified)` edit at its exact path

**Steps:**
1. Run the audit in the worktree: `RUSTC_WRAPPER= cargo run -q -p xtask -- lint-log-redaction`. Record the full violation list (file:line + snippet) into the task report.
2. For each violation, classify and remediate:
   a. Value is a URL/URI or contains a URL/URI → wrap at the call site with `camel_api::redact::redact_url` / `redact_url_fail_closed` (add the import; credential-free values pass through per ADR-0076).
   b. Value is a bare IP / host:port / socket address with provably no credential substrate AND wrapping is semantically wrong (e.g. `bound_addr.ip()` on a listener bind log) → add an allowlist entry `<relative path>:<line>` to `scripts/xtask/allowlist-log-redaction.txt` with a `#` justification comment on the line above naming the value and why it is credential-free.
   c. Value is dynamic host/endpoint text that is not a URL → prefer a redact helper; if none applies, allowlist with justification per (b). No other dispositions exist — every site gets exactly one.
3. Re-run `RUSTC_WRAPPER= cargo run -q -p xtask -- lint-log-redaction` until OK (0 violations).
4. For every crate with a code edit (not allowlist-only): `RUSTC_WRAPPER= cargo clippy -p <crate> --all-targets -- -D warnings` clean and `RUSTC_WRAPPER= cargo test -p <crate> --lib` green.
5. `RUSTC_WRAPPER= cargo fmt --all --check` clean.
6. Run every sibling xtask lint gate (each must exit 0 before the atomic commit): `RUSTC_WRAPPER= cargo run -q -p xtask -- lint-unwrap`, `lint-secrets`, `lint-non-exhaustive`, `lint-log-levels`, `lint-ignore`, `lint-publish-cycles`, `lint-publish-registration`, `lint-component-deps`, `lint-gate-forwarding`, `lint-context-citations`, `lint-metric-labels`.
7. Report: per-site table (site → disposition a/b/c → one-line rationale).

**Tests:**
- `corpus_clean_after_remediation`: full-workspace audit command → exit code 0 with output `lint-log-redaction: OK (0 violations)`. Command: `RUSTC_WRAPPER= cargo run -q -p xtask -- lint-log-redaction`. Expected GREEN only after step 3.
- `allowlist_entries_justified`: every non-comment line in `scripts/xtask/allowlist-log-redaction.txt` added by this task has a preceding `#` justification line naming the value and credential-free rationale. Verified by reading the file in review.
- `touched_crates_still_green`: per touched crate, `RUSTC_WRAPPER= cargo test -p <crate> --lib` exits 0.

**Acceptance:**
- `RUSTC_WRAPPER= cargo run -q -p xtask -- lint-log-redaction` prints OK (0 violations).
- Every newly-caught site has exactly one disposition; zero unclassified sites.
- `RUSTC_WRAPPER= cargo clippy -p xtask --all-targets -- -D warnings` and fmt clean; touched-crate clippy/test green.
- All 11 sibling lint gates (step 6 list) exit 0.
- No changes outside log statements, their imports, and the allowlist file.

- [x] 2.2
