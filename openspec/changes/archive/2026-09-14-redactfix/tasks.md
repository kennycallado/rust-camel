# Tasks: redactfix

## camel-component-http

### Task 1.1: Fail-closed Err arm + char-boundary cap in `redact_url_for_diagnostics`

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Add a private helper immediately after `redact_url_for_diagnostics` (currently at ~line 2892):
   `fn truncate_utf8_safe(s: &mut String, max: usize)` — if `s.len() <= max`, no-op; else compute the largest index `i <= max` where `s.is_char_boundary(i)` holds (walk down from `max` at most 3 bytes), then `s.truncate(i)`. This replaces every raw `s.truncate(MAX_URL_LOG_LEN)` call.
2. Replace both Ok-arm `s.truncate(MAX_URL_LOG_LEN)` sites (query-present path and plain path) with `truncate_utf8_safe(&mut s, MAX_URL_LOG_LEN)`. Behavior on the `Ok` arm is identical because `url` serialization is ASCII; `MAX_URL_LOG_LEN` stays 256. The Err-arm truncate site is replaced wholesale by step 3.
3. Rewrite the `Err(_)` arm to, in order:
   a. Compute the authority window: if `raw` contains `"//"`, the window starts at `raw.find("//") + 2` and ends at the first `/`, `?`, or `#` at or after that start, extending to the END OF INPUT if no terminator follows (a terminator-less authority such as `http://u:secretpw@host:99999` still carries credentials and must suppress). If the window slice contains `'@'`, return `"[redacted]".to_string()` immediately — nothing else of the input is rendered.
   b. Else, if `raw` contains `'?'`, build the output as `raw[..first_question_index]` + `"?[redacted]"`.
   c. Else the output is `raw.to_string()`.
   d. Apply `truncate_utf8_safe(&mut s, MAX_URL_LOG_LEN)` and return `s`. (For a long pre-query body the cap governs: no query byte survives because step b already dropped it; the suffix appears only when it fits.)
4. Add the following unit tests inside the existing `mod tests` block that holds `redact_url_masks_userinfo_and_query` (~line 3669), following its plain `#[test]` style. Each test that names an "unparseable" fixture first asserts `url::Url::parse(<fixture>).is_err()` as a precondition so fixture drift fails loudly instead of vacuously passing.
5. Run the test filter and confirm the fail-first split against the current implementation: `redact_url_suppresses_unparseable_authority_credentials`, `redact_url_unparseable_query_redacted_short_and_long`, and `redact_url_unparseable_utf8_straddle_no_panic` MUST FAIL (credential fixtures leak; the straddle panics on the byte-level truncate). `redact_url_at_sign_outside_authority_window_visible` and `redact_url_bd_repro_never_leaks_credentials` are GUARD tests that may already pass pre-fix (the at-sign fixture round-trips identically through the current Err arm; the bd-repro string parses — `%` survives userinfo un-validated — and the current Ok arm already masks it). Then implement steps 1-3 and confirm all pass. Do not modify the four existing `redact_url_*` tests or their assertions.

**Tests:** (executable spec — name, arrange, act, assert)
- `redact_url_suppresses_unparseable_authority_credentials`: fixtures `http://u:secretpw@/x` (empty host), `http://u:secretpw@host:99999/x` (invalid port), `http://u:secretpw@host:99999` (invalid port, no path terminator — window extends to end of input), `//u:secretpw@h/x` (protocol-relative, no base) → for each: assert `url::Url::parse(fixture).is_err()`; call `redact_url_for_diagnostics(fixture)`; assert output == `"[redacted]"`.
- `redact_url_bd_repro_never_leaks_credentials`: fixture `http://user:pa%ss@host/path` (bd rc-2i5c5 repro; do NOT assert which arm handles it) → call redact; assert output contains neither `"user:pa%ss"` nor `"pa%ss"`.
- `redact_url_unparseable_query_redacted_short_and_long`: short fixture `http://host:99999/path?token=shortsecret` → assert parse fails; assert output == `http://host:99999/path?[redacted]` (ends with the suffix). Long fixture: `http://host:99999/` + `"a".repeat(300)` + `?token=longsecret` → assert parse fails; assert output contains no byte of `longsecret`, output length <= 256.
- `redact_url_unparseable_utf8_straddle_no_panic`: fixture `"a"` + `"é".repeat(200)` (401 bytes, multibyte straddling the 256 cap, no `//`, no `?`, no `@`) → call redact; assert it does not panic, `output.len()` <= 256, `output.len()` >= 253, and `fixture.is_char_boundary(output.len())` is true (boundary-aligned cut).
- `redact_url_at_sign_outside_authority_window_visible`: fixtures `http://host:99999/x@y` and `mailto:user@example.com` → call redact; assert output == fixture in both cases (not suppressed, no query to drop, under the cap). Note: `mailto:user@example.com` PARSES successfully (cannot-be-a-base opaque URL, empty username/password) and exercises the parse-success arm with a byte-identical round-trip — do NOT add an `is_err()` precondition for it; `http://host:99999/x@y` does fail parse and exercises the Err arm.
- Existing tests `redact_url_masks_userinfo_and_query`, `redact_url_keeps_clean_urls_visible`, `redact_url_masks_password_only_userinfo`, `redact_url_truncates_unparseable` must pass WITHOUT modification (parse-success arm golden + credential-free truncation contract).

Command: `cargo test -p camel-component-http --lib redact_url` — all listed tests pass; expected before implementation: suppresses/query/straddle tests fail (leaks present, straddle panics), the two guard tests (at-sign, bd-repro) may already pass, existing four pass.

**Acceptance:**
- `cargo test -p camel-component-http --lib redact_url` exits 0 with all new + existing tests passing.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0 (or `cargo fmt` applied).
- The four pre-existing `redact_url_*` tests are byte-identical to the blessed spec's parse-success golden behavior (no assertion edits).

- [x] 1.1

### Task 1.2: Fence-path contract test — unparseable override never leaks credentials

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified — tests only)

**Steps:**
1. Add one test `armed_fence_rejects_unparseable_override_redacted` immediately after `armed_fence_rejects_unknown_host_redacted` (~line 8748), copying its structure: build `HttpEndpointConfig::from_uri("http://x?allowedUriHosts=api.internal:8443,cdn.example.com")`, set header `CamelHttpUri` to `http://u:fencesecret@evil.example.com:99999/x` (invalid port → `url::Url::parse` fails → `uri_host_allowed` returns `Ok(false)` → fence error renders through the redaction path).
2. Assert on `HttpProducer::resolve_url(&exchange, &cfg)` error text: contains `"allowedUriHosts fence"` (names the fence), contains `"[redacted]"` (suppression sentinel), contains neither `"fencesecret"` nor `"u:"` (credential bytes).
3. Fail-first is N/A for this task: it lands after task 1.1's fix in the same branch, so the test must PASS as written. Its assertions target the fence contract (sentinel present, credential bytes absent, fence named), which is what guards against regression of the helper's Err arm.

**Tests:** (executable spec)
- `armed_fence_rejects_unparseable_override_redacted`: cfg with armed fence + unparseable credential-bearing `CamelHttpUri` override → `HttpProducer::resolve_url` returns Err → error message names the fence, shows `[redacted]`, and carries no byte of `fencesecret` nor the `u:` userinfo prefix.

Command: `cargo test -p camel-component-http --lib armed_fence` — all armed-fence tests (existing three rejection/allow tests plus the new one) pass.

**Acceptance:**
- `cargo test -p camel-component-http --lib armed_fence` exits 0.
- `cargo test -p camel-component-http --lib` exits 0 (no regression in the tests module).
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- No production-code diff in this task (tests-only, verifiable via `git diff --stat` showing only lib.rs test-module additions — production lines unchanged relative to task 1.1's commit).

- [x] 1.2
