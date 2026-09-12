# Tasks: url-redact-password

## camel-component-http

### Task 1.1: Widen `redact_url_for_diagnostics` masking to password-only userinfo

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. In `redact_url_for_diagnostics` (search `pub(crate) fn redact_url_for_diagnostics`,
   ~line 2892), change the masking gate from
   `if !u.username().is_empty() {` to
   `if !u.username().is_empty() || u.password().is_some() {`.
   (Verified against url 2.5.8 source: `Url::password(&self) ->
   Option<&str>` — `None` when absent.)
   Change NOTHING else in the function — username still becomes `***`,
   password still becomes `None`, query redaction and the 256-char
   truncation stay untouched.
2. In the test module marked `// Security: credential redaction (audit
   2026-08-31, finding F3-1)` (~line 3673), append the unit test
   `redact_url_masks_password_only_userinfo` specified below, directly
   after `redact_url_keeps_clean_urls_visible`.
3. In the fence test module, directly after
   `armed_fence_rejects_unknown_host_redacted` (~line 8739), append the
   fence-path test `armed_fence_rejects_password_only_userinfo_redacted`
   specified below, mirroring that test's arrange style exactly:
   build the config via `HttpEndpointConfig::from_uri`, set the override
   via `exchange.input.set_header("CamelHttpUri", serde_json::Value::String(..))`,
   then drive the failure with
   `HttpProducer::resolve_url(&exchange, &cfg).expect_err(..)`.
4. Run the tests: `cargo test -p camel-component-http` (in the
   worktree). All must pass, including the pre-existing
   `redact_url_masks_userinfo_and_query`,
   `redact_url_keeps_clean_urls_visible`, and
   `armed_fence_rejects_unknown_host_redacted` unmodified.
5. Run `cargo fmt --check --all` and
   `cargo clippy -p camel-component-http --all-targets -- -D warnings`;
   both must be clean (fix formatting in the new tests only if fmt
   objects).

**Tests:** (executable spec — write EXACTLY these, no additional test design)
- `redact_url_masks_password_only_userinfo`:
  - arrange: the existing redaction test module is in scope
  - act: call `redact_url_for_diagnostics` on `"http://:pwsecret@host.example/"`
  - assert (in this order): result `!contains("pwsecret")` FIRST, then
    `assert_eq!(result, "http://***@host.example/")` — the exact masked
    shape
  - act: call it on `"http://user:pw2@host.example/api"` (regression)
  - assert: `!contains("pw2")` and
    `assert_eq!(result, "http://***@host.example/api")`
  - act: call it on `"http://host.example/api"` (regression)
  - assert: `assert_eq!(result, "http://host.example/api")` — byte-identical
  - command: `cargo test -p camel-component-http redact_url_masks_password_only_userinfo`
  - expected: FAILS before step 1 (password-only leaks `pwsecret`), PASSES after
- `armed_fence_rejects_password_only_userinfo_redacted`:
  - arrange: `HttpEndpointConfig::from_uri("http://x?allowedUriHosts=api.internal:8443,cdn.example.com").unwrap()`;
    exchange whose input header `CamelHttpUri` is
    `"http://:passwordonly@evil.example.com/x?token=querysecret"`
  - act: `HttpProducer::resolve_url(&exchange, &cfg).expect_err("password-only override outside the fence must fail resolution")`
  - assert: message `!contains("passwordonly")`, `!contains("querysecret")`,
    AND `contains("http://***@evil.example.com/x?[redacted]")` — the
    exact `***@` masked shape pinned on the fence path
  - command: `cargo test -p camel-component-http armed_fence_rejects_password_only_userinfo_redacted`
  - expected: FAILS before step 1 (error echoes `:passwordonly@`), PASSES after

**Acceptance:**
- `cargo test -p camel-component-http` exits 0
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0
- `cargo fmt --check --all` exits 0
- `redact_url_for_diagnostics` stays `pub(crate)` with an unchanged
  signature (no public API change)
- The three pre-existing tests named in step 4 pass unmodified

- [x] 1.1
