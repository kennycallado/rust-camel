# Tasks: redact2

## Phase 1: Canonical module and call-site migration (rc-924sb)

### Task 1.1: Create `camel_api::redact` module with landed semantics verbatim

**Files:**
- `crates/camel-api/src/redact.rs` (new)
- `crates/camel-api/src/lib.rs` (modified: add `pub mod redact;` in alphabetical order between `recipient_list` and `resequencer`)
- `crates/camel-api/CONTEXT.md` (modified: add a `redact` glossary entry — the file is a `## Language` glossary; place it near the line-186 `EndpointUri` entry that documents `to_redacted_string`, citing ADR-0051 and distinguishing the module from `endpoint_uri::to_redacted_string`)

**Steps:**
1. Create `crates/camel-api/src/redact.rs` — std-only, no new dependencies, unconditional (no feature gates). Module docs state: canonical string-based URL redaction for diagnostic surfaces (ADR-0051); distinct from `endpoint_uri::to_redacted_string` (catalog-driven, authored-URI layer).
2. Implement the private core with EXACTLY the landed semantics (copy from camel-jms `config.rs:464` `mask_authority_windows` and `config.rs:493` `truncate_utf8_safe`): window = starts after a `//` plus any run of extra slashes, ends at next `/`, `?`, `#`; mask bytes window-start..last-`@` as `***`, reverse-offset order, dedup equal ranges; idempotent.
3. Implement `pub fn redact_url(raw: &str) -> String`: window mask, then truncate at earliest `?`/`#`, append one `?[redacted]`/`#[redacted]` sentinel per distinct introducer in first-occurrence order (compose-both, `#[redacted]?[redacted]` when `#` precedes `?`), sentinel budget (22/11/0 bytes) reserved via `truncate_utf8_safe(&mut out, 256 - sentinel_total)` BEFORE appending, final `truncate_utf8_safe(&mut out, 256)`. Source: camel-config `config.rs:1409`.
4. Implement `pub fn redact_url_fail_closed(raw: &str) -> String`: if any window carries `@` return `"[redacted]"`; else `redact_url(raw)`.
5. Implement `pub fn window_has_at_sign(raw: &str) -> bool`: same window enumeration as the mask (source: camel-http `lib.rs:3030`).
6. Implement `pub fn redact_url_with_query_allowlist(raw: &str, sensitive_key_substrings: &[&str]) -> String`: window mask, split query at first `?`, per-pair: lowercased key contains any sensitive substring → `k=<redacted>`, else pair kept; fragment dropped at first `#` with `#[redacted]` appended after a `truncate_utf8_safe(&mut out, 245)` reservation; final 256 cap. Source: camel-jms `config.rs:410` with the key list parametrized.
7. Write `#[cfg(test)] mod tests` in `redact.rs`, defining `const JMS_KEYS: &[&str] = &["password", "passwd", "secret", "credential", "token", "username", "user"];` for the allowlist tests — migrate the pure helper-behavior tests: all tests from `crates/camel-config/src/config_tests/url_redaction_tests.rs`, the redact-behavior tests from camel-jms `config.rs` (`redact_exact_*`, `redact_broker_url_masks_userinfo_and_sensitive_query`, sentinel/cap/later-window pins) and `component.rs` (`redact_url_*` block), and the pure-string redact tests from camel-http `lib.rs` (sentinel compose, window mask, cap pins — NOT the parse-arm tests that need the url crate). Adapt only the call paths to the new module fns; keep fixture strings and expected outputs byte-identical. Include the two spec-pinned executable scenarios: `redact_url("https://***@host/p?a=1#f")` == `"https://***@host/p?[redacted]#[redacted]"` (idempotence) and the strict-variant/compose fixtures.
8. Register `pub mod redact;` in lib.rs; add the CONTEXT.md paragraph.

**Tests:** (in `redact.rs` `mod tests`)
- `strict_masks_windows_and_composes_sentinels`: `redact_url("https://user:pass@h/p?a=1#t=x")` → `"https://***@h/p?[redacted]#[redacted]"`; `redact_url("https://h//u2:p2@evil/")` masks `u2:p2`, renders `***@evil`.
- `fail_closed_suppresses_window_with_at`: `redact_url_fail_closed("http://u:secretpw@host:99999/x")` → `"[redacted]"`.
- `sentinel_never_splits_at_cap`: URL with >245 pre-sentinel bytes + query + fragment → output <= 256 bytes, ends with complete `?[redacted]#[redacted]`, UTF-8 boundary cut.
- `idempotent_mask_composition_pin`: `redact_url("https://***@host/p?a=1#f")` → `"https://***@host/p?[redacted]#[redacted]"`.
- `allowlist_keeps_benign_and_redacts_sensitive`: `redact_url_with_query_allowlist("tcp://host:61616?password=p&user=u&keepAlive=true", JMS_KEYS)` → `password=<redacted>`, `user=<redacted>`, `keepAlive=true` visible.
- plus the migrated landed tests (command: `cargo test -p camel-api redact::` — all pass immediately; this is a verbatim move, no failing-test-first).
- `command`: `cargo test -p camel-api --lib redact` — expected: pass.
- `command`: `cargo fmt --check` and `cargo clippy -p camel-api -- -D warnings` — expected: exit 0.

**Acceptance:**
- `cargo test -p camel-api --lib redact` exits 0 with the migrated tests present.
- `cargo clippy -p camel-api -- -D warnings` exits 0; `cargo fmt --check` exits 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api --no-deps` exits 0 (doc comments on all 4 pub items, valid intra-doc links).
- camel-api Cargo.toml unchanged (no new deps).

- [x] 1.1

### Task 1.2: Migrate camel-config `redact_url` to the canonical helper

**Files:**
- `crates/camel-config/src/config.rs` (modified: delete local `redact_url` ~line 1409 and `truncate_utf8_safe` ~line 1464; both Debug call sites — `CacheRepoConfig` Debug ~line 1492 and the repo Debug ~line 746 — call `camel_api::redact::redact_url`)
- `crates/camel-config/src/config_tests/url_redaction_tests.rs` (deleted)

**Steps:**
1. Replace the two `redact_url` call sites with `camel_api::redact::redact_url`.
2. Delete the local `redact_url` and `truncate_utf8_safe` fns and their doc comments.
3. Delete `config_tests/url_redaction_tests.rs` and its `mod` registration (~line 1362) — behavior moved to camel-api in task 1.1.
4. Add 2 thin delegation pins in the existing config test module: `redact_url_delegation_pin_identity` asserting `CacheRepoConfig` Debug on a fixture with userinfo+query renders the canonical output; `redact_url_delegation_pin_shared_fixture` asserting the cross-surface identity fixture through the Debug surface — a `CacheRepoConfig` carrying `http://h:99999/p?token=secret` renders Debug output containing `http://h:99999/p?[redacted]` exactly (spec cross-surface identity input), and a config carrying `redis://h:6379/0?password=x` renders `redis://h:6379/0?[redacted]`.

**Tests:**
- `redact_url_delegation_pin_identity`: CacheRepoConfig Debug on `redis://admin:pw@h:6379/0?password=x` → contains `***@h:6379/0?[redacted]`, not `admin`, not `pw`.
- `redact_url_delegation_pin_shared_fixture`: `CacheRepoConfig` Debug on `url = "http://h:99999/p?token=secret"` renders `http://h:99999/p?[redacted]`; Debug on `url = "redis://h:6379/0?password=x"` renders `redis://h:6379/0?[redacted]`.
- Existing repo-Debug tests keep passing unchanged (they pin behavior through the Debug surface).
- `command`: `cargo test -p camel-config --lib` — expected: pass.
- `command`: `cargo clippy -p camel-config -- -D warnings` — expected: exit 0.

**Acceptance:**
- `cargo test -p camel-config --lib` exits 0; `url_redaction_tests.rs` no longer exists.
- `rg -n 'fn redact_url|fn truncate_utf8_safe' crates/camel-config/src/` returns nothing.
- No behavior change: all pre-existing camel-config tests pass unmodified.

- [x] 1.2

### Task 1.3: Migrate camel-jms `redact_url` and `redact_broker_url` to canonical helpers

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified: `redact_broker_url` becomes a thin wrapper `camel_api::redact::redact_url_with_query_allowlist(raw, BROKER_SENSITIVE_KEY_SUBSTRINGS)` with the unchanged 7-entry key list; delete `mask_authority_windows` and `truncate_utf8_safe`; keep BrokerConfig Debug untouched)
- `crates/components/camel-jms/src/component.rs` (modified: local `redact_url` ~line 950 delegates to `camel_api::redact::redact_url`; drop the `mask_authority_windows, truncate_utf8_safe` import from config ~line 26)

**Steps:**
1. In `config.rs`, add `const BROKER_SENSITIVE_KEY_SUBSTRINGS: &[&str] = &["password", "passwd", "secret", "credential", "token", "username", "user"];` and reduce `redact_broker_url` to the delegation (keep the pub(crate) signature and its doc comment, updated to name the canonical backer).
2. Delete `mask_authority_windows` and `truncate_utf8_safe` from `config.rs`.
3. In `component.rs`, replace the local `redact_url` body with a delegation to `camel_api::redact::redact_url`; fix imports.
4. Keep the camel-jms tests that pin Debug/log surfaces (`broker_config_debug_redacts_password`, validate-error redaction, `redact_url_strips_*` in component.rs) — they now exercise the delegation; delete only tests that referenced the deleted private helpers directly if any.

**Tests:**
- Existing `redact_broker_url_masks_userinfo_and_sensitive_query` and `redact_exact_*` pins in `config.rs` pass unchanged (allowlist semantics preserved exactly).
- Existing `redact_url_strips_userinfo_with_password` and siblings in `component.rs` pass unchanged.
- `redact_broker_url_delegation_pin`: `redact_broker_url("tcp://h:61616?keepAlive=true")` → `tcp://h:61616?keepAlive=true` (benign params still visible — ActiveMQ diagnosability).
- `redact_url_shared_fixture_pin`: `camel_api::redact::redact_url("http://h:99999/p?token=secret")` == `"http://h:99999/p?[redacted]"` and the component `redact_url` on the same input returns the same bytes (spec cross-surface identity fixture, jms surface).
- `command`: `cargo test -p camel-component-jms --lib` — expected: pass.
- `command`: `cargo clippy -p camel-component-jms -- -D warnings` — expected: exit 0.

**Acceptance:**
- `cargo test -p camel-component-jms --lib` exits 0.
- `rg -n 'fn mask_authority_windows|fn truncate_utf8_safe' crates/components/camel-jms/src/` returns nothing.
- Broker key list unchanged (7 entries, same order).

- [x] 1.3

### Task 1.4: Compose camel-http over the canonical helpers

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified: `redact_url_for_diagnostics` ~line 2909 rewritten over camel-api helpers; delete `mask_rendered_windows` ~line 3001, `window_has_at_sign` ~line 3030, `truncate_utf8_safe` ~line 3050; test adjustments)

**Steps:**
1. Parsed arm: keep `url::Url::parse`; fail-closed guard becomes `!u.cannot_be_a_base() && u.host_str().is_none() && camel_api::redact::window_has_at_sign(raw)` → `"[redacted]"`; accessor mask (`set_username("***")`, `set_password(None)`); KEEP query and fragment set on the parsed URL; `u.to_string()`; then `camel_api::redact::redact_url(&s)` (canonical mask + sentinel compose + cap). Delete the local trailing-`?`/`#` pop loop, the `mask_rendered_windows` call, the local sentinel-compose block, and the local cap calls — the canonical call owns them.
2. Err arm: body becomes `camel_api::redact::redact_url_fail_closed(raw)`.
3. Delete `mask_rendered_windows`, `window_has_at_sign`, `truncate_utf8_safe` from lib.rs.
4. Tests in lib.rs need NO expectation updates: all 27 landed `redact_url_*` tests pass unchanged — no landed parse-arm fixture carries a fragment containing `?`, so compose-both changes nothing they pin; the unification is pinned only by the new test. Add `parse_success_fragment_composes` pinning `redact_url_for_diagnostics("https://h/p#access_token=x")` == `"https://h/p#[redacted]"` and `("https://h/cb#f?state=x")` == `"https://h/cb#[redacted]?[redacted]"` (spec scenarios). Add `err_arm_delegation_pin`: an unparseable input with `@` in window → `"[redacted]"`; the cross-surface fixture `http://h:99999/p?token=secret` → `http://h:99999/p?[redacted]`.

**Tests:**
- All 27 landed `redact_url_*` tests in lib.rs pass unchanged (endpoint Debug, request/response log, fence tests included; the override-fence redaction pins live in lib.rs ~9302-9388, the redirect-hop fence pins in ssrf.rs).
- `parse_success_fragment_composes`, `err_arm_delegation_pin`: as specified above.
- `command`: `cargo test -p camel-component-http --lib redact` — expected: pass.
- `command`: `cargo test -p camel-component-http --lib` — expected: pass (endpoint Debug, request/response log tests unchanged).
- `command`: `cargo clippy -p camel-component-http -- -D warnings` — expected: exit 0.

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0.
- `rg -n 'fn mask_rendered_windows|fn window_has_at_sign|fn truncate_utf8_safe' crates/components/camel-http/src/lib.rs` returns nothing.
- `redact_url_for_diagnostics` makes exactly one canonical redaction call per arm (`redact_url` in the Ok arm, `redact_url_fail_closed` in the Err arm); the Ok arm's fail-closed guard additionally calls `camel_api::redact::window_has_at_sign`.

- [x] 1.4

## Phase 2: Encoded-credential masking in the allowlist variant (rc-r7v8s)

### Task 2.1: Failing tests for the minimal-decode rule and key decode

**Files:**
- `crates/camel-api/src/redact.rs` (modified: tests only)

**Steps:**
1. Add tests to `mod tests` (all initially FAILING against the task-1.1 implementation):
   - `allowlist_masks_percent_encoded_credentials_uppercase`: `redact_url_with_query_allowlist("tcp://h:61616?redirect=http%3A%2F%2Fuser%3Asecret%40host", JMS_KEYS)` → no `secret`, no `user%3Asecret%40`; `redirect` renders `<redacted>`; host `h:61616` visible.
   - `allowlist_masks_percent_encoded_credentials_lowercase`: same shape fully lowercased: `redact_url_with_query_allowlist("tcp://h:61616?redirect=http%3a%2f%2fuser%3asecret%40host", JMS_KEYS)` → `redirect` renders `<redacted>`, no `secret`.
   - `allowlist_masks_literal_at_bypass`: `"tcp://h:61616?next=%2F%2Fuser:pass@host"` → `<redacted>`, no `pass`.
   - `allowlist_masks_fully_literal_credential_pair`: `"tcp://h:61616?next=user:pass@host"` → `<redacted>`, no `pass`.
   - `allowlist_keeps_lone_email_value`: `"tcp://h:61616?contact=admin%40corp.example"` → pair visible verbatim.
   - `allowlist_masks_credential_shaped_userhostport`: `"tcp://h:61616?next=user%40host%3Aport"` → `<redacted>` (over-mask stance, ADR-0051).
   - `allowlist_decodes_percent_encoded_sensitive_key`: `"tcp://host:61616?pass%77ord=shortsecret"` → `pass%77ord=<redacted>`, no `shortsecret`.
2. Run and confirm the red state: 6 of the 7 fail (assertion failures, not compile failures) against the task-1.1 implementation; `allowlist_keeps_lone_email_value` already passes and stays as a passing regression pin guarding the under-redaction boundary.

**Tests:**
- `command`: `cargo test -p camel-api --lib redact::tests::allowlist_` — expected: 6 failures + 1 pass (`allowlist_keeps_lone_email_value`) before task 2.2.

**Acceptance:**
- The 7 tests exist with the exact fixtures above; 6 fail and the email-visibility pin passes against the current implementation.

- [x] 2.1

### Task 2.2: Implement the minimal-decode matcher and `%HH` key decode

**Files:**
- `crates/camel-api/src/redact.rs` (modified)
- `crates/components/camel-jms/src/config.rs` (modified: one delegation-pin test addition)

**Steps:**
1. Add private `fn minimal_decode_pair(pair: &str) -> String`: single left-to-right pass; `%40`→`@`, `%3a`/`%3A`→`:`, `%2f`/`%2F`→`/`; all other bytes copied verbatim (including other `%HH` sequences and stray `%`).
2. In `redact_url_with_query_allowlist`, for each query pair: extract the raw key (bytes before the first `=`); compute the match key by single-pass `%HH` decode (any two hex digits after `%`, case-insensitive, invalid sequences copied verbatim) then lowercase; if any sensitive substring is contained in the match key → render `{raw_key}=<redacted>`; else if `minimal_decode_pair(pair)` contains `@` AND (`:` OR `//`) → render `<redacted>`; else keep the pair verbatim.
3. Add camel-jms delegation pin `broker_debug_masks_encoded_credentials`: `format!("{:?}", BrokerConfig)` on a broker URL containing `?redirect=http%3A%2F%2Fuser%3Asecret%40host` shows neither `secret` nor `user%3Asecret%40`, and shows the `<redacted>` marker for the `redirect` pair (jms spec scenario).
4. Make the 7 failing tests from task 2.1 pass; whole-suite green.

**Tests:**
- The 7 tests from task 2.1 now pass.
- `broker_debug_masks_encoded_credentials` (camel-jms) passes.
- `command`: `cargo test -p camel-api --lib redact && cargo test -p camel-component-jms --lib` — expected: pass.

**Acceptance:**
- `cargo test -p camel-api --lib` and `cargo test -p camel-component-jms --lib` exit 0.
- `cargo clippy -p camel-api -- -D warnings` and `cargo clippy -p camel-component-jms -- -D warnings` exit 0.
- Sensitive-substring matching still hits plain literal keys (`pass%77ord` AND `password` both redact).

- [x] 2.2

## Phase 3: Backslash authority windows (rc-f05q8)

### Task 3.1: Failing tests for scheme-gated backslash windows

**Files:**
- `crates/camel-api/src/redact.rs` (modified: tests only)
- `crates/components/camel-http/src/lib.rs` (modified: tests only)

**Steps:**
1. Add camel-api tests (initially FAILING):
   - `backslash_run_non_special_scheme_masked`: `redact_url("foo:\\\\user:pass@evil/")` masks `user:pass` (renders `***@evil`), `redact_url_fail_closed` on the same input → `"[redacted]"`; sibling visibility pin `redact_url("foo:\\\\clean/path")` → unchanged (http spec scenario's clean-backslash given).
   - `backslash_single_after_multi_char_scheme_masked`: `redact_url("http:\\user:pass@evil\\path")` masks `user:pass` (single backslash after the multi-character `http` scheme).
   - `backslash_single_after_one_char_scheme_credential_shaped_masked`: `redact_url("x:\\user:pass@evil")` masks `user:pass`.
   - `drive_path_stays_visible`: `redact_url("C:\\Users\\x@corp\\file")` → unchanged.
   - `unc_path_stays_visible`: `redact_url("\\\\server\\x@y")` → unchanged.
   - `window_has_at_sign_sees_backslash_windows`: `window_has_at_sign("foo:\\\\u:p@e/")` → true; `window_has_at_sign("C:\\Users\\x@corp\\file")` → false.
2. Add camel-http tests (initially FAILING where behavior changes):
   - `non_special_backslash_authority_masked`: `redact_url_for_diagnostics("foo:\\\\user:pass@evil/")` renders no credential byte (spec scenario) AND the clean sibling `redact_url_for_diagnostics("foo:\\\\clean/path")` stays visible (spec scenario's second given).
   - `one_char_scheme_credential_content_masked`: `redact_url_for_diagnostics("x:\\user:pass@evil")` renders no credential byte (spec scenario).
   - `drive_and_unc_inputs_stay_visible`: both `C:\Users\x@corp\file` and `\\server\x@y` render under query-redaction/cap rules only (spec scenario).
3. Run and confirm the new tests fail (backslash cases) while the drive/UNC visibility pins may already pass — record which.

**Tests:**
- `command`: `cargo test -p camel-api --lib redact::tests::backslash_` — expected: the masking tests fail before task 3.2.
- `command`: `cargo test -p camel-api --lib redact::tests::drive_` and `cargo test -p camel-api --lib redact::tests::unc_` and `cargo test -p camel-api --lib redact::tests::window_has_at_sign_sees_backslash_windows` — expected: the drive/UNC visibility pins pass; `window_has_at_sign_sees_backslash_windows` fails (backslash windows invisible to the old scan).
- `command`: `cargo test -p camel-component-http --lib -- backslash one_char_scheme drive_and_unc non_special` — expected: the two masking tests fail before task 3.2.

**Acceptance:**
- The backslash masking tests exist and fail; the drive/UNC visibility tests exist and pass.

- [x] 3.1

### Task 3.2: Implement window enumeration with scheme-gated backslash runs

**Files:**
- `crates/camel-api/src/redact.rs` (modified)

**Steps:**
1. Replace the `//`-only window enumeration (used by the mask, `window_has_at_sign`, and both fail-closed checks) with a run scanner: enumerate maximal runs of `/` and `\` over the input; a window runs from the run's end to the next `/`, `?`, or `#`.
2. Gate the runs: pure-slash runs open a window at length >= 2 (landed behavior — `//` and longer, dedup windows sharing one slash run); a backslash-bearing run of length >= 2 opens a window iff an RFC 3986 scheme prefix (`[a-zA-Z][a-zA-Z0-9+.-]*:`) sits immediately before the run; a single-`\` run opens a window iff the prefix has >= 2 characters, OR the prefix has exactly 1 character AND the candidate window content contains `:` before its last `@`.
3. Keep reverse-offset masking and dedup semantics unchanged; windows from one run are visited in the same order as before.
4. Make all Phase-3 failing tests pass; run the full four-crate suites to prove no regression (all Phase-1/2 pins hold).

**Tests:**
- All task 3.1 tests pass; `cargo test -p camel-api --lib`, `cargo test -p camel-config --lib`, `cargo test -p camel-component-jms --lib`, `cargo test -p camel-component-http --lib` exit 0.
- `command`: `cargo clippy -p camel-api -- -D warnings` — expected: exit 0.

**Acceptance:**
- All four crate lib suites green; clippy and fmt clean.
- The `redact_url_backslash_authority_ruling` pin (special-scheme parsed-arm normalization) still passes unchanged.

- [x] 3.2
