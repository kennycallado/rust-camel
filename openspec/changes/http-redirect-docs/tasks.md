# Tasks — http-redirect-docs

## T1 — Error hint for unfollowed redirects

- **Files**: `crates/components/camel-http/src/lib.rs` (throw-exception block, ~line 3588)
- **Steps**:
  1. In the `throw_exception_on_failure` block that builds `CamelError::HttpOperationFailed`, compute `redirect_not_followed = !config.follow_redirects && matches!(status_code, 301 | 302 | 303 | 307 | 308)`.
  2. When true, append ` (redirect not followed; set followRedirects=true)` to `status_text` before constructing the error.
  3. Do not touch `method`, `url` (keeps `redact_url_for_diagnostics`), `status_code`, `response_body`, or the `CamelError` type.
- **Tests** (`crates/components/camel-http/src/lib_tests.rs`):
  - `follow_redirects_false_error_names_the_param` — arrange: `start_redirect_server()`, default config (`followRedirects=false`), endpoint `?throwExceptionOnFailure=true&allowInternal=true`; act: `producer.oneshot`; assert: `Err`, `err.to_string()` contains `302 Found (redirect not followed; set followRedirects=true)` and does not contain the `Location` value (`/final` target URL is same-origin, so assert no `Location` header value leakage via the body — the error must not name the redirect target).
  - `not_modified_304_has_no_redirect_hint` — arrange: `HttpTestServer` fixed `304 Not Modified` responder, `throwExceptionOnFailure=true`; act: `oneshot`; assert: `Err`, message has no `redirect not followed`.
  - `error_hint_preserves_url_redaction` — arrange: redirect server, endpoint `http://user:secret@127.0.0.1:PORT/?throwExceptionOnFailure=true&allowInternal=true`; act: `oneshot`; assert: `Err`, message contains no `secret`, no `user:secret`, and carries the `***@` redaction mask.
- **Acceptance**: hint only for 301/302/303/307/308 with `followRedirects=false`; `CamelError` shape unchanged; redaction intact.
- [x] T1

## T2 — 302 followed when followRedirects=true (error-path counterpart)

- **Files**: `crates/components/camel-http/src/lib_tests.rs`
- **Steps**: extend/mirror the existing `test_follow_redirects_true_follows_redirect` shape (no new server helper).
- **Tests**:
  - `follow_redirects_true_302_is_followed` — arrange: `start_redirect_server()`, `HttpConfig::default().with_follow_redirects(true)`, `?allowInternal=true`; act: `oneshot`; assert: success, body `{"status":"final"}` from the redirect target.
- **Acceptance**: `followRedirects=true` path unchanged and covered at body level.
- [x] T2

## T3 — Docs page correction

- **Files**: `docs/src/components/http.md`
- **Steps**:
  1. Fix the producer sentence (line ~73): the Producer does not follow redirects by default; a 3xx is returned as-is and fails the exchange with `throwExceptionOnFailure=true`.
  2. State that `followRedirects=true` enables the SSRF-validated hop loop and name the trade-off (following redirects widens egress; off by default for that reason).
  3. Add a Producer options table: `followRedirects` (default `false`), `maxRedirects` (default `10`, accepted `0..=20`; `0` sends once and returns the redirect as-is; no effect without `followRedirects=true`).
  4. Cross-link ADR-0071 for the SSRF policy.
- **Tests**: `cd docs && mdbook test` (guide build); wording checked by review.
- **Acceptance**: page no longer claims redirects are followed by default; both parameters documented with defaults and bounds; ADR-0071 linked.
- [x] T3
